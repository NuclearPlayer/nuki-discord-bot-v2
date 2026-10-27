pub mod fields;
pub mod report;
pub mod timeout;

use rig_core::message::Message;
use serde::Deserialize;

use fields::missing_fields;
use report::{Report, ReportBody};

use super::{Outcome, Reply, TurnContext, TurnOptions, WorkflowDeps, llm_turn};
use crate::discord::context::{build_history, jev_lines};
use crate::llm::agent::TurnResult;
use crate::prompts::system::{SystemPrompt, WorkflowPrompt};
use crate::store::intakes::{Intake, IntakeKind};

const ALL_FIELDS_PRESENT: &str = "All fields are present.";
const FIRST_FEATURE_TURN: &str = "This is the first turn. First find if the feature exists.";
const INACTIVE_INSTRUCTION: &str = "The reporter does not answer. Do the final checks now. The bot does not send your text to the thread.";
const REPORT_INSTRUCTION: &str = "Write the report of this conversation. Use \"unknown\" for information that the conversation does not give.";

/// The handler calls `run_turn` and `complete_inactive` with the intake record.
pub struct IntakeWorkflow {
    pub deps: WorkflowDeps,
}

impl IntakeWorkflow {
    /// Handles one turn in an intake thread: asks for the missing fields, or replies and reports.
    pub async fn run_turn(&self, ctx: &TurnContext, intake: &Intake) -> anyhow::Result<Outcome> {
        let missing = self.missing_fields(ctx, intake).await?;
        if missing.is_empty() {
            return self.reply_and_report(ctx, intake).await;
        }
        self.ask_for(ctx, intake, &missing).await
    }

    /// Completes an intake when its reporter does not answer, with the fields that are missing at
    /// that time.
    pub async fn complete_inactive(
        &self,
        ctx: &TurnContext,
        intake: &Intake,
    ) -> anyhow::Result<Report> {
        let missing = self.missing_fields(ctx, intake).await?;
        let options = TurnOptions {
            extra: None,
            instruction: Some(INACTIVE_INSTRUCTION),
        };
        let result = self.turn(ctx, intake, options).await?;
        self.complete_report(ctx, intake, &result, missing).await
    }

    async fn missing_fields(
        &self,
        ctx: &TurnContext,
        intake: &Intake,
    ) -> anyhow::Result<Vec<String>> {
        missing_fields(
            &self.deps.jev,
            intake.kind,
            jev_lines(&ctx.history),
            &ctx.stats,
        )
        .await
    }

    async fn ask_for(
        &self,
        ctx: &TurnContext,
        intake: &Intake,
        missing: &[String],
    ) -> anyhow::Result<Outcome> {
        let extra = missing_fields_note(ctx, intake.kind, missing);
        let options = TurnOptions {
            extra: Some(&extra),
            instruction: None,
        };
        let result = self.turn(ctx, intake, options).await?;
        Ok(Outcome::Intake {
            reply: non_empty_reply(result),
            report: None,
        })
    }

    async fn reply_and_report(
        &self,
        ctx: &TurnContext,
        intake: &Intake,
    ) -> anyhow::Result<Outcome> {
        let options = TurnOptions {
            extra: Some(ALL_FIELDS_PRESENT),
            instruction: None,
        };
        let result = self.turn(ctx, intake, options).await?;
        let report = self.complete_report(ctx, intake, &result, vec![]).await?;
        Ok(Outcome::Intake {
            reply: non_empty_reply(result),
            report: Some(report),
        })
    }

    async fn turn(
        &self,
        ctx: &TurnContext,
        intake: &Intake,
        options: TurnOptions<'_>,
    ) -> anyhow::Result<TurnResult> {
        llm_turn(&self.deps, ctx, workflow_prompt(intake.kind), options).await
    }

