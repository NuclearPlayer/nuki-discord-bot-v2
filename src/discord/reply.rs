//! Applies each `Outcome` in Discord.

use chrono::Utc;
use serenity::builder::CreateMessage;
use serenity::http::Http;
use serenity::model::id::{ChannelId, MessageId};

use crate::app::App;
use crate::stats::event::{Event, ReplySent};
use crate::stats::recorder::RunStats;
use crate::store::Store;
use crate::workflows::{Outcome, Reply};

/// Discord requires a thread name of 1 to 100 characters.
const THREAD_NAME_CHARS: usize = 100;

/// Discord rejects a message with more than 2000 characters.
const MESSAGE_CHARS: usize = 2000;

/// The channel and the reply target for the outcome of one turn.
pub struct AppliedTurn {
    /// The channel where Nuki sends her message.
    pub channel_id: ChannelId,
    /// The message that Nuki's reply refers to.
    pub reply_to: MessageId,
}

/// Returns the start of the content on one line. If the content is empty, returns the author
/// name.
pub fn thread_name(content: &str, author_name: &str) -> String {
    let one_line = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.is_empty() {
        return author_name.to_string();
    }
    one_line.chars().take(THREAD_NAME_CHARS).collect()
}

/// Applies the outcome of a turn, in the run of `stats`.
pub async fn apply_outcome(
    app: &App,
    stats: &RunStats,
    applied: &AppliedTurn,
    outcome: Outcome,
) -> anyhow::Result<()> {
    match outcome {
        Outcome::Nothing => Ok(()),
        Outcome::Reply(reply) => send_reply(&app.http, &app.store, stats, applied, reply).await,
    }
}

/// Sends the text in parts that Discord accepts. Sends the first part as the reply, and the other
/// parts as new messages in the same channel, not as replies. Stores the tool turn with the ID
/// of the first message. Records the IDs of all parts.
async fn send_reply(
    http: &Http,
    store: &Store,
    stats: &RunStats,
    applied: &AppliedTurn,
    reply: Reply,
) -> anyhow::Result<()> {
    let mut parts = split_reply(&reply.text).into_iter();
    let first = parts.next().expect("split_reply returns at least one part");
    let message = CreateMessage::new()
        .content(first)
        .reference_message((applied.channel_id, applied.reply_to));
    let sent = applied.channel_id.send_message(http, message).await?;
    save_tool_turn(store, applied.channel_id, sent.id, &reply).await?;
    let mut message_ids = vec![sent.id.get()];
    for part in parts {
        let sent = applied
            .channel_id
            .send_message(http, CreateMessage::new().content(part))
            .await?;
        message_ids.push(sent.id.get());
    }
    let reply_sent = ReplySent {
        message_ids,
        reply_to: Some(applied.reply_to.get()),
        text: reply.text,
    };
    stats.record(None, Event::ReplySent(reply_sent)).await;
    Ok(())
}

/// Stores the tool messages of the reply with the ID of the message that starts the reply.
async fn save_tool_turn(
    store: &Store,
    channel_id: ChannelId,
    message_id: MessageId,
    reply: &Reply,
) -> anyhow::Result<()> {
    if reply.turn_messages.is_empty() {
        return Ok(());
    }
    store
        .tool_turns()
        .save(
            channel_id.get(),
            message_id.get(),
            &reply.turn_messages,
            Utc::now(),
        )
        .await?;
    Ok(())
}

/// Splits the text into parts of at most `MESSAGE_CHARS` characters. If possible, a part ends at
/// a line break. If a line is longer than the limit, splits the line at a character boundary.
/// Discards an empty line at the start of a part, because Discord rejects an empty message.
fn split_reply(text: &str) -> Vec<String> {
    text.split('\n')
        .flat_map(line_pieces)
        .fold(Vec::new(), |mut parts: Vec<String>, piece| {
            match parts.last_mut() {
                Some(part) if fits(part, &piece) => {
                    part.push('\n');
                    part.push_str(&piece);
                }
                Some(_) if piece.is_empty() => {}
                _ => parts.push(piece),
            }
            parts
        })
}

