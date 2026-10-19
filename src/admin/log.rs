use std::sync::Arc;

use serenity::builder::CreateMessage;
use serenity::constants::MESSAGE_CODE_LIMIT;
use serenity::http::Http;
use serenity::model::channel::MessageFlags;
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

        let message = CreateMessage::new()
            .content(error_message(workflow, conversation_channel_id, error))
            .flags(MessageFlags::SUPPRESS_EMBEDS);
        let sent = ChannelId::new(self.channel_id)
            .send_message(&self.http, message)
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
    use serde_json::{Value, json};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    use super::*;
    use crate::test_support::{discord_http, json_response};

    const ADMIN_CHANNEL_ID: u64 = 1200000000000000009;
    const SENT_MESSAGE: &str =
        include_str!("../../tests/fixtures/discord/admin_log_sent_message.json");

    #[tokio::test]
    async fn report_error_sends_the_error_message_without_link_embeds() {
        let discord = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!(
                "/api/v10/channels/{ADMIN_CHANNEL_ID}/messages"
            )))
            .respond_with(json_response(200, SENT_MESSAGE))
            .mount(&discord)
            .await;
        let admin_log = AdminLog {
            http: Arc::new(discord_http(&discord)),
            channel_id: ADMIN_CHANNEL_ID,
        };

        admin_log
            .report_error(
                "Help",
                333,
                &anyhow!("see https://github.com/nukeop/nuclear"),
            )
            .await;

        let requests = discord.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(
            body,
            json!({
                "content": "⚠️ Help failed in <#333>: see https://github.com/nukeop/nuclear",
                "flags": MessageFlags::SUPPRESS_EMBEDS.bits(),
                "tts": false,
                "embeds": [],
                "sticker_ids": [],
                "attachments": [],
                "enforce_nonce": false
            })
        );
    }

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
