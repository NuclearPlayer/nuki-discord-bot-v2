//! Does the Discord actions of an `Outcome`.

use chrono::Utc;
use serenity::builder::CreateMessage;
use serenity::http::Http;
use serenity::model::id::{ChannelId, MessageId};

use crate::admin::reports::report_embed;
use crate::app::App;
use crate::stats::event::{Event, IntakeReported, ReplySent};
use crate::stats::recorder::RunStats;
use crate::store::Store;
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

/// What completes an intake and sends its report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportedBy {
    /// The conversation in the thread gave all fields.
    Reporter,
    /// The reporter did not answer for the intake timeout.
    Timeout,
}

/// The start of the content on one line, or the author name when the content is empty.
pub fn thread_name(content: &str, author_name: &str) -> String {
    let one_line = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.is_empty() {
        return author_name.to_string();
    }
    one_line.chars().take(THREAD_NAME_CHARS).collect()
}

/// Applies the outcome of a turn outside an intake, in the run of `stats`. Only an intake turn
/// has an intake outcome.
pub async fn apply_outcome(
    app: &App,
    stats: &RunStats,
    applied: &AppliedTurn,
    outcome: Outcome,
) -> anyhow::Result<()> {
    match outcome {
        Outcome::Nothing => Ok(()),
        Outcome::Reply(reply) => send_reply(&app.http, &app.store, stats, applied, reply).await,
        Outcome::Intake { .. } => {
            anyhow::bail!("a turn outside an intake returned an intake outcome")
        }
    }
}

/// Applies the outcome of a turn of `intake`: the reply, then the report, which closes the intake.
pub async fn apply_intake_outcome(
    app: &App,
    stats: &RunStats,
    applied: &AppliedTurn,
    intake: &Intake,
    outcome: Outcome,
    reported_by: ReportedBy,
) -> anyhow::Result<()> {
    let Outcome::Intake { reply, report } = outcome else {
        return apply_outcome(app, stats, applied, outcome).await;
    };
    if let Some(reply) = reply {
        send_reply(&app.http, &app.store, stats, applied, reply).await?;
    }
    if let Some(report) = report {
        let admin_log = ChannelId::new(app.config.admin_log_channel_id);
        send_report(
            &app.http,
            &app.store,
            admin_log,
            stats,
            intake,
            &report,
            reported_by,
        )
        .await?;
    }
    Ok(())
}

