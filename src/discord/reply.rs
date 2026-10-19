//! Does the Discord actions of an `Outcome`.

use std::time::Instant;

use chrono::Utc;
use serenity::builder::CreateMessage;
use serenity::model::id::{ChannelId, MessageId};

use crate::admin::reports::report_embed;
use crate::app::App;
use crate::routing::category::Category;
use crate::routing::trigger::Trigger;
use crate::store::intakes::Intake;
use crate::workflows::intake::report::Report;
use crate::workflows::{Outcome, Reply};

/// Discord requires a thread name of 1 to 100 characters.
const THREAD_NAME_CHARS: usize = 100;

/// Where and how the outcome of one turn goes to Discord.
pub struct AppliedTurn {
    pub trigger: Trigger,
    pub category: Option<Category>,
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
        Outcome::Reply(reply) => {
            send_reply(app, applied, reply).await?;
            start_cooldown_after_unsolicited_chat(app, applied);
            Ok(())
        }
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

/// Sends the text, and stores the tool turn with the ID of the sent message.
async fn send_reply(app: &App, applied: &AppliedTurn, reply: Reply) -> anyhow::Result<()> {
    let message = CreateMessage::new().content(reply.text);
    let message = match applied.reply_to {
        Some(message_id) => message.reference_message((applied.channel_id, message_id)),
        None => message,
    };
    let sent = applied.channel_id.send_message(&app.http, message).await?;
    if reply.turn_messages.is_empty() {
        return Ok(());
    }
    app.store
        .tool_turns()
        .save(
            applied.channel_id.get(),
            sent.id.get(),
            &reply.turn_messages,
            Utc::now(),
        )
        .await?;
    Ok(())
}

fn start_cooldown_after_unsolicited_chat(app: &App, applied: &AppliedTurn) {
    if applied.trigger == Trigger::Unsolicited && applied.category == Some(Category::Chat) {
        app.cooldowns
            .start(applied.channel_id.get(), Instant::now());
    }
}

/// Sends the report to the reports channel, then closes the intake.
async fn send_report(app: &App, intake: &Intake, report: &Report) -> anyhow::Result<()> {
    ChannelId::new(app.config.reports_channel_id)
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
}
