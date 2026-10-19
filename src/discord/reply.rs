//! Does the Discord actions of an `Outcome`.

use chrono::Utc;
use serenity::builder::CreateMessage;
use serenity::model::id::{ChannelId, MessageId};

use crate::admin::reports::report_embed;
use crate::app::App;
use crate::store::intakes::Intake;
use crate::workflows::intake::report::Report;
use crate::workflows::{Outcome, Reply};

/// Discord requires a thread name of 1 to 100 characters.
const THREAD_NAME_CHARS: usize = 100;

/// Discord rejects a message with more than 2000 characters.
const MESSAGE_CHARS: usize = 2000;

/// Where and how the outcome of one turn goes to Discord.
pub struct AppliedTurn {
    /// The channel where Nuki sends her message.
    pub channel_id: ChannelId,
    /// `None` only for the first turn of an intake in a new thread: the message of the user is
    /// in the parent channel, so Nuki sends a usual message in the thread.
    pub reply_to: Option<MessageId>,
}

/// The start of the content on one line, or the author name when the content is empty.
pub fn thread_name(content: &str, author_name: &str) -> String {
    let one_line = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.is_empty() {
        return author_name.to_string();
    }
    one_line.chars().take(THREAD_NAME_CHARS).collect()
}

/// Applies the outcome of a turn outside an intake. Only an intake turn has an intake outcome.
pub async fn apply_outcome(
    app: &App,
    applied: &AppliedTurn,
    outcome: Outcome,
) -> anyhow::Result<()> {
    match outcome {
        Outcome::Nothing => Ok(()),
        Outcome::Reply(reply) => send_reply(app, applied, reply).await,
        Outcome::Intake { .. } => {
            anyhow::bail!("a turn outside an intake returned an intake outcome")
        }
    }
}

/// Applies the outcome of a turn of `intake`: the reply, then the report, which closes the intake.
pub async fn apply_intake_outcome(
    app: &App,
    applied: &AppliedTurn,
    intake: &Intake,
    outcome: Outcome,
) -> anyhow::Result<()> {
    let Outcome::Intake { reply, report } = outcome else {
        return apply_outcome(app, applied, outcome).await;
    };
    if let Some(reply) = reply {
        send_reply(app, applied, reply).await?;
    }
    if let Some(report) = report {
        send_report(app, intake, &report).await?;
    }
    Ok(())
}

/// Sends the text in parts that Discord accepts: the first part as the reply, the rest as usual
/// messages in the same channel. Stores the tool turn with the ID of the first message.
async fn send_reply(app: &App, applied: &AppliedTurn, reply: Reply) -> anyhow::Result<()> {
    let mut parts = split_reply(&reply.text).into_iter();
    let first = parts.next().expect("split_reply returns at least one part");
    let message = CreateMessage::new().content(first);
    let message = match applied.reply_to {
        Some(message_id) => message.reference_message((applied.channel_id, message_id)),
        None => message,
    };
    let sent = applied.channel_id.send_message(&app.http, message).await?;
    save_tool_turn(app, applied.channel_id, sent.id, &reply).await?;
    for part in parts {
        applied
            .channel_id
            .send_message(&app.http, CreateMessage::new().content(part))
            .await?;
    }
    Ok(())
}

/// Stores the tool messages of the reply with the ID of the message that starts the reply.
async fn save_tool_turn(
    app: &App,
    channel_id: ChannelId,
    message_id: MessageId,
    reply: &Reply,
) -> anyhow::Result<()> {
    if reply.turn_messages.is_empty() {
        return Ok(());
    }
    app.store
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

/// Splits the text into parts of at most `MESSAGE_CHARS` characters. A part ends at a line break
/// where possible. A line longer than the limit splits at a character boundary. An empty line at
/// the start of a part is dropped, because Discord rejects an empty message.
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

/// The line in pieces of at most `MESSAGE_CHARS` characters. An empty line is one empty piece.
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

/// Whether the piece fits after `part` on a new line.
fn fits(part: &str, piece: &str) -> bool {
    part.chars().count() + 1 + piece.chars().count() <= MESSAGE_CHARS
}

/// Sends the report to the admin log channel, then closes the intake.
async fn send_report(app: &App, intake: &Intake, report: &Report) -> anyhow::Result<()> {
    ChannelId::new(app.config.admin_log_channel_id)
        .send_message(&app.http, CreateMessage::new().embed(report_embed(report)))
        .await?;
    app.store.intakes().mark_reported(intake.id).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
