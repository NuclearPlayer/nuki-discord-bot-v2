use std::sync::Arc;

use serenity::builder::CreateMessage;
use serenity::constants::MESSAGE_CODE_LIMIT;
use serenity::http::Http;
use serenity::model::channel::MessageFlags;
use serenity::model::id::ChannelId;

use crate::stats::event::{ErrorRaised, Event};
use crate::stats::recorder::{EventScope, Stats};

/// Records the errors of failed workflows, and sends them to the admin log channel.
pub struct AdminLog {
    pub http: Arc<Http>,
    pub channel_id: u64,
    pub stats: Stats,
}

impl AdminLog {
    /// Records the `error` event in `scope`, and sends the error with the channel of `scope`.
    pub async fn report_error(&self, scope: EventScope, workflow: &str, error: &anyhow::Error) {
        let conversation_channel_id = scope.channel_id;
        let raised = ErrorRaised {
            workflow: workflow.to_string(),
            error: format!("{error:#}"),
        };
        self.stats.record(scope, Event::ErrorRaised(raised)).await;
        tracing::error!(
            workflow,
            conversation_channel_id = ?conversation_channel_id,
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

/// An error outside a channel has no place in the message.
fn error_message(
    workflow: &str,
    conversation_channel_id: Option<u64>,
    error: &anyhow::Error,
) -> String {
    let place = match conversation_channel_id {
        Some(channel_id) => format!(" in <#{channel_id}>"),
        None => String::new(),
    };
    format!("⚠️ {workflow} failed{place}: {error:#}")
        .chars()
        .take(MESSAGE_CODE_LIMIT)
        .collect()
}

#[cfg(test)]
mod tests {
    use anyhow::{Context, anyhow};
    use chrono::Utc;
    use serde_json::{Value, json};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    use super::*;
    use crate::stats::test_support::{events_between, run_event, test_run};
    use crate::store::Store;
    use crate::test_support::{discord_http, json_response};

    const ADMIN_CHANNEL_ID: u64 = 1200000000000000009;
    const SENT_MESSAGE: &str =
        include_str!("../../tests/fixtures/discord/admin_log_sent_message.json");

    /// The admin log on a Discord mock that accepts the messages of the admin log channel.
    async fn admin_log(discord: &MockServer, store: &Store) -> AdminLog {
        Mock::given(method("POST"))
            .and(path(format!(
                "/api/v10/channels/{ADMIN_CHANNEL_ID}/messages"
            )))
            .respond_with(json_response(200, SENT_MESSAGE))
            .mount(discord)
            .await;
        AdminLog {
            http: Arc::new(discord_http(discord)),
            channel_id: ADMIN_CHANNEL_ID,
            stats: Stats::new(store.events()),
        }
    }

    #[tokio::test]
    async fn report_error_sends_the_error_message_without_link_embeds() {
        let discord = MockServer::start().await;
        let (store, run) = test_run().await;
        let admin_log = admin_log(&discord, &store).await;

        admin_log
            .report_error(
                run.scope(None),
                "Help",
                &anyhow!("see https://github.com/nukeop/nuclear"),
            )
            .await;

        let requests = discord.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(
            body,
            json!({
                "content": "⚠️ Help failed in <#20>: see https://github.com/nukeop/nuclear",
                "flags": MessageFlags::SUPPRESS_EMBEDS.bits(),
                "tts": false,
                "embeds": [],
                "sticker_ids": [],
                "attachments": [],
                "enforce_nonce": false
            })
        );
    }

    #[tokio::test]
    async fn report_error_records_the_error_event() {
        let discord = MockServer::start().await;
        let (store, run) = test_run().await;
        let admin_log = admin_log(&discord, &store).await;
        let error = Err::<(), _>(anyhow!("connection refused"))
            .context("the Jev request failed")
            .unwrap_err();
        let before = Utc::now();

        admin_log
            .report_error(run.scope(None), "help_request", &error)
            .await;

        let after = Utc::now();
        assert_eq!(
            events_between(&store, before, after).await,
            vec![run_event(
                "error",
                json!({
                    "workflow": "help_request",
                    "error": "the Jev request failed: connection refused",
                }),
            )]
        );
    }

    #[test]
    fn error_message_contains_the_workflow_the_channel_and_the_error_chain() {
        let error = Err::<(), _>(anyhow!("connection refused"))
            .context("the Jev request failed")
            .unwrap_err();

        let message = error_message("Help", Some(333), &error);

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

        let message = error_message("Chat", Some(333), &error);

        assert_eq!(message, format!("{prefix}{}", "é".repeat(kept)));
    }
}
