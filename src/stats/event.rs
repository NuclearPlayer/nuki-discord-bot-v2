use std::collections::BTreeMap;

use serde::Serialize;

use crate::routing::trigger::Trigger;
use crate::store::intakes::IntakeKind;

/// One action of the bot. The data of each variant is the `data` column of its row.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    MessageReceived(MessageReceived),
    UnitRouted(UnitRouted),
    JevCall(JevCall),
    LlmCall(LlmCall),
    ToolCall(ToolCall),
    WorkflowFinished(WorkflowFinished),
    ReplySent(ReplySent),
    ReactionAdded(ReactionAdded),
    EmojiCreated(EmojiCreated),
    ThreadCreated(ThreadCreated),
    IntakeOpened(IntakeOpened),
    IntakeReported(IntakeReported),
    ModelChanged(ModelChanged),
    ErrorRaised(ErrorRaised),
}

impl Event {
    pub fn kind(&self) -> &'static str {
        match self {
            Event::MessageReceived(_) => "message.received",
            Event::UnitRouted(_) => "unit.routed",
            Event::JevCall(_) => "jev.call",
            Event::LlmCall(_) => "llm.call",
            Event::ToolCall(_) => "tool.call",
            Event::WorkflowFinished(_) => "workflow.finished",
            Event::ReplySent(_) => "reply.sent",
            Event::ReactionAdded(_) => "reaction.added",
            Event::EmojiCreated(_) => "emoji.created",
            Event::ThreadCreated(_) => "thread.created",
            Event::IntakeOpened(_) => "intake.opened",
            Event::IntakeReported(_) => "intake.reported",
            Event::ModelChanged(_) => "model.changed",
            Event::ErrorRaised(_) => "error",
        }
    }

    pub fn data(&self) -> serde_json::Value {
        let data = match self {
            Event::MessageReceived(data) => serde_json::to_value(data),
            Event::UnitRouted(data) => serde_json::to_value(data),
            Event::JevCall(data) => serde_json::to_value(data),
            Event::LlmCall(data) => serde_json::to_value(data),
            Event::ToolCall(data) => serde_json::to_value(data),
            Event::WorkflowFinished(data) => serde_json::to_value(data),
            Event::ReplySent(data) => serde_json::to_value(data),
            Event::ReactionAdded(data) => serde_json::to_value(data),
            Event::EmojiCreated(data) => serde_json::to_value(data),
            Event::ThreadCreated(data) => serde_json::to_value(data),
            Event::IntakeOpened(data) => serde_json::to_value(data),
            Event::IntakeReported(data) => serde_json::to_value(data),
            Event::ModelChanged(data) => serde_json::to_value(data),
            Event::ErrorRaised(data) => serde_json::to_value(data),
        };
        // The data structs have only string map keys, so `to_value` cannot fail.
        data.expect("event data is JSON")
    }
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum JevPurpose {
    Category,
    HelpQuality,
    IntakeFields,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LlmPurpose {
    Turn,
    Extract,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowOutcome {
    Reply,
    Nothing,
    Intake,
    Error,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct MessageReceived {
    pub message_id: u64,
    pub trigger: Trigger,
    pub mentions_nuki: bool,
    pub mentions_nuki_role: bool,
    pub replies_to_nuki: bool,
    pub attachments: Vec<Attachment>,
    pub content: String,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Attachment {
    pub id: u64,
    pub filename: String,
    pub content_type: Option<String>,
    pub size: u64,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct UnitRouted {
    pub message_count: u64,
    pub probabilities: BTreeMap<String, f64>,
    pub decision: String,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct JevCall {
    pub purpose: JevPurpose,
    pub questions: Vec<String>,
    pub answers: serde_json::Value,
    pub model: Option<String>,
    pub duration_ms: u64,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
    pub error: Option<String>,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct LlmCall {
    pub purpose: LlmPurpose,
    pub call_index: u64,
    pub requested_model: String,
    pub response_model: Option<String>,
    pub provider: Option<String>,
    pub response_id: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
    pub cost_estimated: bool,
    pub finish_reason: Option<String>,
    pub duration_ms: u64,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub code: String,
    pub operations: Vec<String>,
    pub duration_ms: u64,
    pub error: Option<String>,
    pub result_bytes: u64,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct WorkflowFinished {
    pub workflow: String,
    pub outcome: WorkflowOutcome,
    pub duration_ms: u64,
    pub error: Option<String>,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct ReplySent {
    pub message_ids: Vec<u64>,
    pub reply_to: Option<u64>,
    pub text: String,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct ReactionAdded {
    pub message_id: u64,
    pub emoji: String,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct EmojiCreated {
    pub emoji_id: u64,
    pub name: String,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct ThreadCreated {
    pub thread_id: u64,
    pub message_id: u64,
    pub name: String,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct IntakeOpened {
    pub intake_id: i64,
    pub intake_kind: IntakeKind,
    pub thread_id: u64,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct IntakeReported {
    pub intake_id: i64,
    pub intake_kind: IntakeKind,
    pub complete: bool,
    pub missing: Vec<String>,
    pub by_timeout: bool,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct ModelChanged {
    pub previous: String,
    pub new: String,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct ErrorRaised {
    pub workflow: String,
    pub error: String,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;

    fn one_of_each_kind() -> Vec<Event> {
        vec![
            Event::MessageReceived(MessageReceived {
                message_id: 1,
                trigger: Trigger::Direct,
                mentions_nuki: true,
                mentions_nuki_role: false,
                replies_to_nuki: false,
                attachments: vec![],
                content: "hi".to_string(),
            }),
            Event::UnitRouted(UnitRouted {
                message_count: 1,
                probabilities: BTreeMap::new(),
                decision: "run chat".to_string(),
            }),
            Event::JevCall(JevCall {
                purpose: JevPurpose::Category,
                questions: vec![],
                answers: json!({}),
                model: None,
                duration_ms: 0,
                input_tokens: None,
                output_tokens: None,
                cost_usd: None,
                error: None,
            }),
            Event::LlmCall(llm_call()),
            Event::ToolCall(ToolCall {
                code: String::new(),
                operations: vec![],
                duration_ms: 0,
                error: None,
                result_bytes: 0,
            }),
            Event::WorkflowFinished(WorkflowFinished {
                workflow: "chat".to_string(),
                outcome: WorkflowOutcome::Reply,
                duration_ms: 0,
                error: None,
            }),
            Event::ReplySent(ReplySent {
                message_ids: vec![2],
                reply_to: None,
                text: "hello".to_string(),
            }),
            Event::ReactionAdded(ReactionAdded {
                message_id: 1,
                emoji: "👍".to_string(),
            }),
            Event::EmojiCreated(EmojiCreated {
                emoji_id: 3,
                name: "nuki".to_string(),
            }),
            Event::ThreadCreated(ThreadCreated {
                thread_id: 4,
                message_id: 1,
                name: "Bug".to_string(),
            }),
            Event::IntakeOpened(IntakeOpened {
                intake_id: 5,
                intake_kind: IntakeKind::Bug,
                thread_id: 4,
            }),
            Event::IntakeReported(IntakeReported {
                intake_id: 5,
                intake_kind: IntakeKind::Bug,
                complete: true,
                missing: vec![],
                by_timeout: false,
            }),
            Event::ModelChanged(ModelChanged {
                previous: "a/b".to_string(),
                new: "c/d".to_string(),
            }),
            Event::ErrorRaised(ErrorRaised {
                workflow: "help".to_string(),
                error: "failed".to_string(),
            }),
        ]
    }

    fn llm_call() -> LlmCall {
        LlmCall {
            purpose: LlmPurpose::Turn,
            call_index: 2,
            requested_model: "anthropic/claude-sonnet-4".to_string(),
            response_model: Some("anthropic/claude-sonnet-4-20250514".to_string()),
            provider: Some("Anthropic".to_string()),
            response_id: Some("gen-123".to_string()),
            input_tokens: Some(1200),
            output_tokens: Some(80),
            total_tokens: Some(1280),
            cached_input_tokens: Some(1000),
            cache_write_tokens: None,
            reasoning_tokens: Some(0),
            cost_usd: Some(0.0042),
            cost_estimated: false,
            finish_reason: Some("stop".to_string()),
            duration_ms: 950,
        }
    }

    #[test]
    fn kind_is_the_dotted_event_name() {
        let kinds: Vec<&str> = one_of_each_kind().iter().map(Event::kind).collect();

        assert_eq!(
            kinds,
            [
                "message.received",
                "unit.routed",
                "jev.call",
                "llm.call",
                "tool.call",
                "workflow.finished",
                "reply.sent",
                "reaction.added",
                "emoji.created",
                "thread.created",
                "intake.opened",
                "intake.reported",
                "model.changed",
                "error",
            ]
        );
    }

    #[test]
    fn data_is_the_fields_of_the_variant() {
        assert_eq!(
            Event::LlmCall(llm_call()).data(),
            json!({
                "purpose": "turn",
                "call_index": 2,
                "requested_model": "anthropic/claude-sonnet-4",
                "response_model": "anthropic/claude-sonnet-4-20250514",
                "provider": "Anthropic",
                "response_id": "gen-123",
                "input_tokens": 1200,
                "output_tokens": 80,
                "total_tokens": 1280,
                "cached_input_tokens": 1000,
                "cache_write_tokens": null,
                "reasoning_tokens": 0,
                "cost_usd": 0.0042,
                "cost_estimated": false,
                "finish_reason": "stop",
                "duration_ms": 950,
            })
        );
    }
}
