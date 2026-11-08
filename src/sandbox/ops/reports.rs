use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serenity::builder::{CreateMessage, CreateThread};
use serenity::http::Http;
use serenity::model::channel::Message;
use serenity::model::id::{ChannelId, MessageId};

use super::discord::{DiscordOpError, parse_id};
use crate::admin::reports::{Report, ReportKind, report_embed};
use crate::discord::history::is_thread;
use crate::discord::reply::thread_name;
use crate::sandbox::operation::{Operation, OperationTable, Param};
use crate::stats::event::{Event, ReportSent, ThreadCreated};
use crate::stats::recorder::RunStats;

const NAMESPACE: &str = "reports";

/// The client and the data that the `reports.*` operations use.
pub struct ReportsOps {
    pub http: Arc<Http>,
    pub admin_log_channel_id: u64,
    /// The recorder of the run in which the scripts call the operations.
    pub stats: RunStats,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendParams {
    kind: ReportKind,
    channel_id: String,
    message_id: String,
    title: String,
    summary: String,
    details: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SentReport {
    thread_id: String,
}

/// Returns the operations that send reports to the team.
pub fn reports_table(ops: ReportsOps) -> OperationTable {
    let ops = Arc::new(ops);
    OperationTable::new(vec![send(&ops)])
}

fn send(ops: &Arc<ReportsOps>) -> Operation {
    Operation::with_handle(
        ops,
        NAMESPACE,
        "send",
        "Send a bug report or a feature request to the team. If the message is not in a thread, makes a thread from it.",
        vec![
            Param {
                name: "kind",
                ts_type: "\"bug\" | \"feature\"",
                optional: false,
                description: "A bug report or a feature request.",
            },
            Param {
                name: "channelId",
                ts_type: "string",
                optional: false,
                description: "The ID of the channel or the thread of the message.",
            },
            Param {
                name: "messageId",
                ts_type: "string",
                optional: false,
                description: "The ID of the message of the reporter.",
            },
            Param {
                name: "title",
                ts_type: "string",
                optional: false,
                description: "A short title.",
            },
            Param {
                name: "summary",
                ts_type: "string",
                optional: false,
                description: "The problem or the request, in a few sentences.",
            },
            Param {
                name: "details",
                ts_type: "string",
                optional: true,
                description: "Facts such as the version, the platform, a fix in a newer version, or a feature that already exists.",
            },
        ],
        "{ threadId: string }",
        |ops, params: SendParams| async move { ops.send(params).await },
    )
}

impl ReportsOps {
    /// Sends the report embed to the admin log channel, and records `report.sent`. Returns the
    /// thread of the report.
    async fn send(&self, params: SendParams) -> Result<SentReport, DiscordOpError> {
        let channel_id = ChannelId::new(parse_id(&params.channel_id)?);
        let message_id = MessageId::new(parse_id(&params.message_id)?);
        let message = channel_id.message(&self.http, message_id).await?;
        let thread_id = self.thread_of(channel_id, &message).await?;
        let reporter_id = message.author.id.get();
        let report = Report {
            kind: params.kind,
            title: params.title,
            summary: params.summary,
            details: params.details,
            reporter_id,
            thread_id: thread_id.get(),
        };
        ChannelId::new(self.admin_log_channel_id)
            .send_message(
                &self.http,
                CreateMessage::new().embed(report_embed(&report)),
            )
            .await?;
        let sent = ReportSent {
            kind: report.kind,
            thread_id: report.thread_id,
            message_id: message_id.get(),
            title: report.title,
        };
        self.stats
            .record(Some(reporter_id), Event::ReportSent(sent))
            .await;
        Ok(SentReport {
            thread_id: thread_id.to_string(),
        })
    }

    /// If `channel_id` is a thread, returns that thread. If not, makes a public thread from
    /// `message`, and records `thread.created`.
    async fn thread_of(
        &self,
        channel_id: ChannelId,
        message: &Message,
    ) -> Result<ChannelId, DiscordOpError> {
        let channel = channel_id
            .to_channel(&self.http)
            .await?
            .guild()
            .ok_or(DiscordOpError::NotInServer(channel_id.get()))?;
        if is_thread(&channel) {
            return Ok(channel.id);
        }
        let name = thread_name(&message.content, message.author.display_name());
        let thread = channel
            .id
            .create_thread_from_message(&self.http, message.id, CreateThread::new(name))
            .await?;
        let created = ThreadCreated {
            thread_id: thread.id.get(),
            message_id: message.id.get(),
            name: thread.name,
        };
        self.stats
            .record(Some(message.author.id.get()), Event::ThreadCreated(created))
            .await;
        Ok(thread.id)
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::{Value, json};
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer};

    use super::*;
    use crate::sandbox::runtime::{Sandbox, SandboxLimits, ScriptOutcome};
    use crate::sandbox::test_support::value;
    use crate::stats::test_support::{RunEvent, events_between, run_event, test_run};
    use crate::test_support::{discord_http, json_response};

    const TEXT_CHANNEL: &str =
        include_str!("../../../tests/fixtures/discord/history_text_channel.json");
    const THREAD: &str = include_str!("../../../tests/fixtures/discord/history_thread.json");
    const ADMIN_SENT_MESSAGE: &str =
        include_str!("../../../tests/fixtures/discord/admin_log_sent_message.json");

    const ADMIN_CHANNEL_ID: u64 = 1200000000000000009;
    const TEXT_CHANNEL_ID: u64 = 1200000000000000001;
    /// The thread that Discord makes from the message `THREAD_ID`, as threads have the ID of
    /// their starter message.
    const THREAD_ID: u64 = 1100000000000000003;
    const MESSAGE_IN_THREAD_ID: u64 = 1100000000000000007;
    const REPORTER_ID: u64 = 1300000000000000002;
    const QUESTION: &str = "nuclear is playing wrong streams, what should I do?";

    const SEND_FROM_CHANNEL: &str = "return await reports.send({ kind: 'bug', channelId: '1200000000000000001', messageId: '1100000000000000003', title: 'Wrong streams', summary: 'Nuclear plays the wrong stream for some tracks.', details: 'Version 1.3.0 on Linux.' });";
    const SEND_FROM_THREAD: &str = "return await reports.send({ kind: 'feature', channelId: '1100000000000000003', messageId: '1100000000000000007', title: 'Stream picker', summary: 'The reporter wants to pick the stream of a track.' });";

    fn message_json(id: u64, channel_id: u64, content: &str) -> String {
        json!({
            "id": id.to_string(),
            "channel_id": channel_id.to_string(),
            "author": { "id": REPORTER_ID.to_string(), "username": "bob", "global_name": "Bob" },
            "content": content,
            "timestamp": "2026-10-19T12:35:00.000000+00:00",
            "tts": false,
            "mention_everyone": false,
            "mentions": [],
            "mention_roles": [],
            "attachments": [],
            "embeds": [],
            "pinned": false,
            "type": 0
        })
        .to_string()
    }

    async fn mount_get(server: &MockServer, url_path: String, body: &str) {
        Mock::given(method("GET"))
            .and(path(url_path))
            .respond_with(json_response(200, body))
            .mount(server)
            .await;
    }

    async fn mount_admin_channel(server: &MockServer) {
        Mock::given(method("POST"))
            .and(path(format!(
                "/api/v10/channels/{ADMIN_CHANNEL_ID}/messages"
            )))
            .respond_with(json_response(200, ADMIN_SENT_MESSAGE))
            .mount(server)
            .await;
    }

    /// Mounts the text channel, the question in it, and the thread that Discord makes from the
    /// question. Discord makes the thread only one time.
    async fn mount_question_in_text_channel(server: &MockServer) {
        mount_get(
            server,
            format!("/api/v10/channels/{TEXT_CHANNEL_ID}"),
            TEXT_CHANNEL,
        )
        .await;
        mount_get(
            server,
            format!("/api/v10/channels/{TEXT_CHANNEL_ID}/messages/{THREAD_ID}"),
            &message_json(THREAD_ID, TEXT_CHANNEL_ID, QUESTION),
        )
        .await;
        Mock::given(method("POST"))
            .and(path(format!(
                "/api/v10/channels/{TEXT_CHANNEL_ID}/messages/{THREAD_ID}/threads"
            )))
            .and(body_json(json!({ "name": QUESTION })))
            .respond_with(json_response(201, THREAD))
            .expect(1)
            .mount(server)
            .await;
        mount_admin_channel(server).await;
    }

    /// Mounts the thread and a message in it. Discord makes no thread.
    async fn mount_message_in_thread(server: &MockServer) {
        mount_get(server, format!("/api/v10/channels/{THREAD_ID}"), THREAD).await;
        mount_get(
            server,
            format!("/api/v10/channels/{THREAD_ID}/messages/{MESSAGE_IN_THREAD_ID}"),
            &message_json(MESSAGE_IN_THREAD_ID, THREAD_ID, "still wrong streams"),
        )
        .await;
        Mock::given(method("POST"))
            .and(path(format!(
                "/api/v10/channels/{THREAD_ID}/messages/{MESSAGE_IN_THREAD_ID}/threads"
            )))
            .respond_with(json_response(201, THREAD))
            .expect(0)
            .mount(server)
            .await;
        mount_admin_channel(server).await;
    }

    async fn run_in(server: &MockServer, stats: RunStats, code: &str) -> ScriptOutcome {
        let ops = ReportsOps {
            http: Arc::new(discord_http(server)),
            admin_log_channel_id: ADMIN_CHANNEL_ID,
            stats,
        };
        Sandbox::new(Arc::new(reports_table(ops)), SandboxLimits::production())
            .run(code)
            .await
            .outcome
    }

    /// Runs `code` in a new run. Returns the outcome and the events of the run.
    async fn run_recorded(server: &MockServer, code: &str) -> (ScriptOutcome, Vec<RunEvent>) {
        let (store, stats) = test_run().await;
        let before = Utc::now();
        let outcome = run_in(server, stats, code).await;
        let after = Utc::now();
        (outcome, events_between(&store, before, after).await)
    }

    /// Returns the body of the only message to the admin log channel.
    async fn admin_message(server: &MockServer) -> Value {
        let admin_path = format!("/api/v10/channels/{ADMIN_CHANNEL_ID}/messages");
        let requests = server.received_requests().await.unwrap();
        let [request] = requests
            .iter()
            .filter(|request| request.url.path() == admin_path)
            .collect::<Vec<_>>()
            .try_into()
            .unwrap();
        serde_json::from_slice(&request.body).unwrap()
    }

    fn reporter_event(kind: &str, data: Value) -> RunEvent {
        RunEvent {
            user_id: Some(REPORTER_ID),
            ..run_event(kind, data)
        }
    }

    #[tokio::test]
    async fn send_in_a_text_channel_makes_a_thread_from_the_message_and_returns_it() {
        let server = MockServer::start().await;
        mount_question_in_text_channel(&server).await;

        let (outcome, _) = run_recorded(&server, SEND_FROM_CHANNEL).await;

        assert_eq!(outcome, value(json!({ "threadId": "1100000000000000003" })));
    }

    #[tokio::test]
    async fn send_sends_the_report_embed_to_the_admin_log_channel() {
        let server = MockServer::start().await;
        mount_question_in_text_channel(&server).await;

        run_recorded(&server, SEND_FROM_CHANNEL).await;

        assert_eq!(
            admin_message(&server).await,
            json!({
                "tts": false,
                "embeds": [{
                    "type": "rich",
                    "title": "🐛 Bug: Wrong streams",
                    "description": "Nuclear plays the wrong stream for some tracks.",
                    "color": 0xE74C3C,
                    "fields": [
                        { "name": "Reporter", "value": "<@1300000000000000002>", "inline": true },
                        { "name": "Thread", "value": "<#1100000000000000003>", "inline": true },
                        { "name": "Details", "value": "Version 1.3.0 on Linux.", "inline": false },
                    ],
                }],
                "sticker_ids": [],
                "attachments": [],
                "enforce_nonce": false
            })
        );
    }

    #[tokio::test]
    async fn send_in_a_text_channel_records_the_new_thread_and_the_report() {
        let server = MockServer::start().await;
        mount_question_in_text_channel(&server).await;

        let (_, events) = run_recorded(&server, SEND_FROM_CHANNEL).await;

        assert_eq!(
            events,
            vec![
                reporter_event(
                    "thread.created",
                    json!({
                        "thread_id": THREAD_ID,
                        "message_id": THREAD_ID,
                        "name": "nuclear is playing wrong streams, what should I do?",
                    }),
                ),
                reporter_event(
                    "report.sent",
                    json!({
                        "kind": "bug",
                        "thread_id": THREAD_ID,
                        "message_id": THREAD_ID,
                        "title": "Wrong streams",
                    }),
                ),
            ]
        );
    }

    #[tokio::test]
    async fn send_in_a_thread_uses_that_thread() {
        let server = MockServer::start().await;
        mount_message_in_thread(&server).await;

        let (outcome, _) = run_recorded(&server, SEND_FROM_THREAD).await;

        assert_eq!(outcome, value(json!({ "threadId": "1100000000000000003" })));
    }

    #[tokio::test]
    async fn send_in_a_thread_records_only_the_report() {
        let server = MockServer::start().await;
        mount_message_in_thread(&server).await;

        let (_, events) = run_recorded(&server, SEND_FROM_THREAD).await;

        assert_eq!(
            events,
            vec![reporter_event(
                "report.sent",
                json!({
                    "kind": "feature",
                    "thread_id": THREAD_ID,
                    "message_id": MESSAGE_IN_THREAD_ID,
                    "title": "Stream picker",
                }),
            )]
        );
    }
}
