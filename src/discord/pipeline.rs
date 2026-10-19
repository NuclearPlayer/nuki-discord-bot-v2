//! The message pipeline of a debounced unit: routing, the workflow, and the outcome.

use std::sync::Arc;
use std::time::Instant;

use serenity::http::Http;
use serenity::model::channel::{GuildChannel, Message};
use serenity::model::id::{RoleId, UserId};
use tokio::sync::mpsc;

use crate::app::App;
use crate::discord::attachments::ResolvedMessage;
use crate::discord::context::jev_lines;
use crate::discord::events::{message_received, unit_routed};
use crate::discord::handler::facts_of;
use crate::discord::history::{fetch_channel, fetch_unit_context};
use crate::discord::intake_thread::{continue_intake, intake_workflow, start_intake};
use crate::discord::reply::{AppliedTurn, apply_outcome};
use crate::discord::turn_context::turn_context;
use crate::routing::category::Category;
use crate::routing::dispatch::{Decision, decide_with_cooldown, describe};
use crate::routing::trigger::{ClassifiedTrigger, MessageFacts, Trigger, trigger_of_unit};
use crate::stats::event::Event;
use crate::stats::recorder::{RunStats, run_id_of_unit};
use crate::store::intakes::IntakeKind;
use crate::workflows::{Workflow, timed};

/// The workflow name in the admin log for an error before the bot picks a workflow.
const ROUTING: &str = "routing";

/// Reads the units of the debouncer, and processes each unit in its own task.
pub fn spawn_unit_reader(app: Arc<App>, mut units: mpsc::UnboundedReceiver<Vec<Message>>) {
    tokio::spawn(async move {
        while let Some(unit) = units.recv().await {
            tokio::spawn(process_unit(Arc::clone(&app), unit));
        }
    });
}

/// Runs the pipeline for one unit, as one run. An error goes to the admin log, and Nuki sends
/// nothing.
pub async fn process_unit(app: Arc<App>, unit: Vec<Message>) {
    let stats = unit_run(&app, &unit);
    if let Err(failure) = handle_unit(&app, &unit, &stats).await {
        app.admin_log
            .report_error(stats.scope(None), failure.workflow, &failure.error)
            .await;
    }
}