    /// Extracts the report from the conversation and Nuki's last turn.
    async fn complete_report(
        &self,
        ctx: &TurnContext,
        intake: &Intake,
        last_turn: &TurnResult,
        missing: Vec<String>,
    ) -> anyhow::Result<Report> {
        let system_prompt = SystemPrompt::build(workflow_prompt(intake.kind), None, &ctx.runtime);
        let history = extraction_history(ctx, last_turn)?;
        let model = self.deps.model.current();
        let llm = &self.deps.llm;
        let body = match intake.kind {
            IntakeKind::Bug => ReportBody::Bug(
                llm.extract(
                    &model,
                    &system_prompt,
                    history,
                    REPORT_INSTRUCTION,
                    &ctx.stats,
                )
                .await?,
            ),
            IntakeKind::Feature => ReportBody::Feature(
                llm.extract(
                    &model,
                    &system_prompt,
                    history,
                    REPORT_INSTRUCTION,
                    &ctx.stats,
                )
                .await?,
            ),
        };
        Ok(Report {
            body,
            reporter_id: intake.reporter_id,
            thread_id: intake.thread_id,
            missing,
        })
    }
}

/// Builds the extraction history: the conversation, then Nuki's tool turn, then her final text
/// if the text is not empty. The tool results are the evidence for `fixed_in` and
/// `already_exists`.
fn extraction_history(
    ctx: &TurnContext,
    last_turn: &TurnResult,
) -> Result<Vec<Message>, serde_json::Error> {
    let conversation = build_history(&ctx.history, &ctx.stored_turns)?;
    let tool_turn = last_turn
        .turn_messages
        .iter()
        .map(Message::deserialize)
        .collect::<Result<Vec<_>, _>>()?;
    let final_text = final_text_message(&last_turn.text);
    Ok(conversation
        .into_iter()
        .chain(tool_turn)
        .chain(final_text)
        .collect())
}

fn final_text_message(text: &str) -> Option<Message> {
    if text.is_empty() {
        return None;
    }
    Some(Message::assistant(text))
}

fn missing_fields_note(ctx: &TurnContext, kind: IntakeKind, missing: &[String]) -> String {
    let note = format!("Missing fields: {}.", missing.join(", "));
    if kind == IntakeKind::Feature && is_first_turn(ctx) {
        return format!("{note}\n{FIRST_FEATURE_TURN}");
    }
    note
}

fn workflow_prompt(kind: IntakeKind) -> WorkflowPrompt {
    match kind {
        IntakeKind::Bug => WorkflowPrompt::BugIntake,
        IntakeKind::Feature => WorkflowPrompt::FeatureIntake,
    }
}

/// Returns `true` if the history has no message from Nuki.
fn is_first_turn(ctx: &TurnContext) -> bool {
    !ctx.history.iter().any(|resolved| resolved.message.is_nuki)
}

