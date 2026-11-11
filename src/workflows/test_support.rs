use std::collections::HashMap;
use std::sync::Arc;

use chrono::{TimeZone, Utc};
use serde_json::{Value, json};
use wiremock::MockServer;

use super::{TurnContext, WorkflowDeps};
use crate::decisions::client::JevClient;
use crate::decisions::test_support::mount_jev_answers_in_sequence;
use crate::discord::chat_message::ChatMessage;
use crate::discord::test_support::resolved;
use crate::llm::agent::LlmClient;
use crate::llm::selection::ActiveModel;
use crate::prompts::system::RuntimeContext;
use crate::sandbox::ops::reports::ReportThread;
use crate::sandbox::runtime::{Sandbox, SandboxLimits};
use crate::sandbox::test_support::echo_table;
use crate::stats::test_support::unrecorded_run;
use crate::store::Store;

/// Jev's probability for a reply that is short enough. It is below the threshold of `deps`.
pub(crate) const SHORT_ENOUGH: f64 = 0.1;
/// Jev's probability for a reply that is too long. It is above the threshold of `deps`.
pub(crate) const TOO_LONG: f64 = 0.9;

pub(crate) async fn deps(server: &MockServer) -> WorkflowDeps {
    let settings = Store::open_in_memory().await.unwrap().settings();
    WorkflowDeps {
        llm: Arc::new(LlmClient::new("test-key", &server.uri())),
        jev: Arc::new(JevClient::new("jev-key".to_string(), server.uri())),
        model: Arc::new(ActiveModel::load(settings, "test/model").await.unwrap()),
        threshold_too_long: 0.5,
    }
}

/// Returns Jev's answer to the reply length question, with `probability` that the reply is too
/// long.
pub(crate) fn too_long_answer(probability: f64) -> Value {
    json!({ "reply_length": { "type": "noul", "noul": probability } })
}

/// Mounts one Jev response to the reply length question for each item of `probabilities`. Jev
/// gives each response one time, in the sequence of `probabilities`.
pub(crate) async fn mount_too_long_probabilities(server: &MockServer, probabilities: &[f64]) {
    let answers: Vec<Value> = probabilities
        .iter()
        .map(|probability| too_long_answer(*probability))
        .collect();
    mount_jev_answers_in_sequence(server, &answers).await;
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
        report_thread: ReportThread::default(),
    }
}

fn runtime() -> RuntimeContext {
    RuntimeContext {
        bot_name: "Nuki".to_string(),
        bot_user_id: 1700000000000000001,
        emoji: vec![],
        channel_name: "general".to_string(),
        channel_id: 1200000000000000001,
        now: Utc.with_ymd_and_hms(2026, 10, 19, 12, 40, 0).unwrap(),
    }
}
