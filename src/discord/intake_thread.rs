//! Starts an intake in its thread, runs the turns in an open intake thread, and completes an
//! inactive intake.

use chrono::Utc;
use serenity::builder::CreateThread;
use serenity::model::channel::{GuildChannel, Message};
use serenity::model::id::{ChannelId, MessageId};

use crate::app::App;
use crate::discord::history::{fetch_channel, fetch_thread_history, is_thread};
use crate::discord::http_error::is_not_found;
use crate::discord::pipeline::{first_message, last_message};
use crate::discord::reply::{AppliedTurn, ReportedBy, apply_intake_outcome, thread_name};
use crate::discord::turn_context::turn_context;
use crate::routing::category::Category;
use crate::routing::trigger::Trigger;
use crate::stats::event::{Event, IntakeOpened, ThreadCreated};
use crate::stats::recorder::{EventScope, RunStats, run_id_of_timeout};
use crate::store::intakes::{Intake, IntakeKind, NewIntake};
use crate::workflows::{Outcome, TurnContext, timed};

/// The intake's thread, and the message that Nuki replies to in the thread.
struct IntakeThread {
    channel: GuildChannel,
    reply_to: Option<MessageId>,
}

/// Returns the admin log's workflow name for an intake of `kind`: the name of the category that
/// starts the intake.
pub(super) fn intake_workflow(kind: IntakeKind) -> &'static str {
    let category = match kind {
        IntakeKind::Bug => Category::BugReport,
        IntakeKind::Feature => Category::FeatureRequest,
    };
    category.name()
}

/// Runs a turn of an open intake, in the run of `stats`. The intake stays active only if the
/// reporter wrote the last message of the unit. In that case, sets the intake's last activity to
/// the time of that message.
pub(super) async fn continue_intake(
    app: &App,
    unit: &[Message],
    intake: Intake,
    stats: &RunStats,
) -> anyhow::Result<()> {
    let last = last_message(unit);
    if last.author.id.get() == intake.reporter_id {
        app.store
            .intakes()
            .touch(intake.id, *last.timestamp)
            .await?;
    }
    let thread = fetch_intake_thread(app, &intake).await?;
    let ctx = thread_turn_context(app, &intake, &thread, unit.len(), stats.clone()).await?;
    let _typing = last.channel_id.start_typing(&app.http);
    let workflow = intake_workflow(intake.kind);
    let outcome = timed(stats, workflow, app.intake.run_turn(&ctx, &intake)).await?;
    let applied = AppliedTurn {
        channel_id: last.channel_id,
        reply_to: Some(last.id),
    };
    apply_intake_outcome(app, stats, &applied, &intake, outcome, ReportedBy::Reporter).await
}

/// Completes an intake when its reporter does not answer. The completion has two steps:
///
/// 1. Fetches the thread. If the thread was deleted, closes the intake without a report.
/// 2. Completes the intake as one run in the thread.
///
/// If an error occurs, sends the error to the admin log. Before the run, the error has the
/// thread's scope. After the start of the run, the error has the run's scope.
pub(crate) async fn complete_inactive_intake(app: &App, intake: &Intake) {
    let thread = match fetch_intake_thread(app, intake).await {
        Ok(thread) => thread,
        Err(error) => {
            if let Err(error) = close_if_deleted(app, intake, error).await {
                report_failure(app, intake, thread_scope(intake), &error).await;
            }
            return;
        }
    };
    let stats = app.stats.run(
        run_id_of_timeout(intake.id, Utc::now()),
        thread.guild_id.get(),
        intake.thread_id,
    );
    if let Err(error) = complete(app, intake, &thread, &stats).await {
        report_failure(app, intake, stats.scope(None), &error).await;
    }
}

async fn fetch_intake_thread(app: &App, intake: &Intake) -> anyhow::Result<GuildChannel> {
    fetch_channel(app, ChannelId::new(intake.thread_id)).await
}

/// If `error` says that the intake's thread was deleted, closes the intake. If not, returns the
/// error.
async fn close_if_deleted(app: &App, intake: &Intake, error: anyhow::Error) -> anyhow::Result<()> {
    if !is_deleted(&error) {
        return Err(error);
    }
    close_deleted(app, intake).await
}

/// Sends the error to the admin log. Touches the intake, because the next attempt must wait one
/// full intake timeout.
async fn report_failure(app: &App, intake: &Intake, scope: EventScope, error: &anyhow::Error) {
    app.admin_log
        .report_error(scope, intake_workflow(intake.kind), error)
        .await;
    postpone(app, intake).await;
}

async fn postpone(app: &App, intake: &Intake) {
    if let Err(error) = app.store.intakes().touch(intake.id, Utc::now()).await {
        tracing::error!(
            intake_id = intake.id,
            "could not postpone the intake after a failed completion: {error}"
        );
    }
}