fn non_empty_reply(result: TurnResult) -> Option<Reply> {
    if result.text.is_empty() {
        return None;
    }
    Some(Reply::from(result))
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use serde_json::{Value, json};
    use wiremock::MockServer;

    use super::report::{BugReport, FixedIn, ReportBody};
    use super::*;
    use crate::decisions::test_support::mount_jev_answers;
    use crate::discord::chat_message::ChatMessage;
    use crate::discord::test_support::{nuki_message, user_message};
    use crate::llm::script_tool::with_reminder;
    use crate::llm::test_support::{
        assert_echo_tool_turn, assert_run_script_tool, llm_request_bodies, messages,
        messages_after_system_prompt, mount_llm_replies,
    };
    use crate::stats::test_support::{events_between, of_kind, run_event, test_run};
    use crate::store::intakes::IntakeStatus;
    use crate::workflows::Reply;
    use crate::workflows::test_support::{
        bug_field_answers, bug_field_names, bug_fields_missing, context, deps,
        missing_bug_field_names, user_turn_text,
    };

    const EMPTY_FINAL_TEXT: &str =
        include_str!("../../../tests/fixtures/llm/empty_final_text.json");
    const BUG_FINAL_REPLY: &str =
        include_str!("../../../tests/fixtures/workflows/intake_bug_final_reply.json");
    const BUG_REPORT_SUBMIT: &str =
        include_str!("../../../tests/fixtures/workflows/intake_bug_report_submit.json");
    const BUG_INACTIVE_CHECKS: &str =
        include_str!("../../../tests/fixtures/workflows/intake_bug_inactive_checks.json");
    const BUG_INACTIVE_REPORT_SUBMIT: &str =
        include_str!("../../../tests/fixtures/workflows/intake_bug_inactive_report_submit.json");
    const RUN_SCRIPT_CALL: &str = include_str!("../../../tests/fixtures/llm/run_script_call.json");

    const BOB_ID: u64 = 1300000000000000002;
    const THREAD_ID: u64 = 1400000000000000001;

    fn intake(kind: IntakeKind) -> Intake {
        let created_at = Utc.with_ymd_and_hms(2026, 10, 19, 12, 30, 0).unwrap();
        Intake {
            id: 1,
            kind,
            thread_id: THREAD_ID,
            origin_channel_id: 1200000000000000001,
            origin_message_id: 1,
            reporter_id: BOB_ID,
            status: IntakeStatus::Open,
            last_activity_at: created_at,
            created_at,
        }
    }

    async fn workflow(server: &MockServer) -> IntakeWorkflow {
        IntakeWorkflow {
            deps: deps(server).await,
        }
    }

    fn bug_fields_present() -> Value {
        bug_field_answers(|_| 0.9)
    }

    fn report_instruction() -> Value {
        json!({
            "role": "user",
            "content": "Write the report of this conversation. Use \"unknown\" for information that the conversation does not give."
        })
    }

    fn alice_message(id: u64, content: &str) -> ChatMessage {
        ChatMessage {
            author_id: 1300000000000000003,
            author_name: "Alice".to_string(),
            author_roles: vec![],
            ..user_message(id, content)
        }
    }

    fn complete_bug_conversation() -> Vec<ChatMessage> {
        vec![
            user_message(
                1,
                "playback stops after one track, any album, v1.4.0 AppImage on Linux",
            ),
            nuki_message(2, "Which plugins and sources do you use?"),
            user_message(3, "no plugins, MusicBrainz and YouTube"),
        ]
    }

    fn complete_bug_report() -> Report {
        Report {
            body: ReportBody::Bug(BugReport {
                title: "Playback stops after one track".to_string(),
                version: "v1.4.0".to_string(),
                platform: "Linux, AppImage".to_string(),
                plugins: "none".to_string(),
                sources: "MusicBrainz metadata, YouTube streaming".to_string(),
                item: "Every album, for example Discovery by Daft Punk".to_string(),
                summary: "Playback stops after the first track of an album. The queue keeps the next tracks, but Nuclear does not start them."
                    .to_string(),
                fixed_in: Some(FixedIn {
                    version: "v1.5.0".to_string(),
                    commit: "3f9c2ab".to_string(),
                }),
            }),
            reporter_id: BOB_ID,
            thread_id: THREAD_ID,
            missing: vec![],
        }
    }

    /// Returns the history, the tool turn, the final reply, and the report instruction.
    fn extraction_messages_with_tool_turn() -> Value {
        json!([
            {
                "role": "user",
                "content": user_turn_text(1, "playback stops after one track, any album, v1.4.0 AppImage on Linux")
            },
            { "role": "assistant", "content": "Which plugins and sources do you use?" },
            {
                "role": "user",
                "content": user_turn_text(3, "no plugins, MusicBrainz and YouTube")
            },
            {
                "role": "assistant",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {
                        "name": "run_script",
                        "arguments": "{\"code\":\"return await test.echo({ a: 1 })\"}"
                    }
                }]
            },
            {
                "role": "tool",
                "tool_call_id": "call_1",
                "content": with_reminder("{\"value\":{\"a\":1}}")
            },
            {
                "role": "assistant",
                "content": "Nuclear v1.5.0 has a fix for gapless playback that possibly fixes this. Please update. I sent the report to the team."
            },
            report_instruction()
        ])
    }

    #[tokio::test]
    async fn a_turn_with_missing_fields_asks_for_them() {
        let server = MockServer::start().await;
        mount_jev_answers(&server, bug_fields_missing()).await;
        mount_llm_replies(
            &server,
            &[include_str!(
                "../../../tests/fixtures/workflows/intake_bug_ask.json"
            )],
        )
        .await;
        let ctx = context(vec![
            user_message(1, "playback stops after one track"),
            nuki_message(2, "Which Nuclear version do you use?"),
            user_message(3, "v1.4.0, with MusicBrainz and YouTube, on any album"),
        ])
        .await;

        let outcome = workflow(&server)
            .await
            .run_turn(&ctx, &intake(IntakeKind::Bug))
            .await
            .unwrap();

        assert_eq!(
            outcome,
            Outcome::Intake {
                reply: Some(Reply {
                    text:
                        "Thanks! Which operating system are you on, and which plugins do you use?"
                            .to_string(),
                    turn_messages: vec![],
                }),
                report: None,
            }
        );
        assert_run_script_tool(&llm_request_bodies(&server).await[0]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_turn_with_all_fields_replies_and_reports() {
        let server = MockServer::start().await;
        mount_jev_answers(&server, bug_fields_present()).await;
        mount_llm_replies(
            &server,
            &[RUN_SCRIPT_CALL, BUG_FINAL_REPLY, BUG_REPORT_SUBMIT],
        )
        .await;
        let ctx = context(complete_bug_conversation()).await;

        let outcome = workflow(&server)
            .await
            .run_turn(&ctx, &intake(IntakeKind::Bug))
            .await
            .unwrap();

        let Outcome::Intake {
            reply: Some(reply),
            report: Some(report),
        } = outcome
        else {
            panic!("expected an intake outcome with a reply and a report, got {outcome:?}");
        };
        assert_eq!(
            reply.text,
            "Nuclear v1.5.0 has a fix for gapless playback that possibly fixes this. Please update. I sent the report to the team."
        );
        assert_echo_tool_turn(messages(reply.turn_messages));
        assert_eq!(report, complete_bug_report());
        let bodies = llm_request_bodies(&server).await;
        assert_run_script_tool(&bodies[0]);
        assert_eq!(
            messages_after_system_prompt(&bodies[2]),
            extraction_messages_with_tool_turn()
        );
    }

    #[tokio::test]
    async fn an_all_fields_turn_with_empty_text_has_no_reply() {
        let server = MockServer::start().await;
        mount_jev_answers(&server, bug_fields_present()).await;
        mount_llm_replies(&server, &[EMPTY_FINAL_TEXT, BUG_REPORT_SUBMIT]).await;
        let (store, stats) = test_run().await;
        let ctx = TurnContext {
            stats,
            ..context(complete_bug_conversation()).await
        };
        let before = Utc::now();

        let outcome = workflow(&server)
            .await
            .run_turn(&ctx, &intake(IntakeKind::Bug))
            .await
            .unwrap();

        let after = Utc::now();
        assert_eq!(
            outcome,
            Outcome::Intake {
                reply: None,
                report: Some(complete_bug_report()),
            }
        );
        let submit: Value = serde_json::from_str(BUG_REPORT_SUBMIT).unwrap();
        let submitted_text =
            &submit["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"];
        let submitted: Value = serde_json::from_str(submitted_text.as_str().unwrap()).unwrap();
        let events = events_between(&store, before, after).await;
        let extract_calls: Vec<_> = of_kind(events.clone(), "llm.call")
            .into_iter()
            .filter(|event| event.data["purpose"] == "extract")
            .collect();
        assert_eq!(
            of_kind(events, "jev.call"),
            vec![run_event(
                "jev.call",
                json!({
                    "purpose": "intake_fields",
                    "questions": bug_field_names(),
                    "answers": bug_fields_present(),
                    "model": "typesafe/jev-1.14-20261014",
                    "input_tokens": null,
                    "output_tokens": null,
                    "cost_usd": null,
                    "error": null,
                })
            )]
        );
        assert_eq!(
            extract_calls,
            vec![run_event(
                "llm.call",
                json!({
                    "purpose": "extract",
                    "call_index": 1,
                    "requested_model": "test/model",
                    "response_model": "test/model",
                    "provider": null,
                    "response_id": "gen-intake-report",
                    "input_tokens": 90,
                    "output_tokens": 60,
                    "total_tokens": 150,
                    "cached_input_tokens": null,
                    "cache_write_tokens": null,
                    "reasoning_tokens": null,
                    "cost_usd": null,
                    "cost_estimated": false,
                    "finish_reason": "tool_calls",
                    "output": [{
                        "type": "toolcall",
                        "id": { "provider": "call_submit" },
                        "function": { "name": "submit", "arguments": submitted },
                        "native": {
                            "item": {
                                "id": "call_submit",
                                "type": "function",
                                "function": { "name": "submit", "arguments": submitted_text }
                            },
                            // Rig's hash of the tools and the system prompt of the request.
                            "fingerprint": "7c0d5eeed8cb7cc9"
                        }
                    }],
                })
            )]
        );
    }

    #[tokio::test]
    async fn an_empty_final_text_stays_out_of_the_report_history() {
        let ctx = context(vec![
            user_message(1, "playback stops after one track, v1.4.0"),
            nuki_message(2, "Which operating system are you on?"),
        ])
        .await;
        let last_turn = TurnResult {
            text: String::new(),
            turn_messages: vec![],
        };

        let history = extraction_history(&ctx, &last_turn).unwrap();

        assert_eq!(
            history,
            [
                Message::user(user_turn_text(1, "playback stops after one track, v1.4.0")),
                Message::assistant("Which operating system are you on?"),
            ]
        );
    }

    #[tokio::test]
    async fn the_report_names_the_intake_reporter() {
        let server = MockServer::start().await;
        mount_jev_answers(&server, bug_fields_missing()).await;
        mount_llm_replies(&server, &[BUG_INACTIVE_CHECKS, BUG_INACTIVE_REPORT_SUBMIT]).await;
        let ctx = context(vec![
            alice_message(1, "Bob, tell Nuki about your playback bug here."),
            user_message(2, "playback stops after one track, v1.4.0"),
            nuki_message(3, "Which operating system are you on?"),
        ])
        .await;

        let report = workflow(&server)
            .await
            .complete_inactive(&ctx, &intake(IntakeKind::Bug))
            .await
            .unwrap();

        assert_eq!(report.reporter_id, BOB_ID);
    }

    #[tokio::test]
    async fn complete_inactive_reports_with_the_missing_fields_and_no_reply() {
        let server = MockServer::start().await;
        mount_jev_answers(&server, bug_fields_missing()).await;
        mount_llm_replies(&server, &[BUG_INACTIVE_CHECKS, BUG_INACTIVE_REPORT_SUBMIT]).await;
        let ctx = context(vec![
            user_message(
                1,
                "playback stops after one track, v1.4.0, MusicBrainz and YouTube",
            ),
            nuki_message(
                2,
                "Which operating system are you on, and which plugins do you use?",
            ),
        ])
        .await;

        let report = workflow(&server)
            .await
            .complete_inactive(&ctx, &intake(IntakeKind::Bug))
            .await
            .unwrap();

        assert_eq!(
            report,
            Report {
                body: ReportBody::Bug(BugReport {
                    title: "Playback stops after one track".to_string(),
                    version: "v1.4.0".to_string(),
                    platform: "unknown".to_string(),
                    plugins: "unknown".to_string(),
                    sources: "MusicBrainz metadata, YouTube streaming".to_string(),
                    item: "unknown".to_string(),
                    summary: "Playback stops after the first track. The reporter did not give the platform, the plugins, or the item."
                        .to_string(),
                    fixed_in: None,
                }),
                reporter_id: BOB_ID,
                thread_id: THREAD_ID,
                missing: missing_bug_field_names(),
            }
        );
        let bodies = llm_request_bodies(&server).await;
        let messages = bodies[0]["messages"].as_array().unwrap();
        assert_eq!(
            messages.last().unwrap(),
            &json!({
                "role": "user",
                "content": "The reporter does not answer. Do the final checks now. The bot does not send your text to the thread."
            })
        );
        assert_run_script_tool(&bodies[0]);
    }
}
