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
use crate::sandbox::runtime::Sandbox;
use crate::store::settings::ModelSelection;

/// The production base URL of the OpenRouter API.
pub const OPENROUTER_BASE_URL: &str = OPENROUTER.base_url;

/// Rig counts model calls, not tool calls: 10 calls of `run_script` and one final answer.
const MAX_MODEL_CALLS: usize = 11;

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
    pub model: ModelSelection,
    pub system_prompt: String,
    pub history: Vec<Message>,
    pub prompt: Message,
    pub sandbox: Option<Arc<Sandbox>>,
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

    /// Runs one LLM turn: the model can call `run_script` up to 10 times, then it must answer.
    pub async fn run(&self, request: TurnRequest) -> Result<TurnResult, LlmError> {
        let agent = self.agent(&request);
        let response = agent
            .prompt(request.prompt.clone())
            .history(request.history)
            .max_turns(MAX_MODEL_CALLS)
            .await?;
        Ok(TurnResult {
            text: response.output().trim().to_string(),
            turn_messages: tool_turn(&request.prompt, &response.messages)?,
        })
    }

    fn agent(&self, request: &TurnRequest) -> Agent {
        let builder = AgentBuilder::new(self.client.completion(&request.model.model_id))
            .preamble(&request.system_prompt);
        match &request.sandbox {
            Some(sandbox) => builder
                .dynamic_tool(run_script_tool(Arc::clone(sandbox)))
                .build(),
            None => builder.build(),
        }
    }

    pub async fn extract<T>(
        &self,
        model: &ModelSelection,
        system_prompt: &str,
        history: Vec<Message>,
        instruction: &str,
    ) -> Result<T, LlmError>
    where
        T: JsonSchema + DeserializeOwned + Serialize + Send + Sync + 'static,
    {
        let extractor = ExtractorBuilder::<T>::new(self.client.completion(&model.model_id))
            .append_preamble(system_prompt)
            .build();
        let response = extractor.extract(instruction).history(history).await?;
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
    use crate::test_support::json_response;

    const FINAL_TEXT: &str = include_str!("../../tests/fixtures/llm/final_text.json");
    const RUN_SCRIPT_CALL: &str = include_str!("../../tests/fixtures/llm/run_script_call.json");

    fn sandbox() -> Arc<Sandbox> {
        Arc::new(Sandbox::new(
            Arc::new(echo_table()),
            SandboxLimits::production(),
        ))
    }

    fn test_model() -> ModelSelection {
        ModelSelection {
            model_id: "test/model".to_string(),
            input_modalities: vec!["text".to_string()],
        }
    }

    fn request(sandbox: Option<Arc<Sandbox>>) -> TurnRequest {
        TurnRequest {
            model: test_model(),
            system_prompt: "You are Nuki.".to_string(),
            history: vec![Message::user("Hi Nuki."), Message::assistant("Hello.")],
            prompt: Message::user("Echo a = 1."),
            sandbox,
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

        let result = client(&server).run(request(None)).await.unwrap();

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

        let result = client(&server).run(request(Some(sandbox()))).await.unwrap();

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

        let result = client(&server).run(request(Some(sandbox()))).await.unwrap();

        assert_eq!(result.text, "");
        assert_echo_tool_turn(messages(result.turn_messages));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn run_sends_the_system_prompt_and_the_tool_declaration() {
        let server = chat_server(&[FINAL_TEXT]).await;

        client(&server).run(request(Some(sandbox()))).await.unwrap();

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
    async fn run_fails_when_the_model_calls_the_tool_more_than_10_times() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(json_response(200, RUN_SCRIPT_CALL))
            .expect(11)
            .mount(&server)
            .await;

        let error = client(&server)
            .run(request(Some(sandbox())))
            .await
            .unwrap_err();

        assert!(
            matches!(error, LlmError::MaxTurns { max_turns: 11 }),
            "unexpected error: {error:?}"
        );
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
    struct BugFields {
        title: String,
        severity: Option<String>,
    }

    #[tokio::test]
    async fn extract_returns_the_typed_value() {
        let server =
            chat_server(&[include_str!("../../tests/fixtures/llm/submit_call.json")]).await;

        let fields: BugFields = client(&server)
            .extract(
                &test_model(),
                "You read bug reports.",
                vec![Message::user("Playback stops after one track.")],
                "Extract the bug fields.",
            )
            .await
            .unwrap();

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
}
