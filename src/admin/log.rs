use std::sync::Arc;

use serenity::constants::MESSAGE_CODE_LIMIT;
use serenity::http::Http;
use serenity::model::id::ChannelId;

/// Sends the errors of failed workflows to the admin log channel.
pub struct AdminLog {
    pub http: Arc<Http>,
    pub channel_id: u64,
}

impl AdminLog {
    pub async fn report_error(
        &self,
        workflow: &str,
        conversation_channel_id: u64,
        error: &anyhow::Error,
    ) {
        tracing::error!(
            workflow,
            conversation_channel_id,
            "workflow failed: {error:#}"
        );

        let message = error_message(workflow, conversation_channel_id, error);
        let sent = ChannelId::new(self.channel_id)
            .say(&self.http, message)
            .await;
        if let Err(send_error) = sent {
            tracing::error!(
                admin_channel_id = self.channel_id,
                "could not send the error to the admin log: {send_error}"
            );
        }
    }
}

fn error_message(workflow: &str, conversation_channel_id: u64, error: &anyhow::Error) -> String {
    format!("⚠️ {workflow} failed in <#{conversation_channel_id}>: {error:#}")
        .chars()
        .take(MESSAGE_CODE_LIMIT)
        .collect()
}

#[cfg(test)]
mod tests {
    use anyhow::{Context, anyhow};

    use super::*;

    #[test]
    fn error_message_contains_the_workflow_the_channel_and_the_error_chain() {
        let error = Err::<(), _>(anyhow!("connection refused"))
            .context("the Jev request failed")
            .unwrap_err();

        let message = error_message("Help", 333, &error);

        assert_eq!(
            message,
            "⚠️ Help failed in <#333>: the Jev request failed: connection refused"
        );
    }

    #[test]
    fn error_message_is_truncated_to_2000_characters() {
        let error = anyhow!("é".repeat(3000));
        let prefix = "⚠️ Chat failed in <#333>: ";
        let kept = 2000 - prefix.chars().count();

        let message = error_message("Chat", 333, &error);

        assert_eq!(message, format!("{prefix}{}", "é".repeat(kept)));
    }
}
