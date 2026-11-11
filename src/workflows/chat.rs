use super::reply_length::short_reply;
use super::{Outcome, Reply, TurnContext, WorkflowDeps, llm_turn};
use crate::prompts::system::WorkflowPrompt;

pub struct ChatWorkflow {
    pub deps: WorkflowDeps,
}

impl ChatWorkflow {
    /// Runs one turn with the workflow prompt `prompt`. An empty text is a valid answer. If the
    /// text is empty, Nuki only reacted, and the bot sends nothing. If the text is not empty,
    /// the length check of `short_reply` gives the text of the reply. The reply keeps the tool
    /// turn of the LLM turn. If the turn sent a report, the reply goes to the thread of the
    /// report.
    pub async fn run(&self, ctx: &TurnContext, prompt: WorkflowPrompt) -> anyhow::Result<Outcome> {
        let result = llm_turn(&self.deps, ctx, prompt).await?;
        if result.text.is_empty() {
            return Ok(Outcome::Nothing);
        }
        let text = short_reply(&self.deps, ctx, prompt, result.text).await?;
        Ok(Outcome::Reply(Reply {
            text,
            turn_messages: result.turn_messages,
            report_thread_id: ctx.report_thread.get(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::{Value, json};
    use wiremock::MockServer;

    use super::*;
    use crate::decisions::test_support::jev_request_bodies;
    use crate::discord::test_support::user_message;
    use crate::llm::test_support::{
        assert_echo_tool_turn, assert_run_script_tool, llm_request_bodies, llm_text_reply,
        messages, mount_llm_replies,
    };
    use crate::prompts::system::SystemPrompt;
    use crate::sandbox::operation::{Operation, OperationTable};
    use crate::sandbox::ops::reports::ReportThread;
    use crate::sandbox::runtime::{Sandbox, SandboxLimits};
    use crate::workflows::test_support::{
        SHORT_ENOUGH, TOO_LONG, context, deps, mount_too_long_probabilities,
    };

    const CHAT_REPLY: &str = include_str!("../../tests/fixtures/workflows/chat_reply.json");
    const SHORTER: &str = "900 hours? Touch grass.";
    const EMPTY_REPLY: &str = include_str!("../../tests/fixtures/llm/empty_final_text.json");
    const RUN_SCRIPT_CALL: &str = include_str!("../../tests/fixtures/llm/run_script_call.json");
    const REPORT_THREAD_ID: u64 = 1100000000000000003;

    /// Starts a server where the LLM gives `replies`, and Jev answers the reply length question
    /// with `probabilities`. Each server gives its responses in sequence.
    async fn server_with(replies: &[&str], probabilities: &[f64]) -> MockServer {
        let server = MockServer::start().await;
        mount_llm_replies(&server, replies).await;
        mount_too_long_probabilities(&server, probabilities).await;
        server
    }

    async fn run_chat(server: &MockServer) -> Outcome {
        run_with(server, WorkflowPrompt::Chat).await
    }

    async fn run_with(server: &MockServer, prompt: WorkflowPrompt) -> Outcome {
        let workflow = ChatWorkflow {
            deps: deps(server).await,
        };
        let message = user_message(1, "my playlist is 900 hours of vaporwave");
        let ctx = context(vec![message]).await;
        workflow.run(&ctx, prompt).await.unwrap()
    }

    #[tokio::test]
    async fn the_workflow_sends_the_system_prompt_of_the_given_workflow_prompt() {
        let server = server_with(&[CHAT_REPLY], &[SHORT_ENOUGH]).await;

        run_with(&server, WorkflowPrompt::Help).await;

        let ctx = context(vec![]).await;
        assert_eq!(
            llm_request_bodies(&server).await[0]["messages"][0],
            json!({
                "role": "system",
                "content": [{
                    "type": "text",
                    "text": SystemPrompt::build(WorkflowPrompt::Help, &ctx.runtime),
                }],
            })
        );
    }

    #[tokio::test]
    async fn chat_replies_with_the_text_of_the_llm() {
        let server = server_with(&[CHAT_REPLY], &[SHORT_ENOUGH]).await;

        let outcome = run_chat(&server).await;

        assert_eq!(
            outcome,
            Outcome::Reply(Reply {
                text: "900 hours? Touch grass, then come back for hour 901.".to_string(),
                turn_messages: vec![],
                report_thread_id: None,
            })
        );
    }

    #[tokio::test]
    async fn chat_replies_with_the_rewrite_when_the_text_of_the_llm_is_too_long() {
        let server = server_with(
            &[CHAT_REPLY, &llm_text_reply(SHORTER)],
            &[TOO_LONG, SHORT_ENOUGH],
        )
        .await;

        let outcome = run_chat(&server).await;

        assert_eq!(
            outcome,
            Outcome::Reply(Reply {
                text: SHORTER.to_string(),
                turn_messages: vec![],
                report_thread_id: None,
            })
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_run_that_sends_a_report_replies_in_the_thread_of_the_report() {
        let server = server_with(&[RUN_SCRIPT_CALL, CHAT_REPLY], &[SHORT_ENOUGH]).await;

        let outcome = run_with_report(&server).await;

        let Outcome::Reply(reply) = outcome else {
            panic!("expected a reply, got {outcome:?}");
        };
        assert_eq!(reply.report_thread_id, Some(REPORT_THREAD_ID));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_rewrite_goes_to_the_thread_of_the_report() {
        let server = server_with(
            &[RUN_SCRIPT_CALL, CHAT_REPLY, &llm_text_reply(SHORTER)],
            &[TOO_LONG, SHORT_ENOUGH],
        )
        .await;

        let outcome = run_with_report(&server).await;

        let Outcome::Reply(reply) = outcome else {
            panic!("expected a reply, got {outcome:?}");
        };
        assert_eq!(
            (reply.text.as_str(), reply.report_thread_id),
            (SHORTER, Some(REPORT_THREAD_ID))
        );
    }

    /// Runs the Help workflow with a sandbox whose `test.echo` sends a report.
    async fn run_with_report(server: &MockServer) -> Outcome {
        let workflow = ChatWorkflow {
            deps: deps(server).await,
        };
        let ctx = context(vec![user_message(1, "the queue crashes Nuclear")]).await;
        let ctx = TurnContext {
            sandbox: reporting_sandbox(ctx.report_thread.clone()),
            ..ctx
        };
        workflow.run(&ctx, WorkflowPrompt::Help).await.unwrap()
    }

    /// Returns a sandbox with `test.echo`, which sets `report_thread` as `reports.send` does.
    fn reporting_sandbox(report_thread: ReportThread) -> Arc<Sandbox> {
        let echo = Operation::typed(
            "test",
            "echo",
            "Sets the report thread.",
            vec![],
            "unknown",
            move |params: Value| {
                report_thread.set(REPORT_THREAD_ID);
                async { Ok::<_, String>(params) }
            },
        );
        Arc::new(Sandbox::new(
            Arc::new(OperationTable::new(vec![echo])),
            SandboxLimits::production(),
        ))
    }

    #[tokio::test]
    async fn chat_returns_nothing_when_the_text_is_empty() {
        let server = server_with(&[EMPTY_REPLY], &[]).await;

        let outcome = run_chat(&server).await;

        assert_eq!(outcome, Outcome::Nothing);
    }

    #[tokio::test]
    async fn an_empty_text_makes_no_jev_call() {
        let server = server_with(&[EMPTY_REPLY], &[SHORT_ENOUGH]).await;

        run_chat(&server).await;

        assert_eq!(jev_request_bodies(&server).await, Vec::<Value>::new());
    }

    #[tokio::test]
    async fn chat_gives_the_run_script_tool_to_the_llm() {
        let server = server_with(&[CHAT_REPLY], &[SHORT_ENOUGH]).await;

        run_chat(&server).await;

        assert_run_script_tool(&llm_request_bodies(&server).await[0]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn chat_keeps_the_tool_turn_in_the_reply() {
        let server = server_with(&[RUN_SCRIPT_CALL, CHAT_REPLY], &[SHORT_ENOUGH]).await;

        let outcome = run_chat(&server).await;

        let Outcome::Reply(reply) = outcome else {
            panic!("expected a reply, got {outcome:?}");
        };
        assert_eq!(
            reply.text,
            "900 hours? Touch grass, then come back for hour 901."
        );
        assert_echo_tool_turn(messages(reply.turn_messages));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_rewrite_keeps_the_tool_turn_of_the_first_turn() {
        let server = server_with(
            &[RUN_SCRIPT_CALL, CHAT_REPLY, &llm_text_reply(SHORTER)],
            &[TOO_LONG, SHORT_ENOUGH],
        )
        .await;

        let outcome = run_chat(&server).await;

        let Outcome::Reply(reply) = outcome else {
            panic!("expected a reply, got {outcome:?}");
        };
        assert_eq!(reply.text, SHORTER);
        assert_echo_tool_turn(messages(reply.turn_messages));
    }
}
