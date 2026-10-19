use chrono::SecondsFormat;
use serde::Serialize;
use serenity::model::channel::{Attachment, Message};
use serenity::model::guild::Emoji;

/// A message as scripts see it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MessageView {
    id: String,
    channel_id: String,
    author: AuthorView,
    content: String,
    timestamp: String,
    attachments: Vec<AttachmentView>,
    reply_to: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthorView {
    id: String,
    name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AttachmentView {
    id: String,
    filename: String,
    content_type: Option<String>,
    size: u32,
}

impl From<&Message> for MessageView {
    fn from(message: &Message) -> Self {
        Self {
            id: message.id.to_string(),
            channel_id: message.channel_id.to_string(),
            author: AuthorView {
                id: message.author.id.to_string(),
                name: message.author.name.clone(),
            },
            content: message.content.clone(),
            timestamp: message.timestamp.to_rfc3339_opts(SecondsFormat::Secs, true),
            attachments: message
                .attachments
                .iter()
                .map(AttachmentView::from)
                .collect(),
            reply_to: message
                .message_reference
                .as_ref()
                .and_then(|reference| reference.message_id)
                .map(|id| id.to_string()),
        }
    }
}

impl From<&Attachment> for AttachmentView {
    fn from(attachment: &Attachment) -> Self {
        Self {
            id: attachment.id.to_string(),
            filename: attachment.filename.clone(),
            content_type: attachment.content_type.clone(),
            size: attachment.size,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SearchResult {
    pub(super) total_results: u64,
    pub(super) messages: Vec<MessageView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct EmojiView {
    id: String,
    name: String,
    /// The text that shows the emoji in a message: `<:name:id>` or `<a:name:id>`.
    text: String,
}

impl From<&Emoji> for EmojiView {
    fn from(emoji: &Emoji) -> Self {
        Self {
            id: emoji.id.to_string(),
            name: emoji.name.clone(),
            text: emoji.to_string(),
        }
    }
}
