use std::collections::HashMap;
use std::sync::Arc;

use chrono::{TimeZone, Utc};
use serde_json::{Value, json};
use wiremock::MockServer;

use super::{TurnContext, WorkflowDeps};
use crate::decisions::client::JevClient;
use crate::discord::attachments::ResolvedMessage;
use crate::discord::chat_message::ChatMessage;
use crate::llm::agent::LlmClient;
use crate::llm::selection::ActiveModel;
use crate::prompts::system::RuntimeContext;
use crate::routing::trigger::Trigger;
use crate::sandbox::runtime::{Sandbox, SandboxLimits};
use crate::sandbox::test_support::echo_table;
use crate::store::Store;

pub(crate) async fn deps(server: &MockServer) -> WorkflowDeps {
    let settings = Store::open_in_memory().await.unwrap().settings();
    WorkflowDeps {
        llm: Arc::new(LlmClient::new("test-key", &server.uri())),
        jev: Arc::new(JevClient::new("jev-key".to_string(), server.uri())),
        model: Arc::new(ActiveModel::load(settings, "test/model").await.unwrap()),
        help_quality_threshold: 0.7,
    }
}

pub(crate) const RUNTIME_SECTION: &str = "## Current context\n\
    \n\
    You are Nuki, with the user ID 1700000000000000001. Users mention you with <@1700000000000000001>.\n\
    Custom emoji of this server: none\n\
    Current channel: #general\n\
    Current time: 2026-10-19T12:40:00Z";

/// The Jev answers for the bug fields. The answers are not in the sequence of the file, and
/// `platform` is exactly one half.
pub(crate) fn bug_fields_missing() -> Value {
    json!({
        "item": { "type": "noul", "noul": 0.51 },
        "plugins": { "type": "noul", "noul": 0.2 },
        "version": { "type": "noul", "noul": 0.93 },
        "sources": { "type": "noul", "noul": 0.8 },
        "platform": { "type": "noul", "noul": 0.5 }
    })
}

pub(crate) fn user_turn_text(id: u64, content: &str) -> String {
    format!(
        "[2026-10-19T12:35:00Z][msg:{id}][Bob (1300000000000000002) · Linux, supporter]: {content}"
    )
}

pub(crate) fn context(trigger: Trigger, messages: Vec<ChatMessage>) -> TurnContext {
    TurnContext {
        trigger,
        channel_name: "general".to_string(),
        history: messages.into_iter().map(without_attachments).collect(),
        unit_len: 1,
        stored_turns: HashMap::new(),
        runtime: runtime(),
        sandbox: Arc::new(Sandbox::new(
            Arc::new(echo_table()),
            SandboxLimits::production(),
        )),
    }
}

fn without_attachments(message: ChatMessage) -> ResolvedMessage {
    ResolvedMessage {
        message,
        attachments: vec![],
    }
}

fn runtime() -> RuntimeContext {
    RuntimeContext {
        bot_name: "Nuki".to_string(),
        bot_user_id: 1700000000000000001,
        emoji: vec![],
        channel_name: "general".to_string(),
        now: Utc.with_ymd_and_hms(2026, 10, 19, 12, 40, 0).unwrap(),
    }
}
