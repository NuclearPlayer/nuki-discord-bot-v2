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
use crate::routing::classify::Classification;
use crate::routing::dispatch::{Decision, decide_in_intake, decide_with_cooldown, describe};
use crate::routing::trigger::{
    ClassifiedTrigger, MessageFacts, Trigger, trigger_of_unit, unit_addresses_nuki,
};
use crate::stats::event::Event;
use crate::stats::recorder::{RunStats, run_id_of_unit};
use crate::store::intakes::{Intake, IntakeKind};
use crate::workflows::{Workflow, timed};

/// The workflow name for the admin log, if an error occurs before the bot selects a workflow.
const ROUTING: &str = "routing";

/// Reads the units of the debouncer, and processes each unit in its own task.
pub fn spawn_unit_reader(app: Arc<App>, mut units: mpsc::UnboundedReceiver<Vec<Message>>) {
    tokio::spawn(async move {
        while let Some(unit) = units.recv().await {
            tokio::spawn(process_unit(Arc::clone(&app), unit));
        }
    });
}

/// Runs the pipeline for one unit, as one run. If the pipeline fails, sends the error to the
/// admin log, and Nuki sends nothing.
pub async fn process_unit(app: Arc<App>, unit: Vec<Message>) {
    let stats = unit_run(&app, &unit);
    if let Err(failure) = handle_unit(&app, &unit, &stats).await {
        app.admin_log
            .report_error(stats.scope(None), failure.workflow, &failure.error)
            .await;
    }
}

/// Returns the recorder of the unit's run, in the server and the channel of the unit.
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

/// A pipeline error, with the workflow name for the admin log.
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
            route_in_intake(app, unit, intake, unit_addresses_nuki(&facts), stats).await
        }
        Trigger::Direct => route(app, unit, ClassifiedTrigger::Direct, stats).await,
        Trigger::Unsolicited => route(app, unit, ClassifiedTrigger::Unsolicited, stats).await,
    }
}

/// Records a `message.received` event for each message of the unit, with the unit's trigger.
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

/// Gets Nuki's managed role in the unit's server: the role that has Nuki as its bot. Only a unit
/// that mentions a role uses this role. For the other units, sends no request for the server's
/// roles.
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

/// Runs the intake turn for a unit in the intake's thread. If the unit does not address Nuki,
/// Jev classifies the unit first. If `ignore` is the top category, Nuki skips the unit and the
/// intake stays as it is.
async fn route_in_intake(
    app: &App,
    unit: &[Message],
    intake: Intake,
    addresses_nuki: bool,
    stats: &RunStats,
) -> Result<(), Failure> {
    if !addresses_nuki {
        let decision = classify_in_intake(app, unit, stats)
            .await
            .in_workflow(ROUTING)?;
        if let Decision::Skip(_) = decision {
            return Ok(());
        }
    }
    let workflow = intake_workflow(intake.kind);
    continue_intake(app, unit, intake, stats)
        .await
        .in_workflow(workflow)
}

/// Classifies a unit in the intake's thread with all categories, and decides if the intake turn
/// runs. Logs and records the decision.
async fn classify_in_intake(
    app: &App,
    unit: &[Message],
    stats: &RunStats,
) -> anyhow::Result<Decision> {
    let classified = classify_unit(app, unit, ClassifiedTrigger::Unsolicited, stats).await?;
    let decision = decide_in_intake(&classified.classification);
    report_decision(stats, unit, &classified, &decision).await;
    Ok(decision)
}

/// Classifies the unit with its history. Decides what to run with the channel's chat cooldown,
/// and logs and records the decision. If the unit runs, returns the unit as `Routed`.
async fn classify(
    app: &App,
    unit: &[Message],
    trigger: ClassifiedTrigger,
    stats: &RunStats,
) -> anyhow::Result<Option<Routed>> {
    let classified = classify_unit(app, unit, trigger, stats).await?;
    let decision = decide_with_cooldown(
        trigger,
        &classified.classification,
        &app.thresholds,
        &app.cooldowns,
        classified.channel.id.get(),
        Instant::now(),
    );
    report_decision(stats, unit, &classified, &decision).await;
    let Decision::Run(category) = decision else {
        return Ok(None);
    };
    Ok(Some(Routed {
        trigger,
        category,
        channel: classified.channel,
        history: classified.history,
    }))
}

/// A unit's channel and history, with Jev's classification of the unit.
struct Classified {
    channel: GuildChannel,
    history: Vec<ResolvedMessage>,
    classification: Classification,
}

/// Gets the unit's channel and history, and sends them to Jev for the classification.
async fn classify_unit(
    app: &App,
    unit: &[Message],
    trigger: ClassifiedTrigger,
    stats: &RunStats,
) -> anyhow::Result<Classified> {
    let channel = fetch_channel(app, last_message(unit).channel_id).await?;
    let history = fetch_unit_context(app, &channel, unit).await?;
    let lines = jev_lines(&history);
    let (history_lines, unit_lines) = lines.split_at(lines.len() - unit.len());
    let classification = app
        .classifier
        .classify(trigger, history_lines, unit_lines, stats)
        .await?;
    Ok(Classified {
        channel,
        history,
        classification,
    })
}

