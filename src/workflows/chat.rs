use super::{Outcome, Reply, TurnContext, WorkflowDeps, llm_turn};
use crate::prompts::system::WorkflowPrompt;

pub struct ChatWorkflow {
    pub deps: WorkflowDeps,
}

impl ChatWorkflow {
    /// Runs one turn with the workflow prompt `prompt`. An empty text is a valid answer. If the
    /// text is empty, Nuki only reacted, and the bot sends nothing. If the turn sent a report,
    /// the reply goes to the thread of the report.
    pub async fn run(&self, ctx: &TurnContext, prompt: WorkflowPrompt) -> anyhow::Result<Outcome> {
        let result = llm_turn(&self.deps, ctx, prompt).await?;
        if result.text.is_empty() {
            return Ok(Outcome::Nothing);
        }
        Ok(Outcome::Reply(Reply {
            text: result.text,
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
    use crate::discord::test_support::user_message;
    use crate::llm::test_support::{
        assert_echo_tool_turn, assert_run_script_tool, llm_request_bodies, messages,
        mount_llm_replies,
    };
    use crate::prompts::system::SystemPrompt;
    use crate::sandbox::operation::{Operation, OperationTable};
    use crate::sandbox::ops::reports::ReportThread;
    use crate::sandbox::runtime::{Sandbox, SandboxLimits};
    use crate::workflows::test_support::{context, deps};

    const CHAT_REPLY: &str = include_str!("../../tests/fixtures/workflows/chat_reply.json");
    const RUN_SCRIPT_CALL: &str = include_str!("../../tests/fixtures/llm/run_script_call.json");
    const REPORT_THREAD_ID: u64 = 1100000000000000003;

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
        let server = MockServer::start().await;
        mount_llm_replies(&server, &[CHAT_REPLY]).await;

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
        let server = MockServer::start().await;
        mount_llm_replies(&server, &[CHAT_REPLY]).await;

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

    #[tokio::test(flavor = "multi_thread")]
    async fn a_run_that_sends_a_report_replies_in_the_thread_of_the_report() {
        let server = MockServer::start().await;
        mount_llm_replies(&server, &[RUN_SCRIPT_CALL, CHAT_REPLY]).await;
        let workflow = ChatWorkflow {
            deps: deps(&server).await,
        };
        let ctx = context(vec![user_message(1, "the queue crashes Nuclear")]).await;
        let ctx = TurnContext {
            sandbox: reporting_sandbox(ctx.report_thread.clone()),
            ..ctx
        };

        let outcome = workflow.run(&ctx, WorkflowPrompt::Help).await.unwrap();

        let Outcome::Reply(reply) = outcome else {
            panic!("expected a reply, got {outcome:?}");
        };
        assert_eq!(reply.report_thread_id, Some(REPORT_THREAD_ID));
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
        let server = MockServer::start().await;
        mount_llm_replies(
            &server,
            &[include_str!(
                "../../tests/fixtures/llm/empty_final_text.json"
            )],
        )
        .await;

        let outcome = run_chat(&server).await;

        assert_eq!(outcome, Outcome::Nothing);
    }

    #[tokio::test]
    async fn chat_gives_the_run_script_tool_to_the_llm() {
        let server = MockServer::start().await;
        mount_llm_replies(&server, &[CHAT_REPLY]).await;

        run_chat(&server).await;

        assert_run_script_tool(&llm_request_bodies(&server).await[0]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn chat_keeps_the_tool_turn_in_the_reply() {
        let server = MockServer::start().await;
        mount_llm_replies(&server, &[RUN_SCRIPT_CALL, CHAT_REPLY]).await;

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
}
