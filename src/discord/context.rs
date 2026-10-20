use std::collections::HashMap;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use chrono::SecondsFormat;
use rig_core::message::{Message, UserContent};
use serde::Deserialize;
use serde_json::Value;

use crate::discord::attachments::{ResolvedAttachment, ResolvedMessage};
use crate::discord::chat_message::ChatMessage;
use crate::discord::thread_marker::ThreadMarker;

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
        "roles: none".to_string()
    } else {
        format!("roles: {}", roles.join(", "))
    }
}

/// Returns one entry for each message. Each entry has the parts of `message_parts`, one part on
/// each line. For an attachment, the part is the label of the attachment.
pub fn jev_lines(messages: &[ResolvedMessage]) -> Vec<String> {
    messages
        .iter()
        .map(|resolved| {
            message_parts(
                resolved,
                |line| line,
                |attachment| vec![attachment.label().to_string()],
            )
            .join("\n")
        })
        .collect()
}

/// Returns the parts of a message in this sequence:
///
/// - the line of the thread that the message opens
/// - the header
/// - the parts of each attachment
/// - the line of the thread that the message started
///
/// `text` makes a part from a line. `attachment` makes the parts of an attachment.
fn message_parts<T>(
    resolved: &ResolvedMessage,
    text: impl Fn(String) -> T,
    attachment: impl Fn(&ResolvedAttachment) -> Vec<T>,
) -> Vec<T> {
    let marker = resolved.thread.as_ref();
    let opens = marker.and_then(opens_line).map(&text);
    let header_part = text(header(&resolved.message));
    let attachment_parts = resolved.attachments.iter().flat_map(attachment);
    let started = marker.and_then(started_line).map(&text);
    opens
        .into_iter()
        .chain([header_part])
        .chain(attachment_parts)
        .chain(started)
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

/// Nuki's messages become assistant turns. If Nuki's message has attachments or a thread marker,
/// it becomes a user turn, because an assistant turn cannot contain them.
fn turn(resolved: &ResolvedMessage) -> Message {
    let is_plain = resolved.attachments.is_empty() && resolved.thread.is_none();
    if resolved.message.is_nuki && is_plain {
        Message::assistant(resolved.message.content.clone())
    } else {
        user_turn(resolved)
    }
}

fn user_turn(resolved: &ResolvedMessage) -> Message {
    Message::from(message_parts(resolved, UserContent::text, attachment_parts))
}

/// Returns the thread header that goes before the header of the first message in a thread's
/// history.
fn opens_line(marker: &ThreadMarker) -> Option<String> {
    match marker {
        ThreadMarker::Opens {
            name,
            parent_channel_name,
            reply_count,
            has_starter,
        } => {
            let thread = format!(
                "thread \"{name}\" in #{parent_channel_name} · {}",
                replies_text(*reply_count)
            );
            let line = if *has_starter {
                format!("[{thread} · started from this message:]")
            } else {
                format!("[{thread}]")
            };
            Some(line)
        }
        ThreadMarker::Started { .. } => None,
    }
}

/// Returns the line that goes after the attachment labels, for a message that started a thread.
fn started_line(marker: &ThreadMarker) -> Option<String> {
    match marker {
        ThreadMarker::Started { name, reply_count } => Some(format!(
            "[this message started the thread \"{name}\" · {}]",
            replies_text(*reply_count)
        )),
        ThreadMarker::Opens { .. } => None,
    }
}

fn replies_text(count: u32) -> String {
    if count == 1 {
        "1 reply".to_string()
    } else {
        format!("{count} replies")
    }
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
    use rig_core::message::{
        AssistantContent, AudioMediaType, CallId, ProviderCallId, ToolName, ToolResultContent,
        VideoMediaType,
    };

    use super::*;
    use crate::discord::test_support::{nuki_message, resolved, user_message};

    fn started(reply_count: u32) -> Option<ThreadMarker> {
        Some(ThreadMarker::Started {
            name: "nuclear is playing wrong streams, what should I do?".to_string(),
            reply_count,
        })
    }

    fn opens(has_starter: bool) -> Option<ThreadMarker> {
        Some(ThreadMarker::Opens {
            name: "nuclear is playing wrong streams, what should I do?".to_string(),
            parent_channel_name: "general".to_string(),
            reply_count: 5,
            has_starter,
        })
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
            "[2026-10-19T12:35:00Z][msg:1100000000000000002][Bob (1300000000000000002) · roles: Linux, supporter]: <@1700000000000000001> it crashes"
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
            "[2026-10-19T12:35:00Z][msg:1100000000000000002][Bob (1300000000000000002) · roles: none]: hello"
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
                "[2026-10-19T12:35:00Z][msg:1][Bob (1300000000000000002) · roles: Linux, supporter]: hello"
                    .to_string(),
                "[2026-10-19T12:35:00Z][msg:2][Bob (1300000000000000002) · roles: Linux, supporter]: see these\n[attachment 10: cat.png]\n[attachment 11: player.log]"
                    .to_string(),
            ]
        );
    }

    #[test]
    fn the_jev_line_of_a_message_that_started_a_thread_ends_with_the_thread() {
        let messages = [ResolvedMessage {
            thread: started(5),
            ..resolved(
                user_message(1, "nuclear is playing wrong streams, what should I do?"),
                vec![image("[attachment 10: cat.png]")],
            )
        }];

        assert_eq!(
            jev_lines(&messages),
            vec![
                "[2026-10-19T12:35:00Z][msg:1][Bob (1300000000000000002) · roles: Linux, supporter]: nuclear is playing wrong streams, what should I do?\n[attachment 10: cat.png]\n[this message started the thread \"nuclear is playing wrong streams, what should I do?\" · 5 replies]"
                    .to_string(),
            ]
        );
    }

    #[test]
    fn a_thread_with_one_reply_says_reply() {
        let messages = [ResolvedMessage {
            thread: started(1),
            ..resolved(user_message(1, "hello"), vec![])
        }];

        assert_eq!(
            jev_lines(&messages),
            vec![
                "[2026-10-19T12:35:00Z][msg:1][Bob (1300000000000000002) · roles: Linux, supporter]: hello\n[this message started the thread \"nuclear is playing wrong streams, what should I do?\" · 1 reply]"
                    .to_string(),
            ]
        );
    }

    #[test]
    fn the_turn_of_a_message_that_started_a_thread_ends_with_the_thread() {
        let messages = [ResolvedMessage {
            thread: started(5),
            ..resolved(
                user_message(1, "see this"),
                vec![image("[attachment 10: cat.png]")],
            )
        }];

        let history = build_history(&messages, &HashMap::new()).unwrap();

        assert_eq!(
            history,
            vec![Message::from(vec![
                UserContent::text(
                    "[2026-10-19T12:35:00Z][msg:1][Bob (1300000000000000002) · roles: Linux, supporter]: see this"
                ),
                UserContent::text("[attachment 10: cat.png]"),
                UserContent::image_url(
                    "https://cdn.discordapp.com/attachments/1/2/cat.png",
                    None,
                    None
                ),
                UserContent::text(
                    "[this message started the thread \"nuclear is playing wrong streams, what should I do?\" · 5 replies]"
                ),
            ])]
        );
    }

    #[test]
    fn a_message_of_nuki_that_started_a_thread_is_a_user_turn() {
        let messages = [ResolvedMessage {
            thread: started(5),
            ..resolved(nuki_message(2, "Let's look at it in a thread."), vec![])
        }];

        let history = build_history(&messages, &HashMap::new()).unwrap();

        assert_eq!(
            history,
            vec![Message::from(vec![
                UserContent::text(
                    "[2026-10-19T12:35:00Z][msg:2][Nuki (1700000000000000001) · roles: none]: Let's look at it in a thread."
                ),
                UserContent::text(
                    "[this message started the thread \"nuclear is playing wrong streams, what should I do?\" · 5 replies]"
                ),
            ])]
        );
    }

    #[test]
    fn the_jev_line_of_the_starter_of_a_thread_starts_with_the_thread() {
        let messages = [ResolvedMessage {
            thread: opens(true),
            ..resolved(
                user_message(1, "nuclear is playing wrong streams, what should I do?"),
                vec![],
            )
        }];

        assert_eq!(
            jev_lines(&messages),
            vec![
                "[thread \"nuclear is playing wrong streams, what should I do?\" in #general · 5 replies · started from this message:]\n[2026-10-19T12:35:00Z][msg:1][Bob (1300000000000000002) · roles: Linux, supporter]: nuclear is playing wrong streams, what should I do?"
                    .to_string(),
            ]
        );
    }

    #[test]
    fn the_jev_line_of_the_first_message_of_a_thread_without_starter_starts_with_the_thread() {
        let messages = [ResolvedMessage {
            thread: opens(false),
            ..resolved(user_message(1, "any news?"), vec![])
        }];

        assert_eq!(
            jev_lines(&messages),
            vec![
                "[thread \"nuclear is playing wrong streams, what should I do?\" in #general · 5 replies]\n[2026-10-19T12:35:00Z][msg:1][Bob (1300000000000000002) · roles: Linux, supporter]: any news?"
                    .to_string(),
            ]
        );
    }

    #[test]
    fn the_turn_of_the_starter_of_a_thread_starts_with_the_thread() {
        let messages = [
            ResolvedMessage {
                thread: opens(true),
                ..resolved(
                    user_message(1, "nuclear is playing wrong streams, what should I do?"),
                    vec![],
                )
            },
            resolved(nuki_message(2, "Which version do you use?"), vec![]),
        ];

        let history = build_history(&messages, &HashMap::new()).unwrap();

        assert_eq!(
            history,
            vec![
                Message::from(vec![
                    UserContent::text(
                        "[thread \"nuclear is playing wrong streams, what should I do?\" in #general · 5 replies · started from this message:]"
                    ),
                    UserContent::text(
                        "[2026-10-19T12:35:00Z][msg:1][Bob (1300000000000000002) · roles: Linux, supporter]: nuclear is playing wrong streams, what should I do?"
                    ),
                ]),
                Message::assistant("Which version do you use?"),
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
                    "[2026-10-19T12:35:00Z][msg:1][Bob (1300000000000000002) · roles: Linux, supporter]: hello"
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
                    "[2026-10-19T12:35:00Z][msg:2][Nuki (1700000000000000001) · roles: none]: Here is the screenshot."
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
                    "[2026-10-19T12:35:00Z][msg:1][Bob (1300000000000000002) · roles: Linux, supporter]: what is one?"
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
                    "[2026-10-19T12:35:00Z][msg:1][Bob (1300000000000000002) · roles: Linux, supporter]: everything"
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
