use indexmap::IndexMap;
use serde_json::json;

use super::{Outcome, Reply, TurnContext, TurnOptions, Workflow, WorkflowDeps, llm_turn};
use crate::decisions::types::{Answer, Question};
use crate::discord::context::jev_lines;
use crate::prompts::questions::help_quality;
use crate::prompts::system::WorkflowPrompt;
use crate::routing::trigger::Trigger;
use crate::stats::event::JevPurpose;

const HELP_QUALITY_QUESTION: &str = "help_quality";

pub struct HelpWorkflow {
    pub deps: WorkflowDeps,
}

impl Workflow for HelpWorkflow {
    /// Nuki writes a draft, and the bot sends it only if Jev approves the draft.
    async fn run(&self, ctx: &TurnContext) -> anyhow::Result<Outcome> {
        let options = TurnOptions {
            extra: None,
            instruction: None,
            with_tools: true,
        };
        let draft = llm_turn(&self.deps, ctx, WorkflowPrompt::Help, options).await?;
        if draft.text.is_empty() {
            tracing::info!(trigger = ?ctx.trigger, "the help draft is empty, so it is rejected");
            return self.rejected(ctx).await;
        }
        let quality = self.quality(ctx, &draft.text).await?;
        let threshold = self.deps.help_quality_threshold;
        let accepted = quality >= threshold;
        tracing::info!(quality, threshold, accepted, trigger = ?ctx.trigger, "Jev judged the help draft");
        if accepted {
            return Ok(Outcome::Reply(Reply::from(draft)));
        }
        self.rejected(ctx).await
    }
}

impl HelpWorkflow {
    /// Gets Jev's probability that the draft answers the user's question with specific
    /// information.
    async fn quality(&self, ctx: &TurnContext, draft: &str) -> anyhow::Result<f64> {
        let state = json!({
            "conversation": jev_lines(&ctx.history),
            "draft": draft,
        });
        let questions = IndexMap::from([(
            HELP_QUALITY_QUESTION.to_string(),
            Question::Noul {
                instructions: help_quality().to_string(),
            },
        )]);
        let answers = self
            .deps
            .jev
            .decide(state, questions, &ctx.stats, JevPurpose::HelpQuality)
            .await?;
        match answers.get(HELP_QUALITY_QUESTION) {
            Some(Answer::Noul { noul }) => Ok(*noul),
            other => {
                anyhow::bail!("Jev returned no noul answer for {HELP_QUALITY_QUESTION}: {other:?}")
            }
        }
    }

    /// Sends nothing for an unsolicited message. For a direct question, sends a message that
    /// says that Nuki found no good information.
    async fn rejected(&self, ctx: &TurnContext) -> anyhow::Result<Outcome> {
        match ctx.trigger {
            Trigger::Unsolicited => Ok(Outcome::Nothing),
            Trigger::Direct | Trigger::IntakeThread => self.fallback(ctx).await,
        }
    }

