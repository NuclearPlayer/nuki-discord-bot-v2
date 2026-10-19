use chrono::{TimeZone, Utc};

use crate::discord::chat_message::ChatMessage;

pub(crate) fn user_message(id: u64, content: &str) -> ChatMessage {
    ChatMessage {
        id,
        channel_id: 1200000000000000001,
        author_id: 1300000000000000002,
        author_name: "Bob".to_string(),
        author_roles: vec!["Linux".to_string(), "supporter".to_string()],
        is_nuki: false,
        timestamp: Utc.with_ymd_and_hms(2026, 10, 19, 12, 35, 0).unwrap()
            + chrono::Duration::milliseconds(123),
        content: content.to_string(),
        attachments: vec![],
    }
}

pub(crate) fn nuki_message(id: u64, content: &str) -> ChatMessage {
    ChatMessage {
        author_id: 1700000000000000001,
        author_name: "Nuki".to_string(),
        author_roles: vec![],
        is_nuki: true,
        ..user_message(id, content)
    }
}
