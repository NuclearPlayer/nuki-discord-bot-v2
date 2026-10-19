use std::collections::HashMap;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use chrono::SecondsFormat;
use rig_core::message::{Message, UserContent};
use serde::Deserialize;
use serde_json::Value;

use crate::discord::attachments::{ResolvedAttachment, ResolvedMessage};
use crate::discord::chat_message::ChatMessage;

pub fn header(message: &ChatMessage) -> String {
    let timestamp = message.timestamp.to_rfc3339_opts(SecondsFormat::Secs, true);
    format!(
        "[{timestamp}][msg:{}][{} ({}) · {}]: {}",
        message.id,
        message.author_name,
        message.author_id,
        roles_text(&message.author_roles),
        message.content
    )
}

fn roles_text(roles: &[String]) -> String {
    if roles.is_empty() {
        "no roles".to_string()
    } else {
        roles.join(", ")
    }
}

/// One entry per message: the header, then the label of each attachment on its own line.
pub fn jev_lines(messages: &[ResolvedMessage]) -> Vec<String> {
    messages
        .iter()
        .map(|resolved| {
            std::iter::once(header(&resolved.message))
                .chain(
                    resolved
                        .attachments
                        .iter()
                        .map(|attachment| attachment.label().to_string()),
                )
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect()
}

pub fn build_history(
    messages: &[ResolvedMessage],
    stored_turns: &HashMap<u64, Vec<Value>>,
) -> Result<Vec<Message>, serde_json::Error> {
    let turns = messages
        .iter()
        .map(|resolved| {
            let stored = stored_turn(stored_turns, resolved.message.id)?;
            Ok(stored.into_iter().chain([turn(resolved)]))
        })
        .collect::<Result<Vec<_>, serde_json::Error>>()?;
    Ok(turns.into_iter().flatten().collect())
}

fn stored_turn(
    stored_turns: &HashMap<u64, Vec<Value>>,
    message_id: u64,
) -> Result<Vec<Message>, serde_json::Error> {
    stored_turns
        .get(&message_id)
        .into_iter()
        .flatten()
        .map(Message::deserialize)
        .collect()
}

/// A message of Nuki is an assistant turn, unless it has attachments: an assistant turn cannot contain them.
fn turn(resolved: &ResolvedMessage) -> Message {
    if resolved.message.is_nuki && resolved.attachments.is_empty() {
        Message::assistant(resolved.message.content.clone())
    } else {
        user_turn(resolved)
    }
}

fn user_turn(resolved: &ResolvedMessage) -> Message {
    let header_part = UserContent::text(header(&resolved.message));
    let attachment_parts = resolved.attachments.iter().flat_map(attachment_parts);
    Message::from(
        std::iter::once(header_part)
            .chain(attachment_parts)
            .collect::<Vec<_>>(),
    )
}

fn attachment_parts(attachment: &ResolvedAttachment) -> Vec<UserContent> {
    let label = UserContent::text(attachment.label());
    match attachment {
        ResolvedAttachment::Image { url, .. } => {
            vec![label, UserContent::image_url(url, None, None)]
        }
        ResolvedAttachment::Video {
            url, media_type, ..
        } => vec![label, UserContent::video_url(url, Some(media_type.clone()))],
        ResolvedAttachment::Audio {
            data, media_type, ..
        } => vec![
            label,
            UserContent::audio_base64(STANDARD.encode(data), Some(media_type.clone())),
        ],
        ResolvedAttachment::Text { content, .. } => vec![label, UserContent::text(content)],
        ResolvedAttachment::Other { .. } => vec![label],
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use rig_core::message::{
        AssistantContent, AudioMediaType, CallId, ProviderCallId, ToolName, ToolResultContent,
        VideoMediaType,
    };

    use super::*;

    fn user_message(id: u64, content: &str) -> ChatMessage {
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

    fn nuki_message(id: u64, content: &str) -> ChatMessage {
        ChatMessage {
            author_id: 1700000000000000001,
            author_name: "Nuki".to_string(),
            author_roles: vec![],
            is_nuki: true,
            ..user_message(id, content)
        }
    }

    fn resolved(message: ChatMessage, attachments: Vec<ResolvedAttachment>) -> ResolvedMessage {
        ResolvedMessage {
            message,
            attachments,
        }
    }

    fn image(label: &str) -> ResolvedAttachment {
        ResolvedAttachment::Image {
            label: label.to_string(),
            url: "https://cdn.discordapp.com/attachments/1/2/cat.png".to_string(),
        }
    }

    fn text(label: &str, content: &str) -> ResolvedAttachment {
        ResolvedAttachment::Text {
            label: label.to_string(),
            content: content.to_string(),
        }
    }

    #[test]
    fn header_has_the_timestamp_the_id_the_author_and_the_roles() {
        let message = user_message(1100000000000000002, "<@1700000000000000001> it crashes");

        assert_eq!(
            header(&message),
            "[2026-10-19T12:35:00Z][msg:1100000000000000002][Bob (1300000000000000002) · Linux, supporter]: <@1700000000000000001> it crashes"
        );
    }

    #[test]
    fn header_of_an_author_without_roles() {
        let message = ChatMessage {
            author_roles: vec![],
            ..user_message(1100000000000000002, "hello")
        };

        assert_eq!(
            header(&message),
            "[2026-10-19T12:35:00Z][msg:1100000000000000002][Bob (1300000000000000002) · no roles]: hello"
        );
    }

    #[test]
    fn jev_lines_contain_headers_and_labels_only() {
        let messages = [
            resolved(user_message(1, "hello"), vec![]),
            resolved(
                user_message(2, "see these"),
                vec![
                    image("[attachment 10: cat.png]"),
                    text("[attachment 11: player.log]", "crashed"),
                ],
            ),
        ];

        assert_eq!(
            jev_lines(&messages),
            vec![
                "[2026-10-19T12:35:00Z][msg:1][Bob (1300000000000000002) · Linux, supporter]: hello"
                    .to_string(),
                "[2026-10-19T12:35:00Z][msg:2][Bob (1300000000000000002) · Linux, supporter]: see these\n[attachment 10: cat.png]\n[attachment 11: player.log]"
                    .to_string(),
            ]
        );
    }

    #[test]
    fn messages_of_nuki_are_assistant_turns_without_header() {
        let messages = [
            resolved(user_message(1, "hello"), vec![]),
            resolved(nuki_message(2, "Hi! How can I help?"), vec![]),
        ];

        let history = build_history(&messages, &HashMap::new()).unwrap();

        assert_eq!(
            history,
            vec![
                Message::user(
                    "[2026-10-19T12:35:00Z][msg:1][Bob (1300000000000000002) · Linux, supporter]: hello"
                ),
                Message::assistant("Hi! How can I help?"),
            ]
        );
    }

    #[test]
    fn a_message_of_nuki_with_an_attachment_is_a_user_turn() {
        let messages = [resolved(
            nuki_message(2, "Here is the screenshot."),
            vec![image("[attachment 10: cat.png]")],
        )];

        let history = build_history(&messages, &HashMap::new()).unwrap();

        assert_eq!(
            history,
            vec![Message::from(vec![
                UserContent::text(
                    "[2026-10-19T12:35:00Z][msg:2][Nuki (1700000000000000001) · no roles]: Here is the screenshot."
                ),
                UserContent::text("[attachment 10: cat.png]"),
                UserContent::image_url(
                    "https://cdn.discordapp.com/attachments/1/2/cat.png",
                    None,
                    None
                ),
            ])]
        );
    }

    #[test]
    fn a_stored_turn_goes_before_the_message_of_nuki() {
        let stored_turns = HashMap::from([(
            2,
            vec![
                serde_json::json!({
                    "role": "assistant",
                    "content": [{
                        "type": "toolcall",
                        "id": { "provider": "call_1" },
                        "function": { "name": "run_script", "arguments": { "code": "return 1" } }
                    }]
                }),
                serde_json::json!({
                    "role": "user",
                    "content": [{
                        "type": "toolresult",
                        "call": { "provider": "call_1" },
                        "name": "run_script",
                        "content": [{ "type": "text", "text": r#"{"value":1}"# }]
                    }]
                }),
            ],
        )]);
        let messages = [
            resolved(user_message(1, "what is one?"), vec![]),
            resolved(nuki_message(2, "It is 1."), vec![]),
        ];

        let history = build_history(&messages, &stored_turns).unwrap();

        assert_eq!(
            history,
            vec![
                Message::user(
                    "[2026-10-19T12:35:00Z][msg:1][Bob (1300000000000000002) · Linux, supporter]: what is one?"
                ),
                Message::from(AssistantContent::tool_call(
                    "call_1",
                    ToolName::new("run_script").unwrap(),
                    serde_json::json!({ "code": "return 1" }),
                )),
                Message::from(UserContent::tool_result(
                    CallId::from(ProviderCallId::new("call_1").unwrap()),
                    ToolName::new("run_script").unwrap(),
                    vec![ToolResultContent::text(r#"{"value":1}"#)],
                )),
                Message::assistant("It is 1."),
            ]
        );
    }

    #[test]
    fn attachments_become_a_label_and_a_part() {
        let audio_bytes = vec![0x4f, 0x67, 0x67, 0x53];
        let messages = [resolved(
            user_message(1, "everything"),
            vec![
                image("[attachment 10: cat.png]"),
                ResolvedAttachment::Video {
                    label: "[attachment 11: clip.mp4]".to_string(),
                    url: "https://cdn.discordapp.com/attachments/1/2/clip.mp4".to_string(),
                    media_type: VideoMediaType::MP4,
                },
                ResolvedAttachment::Audio {
                    label: "[attachment 12: voice-message.ogg]".to_string(),
                    data: audio_bytes.clone(),
                    media_type: AudioMediaType::OGG,
                },
                text("[attachment 13: player.log]", "crashed"),
                ResolvedAttachment::Other {
                    label: "[attachment 14: archive.zip, 2048 bytes, application/zip]".to_string(),
                },
            ],
        )];

        let history = build_history(&messages, &HashMap::new()).unwrap();

        assert_eq!(
            history,
            vec![Message::from(vec![
                UserContent::text(
                    "[2026-10-19T12:35:00Z][msg:1][Bob (1300000000000000002) · Linux, supporter]: everything"
                ),
                UserContent::text("[attachment 10: cat.png]"),
                UserContent::image_url(
                    "https://cdn.discordapp.com/attachments/1/2/cat.png",
                    None,
                    None
                ),
                UserContent::text("[attachment 11: clip.mp4]"),
                UserContent::video_url(
                    "https://cdn.discordapp.com/attachments/1/2/clip.mp4",
                    Some(VideoMediaType::MP4)
                ),
                UserContent::text("[attachment 12: voice-message.ogg]"),
                UserContent::audio_base64(STANDARD.encode(&audio_bytes), Some(AudioMediaType::OGG)),
                UserContent::text("[attachment 13: player.log]"),
                UserContent::text("crashed"),
                UserContent::text("[attachment 14: archive.zip, 2048 bytes, application/zip]"),
            ])]
        );
    }
}
