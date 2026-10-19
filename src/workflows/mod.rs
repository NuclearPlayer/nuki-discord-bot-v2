pub mod chat;
pub mod help;
pub mod intake;
#[cfg(test)]
pub(crate) mod test_support;

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use anyhow::Context;
use rig_core::message::Message;

use crate::decisions::client::JevClient;
use crate::discord::attachments::ResolvedMessage;
use crate::discord::context::build_history;
use crate::llm::agent::{LlmClient, TurnRequest, TurnResult};
use crate::llm::selection::ActiveModel;
use crate::prompts::system::{RuntimeContext, SystemPrompt, WorkflowPrompt};
use crate::routing::trigger::Trigger;
use crate::sandbox::runtime::Sandbox;
use intake::report::Report;

/// Everything a workflow needs from Discord for one turn.
pub struct TurnContext {
    pub trigger: Trigger,
    pub channel_name: String,
    /// The history, oldest first. The messages of the unit are the last messages.
    pub history: Vec<ResolvedMessage>,
    /// The number of messages of the unit at the end of `history`.
    pub unit_len: usize,
    /// The stored tool turns, by the ID of the message of Nuki that ended the turn.
    pub stored_turns: HashMap<u64, Vec<serde_json::Value>>,
    pub runtime: RuntimeContext,
    /// The sandbox of the `run_script` tool, for the server of the turn.
    pub sandbox: Arc<Sandbox>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub text: String,
    /// The tool calls and results of the turn, which the handler stores with the sent message.
    pub turn_messages: Vec<serde_json::Value>,
}

impl From<TurnResult> for Reply {
    fn from(result: TurnResult) -> Reply {
        Reply {
            text: result.text,
            turn_messages: result.turn_messages,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
#[allow(
    clippy::large_enum_variant,
    reason = "one outcome per turn; a box would only make the matches harder to read"
)]
pub enum Outcome {
    Reply(Reply),
    Nothing,
    Intake {
        reply: Option<Reply>,
        report: Option<Report>,
    },
}

#[derive(Clone)]
pub struct WorkflowDeps {
    pub llm: Arc<LlmClient>,
    pub jev: Arc<JevClient>,
    pub model: Arc<ActiveModel>,
    pub help_quality_threshold: f64,
}

pub trait Workflow {
    fn run(&self, ctx: &TurnContext) -> impl Future<Output = anyhow::Result<Outcome>> + Send;
}

pub struct TurnOptions<'a> {
    /// Text for the system prompt, after the workflow prompt.
    pub extra: Option<&'a str>,
    /// Without it, the last message of the history is the prompt. With it, the full
    /// history is the history and the prompt is a user message with the instruction.
    pub instruction: Option<&'a str>,
    /// Gives the model the `run_script` tool.
    pub with_tools: bool,
}

/// The shared LLM step of all workflows.
pub async fn llm_turn(
    deps: &WorkflowDeps,
    ctx: &TurnContext,
    workflow_prompt: WorkflowPrompt,
    options: TurnOptions<'_>,
) -> anyhow::Result<TurnResult> {
    let messages = build_history(&ctx.history, &ctx.stored_turns)?;
    let (history, prompt) = split_prompt(messages, options.instruction)?;
    let request = TurnRequest {
        model: deps.model.current(),
        system_prompt: SystemPrompt::build(workflow_prompt, options.extra, &ctx.runtime),
        history,
        prompt,
        sandbox: options.with_tools.then(|| Arc::clone(&ctx.sandbox)),
    };
    Ok(deps.llm.run(request).await?)
}

fn split_prompt(
    mut messages: Vec<Message>,
    instruction: Option<&str>,
) -> anyhow::Result<(Vec<Message>, Message)> {
    match instruction {
        Some(text) => Ok((messages, Message::user(text))),
        None => {
            let prompt = messages
                .pop()
                .context("the turn has no message to use as the prompt")?;
            Ok((messages, prompt))
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::MockServer;

    use super::test_support::{RUNTIME_SECTION, context, deps, user_turn_text};
    use super::*;
    use crate::discord::test_support::{nuki_message, user_message};
    use crate::llm::test_support::{llm_request_bodies, mount_llm_replies};
    use crate::prompts::files::{KNOWLEDGE, NUCLEAR, PERSONALITY, WORKFLOW_HELP};

    const FINAL_TEXT: &str = include_str!("../../tests/fixtures/llm/final_text.json");

    #[tokio::test]
    async fn llm_turn_uses_the_last_message_as_the_prompt() {
        let server = MockServer::start().await;
        mount_llm_replies(&server, &[FINAL_TEXT]).await;
        let ctx = context(
            Trigger::Direct,
            vec![
                user_message(1, "hi nuki"),
                nuki_message(2, "Hi! What do you need?"),
                user_message(3, "how do I add a plugin?"),
            ],
        );
        let options = TurnOptions {
            extra: Some("Missing fields: version."),
            instruction: None,
            with_tools: false,
        };

        llm_turn(&deps(&server).await, &ctx, WorkflowPrompt::Help, options)
            .await
            .unwrap();

        let expected_prompt = [
            PERSONALITY.trim(),
            NUCLEAR.trim(),
            KNOWLEDGE.trim(),
            WORKFLOW_HELP.trim(),
            "Missing fields: version.",
            RUNTIME_SECTION,
        ]
        .join("\n\n");
        let bodies = llm_request_bodies(&server).await;
        assert_eq!(bodies[0]["model"], "test/model");
        assert_eq!(
            bodies[0]["messages"],
            json!([
                {
                    "role": "system",
                    "content": [{ "type": "text", "text": expected_prompt }]
                },
                { "role": "user", "content": user_turn_text(1, "hi nuki") },
                { "role": "assistant", "content": "Hi! What do you need?" },
                { "role": "user", "content": user_turn_text(3, "how do I add a plugin?") }
            ])
        );
    }

    #[tokio::test]
    async fn llm_turn_sends_the_instruction_as_the_prompt() {
        let server = MockServer::start().await;
        mount_llm_replies(&server, &[FINAL_TEXT]).await;
        let ctx = context(
            Trigger::IntakeThread,
            vec![
                user_message(1, "playback stops"),
                nuki_message(2, "Which version do you use?"),
            ],
        );
        let options = TurnOptions {
            extra: None,
            instruction: Some("Write the final checks now."),
            with_tools: false,
        };

        llm_turn(&deps(&server).await, &ctx, WorkflowPrompt::Help, options)
            .await
            .unwrap();

        let expected_prompt = [
            PERSONALITY.trim(),
            NUCLEAR.trim(),
            KNOWLEDGE.trim(),
            WORKFLOW_HELP.trim(),
            RUNTIME_SECTION,
        ]
        .join("\n\n");
        let bodies = llm_request_bodies(&server).await;
        assert_eq!(
            bodies[0]["messages"],
            json!([
                {
                    "role": "system",
                    "content": [{ "type": "text", "text": expected_prompt }]
                },
                { "role": "user", "content": user_turn_text(1, "playback stops") },
                { "role": "assistant", "content": "Which version do you use?" },
                { "role": "user", "content": "Write the final checks now." }
            ])
        );
    }
}