/// Logs the decision, and records it as a `unit.routed` event.
async fn report_decision(
    stats: &RunStats,
    unit: &[Message],
    classified: &Classified,
    decision: &Decision,
) {
    tracing::info!(
        channel = classified.channel.name,
        "{}",
        describe(&classified.classification, decision)
    );
    let routed = unit_routed(unit.len(), &classified.classification, decision);
    stats
        .record(
            Some(last_message(unit).author.id.get()),
            Event::UnitRouted(routed),
        )
        .await;
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
    use serde_json::Value;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    use super::*;
    use crate::discord::test_support::intake_thread_scene::{
        BUG_REPORT_ON_TOP, IGNORE_ON_TOP, IntakeThreadScene, NUKI_ASKS_FOR_THE_PLATFORM, UNIT_ID,
        gateway_message_in_server, maintainer_finding, mentioning_nuki, nuki_question, reply_to,
        reporter, thread_message, unit_routed_data,
    };
    use crate::discord::test_support::{GUILD_ROLES, NUKI_USER_ID};
    use crate::test_support::{discord_http, json_response};

    const ROLE_MENTION: &str =
        include_str!("../../tests/fixtures/discord/handler_role_mention.json");

    async fn handle(scene: &IntakeThreadScene, unit: &Value) {
        let unit = vec![gateway_message_in_server(unit)];
        let stats = unit_run(&scene.app, &unit);
        handle_unit(&scene.app, &unit, &stats)
            .await
            .unwrap_or_else(|failure| panic!("{:#}", failure.error));
    }

    #[tokio::test]
    async fn an_ignored_unit_in_an_intake_thread_gets_no_intake_turn() {
        let unit = reply_to(
            UNIT_ID,
            reporter(),
            "yes, exactly, the right one shows up first and then it is gone",
            maintainer_finding(),
        );
        let scene =
            IntakeThreadScene::open(vec![nuki_question(), maintainer_finding()], &unit).await;
        scene.answer_category(&IGNORE_ON_TOP).await;
        scene.answer_intake_turn_with_missing_fields().await;

        handle(&scene, &unit).await;

        assert_eq!(scene.sent_texts().await, Vec::<String>::new());
        assert_eq!(scene.jev_purposes().await, vec!["category"]);
        assert_eq!(
            scene.event_data("unit.routed").await,
            vec![unit_routed_data(&IGNORE_ON_TOP, "skip: ignored")]
        );
        assert_eq!(scene.open_intake().await, Some(scene.intake.clone()));
    }

    #[tokio::test]
    async fn a_unit_in_an_intake_thread_is_classified_with_all_categories() {
        let unit = thread_message(UNIT_ID, reporter(), "1.51.2");
        let scene = IntakeThreadScene::open(vec![nuki_question()], &unit).await;
        scene.answer_category(&BUG_REPORT_ON_TOP).await;
        scene.answer_intake_turn_with_missing_fields().await;

        handle(&scene, &unit).await;

        assert_eq!(
            scene.category_options().await,
            vec![vec![
                "help_request",
                "bug_report",
                "feature_request",
                "chat",
                "ignore"
            ]]
        );
    }

    #[tokio::test]
    async fn a_bug_report_unit_in_an_intake_thread_runs_the_intake_turn() {
        let unit = thread_message(UNIT_ID, reporter(), "1.51.2");
        let scene = IntakeThreadScene::open(vec![nuki_question()], &unit).await;
        scene.answer_category(&BUG_REPORT_ON_TOP).await;
        scene.answer_intake_turn_with_missing_fields().await;

        handle(&scene, &unit).await;

        assert_eq!(scene.sent_texts().await, vec![NUKI_ASKS_FOR_THE_PLATFORM]);
        assert_eq!(
            scene.jev_purposes().await,
            vec!["category", "intake_fields"]
        );
        assert_eq!(
            scene.event_data("unit.routed").await,
            vec![unit_routed_data(&BUG_REPORT_ON_TOP, "continue the intake")]
        );
    }

    #[tokio::test]
    async fn a_mention_of_nuki_in_an_intake_thread_runs_the_intake_turn_without_a_classification() {
        let unit = mentioning_nuki(thread_message(
            UNIT_ID,
            reporter(),
            &format!("<@{NUKI_USER_ID}> 1.51.2"),
        ));
        let scene = IntakeThreadScene::open(vec![nuki_question()], &unit).await;
        scene.answer_intake_turn_with_missing_fields().await;

        handle(&scene, &unit).await;

        assert_eq!(scene.sent_texts().await, vec![NUKI_ASKS_FOR_THE_PLATFORM]);
        assert_eq!(scene.jev_purposes().await, vec!["intake_fields"]);
        assert_eq!(scene.event_data("unit.routed").await, Vec::<Value>::new());
    }

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
