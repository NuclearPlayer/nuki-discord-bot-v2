//! The length check of a reply. Jev judges the draft, and Nuki rewrites a draft that is too long.

use std::collections::HashMap;

use anyhow::bail;
use indexmap::indexmap;
use rig_core::message::Message;
use serde_json::json;

use super::{TurnContext, WorkflowDeps};
use crate::decisions::types::{Answer, Question};
use crate::discord::context::{build_history, jev_lines};
use crate::llm::agent::TurnRequest;
use crate::prompts::files::{DECISION_REPLY_LENGTH, TOOL_TOO_LONG};
use crate::prompts::system::{SystemPrompt, WorkflowPrompt};
use crate::stats::event::{JevPurpose, LlmPurpose};

const REPLY_LENGTH_QUESTION: &str = "reply_length";

/// The number of rewrites that Nuki writes for one reply, at most.
const MAX_REWRITES: usize = 2;

/// Returns the reply that the bot sends for `draft`, the text of the LLM turn. While Jev finds
/// the reply too long, Nuki rewrites it, `MAX_REWRITES` times at most. Jev does not judge the
/// last rewrite. If a rewrite is empty, returns the reply before that rewrite.
pub async fn short_reply(
    deps: &WorkflowDeps,
    ctx: &TurnContext,
    workflow_prompt: WorkflowPrompt,
    draft: String,
) -> anyhow::Result<String> {
    let mut reply = draft;
    for _ in 0..MAX_REWRITES {
        if !is_too_long(deps, ctx, &reply).await? {
            return Ok(reply);
        }
        let rewrite = rewrite(deps, ctx, workflow_prompt, &reply).await?;
        if rewrite.is_empty() {
            return Ok(reply);
        }
        reply = rewrite;
    }
    Ok(reply)
}

/// Asks Jev if `reply` is too long. Jev gets the conversation as the classifier gets it, and
/// the reply as `draft`. The reply is too long if Jev's probability is not below the threshold.
async fn is_too_long(deps: &WorkflowDeps, ctx: &TurnContext, reply: &str) -> anyhow::Result<bool> {
    let lines = jev_lines(&ctx.history);
    let (history_lines, unit_lines) = lines.split_at(lines.len() - ctx.unit_len);
    let state = json!({
        "messages": history_lines,
        "new_messages": unit_lines,
        "draft": reply,
    });
    let question = Question::Noul {
        instructions: DECISION_REPLY_LENGTH.trim().to_string(),
    };
    let questions = indexmap! { REPLY_LENGTH_QUESTION.to_string() => question };
    let mut answers = deps
        .jev
        .decide(state, questions, &ctx.stats, JevPurpose::ReplyLength)
        .await?;
    match answers.shift_remove(REPLY_LENGTH_QUESTION) {
        Some(Answer::Noul { noul }) => Ok(noul >= deps.threshold_too_long),
        Some(_) => {
            bail!("Jev returned an answer to the reply length question that is not a probability")
        }
        None => bail!("Jev returned no answer to the reply length question"),
    }
}

