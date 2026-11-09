pub mod chat;
#[cfg(test)]
pub(crate) mod test_support;
mod timed;

pub use timed::timed;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context;

use crate::decisions::client::JevClient;
use crate::discord::attachments::ResolvedMessage;
use crate::discord::context::build_history;
use crate::llm::agent::{LlmClient, TurnRequest, TurnResult};
use crate::llm::selection::ActiveModel;
use crate::prompts::system::{RuntimeContext, SystemPrompt, WorkflowPrompt};
use crate::sandbox::ops::reports::ReportThread;
use crate::sandbox::runtime::Sandbox;
use crate::stats::recorder::RunStats;

/// All the data from Discord that a workflow uses for one turn.
pub struct TurnContext {
    pub channel_name: String,
    /// The history, oldest first. The messages of the unit are the last messages.
    pub history: Vec<ResolvedMessage>,
    /// The number of unit messages at the end of `history`.
    pub unit_len: usize,
    /// The stored tool turns, by the ID of Nuki's message that ended the turn.
    pub stored_turns: HashMap<u64, Vec<serde_json::Value>>,
    pub runtime: RuntimeContext,
    /// The sandbox of the `run_script` tool, for the server of the turn.
    pub sandbox: Arc<Sandbox>,
    /// The recorder of the run that the turn is part of.
    pub stats: RunStats,
    /// The thread of the report that `reports.send` sent in the turn, if any.
    pub report_thread: ReportThread,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub text: String,
    /// The tool calls and the tool results of the turn. The handler stores them with the sent
    /// message.
    pub turn_messages: Vec<serde_json::Value>,
    /// The thread of the report that the turn sent. If it is set, the reply goes to this thread,
    /// not to the channel of the unit.
    pub report_thread_id: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Reply(Reply),
    Nothing,
}

#[derive(Clone)]
pub struct WorkflowDeps {
    pub llm: Arc<LlmClient>,
    pub jev: Arc<JevClient>,
    pub model: Arc<ActiveModel>,
}

/// Runs the LLM step that all workflows share. The last message of the history is the prompt.
pub async fn llm_turn(
    deps: &WorkflowDeps,
    ctx: &TurnContext,
    workflow_prompt: WorkflowPrompt,
) -> anyhow::Result<TurnResult> {
    let mut history = build_history(&ctx.history, &ctx.stored_turns)?;
    let prompt = history
        .pop()
        .context("the turn has no message to use as the prompt")?;
    let request = TurnRequest {
        model_id: deps.model.current(),
        system_prompt: SystemPrompt::build(workflow_prompt, &ctx.runtime),
        history,
        prompt,
        sandbox: Some(Arc::clone(&ctx.sandbox)),
        stats: ctx.stats.clone(),
    };
    Ok(deps.llm.run(request).await?)
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::MockServer;

    use super::test_support::{context, deps, user_turn_text};
    use super::*;
    use crate::discord::test_support::{nuki_message, user_message};
    use crate::llm::test_support::{
        llm_request_bodies, messages_after_system_prompt, mount_llm_replies,
    };

    const FINAL_TEXT: &str = include_str!("../../tests/fixtures/llm/final_text.json");

    #[tokio::test]
    async fn llm_turn_uses_the_last_message_as_the_prompt() {
        let server = MockServer::start().await;
        mount_llm_replies(&server, &[FINAL_TEXT]).await;
        let ctx = context(vec![
            user_message(1, "hi nuki"),
            nuki_message(2, "Hi! What do you need?"),
            user_message(3, "how do I add a plugin?"),
        ])
        .await;

        llm_turn(&deps(&server).await, &ctx, WorkflowPrompt::Chat)
            .await
            .unwrap();

        let bodies = llm_request_bodies(&server).await;
        assert_eq!(bodies[0]["model"], "test/model");
        assert_eq!(
            messages_after_system_prompt(&bodies[0]),
            json!([
                { "role": "user", "content": user_turn_text(1, "hi nuki") },
                { "role": "assistant", "content": "Hi! What do you need?" },
                { "role": "user", "content": user_turn_text(3, "how do I add a plugin?") }
            ])
        );
    }
}