    /// An empty text is a valid answer. If the text is empty, Nuki only reacted, and the bot
    /// sends nothing.
    async fn fallback(&self, ctx: &TurnContext) -> anyhow::Result<Outcome> {
        let options = TurnOptions {
            extra: None,
            instruction: None,
            with_tools: false,
        };
        let fallback = llm_turn(&self.deps, ctx, WorkflowPrompt::HelpFallback, options).await?;
        if fallback.text.is_empty() {
            return Ok(Outcome::Nothing);
        }
        Ok(Outcome::Reply(Reply {
            text: fallback.text,
            turn_messages: vec![],
        }))
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::Value;
    use wiremock::MockServer;

    use super::*;
    use crate::decisions::client::JEV_MODEL;
    use crate::decisions::test_support::{
        jev_request_bodies, mount_jev_answers, without_instructions,
    };
    use crate::discord::test_support::{nuki_message, user_message};
    use crate::llm::test_support::{
        assert_echo_tool_turn, llm_request_bodies, messages, mount_llm_replies,
    };
    use crate::stats::test_support::{events_between, of_kind, run_event, test_run};
    use crate::workflows::test_support::{context, deps, user_turn_text};

    const HELP_DRAFT: &str = include_str!("../../tests/fixtures/workflows/help_draft.json");
    const HELP_FALLBACK: &str = include_str!("../../tests/fixtures/workflows/help_fallback.json");
    const RUN_SCRIPT_CALL: &str = include_str!("../../tests/fixtures/llm/run_script_call.json");

    const DRAFT_TEXT: &str = "Open Settings → Plugins, click \"Install from folder\", and pick the folder of the plugin.";

    async fn mount_help_quality(server: &MockServer, noul: f64) {
        mount_jev_answers(
            server,
            json!({ "help_quality": { "type": "noul", "noul": noul } }),
        )
        .await;
    }

    async fn run_help(deps: WorkflowDeps, trigger: Trigger) -> Outcome {
        let ctx = context(trigger, vec![user_message(1, "how do I add a plugin?")]).await;
        HelpWorkflow { deps }.run(&ctx).await.unwrap()
    }

    fn fallback_reply() -> Outcome {
        Outcome::Reply(Reply {
            text: "I dug around and found nothing solid on that, sorry. Someone on the server can probably help!".to_string(),
            turn_messages: vec![],
        })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn help_sends_the_draft_when_jev_accepts_it() {
        let server = MockServer::start().await;
        mount_llm_replies(&server, &[RUN_SCRIPT_CALL, HELP_DRAFT]).await;
        mount_help_quality(&server, 0.92).await;

        let outcome = run_help(deps(&server).await, Trigger::Direct).await;

        let Outcome::Reply(reply) = outcome else {
            panic!("expected a reply, got {outcome:?}");
        };
        assert_eq!(reply.text, DRAFT_TEXT);
        assert_echo_tool_turn(messages(reply.turn_messages));
    }

    #[tokio::test]
    async fn help_sends_the_draft_at_the_threshold() {
        let server = MockServer::start().await;
        mount_llm_replies(&server, &[HELP_DRAFT]).await;
        mount_help_quality(&server, 0.6).await;
        let deps = WorkflowDeps {
            help_quality_threshold: 0.6,
            ..deps(&server).await
        };

        let outcome = run_help(deps, Trigger::Unsolicited).await;

        assert_eq!(
            outcome,
            Outcome::Reply(Reply {
                text: DRAFT_TEXT.to_string(),
                turn_messages: vec![],
            })
        );
    }

    #[tokio::test]
    async fn help_discards_a_rejected_draft_for_an_unsolicited_message() {
        let server = MockServer::start().await;
        mount_llm_replies(&server, &[HELP_DRAFT]).await;
        mount_help_quality(&server, 0.31).await;

        let outcome = run_help(deps(&server).await, Trigger::Unsolicited).await;

        assert_eq!(outcome, Outcome::Nothing);
    }

    #[tokio::test]
    async fn help_sends_the_fallback_for_a_rejected_draft_of_a_direct_message() {
        let server = MockServer::start().await;
        mount_llm_replies(&server, &[HELP_DRAFT, HELP_FALLBACK]).await;
        mount_help_quality(&server, 0.31).await;

        let outcome = run_help(deps(&server).await, Trigger::Direct).await;

        assert_eq!(outcome, fallback_reply());
        let fallback_request = &llm_request_bodies(&server).await[1];
        assert_eq!(fallback_request["tools"], Value::Null);
    }

    #[tokio::test]
    async fn help_returns_nothing_for_an_empty_fallback() {
        let server = MockServer::start().await;
        mount_llm_replies(
            &server,
            &[
                HELP_DRAFT,
                include_str!("../../tests/fixtures/llm/empty_final_text.json"),
            ],
        )
        .await;
        mount_help_quality(&server, 0.31).await;

        let outcome = run_help(deps(&server).await, Trigger::Direct).await;

        assert_eq!(outcome, Outcome::Nothing);
    }

    #[tokio::test]
    async fn help_sends_the_conversation_and_the_draft_to_jev() {
        let server = MockServer::start().await;
        mount_llm_replies(&server, &[HELP_DRAFT]).await;
        mount_help_quality(&server, 0.92).await;
        let (store, stats) = test_run().await;
        let ctx = TurnContext {
            stats,
            ..context(
                Trigger::Direct,
                vec![
                    user_message(1, "hi nuki"),
                    nuki_message(2, "Hi! What do you need?"),
                    user_message(3, "how do I add a plugin?"),
                ],
            )
            .await
        };
        let before = Utc::now();

        HelpWorkflow {
            deps: deps(&server).await,
        }
        .run(&ctx)
        .await
        .unwrap();

        let after = Utc::now();
        assert_eq!(
            of_kind(events_between(&store, before, after).await, "jev.call"),
            vec![run_event(
                "jev.call",
                json!({
                    "purpose": "help_quality",
                    "questions": ["help_quality"],
                    "answers": { "help_quality": { "type": "noul", "noul": 0.92 } },
                    "model": "typesafe/jev-1.14-20261014",
                    "input_tokens": null,
                    "output_tokens": null,
                    "cost_usd": null,
                    "error": null,
                })
            )]
        );
        let requests: Vec<Value> = jev_request_bodies(&server)
            .await
            .into_iter()
            .map(without_instructions)
            .collect();
        assert_eq!(
            requests,
            vec![json!({
                "model": JEV_MODEL,
                "state": {
                    "conversation": [
                        user_turn_text(1, "hi nuki"),
                        "[2026-10-19T12:35:00Z][msg:2][Nuki (1700000000000000001) · roles: none]: Hi! What do you need?",
                        user_turn_text(3, "how do I add a plugin?"),
                    ],
                    "draft": DRAFT_TEXT
                },
                "questions": { "help_quality": { "type": "noul" } }
            })]
        );
    }

    #[tokio::test]
    async fn an_empty_draft_does_not_go_to_jev() {
        let server = MockServer::start().await;
        mount_llm_replies(
            &server,
            &[
                include_str!("../../tests/fixtures/llm/empty_final_text.json"),
                HELP_FALLBACK,
            ],
        )
        .await;
        mount_help_quality(&server, 0.92).await;

        let outcome = run_help(deps(&server).await, Trigger::Direct).await;

        assert_eq!(outcome, fallback_reply());
        assert_eq!(jev_request_bodies(&server).await, Vec::<Value>::new());
    }
}