/// Divides the line into pieces of at most `MESSAGE_CHARS` characters. An empty line becomes one
/// empty piece.
fn line_pieces(line: &str) -> Vec<String> {
    if line.is_empty() {
        return vec![String::new()];
    }
    let chars: Vec<char> = line.chars().collect();
    chars
        .chunks(MESSAGE_CHARS)
        .map(|chunk| chunk.iter().collect())
        .collect()
}

/// Returns `true` if `part` has space for the piece on a new line.
fn fits(part: &str, piece: &str) -> bool {
    part.chars().count() + 1 + piece.chars().count() <= MESSAGE_CHARS
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    use super::*;
    use crate::stats::test_support::{events_between, run_event, test_run};
    use crate::test_support::{discord_http, json_response};

    const SENT_MESSAGE: &str = include_str!("../../tests/fixtures/discord/reply_sent_message.json");
    const SENT_MESSAGE_ID: &str = "SENT_MESSAGE_ID";
    const CHANNEL_ID: u64 = 1200000000000000003;

    /// Discord answers the first message with the ID `first_id`, and each subsequent message
    /// with `next_id`.
    async fn mount_sent_messages(discord: &MockServer, first_id: &str, next_id: &str) {
        let messages = format!("/api/v10/channels/{CHANNEL_ID}/messages");
        Mock::given(method("POST"))
            .and(path(&messages))
            .respond_with(json_response(
                200,
                &SENT_MESSAGE.replace(SENT_MESSAGE_ID, first_id),
            ))
            .up_to_n_times(1)
            .mount(discord)
            .await;
        Mock::given(method("POST"))
            .and(path(&messages))
            .respond_with(json_response(
                200,
                &SENT_MESSAGE.replace(SENT_MESSAGE_ID, next_id),
            ))
            .mount(discord)
            .await;
    }

    #[tokio::test]
    async fn send_reply_records_the_sent_message_ids_and_the_text() {
        let discord = MockServer::start().await;
        mount_sent_messages(&discord, "1100000000000000031", "1100000000000000032").await;
        let (store, stats) = test_run().await;
        let text = format!("{}\n{}", "a".repeat(1200), "b".repeat(900));
        let applied = AppliedTurn {
            channel_id: ChannelId::new(CHANNEL_ID),
            reply_to: MessageId::new(1100000000000000001),
        };
        let reply = Reply {
            text: text.clone(),
            turn_messages: vec![],
        };
        let before = Utc::now();

        send_reply(&discord_http(&discord), &store, &stats, &applied, reply)
            .await
            .unwrap();

        let after = Utc::now();
        assert_eq!(
            events_between(&store, before, after).await,
            vec![run_event(
                "reply.sent",
                json!({
                    "message_ids": [1100000000000000031_u64, 1100000000000000032_u64],
                    "reply_to": 1100000000000000001_u64,
                    "text": text,
                }),
            )]
        );
    }

    #[test]
    fn thread_name_takes_the_first_100_characters_on_one_line() {
        let content = format!(
            "Playback stops   after the first track\nwhen I use the  YouTube source. {}",
            "é".repeat(60)
        );

        assert_eq!(
            thread_name(&content, "Bob"),
            format!(
                "Playback stops after the first track when I use the YouTube source. {}",
                "é".repeat(32)
            )
        );
    }

    #[test]
    fn thread_name_uses_the_author_name_for_empty_content() {
        assert_eq!(thread_name("", "Bob"), "Bob");
    }

    #[test]
    fn a_short_reply_is_one_part() {
        assert_eq!(
            split_reply("Open Settings.\nPick a theme."),
            vec!["Open Settings.\nPick a theme."]
        );
    }

    #[test]
    fn a_long_reply_splits_at_line_breaks() {
        let first = "a".repeat(1200);
        let second = "b".repeat(900);
        let third = "c".repeat(1000);
        let text = format!("{first}\n{second}\n{third}");

        assert_eq!(
            split_reply(&text),
            vec![first, format!("{second}\n{third}")]
        );
    }

    #[test]
    fn a_line_longer_than_the_limit_splits_at_a_character_boundary() {
        let text = format!("intro\n{}", "é".repeat(4500));

        assert_eq!(
            split_reply(&text),
            vec![
                "intro".to_string(),
                "é".repeat(2000),
                "é".repeat(2000),
                "é".repeat(500),
            ]
        );
    }
}
