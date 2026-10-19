//! The start of an intake in its thread, and the turns in an open intake thread.

use chrono::Utc;
use serenity::builder::CreateThread;
use serenity::model::channel::{ChannelType, GuildChannel, Message};
use serenity::model::id::{ChannelId, MessageId};

use crate::app::App;
use crate::discord::history::{fetch_channel, fetch_thread_history, turn_context};
use crate::discord::pipeline::{first_message, last_message};
use crate::discord::reply::{AppliedTurn, apply_intake_outcome, thread_name};
use crate::routing::category::Category;
use crate::routing::trigger::Trigger;
use crate::store::intakes::{Intake, IntakeKind, NewIntake};

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
    let thread = fetch_channel(app, last.channel_id).await?;
    let history = fetch_thread_history(app, &intake).await?;
    let ctx = turn_context(
        app,
        Trigger::IntakeThread,
        &thread.name,
        history,
        unit.len(),
    )
    .await?;
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
    let history = fetch_thread_history(app, &intake).await?;
    let ctx = turn_context(app, trigger, &thread.name, history, unit.len()).await?;
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
