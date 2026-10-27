use std::collections::HashMap;
use std::sync::Arc;

use chrono::{TimeZone, Utc};
use serde_json::{Map, Value, json};
use wiremock::MockServer;

use super::{TurnContext, WorkflowDeps};
use crate::decisions::client::JevClient;
use crate::discord::chat_message::ChatMessage;
use crate::discord::test_support::resolved;
use crate::llm::agent::LlmClient;
use crate::llm::selection::ActiveModel;
use crate::prompts::questions::bug_fields;
use crate::prompts::system::RuntimeContext;
use crate::sandbox::runtime::{Sandbox, SandboxLimits};
use crate::sandbox::test_support::echo_table;
use crate::stats::test_support::unrecorded_run;
use crate::store::Store;

pub(crate) async fn deps(server: &MockServer) -> WorkflowDeps {
    let settings = Store::open_in_memory().await.unwrap().settings();
    WorkflowDeps {
        llm: Arc::new(LlmClient::new("test-key", &server.uri())),
        jev: Arc::new(JevClient::new("jev-key".to_string(), server.uri())),
        model: Arc::new(ActiveModel::load(settings, "test/model").await.unwrap()),
    }
}

/// Returns the names of the bug fields, in the file's sequence. The tests do not copy the names,
/// because the names are prompt text.
pub(crate) fn bug_field_names() -> Vec<String> {
    bug_fields().keys().cloned().collect()
}

/// Returns Jev's answers for the bug fields, in the opposite of the file's sequence. The answer
/// for the field at `index` is `probability(index)`.
pub(crate) fn bug_field_answers(probability: impl Fn(usize) -> f64) -> Value {
    let answers: Map<String, Value> = bug_field_names()
        .into_iter()
        .enumerate()
        .rev()
        .map(|(index, name)| (name, json!({ "type": "noul", "noul": probability(index) })))
        .collect();
    Value::Object(answers)
}

/// Returns Jev's answers for the bug fields, with the second and the third field missing. The
/// answer for the second field is equal to one half. From the fourth field, the answers are a
/// little above one half.
pub(crate) fn bug_fields_missing() -> Value {
    bug_field_answers(|index| match index {
        0 => 0.93,
        1 => 0.5,
        2 => 0.2,
        _ => 0.51,
    })
}

/// Returns the names of the fields that `bug_fields_missing` gives as missing.
pub(crate) fn missing_bug_field_names() -> Vec<String> {
    bug_field_names()[1..3].to_vec()
}

pub(crate) fn user_turn_text(id: u64, content: &str) -> String {
    format!(
        "[2026-10-19T12:35:00Z][msg:{id}][Bob (1300000000000000002) · roles: Linux, supporter]: {content}"
    )
}

pub(crate) async fn context(messages: Vec<ChatMessage>) -> TurnContext {
    TurnContext {
        channel_name: "general".to_string(),
        history: messages
            .into_iter()
            .map(|message| resolved(message, vec![]))
            .collect(),
        unit_len: 1,
        stored_turns: HashMap::new(),
        runtime: runtime(),
        sandbox: Arc::new(Sandbox::new(
            Arc::new(echo_table()),
            SandboxLimits::production(),
        )),
        stats: unrecorded_run().await,
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
