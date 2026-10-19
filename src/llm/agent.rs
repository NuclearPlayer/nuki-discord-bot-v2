use std::sync::Arc;

use rig_agent::completion::{PromptError, StructuredOutputError};
use rig_agent::extractor::ExtractorBuilder;
use rig_agent::{Agent, AgentBuilder};
use rig_core::message::{AssistantContent, Message};
use rig_core::providers::openai::wire::OPENROUTER;
use rig_core::providers::openai::{OpenAI, OpenAIConfig};
use schemars::JsonSchema;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use thiserror::Error;

use super::script_tool::run_script_tool;
use super::stats_hook::StatsHook;
use crate::sandbox::runtime::Sandbox;
use crate::stats::event::LlmPurpose;
use crate::stats::recorder::RunStats;

/// The production base URL of the OpenRouter API.
pub const OPENROUTER_BASE_URL: &str = OPENROUTER.base_url;

/// Rig counts model calls, not tool calls: 50 calls of `run_script` and one final answer.
const MAX_MODEL_CALLS: usize = 51;

/// The name of the agent in the logs of rig.
const AGENT_NAME: &str = "Nuki";

#[derive(Debug, Error)]
pub enum LlmError {
    #[error("the agent reached the limit of {max_turns} model calls")]
    MaxTurns { max_turns: usize },
    #[error("the LLM run failed: {0}")]
    Prompt(#[source] Box<PromptError>),
    #[error("the extraction failed: {0}")]
    Extraction(#[source] Box<StructuredOutputError>),
    #[error("a turn message does not serialize: {0}")]
    Json(#[from] serde_json::Error),
    #[error("the messages of the run do not start with the prompt")]
    TranscriptWithoutPrompt,
}

pub struct TurnRequest {
    pub model_id: String,
    pub system_prompt: String,
    pub history: Vec<Message>,
    pub prompt: Message,
    pub sandbox: Option<Arc<Sandbox>>,
    /// The recorder of the run that the turn is part of.
    pub stats: RunStats,
}

#[derive(Debug, PartialEq)]
pub struct TurnResult {
    pub text: String,
    /// The tool calls and the tool results of the turn, as JSON of `rig` messages.
    pub turn_messages: Vec<serde_json::Value>,
}

pub struct LlmClient {
    client: OpenAI,
}

impl LlmClient {
    pub fn new(api_key: &str, base_url: &str) -> LlmClient {
        LlmClient {
            client: OpenAIConfig::with_key(&OPENROUTER, api_key)
                .with_base_url(base_url)
                .client(),
        }
    }

    /// Runs one LLM turn: the model can call `run_script` up to 50 times, then it must answer.
    pub async fn run(&self, request: TurnRequest) -> Result<TurnResult, LlmError> {
        tracing::info!(model = request.model_id, "LLM turn");
        let agent = self.agent(&request);
        let response = agent
            .prompt(request.prompt.clone())
            .history(request.history)
            .max_turns(MAX_MODEL_CALLS)
            .add_hook(StatsHook::new(
                request.stats,
                LlmPurpose::Turn,
                request.model_id,
            ))
            .await?;
        Ok(TurnResult {
            text: response.output().trim().to_string(),
            turn_messages: tool_turn(&request.prompt, &response.messages)?,
        })
    }

    fn agent(&self, request: &TurnRequest) -> Agent {
        let builder = AgentBuilder::new(self.client.completion(&request.model_id))
            .name(AGENT_NAME)
            .preamble(&request.system_prompt);
        match &request.sandbox {
            Some(sandbox) => builder
                .dynamic_tool(run_script_tool(Arc::clone(sandbox), request.stats.clone()))
                .build(),
            None => builder.build(),
        }
    }

    pub async fn extract<T>(
        &self,
        model_id: &str,
        system_prompt: &str,
        history: Vec<Message>,
        instruction: &str,
        stats: &RunStats,
    ) -> Result<T, LlmError>
    where
        T: JsonSchema + DeserializeOwned + Serialize + Send + Sync + 'static,
    {
        let extractor = ExtractorBuilder::<T>::new(self.client.completion(model_id))
            .append_preamble(system_prompt)
            .build();
        let response = extractor
            .extract(instruction)
            .history(history)
            .add_hook(StatsHook::new(
                stats.clone(),
                LlmPurpose::Extract,
                model_id.to_string(),
            ))
            .await?;
        Ok(response.output)
    }
}

/// Serializes the messages that a run added after the prompt, without the final answer.
/// `messages` starts with the prompt. Rig leaves a blank final answer out, so the last
/// message can also be a tool result.
fn tool_turn(prompt: &Message, messages: &[Message]) -> Result<Vec<Value>, LlmError> {
    let added = match messages.split_first() {
        Some((first, added)) if first == prompt => added,
        _ => return Err(LlmError::TranscriptWithoutPrompt),
    };
    let tool_messages = match added.split_last() {
        Some((last, tool_messages)) if is_final_answer(last) => tool_messages,
        _ => added,
    };
    let json = tool_messages
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<_, _>>()?;
    Ok(json)
}

/// An assistant message without tool calls ends the run.
fn is_final_answer(message: &Message) -> bool {
    match message {
        Message::Assistant(assistant) => !assistant
            .content
            .iter()
            .any(|content| matches!(content, AssistantContent::ToolCall(_))),
        _ => false,
    }
}

impl From<PromptError> for LlmError {
    fn from(error: PromptError) -> LlmError {
        match error {
            PromptError::MaxTurns { max_turns, .. } => LlmError::MaxTurns { max_turns },
            other => LlmError::Prompt(Box::new(other)),
        }
    }
}

impl From<StructuredOutputError> for LlmError {
    fn from(error: StructuredOutputError) -> LlmError {
        LlmError::Extraction(Box::new(error))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::Utc;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    use serde::Deserialize;
    use serde_json::json;

    use super::*;
    use crate::llm::test_support::{
        RIG_EXTRACTION_PREAMBLE, assert_echo_tool_turn, llm_request_bodies, messages,
        mount_llm_replies, run_script_tools,
    };
    use crate::sandbox::runtime::SandboxLimits;
    use crate::sandbox::test_support::echo_table;
    use crate::stats::test_support::{
        duration_of, events_between, run_event, test_run, unrecorded_run,
    };
    use crate::test_support::json_response;

    const FINAL_TEXT: &str = include_str!("../../tests/fixtures/llm/final_text.json");
    const RUN_SCRIPT_CALL: &str = include_str!("../../tests/fixtures/llm/run_script_call.json");
    const SUBMIT_CALL: &str = include_str!("../../tests/fixtures/llm/submit_call.json");
    const OPENROUTER_RUN_SCRIPT_CALL: &str =
        include_str!("../../tests/fixtures/llm/openrouter_run_script_call.json");
    const OPENROUTER_FINAL_TEXT: &str =
        include_str!("../../tests/fixtures/llm/openrouter_final_text.json");

    fn sandbox() -> Arc<Sandbox> {
        Arc::new(Sandbox::new(
            Arc::new(echo_table()),
            SandboxLimits::production(),
        ))
    }

    const TEST_MODEL_ID: &str = "test/model";

    fn request(sandbox: Option<Arc<Sandbox>>, stats: RunStats) -> TurnRequest {
        TurnRequest {
            model_id: TEST_MODEL_ID.to_string(),
            system_prompt: "You are Nuki.".to_string(),
            history: vec![Message::user("Hi Nuki."), Message::assistant("Hello.")],
            prompt: Message::user("Echo a = 1."),
            sandbox,
            stats,
        }
    }

    async fn chat_server(replies: &[&str]) -> MockServer {
        let server = MockServer::start().await;
        mount_llm_replies(&server, replies).await;
        server
    }

    fn client(server: &MockServer) -> LlmClient {
        LlmClient::new("test-key", &server.uri())
    }

    #[tokio::test]
    async fn run_returns_the_final_text_without_tools() {
        let server = chat_server(&[FINAL_TEXT]).await;

        let result = client(&server)
            .run(request(None, unrecorded_run().await))
            .await
            .unwrap();

        assert_eq!(
            result,
            TurnResult {
                text: "The echo returned a = 1.".to_string(),
                turn_messages: vec![],
            }
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn run_executes_run_script_and_returns_the_tool_turn() {
        let server = chat_server(&[RUN_SCRIPT_CALL, FINAL_TEXT]).await;

        let result = client(&server)
            .run(request(Some(sandbox()), unrecorded_run().await))
            .await
            .unwrap();

        assert_eq!(result.text, "The echo returned a = 1.");
        assert_echo_tool_turn(messages(result.turn_messages));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn run_keeps_the_tool_result_when_the_final_text_is_empty() {
        let server = chat_server(&[
            RUN_SCRIPT_CALL,
            include_str!("../../tests/fixtures/llm/empty_final_text.json"),
        ])
        .await;

        let result = client(&server)
            .run(request(Some(sandbox()), unrecorded_run().await))
            .await
            .unwrap();

        assert_eq!(result.text, "");
        assert_echo_tool_turn(messages(result.turn_messages));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn run_sends_the_system_prompt_and_the_tool_declaration() {
        let server = chat_server(&[FINAL_TEXT]).await;

        client(&server)
            .run(request(Some(sandbox()), unrecorded_run().await))
            .await
            .unwrap();

        let body = &llm_request_bodies(&server).await[0];
        assert_eq!(
            body["messages"],
            json!([
                {
                    "role": "system",
                    "content": [{ "type": "text", "text": "You are Nuki." }]
                },
                { "role": "user", "content": "Hi Nuki." },
                { "role": "assistant", "content": "Hello." },
                { "role": "user", "content": "Echo a = 1." }
            ])
        );
        assert_eq!(body["tools"], run_script_tools());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn run_fails_when_the_model_calls_the_tool_more_than_50_times() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(json_response(200, RUN_SCRIPT_CALL))
            .expect(51)
            .mount(&server)
            .await;

        let error = client(&server)
            .run(request(Some(sandbox()), unrecorded_run().await))
            .await
            .unwrap_err();

        assert!(
            matches!(error, LlmError::MaxTurns { max_turns: 51 }),
            "unexpected error: {error:?}"
        );
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
    struct BugFields {
        title: String,
        severity: Option<String>,
    }

    async fn extract_bug_fields(server: &MockServer, stats: &RunStats) -> BugFields {
        client(server)
            .extract(
                TEST_MODEL_ID,
                "You read bug reports.",
                vec![Message::user("Playback stops after one track.")],
                "Extract the bug fields.",
                stats,
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn extract_returns_the_typed_value() {
        let server = chat_server(&[SUBMIT_CALL]).await;

        let fields = extract_bug_fields(&server, &unrecorded_run().await).await;

        assert_eq!(
            fields,
            BugFields {
                title: "Playback stops after one track".to_string(),
                severity: None,
            }
        );
        let body = &llm_request_bodies(&server).await[0];
        assert_eq!(
            body["messages"],
            json!([
                {
                    "role": "system",
                    "content": [{
                        "type": "text",
                        "text": format!("{RIG_EXTRACTION_PREAMBLE}You read bug reports.")
                    }]
                },
                {
                    "role": "user",
                    "content": [
                        { "type": "text", "text": "Playback stops after one track." },
                        { "type": "text", "text": "Extract the bug fields." }
                    ]
                }
            ])
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_run_with_a_script_records_its_model_calls_and_its_tool_call() {
        let server = chat_server(&[OPENROUTER_RUN_SCRIPT_CALL, OPENROUTER_FINAL_TEXT]).await;
        let (store, stats) = test_run().await;
        let before = Utc::now();

        client(&server)
            .run(request(Some(sandbox()), stats))
            .await
            .unwrap();

        let after = Utc::now();
        let code = "await test.echo({ a: 1 });\nreturn await test.echo({ b: 2 });";
        let result = r#"{"value":{"b":2}}"#;
        assert_eq!(
            events_between(&store, before, after).await,
            vec![
                run_event(
                    "llm.call",
                    json!({
                        "purpose": "turn",
                        "call_index": 1,
                        "requested_model": "test/model",
                        "response_model": "test/model-20261001",
                        "provider": "Google AI Studio",
                        "response_id": "gen-tool-or",
                        "input_tokens": 1200,
                        "output_tokens": 80,
                        "total_tokens": 1280,
                        "cached_input_tokens": 1000,
                        "cache_write_tokens": 150,
                        "reasoning_tokens": 30,
                        "cost_usd": 0.0042,
                        "cost_estimated": false,
                        "finish_reason": "tool_calls",
                        "output": [{
                            "type": "toolcall",
                            "id": { "provider": "call_1" },
                            "function": {
                                "name": "run_script",
                                "arguments": { "code": code }
                            },
                            "native": {
                                "item": {
                                    "id": "call_1",
                                    "type": "function",
                                    "function": {
                                        "name": "run_script",
                                        "arguments": json!({ "code": code }).to_string()
                                    }
                                },
                                // Rig's hash of the tools and the system prompt of the request.
                                "fingerprint": "a6b08b1500b1daab"
                            }
                        }],
                    })
                ),
                run_event(
                    "tool.call",
                    json!({
                        "code": code,
                        "operations": ["test.echo", "test.echo"],
                        "error": null,
                        "result": result,
                        "result_bytes": result.len(),
                    })
                ),
                run_event(
                    "llm.call",
                    json!({
                        "purpose": "turn",
                        "call_index": 2,
                        "requested_model": "test/model",
                        "response_model": "test/model-20261001",
                        "provider": "Google AI Studio",
                        "response_id": "gen-final-or",
                        "input_tokens": 1320,
                        "output_tokens": 12,
                        "total_tokens": 1332,
                        "cached_input_tokens": 1200,
                        "cache_write_tokens": 0,
                        "reasoning_tokens": 0,
                        "cost_usd": 0.0011,
                        "cost_estimated": false,
                        "finish_reason": "stop",
                        "output": [{ "type": "text", "text": "The echo returned b = 2." }],
                    })
                ),
            ]
        );
    }

    #[tokio::test]
    async fn an_llm_call_records_the_duration_of_the_request() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(json_response(200, FINAL_TEXT).set_delay(Duration::from_millis(50)))
            .mount(&server)
            .await;
        let (store, stats) = test_run().await;

        client(&server).run(request(None, stats)).await.unwrap();

        assert!(duration_of(&store, "llm.call").await >= 50);
    }

    #[tokio::test]
    async fn extract_records_an_llm_call_with_the_purpose_extract() {
        let server = chat_server(&[SUBMIT_CALL]).await;
        let (store, stats) = test_run().await;
        let before = Utc::now();

        extract_bug_fields(&server, &stats).await;

        let after = Utc::now();
        assert_eq!(
            events_between(&store, before, after).await,
            vec![run_event(
                "llm.call",
                json!({
                    "purpose": "extract",
                    "call_index": 1,
                    "requested_model": "test/model",
                    "response_model": "test/model",
                    "provider": null,
                    "response_id": "gen-submit",
                    "input_tokens": 50,
                    "output_tokens": 14,
                    "total_tokens": 64,
                    "cached_input_tokens": null,
                    "cache_write_tokens": null,
                    "reasoning_tokens": null,
                    "cost_usd": null,
                    "cost_estimated": false,
                    "finish_reason": "tool_calls",
                    "output": [{
                        "type": "toolcall",
                        "id": { "provider": "call_submit" },
                        "function": {
                            "name": "submit",
                            "arguments": { "title": "Playback stops after one track", "severity": null }
                        },
                        "native": {
                            "item": {
                                "id": "call_submit",
                                "type": "function",
                                "function": {
                                    "name": "submit",
                                    "arguments": r#"{"title":"Playback stops after one track","severity":null}"#
                                }
                            },
                            // Rig's hash of the tools and the system prompt of the request.
                            "fingerprint": "4a930ff43b485e25"
                        }
                    }],
                })
            )]
        );
    }
}