/// Asks Nuki for a shorter text of `reply`, in a model request that has no tools. The request
/// has the history without the stored tool turns, then `reply` as Nuki's message, then the
/// instruction of `prompts/tools/too-long.md`. The request must contain no tool call and no tool
/// result, because it declares no tools. Nuki cannot send a second report, because she has no
/// tools.
async fn rewrite(
    deps: &WorkflowDeps,
    ctx: &TurnContext,
    workflow_prompt: WorkflowPrompt,
    reply: &str,
) -> anyhow::Result<String> {
    let history = build_history(&ctx.history, &HashMap::new())?
        .into_iter()
        .chain([Message::assistant(reply)])
        .collect();
    let request = TurnRequest {
        model_id: deps.model.current(),
        system_prompt: SystemPrompt::build(workflow_prompt, &ctx.runtime),
        history,
        prompt: Message::user(TOOL_TOO_LONG.trim()),
        sandbox: None,
        purpose: LlmPurpose::Rewrite,
        stats: ctx.stats.clone(),
    };
    Ok(deps.llm.run(request).await?.text)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::Utc;
    use serde_json::{Value, json};
    use wiremock::MockServer;

    use super::*;
    use crate::decisions::client::JEV_MODEL;
    use crate::decisions::test_support::{
        jev_request_bodies, mount_jev_answers, without_instructions,
    };
    use crate::discord::test_support::{nuki_message, user_message};
    use crate::llm::test_support::{
        examples_reminder, llm_request_bodies, llm_text_reply, messages_after_system_prompt,
        mount_llm_replies,
    };
    use crate::prompts::files::TOOL_TOO_LONG;
    use crate::prompts::system::SystemPrompt;
    use crate::routing::test_support::category_answer;
    use crate::stats::test_support::{events_between, run_event, test_run};
    use crate::workflows::test_support::{
        SHORT_ENOUGH, TOO_LONG, context, deps, mount_too_long_probabilities, too_long_answer,
        user_turn_text,
    };

    const UNIT_MESSAGE: &str = "my playlist is 900 hours of vaporwave";
    const DRAFT: &str = "900 hours? Touch grass, then come back for hour 901.";
    const SHORTER: &str = "900 hours? Touch grass.";
    const SHORTEST: &str = "Touch grass.";
    const EMPTY_REPLY: &str = include_str!("../../tests/fixtures/llm/empty_final_text.json");

    /// Starts a server where Jev answers the reply length question with `probabilities`, and the
    /// LLM gives `rewrites`. Each server gives its responses in sequence.
    async fn server_with(probabilities: &[f64], rewrites: &[&str]) -> MockServer {
        let server = MockServer::start().await;
        mount_too_long_probabilities(&server, probabilities).await;
        mount_llm_replies(&server, rewrites).await;
        server
    }

    /// Runs the length check of `DRAFT`, in a conversation that has only `UNIT_MESSAGE`.
    async fn check_draft(server: &MockServer) -> anyhow::Result<String> {
        let ctx = context(vec![user_message(1, UNIT_MESSAGE)]).await;
        check_draft_in(server, &ctx).await
    }

    async fn check_draft_in(server: &MockServer, ctx: &TurnContext) -> anyhow::Result<String> {
        short_reply(
            &deps(server).await,
            ctx,
            WorkflowPrompt::Chat,
            DRAFT.to_string(),
        )
        .await
    }

    /// Returns the Jev request of `check_draft` for the reply `draft`, without the instructions.
    fn length_request(draft: &str) -> Value {
        json!({
            "model": JEV_MODEL,
            "state": {
                "messages": [],
                "new_messages": [user_turn_text(1, UNIT_MESSAGE)],
                "draft": draft,
            },
            "questions": { "reply_length": { "type": "noul" } }
        })
    }

    /// Returns the messages that the rewrite request of `check_draft` has after the system
    /// prompt, for the reply `draft`.
    fn rewrite_messages(draft: &str) -> Value {
        json!([
            { "role": "user", "content": user_turn_text(1, UNIT_MESSAGE) },
            { "role": "assistant", "content": draft },
            instruction_with_examples()
        ])
    }

    fn instruction_with_examples() -> Value {
        json!({
            "role": "user",
            "content": [
                { "type": "text", "text": TOOL_TOO_LONG.trim() },
                { "type": "text", "text": examples_reminder() }
            ]
        })
    }

    /// Returns a stored tool turn: one call of `run_script` and its result.
    fn stored_tool_turn() -> Vec<Value> {
        vec![
            json!({
                "role": "assistant",
                "content": [{
                    "type": "toolcall",
                    "id": { "provider": "call_1" },
                    "function": { "name": "run_script", "arguments": { "code": "return 1" } }
                }]
            }),
            json!({
                "role": "user",
                "content": [{
                    "type": "toolresult",
                    "call": { "provider": "call_1" },
                    "name": "run_script",
                    "content": [{ "type": "text", "text": r#"{"value":1}"# }]
                }]
            }),
        ]
    }

    #[tokio::test]
    async fn a_draft_with_a_probability_below_the_threshold_is_the_reply() {
        let server = server_with(&[0.49], &[&llm_text_reply(SHORTER)]).await;

        let reply = check_draft(&server).await.unwrap();

        assert_eq!(reply, DRAFT);
    }

    #[tokio::test]
    async fn a_draft_with_a_probability_at_the_threshold_gets_a_rewrite() {
        let server = server_with(&[0.5, SHORT_ENOUGH], &[&llm_text_reply(SHORTER)]).await;

        let reply = check_draft(&server).await.unwrap();

        assert_eq!(reply, SHORTER);
    }

    #[tokio::test]
    async fn a_draft_that_jev_accepts_makes_no_rewrite_request() {
        let server = server_with(&[SHORT_ENOUGH], &[&llm_text_reply(SHORTER)]).await;

        check_draft(&server).await.unwrap();

        assert_eq!(llm_request_bodies(&server).await, Vec::<Value>::new());
    }

    #[tokio::test]
    async fn jev_gets_the_conversation_and_the_draft() {
        let server = server_with(&[SHORT_ENOUGH], &[]).await;
        let ctx = TurnContext {
            unit_len: 2,
            ..context(vec![
                user_message(1, "anyone here?"),
                user_message(2, "hey nuki"),
                user_message(3, UNIT_MESSAGE),
            ])
            .await
        };

        check_draft_in(&server, &ctx).await.unwrap();

        let [request]: [Value; 1] = jev_request_bodies(&server).await.try_into().unwrap();
        assert_eq!(
            without_instructions(request),
            json!({
                "model": JEV_MODEL,
                "state": {
                    "messages": [user_turn_text(1, "anyone here?")],
                    "new_messages": [
                        user_turn_text(2, "hey nuki"),
                        user_turn_text(3, UNIT_MESSAGE)
                    ],
                    "draft": DRAFT,
                },
                "questions": { "reply_length": { "type": "noul" } }
            })
        );
    }

    #[tokio::test]
    async fn the_rewrite_that_jev_accepts_is_the_reply() {
        let server = server_with(
            &[TOO_LONG, SHORT_ENOUGH],
            &[&llm_text_reply(SHORTER), &llm_text_reply(SHORTEST)],
        )
        .await;

        let reply = check_draft(&server).await.unwrap();

        assert_eq!(reply, SHORTER);
    }

    #[tokio::test]
    async fn the_rewrite_request_has_the_history_without_tool_turns_then_the_draft_and_the_instruction()
     {
        let server = server_with(&[TOO_LONG, SHORT_ENOUGH], &[&llm_text_reply(SHORTER)]).await;
        let ctx = TurnContext {
            stored_turns: HashMap::from([(2, stored_tool_turn())]),
            ..context(vec![
                user_message(1, "hi nuki"),
                nuki_message(2, "Hi! What do you need?"),
                user_message(3, UNIT_MESSAGE),
            ])
            .await
        };

        short_reply(
            &deps(&server).await,
            &ctx,
            WorkflowPrompt::Help,
            DRAFT.to_string(),
        )
        .await
        .unwrap();

        let [request]: [Value; 1] = llm_request_bodies(&server).await.try_into().unwrap();
        assert_eq!(
            request["messages"],
            json!([
                {
                    "role": "system",
                    "content": [{
                        "type": "text",
                        "text": SystemPrompt::build(WorkflowPrompt::Help, &ctx.runtime),
                    }],
                },
                { "role": "user", "content": user_turn_text(1, "hi nuki") },
                { "role": "assistant", "content": "Hi! What do you need?" },
                { "role": "user", "content": user_turn_text(3, UNIT_MESSAGE) },
                { "role": "assistant", "content": DRAFT },
                instruction_with_examples()
            ])
        );
    }

    #[tokio::test]
    async fn the_rewrite_request_declares_no_tools() {
        let server = server_with(&[TOO_LONG, SHORT_ENOUGH], &[&llm_text_reply(SHORTER)]).await;

        check_draft(&server).await.unwrap();

        let [request]: [Value; 1] = llm_request_bodies(&server).await.try_into().unwrap();
        assert_eq!(request.get("tools"), None);
    }

    #[tokio::test]
    async fn the_second_rewrite_is_the_reply_when_jev_rejects_each_time() {
        let server = MockServer::start().await;
        mount_jev_answers(&server, too_long_answer(TOO_LONG)).await;
        mount_llm_replies(
            &server,
            &[
                &llm_text_reply(SHORTER),
                &llm_text_reply(SHORTEST),
                &llm_text_reply(SHORTER),
            ],
        )
        .await;

        let reply = check_draft(&server).await.unwrap();

        assert_eq!(reply, SHORTEST);
    }

    #[tokio::test]
    async fn nuki_rewrites_the_draft_and_then_the_first_rewrite_when_jev_rejects_each_time() {
        let server = MockServer::start().await;
        mount_jev_answers(&server, too_long_answer(TOO_LONG)).await;
        mount_llm_replies(
            &server,
            &[
                &llm_text_reply(SHORTER),
                &llm_text_reply(SHORTEST),
                &llm_text_reply(SHORTER),
            ],
        )
        .await;

        check_draft(&server).await.unwrap();

        let sent: Vec<Value> = llm_request_bodies(&server)
            .await
            .iter()
            .map(messages_after_system_prompt)
            .collect();
        assert_eq!(
            sent,
            vec![rewrite_messages(DRAFT), rewrite_messages(SHORTER)]
        );
    }

    #[tokio::test]
    async fn jev_judges_the_draft_and_the_first_rewrite_but_not_the_second_rewrite() {
        let server = MockServer::start().await;
        mount_jev_answers(&server, too_long_answer(TOO_LONG)).await;
        mount_llm_replies(
            &server,
            &[&llm_text_reply(SHORTER), &llm_text_reply(SHORTEST)],
        )
        .await;

        check_draft(&server).await.unwrap();

        let requests: Vec<Value> = jev_request_bodies(&server)
            .await
            .into_iter()
            .map(without_instructions)
            .collect();
        assert_eq!(
            requests,
            vec![length_request(DRAFT), length_request(SHORTER)]
        );
    }

    #[tokio::test]
    async fn an_empty_rewrite_keeps_the_draft() {
        let server = server_with(&[TOO_LONG], &[EMPTY_REPLY]).await;

        let reply = check_draft(&server).await.unwrap();

        assert_eq!(reply, DRAFT);
    }

    #[tokio::test]
    async fn an_empty_second_rewrite_keeps_the_first_rewrite() {
        let server = server_with(
            &[TOO_LONG, TOO_LONG],
            &[&llm_text_reply(SHORTER), EMPTY_REPLY],
        )
        .await;

        let reply = check_draft(&server).await.unwrap();

        assert_eq!(reply, SHORTER);
    }

    #[tokio::test]
    async fn the_check_fails_without_an_answer_to_the_reply_length_question() {
        let server = MockServer::start().await;
        mount_jev_answers(&server, json!({})).await;

        let error = check_draft(&server).await.unwrap_err();

        assert_eq!(
            error.to_string(),
            "Jev returned no answer to the reply length question"
        );
    }

    #[tokio::test]
    async fn the_check_fails_on_an_answer_that_is_not_a_probability() {
        let server = MockServer::start().await;
        let choice = category_answer(&[("help", 1.0)])["category"].clone();
        mount_jev_answers(&server, json!({ "reply_length": choice })).await;

        let error = check_draft(&server).await.unwrap_err();

        assert_eq!(
            error.to_string(),
            "Jev returned an answer to the reply length question that is not a probability"
        );
    }

    #[tokio::test]
    async fn the_check_records_the_jev_calls_and_the_rewrite_call_with_their_purposes() {
        let server = server_with(&[TOO_LONG, SHORT_ENOUGH], &[&llm_text_reply(SHORTER)]).await;
        let (store, stats) = test_run().await;
        let ctx = TurnContext {
            stats,
            ..context(vec![user_message(1, UNIT_MESSAGE)]).await
        };
        let before = Utc::now();

        check_draft_in(&server, &ctx).await.unwrap();

        let after = Utc::now();
        assert_eq!(
            events_between(&store, before, after).await,
            vec![
                run_event("jev.call", length_call(TOO_LONG)),
                run_event(
                    "llm.call",
                    json!({
                        "purpose": "rewrite",
                        "call_index": 1,
                        "requested_model": "test/model",
                        "response_model": "test/model",
                        "provider": null,
                        "response_id": "gen-text",
                        "input_tokens": 60,
                        "output_tokens": 6,
                        "total_tokens": 66,
                        "cached_input_tokens": null,
                        "cache_write_tokens": null,
                        "reasoning_tokens": null,
                        "cost_usd": null,
                        "cost_estimated": false,
                        "finish_reason": "stop",
                        "output": [{ "type": "text", "text": SHORTER }],
                    })
                ),
                run_event("jev.call", length_call(SHORT_ENOUGH)),
            ]
        );
    }

    /// Returns the data of the `jev.call` event of a length check that Jev answers with
    /// `probability`.
    fn length_call(probability: f64) -> Value {
        json!({
            "purpose": "reply_length",
            "questions": ["reply_length"],
            "answers": too_long_answer(probability),
            "model": "typesafe/jev-1.14-20261014",
            "input_tokens": null,
            "output_tokens": null,
            "cost_usd": null,
            "error": null,
        })
    }
}