/// The recorder of the run of the unit, in the server and the channel of the unit.
fn unit_run(app: &App, unit: &[Message]) -> RunStats {
    let last = last_message(unit);
    let guild_id = last
        .guild_id
        .expect("the handler drops the messages outside a server");
    app.stats.run(
        run_id_of_unit(last.id.get()),
        guild_id.get(),
        last.channel_id.get(),
    )
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

async fn handle_unit(app: &App, unit: &[Message], stats: &RunStats) -> Result<(), Failure> {
    let channel_id = last_message(unit).channel_id;
    let intake = app
        .store
        .intakes()
        .find_open_by_thread(channel_id.get())
        .await
        .in_workflow(ROUTING)?;
    let nuki_role_id = nuki_role_id(&app.http, app.nuki.user_id, unit)
        .await
        .in_workflow(ROUTING)?;
    let facts: Vec<MessageFacts> = unit
        .iter()
        .map(|message| facts_of(message, app.nuki.user_id, nuki_role_id, intake.is_some()))
        .collect();
    let Some(trigger) = trigger_of_unit(&facts) else {
        return Ok(());
    };
    record_received(stats, unit, &facts, trigger).await;
    match trigger {
        Trigger::IntakeThread => {
            let intake = intake.expect("only the thread of an open intake has this trigger");
            let workflow = intake_workflow(intake.kind);
            continue_intake(app, unit, intake, stats)
                .await
                .in_workflow(workflow)
        }
        Trigger::Direct => route(app, unit, ClassifiedTrigger::Direct, stats).await,
        Trigger::Unsolicited => route(app, unit, ClassifiedTrigger::Unsolicited, stats).await,
    }
}

/// Records a `message.received` event for each message of the unit, with the trigger of the unit.
async fn record_received(
    stats: &RunStats,
    unit: &[Message],
    facts: &[MessageFacts],
    trigger: Trigger,
) {
    for (message, facts) in unit.iter().zip(facts) {
        let received = message_received(message, facts, trigger);
        stats
            .record(
                Some(message.author.id.get()),
                Event::MessageReceived(received),
            )
            .await;
    }
}

/// The managed role of Nuki in the server of the unit: the role whose bot is Nuki. Only a unit
/// that mentions a role needs it, so the other units do not request the roles of the server.
async fn nuki_role_id(
    http: &Http,
    nuki_user_id: u64,
    unit: &[Message],
) -> anyhow::Result<Option<RoleId>> {
    let Some(guild_id) = last_message(unit).guild_id else {
        return Ok(None);
    };
    if unit.iter().all(|message| message.mention_roles.is_empty()) {
        return Ok(None);
    }
    let nuki = UserId::new(nuki_user_id);
    let roles = guild_id.roles(http).await?;
    Ok(roles
        .into_values()
        .find(|role| role.tags.bot_id == Some(nuki))
        .map(|role| role.id))
}

/// A unit that the classification sends to a workflow.
struct Routed {
    trigger: ClassifiedTrigger,
    category: Category,
    channel: GuildChannel,
    history: Vec<ResolvedMessage>,
}

async fn route(
    app: &App,
    unit: &[Message],
    trigger: ClassifiedTrigger,
    stats: &RunStats,
) -> Result<(), Failure> {
    let Some(routed) = classify(app, unit, trigger, stats)
        .await
        .in_workflow(ROUTING)?
    else {
        return Ok(());
    };
    let workflow = routed.category.name();
    run_category(app, unit, routed, stats)
        .await
        .in_workflow(workflow)
}

/// Classifies the unit with its history, decides what to run with the chat cooldown of the
/// channel, and logs and records the decision. A unit that runs goes back as `Routed`.
async fn classify(
    app: &App,
    unit: &[Message],
    trigger: ClassifiedTrigger,
    stats: &RunStats,
) -> anyhow::Result<Option<Routed>> {
    let last = last_message(unit);
    let channel = fetch_channel(app, last.channel_id).await?;
    let history = fetch_unit_context(app, channel.guild_id, unit).await?;
    let lines = jev_lines(&history);
    let (history_lines, unit_lines) = lines.split_at(lines.len() - unit.len());
    let classification = app
        .classifier
        .classify(trigger, history_lines, unit_lines, stats)
        .await?;
    let decision = decide_with_cooldown(
        trigger,
        &classification,
        &app.thresholds,
        &app.cooldowns,
        channel.id.get(),
        Instant::now(),
    );
    tracing::info!(
        channel = channel.name,
        "{}",
        describe(&classification, &decision)
    );
    let routed = unit_routed(unit.len(), &classification, &decision);
    stats
        .record(Some(last.author.id.get()), Event::UnitRouted(routed))
        .await;
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

async fn run_category(
    app: &App,
    unit: &[Message],
    routed: Routed,
    stats: &RunStats,
) -> anyhow::Result<()> {
    let trigger = Trigger::from(routed.trigger);
    match routed.category {
        Category::HelpRequest => run_workflow(app, &app.help, unit, routed, stats).await,
        Category::Chat => run_workflow(app, &app.chat, unit, routed, stats).await,
        Category::BugReport => {
            start_intake(app, unit, &routed.channel, trigger, IntakeKind::Bug, stats).await
        }
        Category::FeatureRequest => {
            start_intake(
                app,
                unit,
                &routed.channel,
                trigger,
                IntakeKind::Feature,
                stats,
            )
            .await
        }
        Category::Ignore => Ok(()),
    }
}

async fn run_workflow(
    app: &App,
    workflow: &impl Workflow,
    unit: &[Message],
    routed: Routed,
    stats: &RunStats,
) -> anyhow::Result<()> {
    let _typing = routed.channel.id.start_typing(&app.http);
    let trigger = Trigger::from(routed.trigger);
    let ctx = turn_context(
        app,
        routed.channel.guild_id,
        trigger,
        &routed.channel.name,
        routed.history,
        unit.len(),
        stats.clone(),
    )
    .await?;
    let outcome = timed(stats, routed.category.name(), workflow.run(&ctx)).await?;
    let applied = AppliedTurn {
        channel_id: routed.channel.id,
        reply_to: Some(last_message(unit).id),
    };
    apply_outcome(app, stats, &applied, outcome).await
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    use super::*;
    use crate::test_support::{discord_http, json_response};

    const NUKI_USER_ID: u64 = 1700000000000000001;
    const ROLE_MENTION: &str =
        include_str!("../../tests/fixtures/discord/handler_role_mention.json");
    const GUILD_ROLES: &str =
        include_str!("../../tests/fixtures/discord/pipeline_guild_roles.json");

    #[tokio::test]
    async fn nuki_role_id_is_the_role_whose_bot_is_nuki() {
        let discord = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v10/guilds/1000000000000000001/roles"))
            .respond_with(json_response(200, GUILD_ROLES))
            .mount(&discord)
            .await;
        let unit: Vec<Message> = vec![serde_json::from_str(ROLE_MENTION).unwrap()];

        let role_id = nuki_role_id(&discord_http(&discord), NUKI_USER_ID, &unit)
            .await
            .unwrap();

        assert_eq!(role_id, Some(RoleId::new(1900000000000000001)));
    }
}