/// Sends the text in parts that Discord accepts: the first part as the reply, the rest as usual
/// messages in the same channel. Stores the tool turn with the ID of the first message, and
/// records the IDs of all parts.
async fn send_reply(
    http: &Http,
    store: &Store,
    stats: &RunStats,
    applied: &AppliedTurn,
    reply: Reply,
) -> anyhow::Result<()> {
    let mut parts = split_reply(&reply.text).into_iter();
    let first = parts.next().expect("split_reply returns at least one part");
    let message = CreateMessage::new().content(first);
    let message = match applied.reply_to {
        Some(message_id) => message.reference_message((applied.channel_id, message_id)),
        None => message,
    };
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
        reply_to: applied.reply_to.map(MessageId::get),
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
async fn send_report(
    http: &Http,
    store: &Store,
    admin_log: ChannelId,
    stats: &RunStats,
    intake: &Intake,
    report: &Report,
    reported_by: ReportedBy,
) -> anyhow::Result<()> {
    admin_log
        .send_message(http, CreateMessage::new().embed(report_embed(report)))
        .await?;
    store.intakes().mark_reported(intake.id).await?;
    let reported = IntakeReported {
        intake_id: intake.id,
        intake_kind: intake.kind,
        complete: report.missing.is_empty(),
        missing: report.missing.clone(),
        by_timeout: reported_by == ReportedBy::Timeout,
    };
    stats
        .record(Some(intake.reporter_id), Event::IntakeReported(reported))
        .await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    use super::*;
    use crate::stats::test_support::{RunEvent, events_between, run_event, test_run};
    use crate::store::intakes::{IntakeKind, NewIntake};
    use crate::test_support::{discord_http, json_response};
    use crate::workflows::intake::report::{FeatureReport, ReportBody};

    const SENT_MESSAGE: &str = include_str!("../../tests/fixtures/discord/reply_sent_message.json");
    const SENT_MESSAGE_ID: &str = "SENT_MESSAGE_ID";
    const CHANNEL_ID: u64 = 1200000000000000003;
    const ADMIN_SENT_MESSAGE: &str =
        include_str!("../../tests/fixtures/discord/admin_log_sent_message.json");
    const ADMIN_CHANNEL_ID: u64 = 1200000000000000009;
    const THREAD_ID: u64 = 1200000000000000004;
    const REPORTER_ID: u64 = 1300000000000000002;

    async fn mount_admin_channel(discord: &MockServer) {
        Mock::given(method("POST"))
            .and(path(format!(
                "/api/v10/channels/{ADMIN_CHANNEL_ID}/messages"
            )))
            .respond_with(json_response(200, ADMIN_SENT_MESSAGE))
            .mount(discord)
            .await;
    }

    async fn open_intake(store: &Store) -> Intake {
        store
            .intakes()
            .create(NewIntake {
                kind: IntakeKind::Feature,
                thread_id: THREAD_ID,
                origin_channel_id: CHANNEL_ID,
                origin_message_id: 1100000000000000001,
                reporter_id: REPORTER_ID,
                created_at: Utc::now(),
            })
            .await
            .unwrap()
    }

    fn feature_report(missing: Vec<String>) -> Report {
        Report {
            body: ReportBody::Feature(FeatureReport {
                title: "Crossfade".to_string(),
                what: "Crossfade between tracks".to_string(),
                why: "unknown".to_string(),
                summary: "The reporter wants a crossfade between tracks.".to_string(),
                already_exists: None,
            }),
            reporter_id: REPORTER_ID,
            thread_id: THREAD_ID,
            missing,
        }
    }

    /// Sends `report` of a new feature intake, and gives the events of the store.
    async fn reported_events(report: Report, reported_by: ReportedBy) -> Vec<RunEvent> {
        let discord = MockServer::start().await;
        mount_admin_channel(&discord).await;
        let (store, stats) = test_run().await;
        let intake = open_intake(&store).await;
        let before = Utc::now();

        send_report(
            &discord_http(&discord),
            &store,
            ChannelId::new(ADMIN_CHANNEL_ID),
            &stats,
            &intake,
            &report,
            reported_by,
        )
        .await
        .unwrap();

        let after = Utc::now();
        events_between(&store, before, after).await
    }

    /// The `intake.reported` event about the reporter of the intake.
    fn intake_reported(data: serde_json::Value) -> RunEvent {
        RunEvent {
            user_id: Some(REPORTER_ID),
            ..run_event("intake.reported", data)
        }
    }

    /// Discord answers the first message with the ID `first_id`, and each later one with `next_id`.
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
            reply_to: Some(MessageId::new(1100000000000000001)),
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

    #[tokio::test]
    async fn send_report_records_an_incomplete_report_by_the_timeout() {
        let report = feature_report(vec!["Why".to_string()]);

        let events = reported_events(report, ReportedBy::Timeout).await;

        assert_eq!(
            events,
            vec![intake_reported(json!({
                "intake_id": 1,
                "intake_kind": "feature",
                "complete": false,
                "missing": ["Why"],
                "by_timeout": true,
            }))]
        );
    }

    #[tokio::test]
    async fn send_report_records_a_complete_report_by_the_reporter() {
        let report = feature_report(vec![]);

        let events = reported_events(report, ReportedBy::Reporter).await;

        assert_eq!(
            events,
            vec![intake_reported(json!({
                "intake_id": 1,
                "intake_kind": "feature",
                "complete": true,
                "missing": [],
                "by_timeout": false,
            }))]
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
