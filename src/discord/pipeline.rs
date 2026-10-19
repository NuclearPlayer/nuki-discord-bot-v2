//! The message pipeline of a debounced unit: routing, the workflow, and the outcome.

use std::sync::Arc;
use std::time::Instant;

use serenity::model::channel::{GuildChannel, Message};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::app::App;
use crate::discord::attachments::ResolvedMessage;
use crate::discord::context::jev_lines;
use crate::discord::handler::facts_of;
use crate::discord::history::{fetch_channel, fetch_unit_context, turn_context};
use crate::discord::intake_thread::{continue_intake, intake_workflow, start_intake};
use crate::discord::reply::{AppliedTurn, apply_outcome};
use crate::routing::category::Category;
use crate::routing::dispatch::{Decision, decide, describe};
use crate::routing::trigger::{ClassifiedTrigger, MessageFacts, Trigger, trigger_of_unit};
use crate::store::intakes::IntakeKind;
use crate::workflows::Workflow;

/// The workflow name in the admin log for an error before the bot picks a workflow.
const ROUTING: &str = "routing";

/// Reads the units of the debouncer, and processes each unit in its own task.
pub fn spawn_unit_reader(
    app: Arc<App>,
    mut units: mpsc::UnboundedReceiver<Vec<Message>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(unit) = units.recv().await {
            tokio::spawn(process_unit(Arc::clone(&app), unit));
        }
    })
}

/// Runs the pipeline for one unit. An error goes to the admin log, and Nuki sends nothing.
pub async fn process_unit(app: Arc<App>, unit: Vec<Message>) {
    let channel_id = last_message(&unit).channel_id;
    if let Err(failure) = handle_unit(&app, &unit).await {
        app.admin_log
            .report_error(failure.workflow, channel_id.get(), &failure.error)
            .await;
    }
}

/// The debouncer sends no empty unit.
pub(super) fn first_message(unit: &[Message]) -> &Message {
    unit.first().expect("a unit has at least one message")
}

/// The debouncer sends no empty unit.
pub(super) fn last_message(unit: &[Message]) -> &Message {
    unit.last().expect("a unit has at least one message")
}

/// An error of the pipeline, with the workflow name for the admin log.
struct Failure {
    workflow: &'static str,
    error: anyhow::Error,
}

trait InWorkflow<T> {
    fn in_workflow(self, workflow: &'static str) -> Result<T, Failure>;
}

impl<T, E: Into<anyhow::Error>> InWorkflow<T> for Result<T, E> {
    fn in_workflow(self, workflow: &'static str) -> Result<T, Failure> {
        self.map_err(|error| Failure {
            workflow,
            error: error.into(),
        })
    }
}

async fn handle_unit(app: &App, unit: &[Message]) -> Result<(), Failure> {
    let channel_id = last_message(unit).channel_id;
    let intake = app
        .store
        .intakes()
        .find_open_by_thread(channel_id.get())
        .await
        .in_workflow(ROUTING)?;
    let facts: Vec<MessageFacts> = unit
        .iter()
        .map(|message| facts_of(message, app.nuki.user_id, intake.is_some()))
        .collect();
    let Some(trigger) = trigger_of_unit(&facts) else {
        return Ok(());
    };
    match trigger {
        Trigger::IntakeThread => {
            let intake = intake.expect("only the thread of an open intake has this trigger");
            let workflow = intake_workflow(intake.kind);
            continue_intake(app, unit, intake)
                .await
                .in_workflow(workflow)
        }
        Trigger::Direct => route(app, unit, ClassifiedTrigger::Direct).await,
        Trigger::Unsolicited => route(app, unit, ClassifiedTrigger::Unsolicited).await,
    }
}

/// A unit that the classification sends to a workflow.
struct Routed {
    trigger: ClassifiedTrigger,
    category: Category,
    channel: GuildChannel,
    history: Vec<ResolvedMessage>,
}

async fn route(app: &App, unit: &[Message], trigger: ClassifiedTrigger) -> Result<(), Failure> {
    let Some(routed) = classify(app, unit, trigger).await.in_workflow(ROUTING)? else {
        return Ok(());
    };
    let workflow = routed.category.name();
    run_category(app, unit, routed).await.in_workflow(workflow)
}

/// Classifies the unit with its history, and logs the decision.
async fn classify(
    app: &App,
    unit: &[Message],
    trigger: ClassifiedTrigger,
) -> anyhow::Result<Option<Routed>> {
    let channel = fetch_channel(app, last_message(unit).channel_id).await?;
    let history = fetch_unit_context(app, unit).await?;
    let lines = jev_lines(&history);
    let (history_lines, unit_lines) = lines.split_at(lines.len() - unit.len());
    let classification = app
        .classifier
        .classify(trigger, history_lines, unit_lines)
        .await?;
    let cooldown_active = app.cooldowns.is_active(channel.id.get(), Instant::now());
    let decision = decide(trigger, &classification, &app.thresholds, cooldown_active);
    tracing::info!(
        channel = channel.name,
        "{}",
        describe(&classification, &decision)
    );
    match decision {
        Decision::Run(category) => Ok(Some(Routed {
            trigger,
            category,
            channel,
            history,
        })),
        Decision::Skip(_) => Ok(None),
    }
}

async fn run_category(app: &App, unit: &[Message], routed: Routed) -> anyhow::Result<()> {
    let trigger = Trigger::from(routed.trigger);
    match routed.category {
        Category::HelpRequest => run_workflow(app, &app.help, unit, routed).await,
        Category::Chat => run_workflow(app, &app.chat, unit, routed).await,
        Category::BugReport => {
            start_intake(app, unit, &routed.channel, trigger, IntakeKind::Bug).await
        }
        Category::FeatureRequest => {
            start_intake(app, unit, &routed.channel, trigger, IntakeKind::Feature).await
        }
        Category::Ignore => Ok(()),
    }
}

async fn run_workflow(
    app: &App,
    workflow: &impl Workflow,
    unit: &[Message],
    routed: Routed,
) -> anyhow::Result<()> {
    let _typing = routed.channel.id.start_typing(&app.http);
    let trigger = Trigger::from(routed.trigger);
    let ctx = turn_context(
        app,
        trigger,
        &routed.channel.name,
        routed.history,
        unit.len(),
    )
    .await?;
    let outcome = workflow.run(&ctx).await?;
    let applied = AppliedTurn {
        trigger,
        category: Some(routed.category),
        channel_id: routed.channel.id,
        reply_to: Some(last_message(unit).id),
    };
    apply_outcome(app, &applied, outcome).await
}
