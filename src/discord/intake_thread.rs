//! The start of an intake in its thread, the turns in an open intake thread, and the
//! completion of an inactive intake.

use chrono::Utc;
use serenity::builder::CreateThread;
use serenity::model::channel::{ChannelType, GuildChannel, Message};
use serenity::model::id::{ChannelId, MessageId};

use crate::app::App;
use crate::discord::history::{fetch_channel, fetch_thread_history};
use crate::discord::http_error::is_not_found;
use crate::discord::pipeline::{first_message, last_message};
use crate::discord::reply::{AppliedTurn, apply_intake_outcome, thread_name};
use crate::discord::turn_context::turn_context;
use crate::routing::category::Category;
use crate::routing::trigger::Trigger;
use crate::store::intakes::{Intake, IntakeKind, NewIntake};
use crate::workflows::{Outcome, TurnContext};

/// The thread of an intake, and the message that Nuki replies to in it.
struct IntakeThread {
    id: ChannelId,
    name: String,
    reply_to: Option<MessageId>,
}

/// The category that starts an intake of `kind`.
pub(super) fn intake_category(kind: IntakeKind) -> Category {
    match kind {
        IntakeKind::Bug => Category::BugReport,
        IntakeKind::Feature => Category::FeatureRequest,
    }
}

/// The workflow name of an intake of `kind` in the admin log.
pub(super) fn intake_workflow(kind: IntakeKind) -> &'static str {
    intake_category(kind).name()
}

/// Runs a turn of an open intake. A message of the reporter keeps the intake active.
pub(super) async fn continue_intake(
    app: &App,
    unit: &[Message],
    intake: Intake,
) -> anyhow::Result<()> {
    let last = last_message(unit);
    if last.author.id.get() == intake.reporter_id {
        app.store
            .intakes()
            .touch(intake.id, *last.timestamp)
            .await?;
    }
    let ctx = thread_turn_context(app, &intake, unit.len()).await?;
    let _typing = last.channel_id.start_typing(&app.http);
    let outcome = app.intake.run_turn(&ctx, &intake).await?;
    let applied = AppliedTurn {
        trigger: Trigger::IntakeThread,
        category: None,
        channel_id: last.channel_id,
        reply_to: Some(last.id),
    };
    apply_intake_outcome(app, &applied, &intake, outcome).await
}

/// Completes an intake whose reporter does not answer. An error goes to the admin log, and the
/// intake is touched, so the next attempt waits one full intake timeout.
pub(crate) async fn complete_inactive_intake(app: &App, intake: &Intake) {
    if let Err(error) = complete_or_close(app, intake).await {
        app.admin_log
            .report_error(intake_workflow(intake.kind), intake.thread_id, &error)
            .await;
        postpone(app, intake).await;
    }
}

async fn postpone(app: &App, intake: &Intake) {
    if let Err(error) = app.store.intakes().touch(intake.id, Utc::now()).await {
        tracing::error!(
            intake_id = intake.id,
            "could not postpone the intake after a failed completion: {error}"
        );
    }
}

/// Sends the report of the intake. An intake whose thread was deleted is closed without a report.
async fn complete_or_close(app: &App, intake: &Intake) -> anyhow::Result<()> {
    let ctx = match thread_turn_context(app, intake, 0).await {
        Ok(ctx) => ctx,
        Err(error) if is_deleted(&error) => return close_deleted(app, intake).await,
        Err(error) => return Err(error),
    };
    let report = app.intake.complete_inactive(&ctx, intake).await?;
    let applied = AppliedTurn {
        trigger: Trigger::IntakeThread,
        category: None,
        channel_id: ChannelId::new(intake.thread_id),
        reply_to: None,
    };
    let outcome = Outcome::Intake {
        reply: None,
        report: Some(report),
    };
    apply_intake_outcome(app, &applied, intake, outcome).await
}

/// Discord answers 404 for the thread when the thread or its parent channel was deleted.
fn is_deleted(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<serenity::Error>()
        .is_some_and(is_not_found)
}

async fn close_deleted(app: &App, intake: &Intake) -> anyhow::Result<()> {
    app.store.intakes().mark_reported(intake.id).await?;
    tracing::warn!(
        intake_id = intake.id,
        thread_id = intake.thread_id,
        "the thread of the intake was deleted; the intake is closed without a report"
    );
    Ok(())
}

/// The turn context of the intake in the server of its thread, with the history of the thread.
async fn thread_turn_context(
    app: &App,
    intake: &Intake,
    unit_len: usize,
) -> anyhow::Result<TurnContext> {
    let thread = fetch_channel(app, ChannelId::new(intake.thread_id)).await?;
    let history = fetch_thread_history(app, thread.guild_id, intake).await?;
    turn_context(
        app,
        thread.guild_id,
        Trigger::IntakeThread,
        &thread.name,
        history,
        unit_len,
    )
    .await
}

/// Opens the intake in a thread, stores its record, and runs its first turn.
pub(super) async fn start_intake(
    app: &App,
    unit: &[Message],
    channel: &GuildChannel,
    trigger: Trigger,
    kind: IntakeKind,
) -> anyhow::Result<()> {
    let last = last_message(unit);
    let thread = intake_thread(app, channel, last).await?;
    let _typing = thread.id.start_typing(&app.http);
    let intake = app
        .store
        .intakes()
        .create(NewIntake {
            kind,
            thread_id: thread.id.get(),
            origin_channel_id: channel.id.get(),
            origin_message_id: first_message(unit).id.get(),
            reporter_id: last.author.id.get(),
            created_at: Utc::now(),
        })
        .await?;
    let guild_id = channel.guild_id;
    let history = fetch_thread_history(app, guild_id, &intake).await?;
    let ctx = turn_context(app, guild_id, trigger, &thread.name, history, unit.len()).await?;
    let outcome = app.intake.run_turn(&ctx, &intake).await?;
    let applied = AppliedTurn {
        trigger,
        category: Some(intake_category(kind)),
        channel_id: thread.id,
        reply_to: thread.reply_to,
    };
    apply_intake_outcome(app, &applied, &intake, outcome).await
}

/// A unit in a thread or a forum post uses that thread. Else the bot makes a public thread from
/// the last message of the unit, and Nuki's first message there is not a reply.
async fn intake_thread(
    app: &App,
    channel: &GuildChannel,
    last: &Message,
) -> anyhow::Result<IntakeThread> {
    if is_thread(channel) {
        return Ok(IntakeThread {
            id: channel.id,
            name: channel.name.clone(),
            reply_to: Some(last.id),
        });
    }
    let name = thread_name(&last.content, last.author.display_name());
    let thread = channel
        .id
        .create_thread_from_message(&app.http, last.id, CreateThread::new(name))
        .await?;
    Ok(IntakeThread {
        id: thread.id,
        name: thread.name,
        reply_to: None,
    })
}

fn is_thread(channel: &GuildChannel) -> bool {
    matches!(
        channel.kind,
        ChannelType::PublicThread | ChannelType::PrivateThread | ChannelType::NewsThread
    )
}