/// Returns the scope of an event in the intake's thread, outside a run.
fn thread_scope(intake: &Intake) -> EventScope {
    EventScope {
        run_id: None,
        guild_id: None,
        channel_id: Some(intake.thread_id),
        user_id: None,
    }
}

/// Completes the intake in its thread, in the run of `stats`, and sends the report.
async fn complete(
    app: &App,
    intake: &Intake,
    thread: &GuildChannel,
    stats: &RunStats,
) -> anyhow::Result<()> {
    let ctx = thread_turn_context(app, intake, thread, 0, stats.clone()).await?;
    let workflow = intake_workflow(intake.kind);
    let outcome = timed(stats, workflow, inactive_outcome(app, &ctx, intake)).await?;
    let applied = AppliedTurn {
        channel_id: ChannelId::new(intake.thread_id),
        reply_to: None,
    };
    apply_intake_outcome(app, stats, &applied, intake, outcome, ReportedBy::Timeout).await
}

/// Returns the outcome of an inactive intake: the report, and no reply.
async fn inactive_outcome(
    app: &App,
    ctx: &TurnContext,
    intake: &Intake,
) -> anyhow::Result<Outcome> {
    let report = app.intake.complete_inactive(ctx, intake).await?;
    Ok(Outcome::Intake {
        reply: None,
        report: Some(report),
    })
}

/// Returns `true` if Discord answered 404 for the thread. Discord answers 404 when the thread or
/// its parent channel was deleted.
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

/// Builds the turn context of the intake in its thread, with the thread's history, in the run of
/// `stats`.
async fn thread_turn_context(
    app: &App,
    intake: &Intake,
    thread: &GuildChannel,
    unit_len: usize,
    stats: RunStats,
) -> anyhow::Result<TurnContext> {
    let history = fetch_thread_history(app, thread, intake).await?;
    turn_context(
        app,
        thread.guild_id,
        Trigger::IntakeThread,
        &thread.name,
        history,
        unit_len,
        stats,
    )
    .await
}

/// Opens the intake in a thread, stores its record, and runs its first turn, in the run of
/// `stats`.
pub(super) async fn start_intake(
    app: &App,
    unit: &[Message],
    channel: &GuildChannel,
    trigger: Trigger,
    kind: IntakeKind,
    stats: &RunStats,
) -> anyhow::Result<()> {
    let last = last_message(unit);
    let thread = intake_thread(app, channel, last, stats).await?;
    let _typing = thread.channel.id.start_typing(&app.http);
    let intake = app
        .store
        .intakes()
        .create(NewIntake {
            kind,
            thread_id: thread.channel.id.get(),
            origin_channel_id: channel.id.get(),
            origin_message_id: first_message(unit).id.get(),
            reporter_id: last.author.id.get(),
            created_at: Utc::now(),
        })
        .await?;
    let opened = IntakeOpened {
        intake_id: intake.id,
        intake_kind: intake.kind,
        thread_id: intake.thread_id,
    };
    stats
        .record(Some(intake.reporter_id), Event::IntakeOpened(opened))
        .await;
    let guild_id = channel.guild_id;
    let history = fetch_thread_history(app, &thread.channel, &intake).await?;
    let ctx = turn_context(
        app,
        guild_id,
        trigger,
        &thread.channel.name,
        history,
        unit.len(),
        stats.clone(),
    )
    .await?;
    let outcome = timed(
        stats,
        intake_workflow(kind),
        app.intake.run_turn(&ctx, &intake),
    )
    .await?;
    let applied = AppliedTurn {
        channel_id: thread.channel.id,
        reply_to: thread.reply_to,
    };
    apply_intake_outcome(app, stats, &applied, &intake, outcome, ReportedBy::Reporter).await
}

/// If the unit is in a thread or a forum post, uses that thread. If not, makes a public thread
/// from the last message of the unit. In that case, Nuki's first message in the new thread is
/// not a reply.
async fn intake_thread(
    app: &App,
    channel: &GuildChannel,
    last: &Message,
    stats: &RunStats,
) -> anyhow::Result<IntakeThread> {
    if is_thread(channel) {
        return Ok(IntakeThread {
            channel: channel.clone(),
            reply_to: Some(last.id),
        });
    }
    let name = thread_name(&last.content, last.author.display_name());
    let thread = channel
        .id
        .create_thread_from_message(&app.http, last.id, CreateThread::new(name))
        .await?;
    let created = ThreadCreated {
        thread_id: thread.id.get(),
        message_id: last.id.get(),
        name: thread.name.clone(),
    };
    stats
        .record(Some(last.author.id.get()), Event::ThreadCreated(created))
        .await;
    Ok(IntakeThread {
        channel: thread,
        reply_to: None,
    })
}
