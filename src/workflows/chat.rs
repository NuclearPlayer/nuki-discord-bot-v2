use super::{Outcome, Reply, TurnContext, TurnOptions, Workflow, WorkflowDeps, llm_turn};
use crate::prompts::system::WorkflowPrompt;

pub struct ChatWorkflow {
    pub deps: WorkflowDeps,
}

impl Workflow for ChatWorkflow {
    /// An empty text is a valid answer. If the text is empty, Nuki only reacted, and the bot
    /// sends nothing.
    async fn run(&self, ctx: &TurnContext) -> anyhow::Result<Outcome> {
        let options = TurnOptions {
            extra: None,
            instruction: None,
            with_tools: true,
        };
        let result = llm_turn(&self.deps, ctx, WorkflowPrompt::Chat, options).await?;
        if result.text.is_empty() {
            return Ok(Outcome::Nothing);
        }
        Ok(Outcome::Reply(Reply::from(result)))
    }
}

#[cfg(test)]
mod tests {
    use wiremock::MockServer;

    use super::*;
    use crate::discord::test_support::user_message;
    use crate::llm::test_support::{assert_run_script_tool, llm_request_bodies, mount_llm_replies};
    use crate::routing::trigger::Trigger;
    use crate::workflows::test_support::{context, deps};

    const CHAT_REPLY: &str = include_str!("../../tests/fixtures/workflows/chat_reply.json");

    async fn run_chat(server: &MockServer) -> Outcome {
        let workflow = ChatWorkflow {
            deps: deps(server).await,
        };
        let ctx = context(
            Trigger::Unsolicited,
            vec![user_message(1, "my playlist is 900 hours of vaporwave")],
        )
        .await;
        workflow.run(&ctx).await.unwrap()
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
            })
        );
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
}
