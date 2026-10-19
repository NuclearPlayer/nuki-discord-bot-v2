use chrono::{DateTime, Utc};
use serenity::model::channel::{Attachment, Message};

#[derive(Clone, Debug, PartialEq)]
pub struct AttachmentMeta {
    pub id: u64,
    pub filename: String,
    pub content_type: Option<String>,
    pub size: u64,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChatMessage {
    pub id: u64,
    pub channel_id: u64,
    pub author_id: u64,
    pub author_name: String,
    pub author_roles: Vec<String>,
    pub is_nuki: bool,
    pub timestamp: DateTime<Utc>,
    pub content: String,
    pub attachments: Vec<AttachmentMeta>,
}

impl AttachmentMeta {
    fn from_serenity(attachment: &Attachment) -> Self {
        Self {
            id: attachment.id.get(),
            filename: attachment.filename.clone(),
            content_type: attachment.content_type.clone(),
            size: u64::from(attachment.size),
            url: attachment.url.clone(),
        }
    }
}

impl ChatMessage {
    pub fn from_serenity(
        message: &Message,
        author_name: String,
        author_roles: Vec<String>,
        nuki_user_id: u64,
    ) -> Self {
        Self {
            id: message.id.get(),
            channel_id: message.channel_id.get(),
            author_id: message.author.id.get(),
            author_name,
            author_roles,
            is_nuki: message.author.id.get() == nuki_user_id,
            timestamp: *message.timestamp,
            content: message.content.clone(),
            attachments: message
                .attachments
                .iter()
                .map(AttachmentMeta::from_serenity)
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    const MESSAGE: &str =
        include_str!("../../tests/fixtures/discord/context_message_with_attachment.json");

    #[test]
    fn from_serenity_maps_a_message_with_an_attachment() {
        let message: Message = serde_json::from_str(MESSAGE).unwrap();

        let chat_message = ChatMessage::from_serenity(
            &message,
            "Bobby".to_string(),
            vec!["Linux".to_string(), "supporter".to_string()],
            1700000000000000001,
        );

        assert_eq!(
            chat_message,
            ChatMessage {
                id: 1100000000000000002,
                channel_id: 1200000000000000001,
                author_id: 1300000000000000002,
                author_name: "Bobby".to_string(),
                author_roles: vec!["Linux".to_string(), "supporter".to_string()],
                is_nuki: false,
                timestamp: Utc.with_ymd_and_hms(2026, 10, 19, 12, 35, 0).unwrap()
                    + chrono::Duration::milliseconds(123),
                content: "<@1700000000000000001> the player crashes, log attached".to_string(),
                attachments: vec![AttachmentMeta {
                    id: 1400000000000000001,
                    filename: "crash.log".to_string(),
                    content_type: Some("text/plain; charset=utf-8".to_string()),
                    size: 4096,
                    url: "https://cdn.discordapp.com/attachments/1200000000000000001/1400000000000000001/crash.log".to_string(),
                }],
            }
        );
    }

    #[test]
    fn from_serenity_marks_a_message_of_nuki() {
        let message: Message = serde_json::from_str(MESSAGE).unwrap();

        let chat_message =
            ChatMessage::from_serenity(&message, "Nuki".to_string(), vec![], 1300000000000000002);

        assert_eq!(
            chat_message,
            ChatMessage {
                id: 1100000000000000002,
                channel_id: 1200000000000000001,
                author_id: 1300000000000000002,
                author_name: "Nuki".to_string(),
                author_roles: vec![],
                is_nuki: true,
                timestamp: Utc.with_ymd_and_hms(2026, 10, 19, 12, 35, 0).unwrap()
                    + chrono::Duration::milliseconds(123),
                content: "<@1700000000000000001> the player crashes, log attached".to_string(),
                attachments: vec![AttachmentMeta {
                    id: 1400000000000000001,
                    filename: "crash.log".to_string(),
                    content_type: Some("text/plain; charset=utf-8".to_string()),
                    size: 4096,
                    url: "https://cdn.discordapp.com/attachments/1200000000000000001/1400000000000000001/crash.log".to_string(),
                }],
            }
        );
    }
}
