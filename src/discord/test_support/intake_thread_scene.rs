use chrono::{TimeZone, Utc};
use serde_json::{Value, json};
use serenity::model::channel::Message;
use tempfile::TempDir;
use wiremock::matchers::{method, path, path_regex, query_param, query_param_is_missing};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::{GUILD_ROLES, NUKI_USER_ID, test_app};
use crate::app::App;
use crate::decisions::test_support::{jev_request_bodies, mount_jev_answers_for};
use crate::llm::test_support::mount_llm_replies;
use crate::routing::test_support::{category_answer, category_probabilities};
use crate::store::intakes::{Intake, IntakeKind, NewIntake};
use crate::store::test_support::all_events;
use crate::test_support::json_response;
use crate::workflows::test_support::{bug_field_names, bug_fields_missing};

const THREAD: &str = include_str!("../../../tests/fixtures/discord/history_thread.json");
const PARENT_CHANNEL: &str =
    include_str!("../../../tests/fixtures/discord/history_text_channel.json");
const THREAD_STARTER: &str =
    include_str!("../../../tests/fixtures/discord/history_thread_starter.json");
const UNKNOWN_MEMBER: &str = include_str!("../../../tests/fixtures/discord/unknown_member.json");
const SENT_MESSAGE: &str = include_str!("../../../tests/fixtures/discord/reply_sent_message.json");
const INTAKE_BUG_ASK: &str = include_str!("../../../tests/fixtures/workflows/intake_bug_ask.json");

const GUILD_ID: u64 = 1000000000000000001;
const PARENT_CHANNEL_ID: u64 = 1200000000000000001;
const THREAD_ID: u64 = 1100000000000000003;
const REPORTER_ID: u64 = 1300000000000000002;
pub(crate) const UNIT_ID: u64 = 1100000000000000006;

pub(crate) const NUKI_ASKS_FOR_THE_PLATFORM: &str =
    "Thanks! Which operating system are you on, and which plugins do you use?";

pub(crate) const IGNORE_ON_TOP: [(&str, f64); 5] = [
    ("help_request", 0.03),
    ("bug_report", 0.12),
    ("feature_request", 0.01),
    ("chat", 0.02),
    ("ignore", 0.82),
];
pub(crate) const BUG_REPORT_ON_TOP: [(&str, f64); 5] = [
    ("help_request", 0.05),
    ("bug_report", 0.86),
    ("feature_request", 0.01),
    ("chat", 0.02),
    ("ignore", 0.06),
];

pub(crate) struct IntakeThreadScene {
    pub(crate) server: MockServer,
    pub(crate) app: App,
    pub(crate) intake: Intake,
    _repos: TempDir,
}

impl IntakeThreadScene {
    pub(crate) async fn open(earlier: Vec<Value>, unit: &Value) -> IntakeThreadScene {
        let server = MockServer::start().await;
        mount_intake_thread(&server, earlier, unit).await;
        let repos = TempDir::new().unwrap();
        let app = test_app(&server, repos.path()).await;
        let intake = app
            .store
            .intakes()
            .create(NewIntake {
                kind: IntakeKind::Bug,
                thread_id: THREAD_ID,
                origin_channel_id: PARENT_CHANNEL_ID,
                origin_message_id: THREAD_ID,
                reporter_id: REPORTER_ID,
                created_at: Utc.with_ymd_and_hms(2026, 10, 19, 12, 36, 0).unwrap(),
            })
            .await
            .unwrap();
        IntakeThreadScene {
            server,
            app,
            intake,
            _repos: repos,
        }
    }

    pub(crate) async fn answer_category(&self, probabilities: &[(&str, f64)]) {
        mount_jev_answers_for(&self.server, "category", category_answer(probabilities)).await;
    }

    pub(crate) async fn answer_intake_turn_with_missing_fields(&self) {
        let first_field = &bug_field_names()[0];
        mount_jev_answers_for(&self.server, first_field, bug_fields_missing()).await;
        mount_llm_replies(&self.server, &[INTAKE_BUG_ASK]).await;
    }

    pub(crate) async fn sent_texts(&self) -> Vec<String> {
        let messages_path = format!("/api/v10/channels/{THREAD_ID}/messages");
        self.server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|request| request.method.as_str() == "POST")
            .filter(|request| request.url.path() == messages_path)
            .map(|request| {
                let body: Value = request.body_json().unwrap();
                body["content"].as_str().unwrap().to_string()
            })
            .collect()
    }

    pub(crate) async fn category_options(&self) -> Vec<Vec<String>> {
        jev_request_bodies(&self.server)
            .await
            .into_iter()
            .filter(|body| body["questions"].get("category").is_some())
            .map(|body| {
                body["questions"]["category"]["criteria"]
                    .as_object()
                    .unwrap()
                    .keys()
                    .cloned()
                    .collect()
            })
            .collect()
    }

    pub(crate) async fn jev_purposes(&self) -> Vec<String> {
        self.event_data("jev.call")
            .await
            .into_iter()
            .map(|data| data["purpose"].as_str().unwrap().to_string())
            .collect()
    }

    pub(crate) async fn event_data(&self, kind: &str) -> Vec<Value> {
        all_events(&self.app.store)
            .await
            .into_iter()
            .filter(|row| row.kind == kind)
            .map(|row| row.data)
            .collect()
    }

    pub(crate) async fn open_intake(&self) -> Option<Intake> {
        self.app
            .store
            .intakes()
            .find_open_by_thread(THREAD_ID)
            .await
            .unwrap()
    }
}

async fn mount_intake_thread(server: &MockServer, earlier: Vec<Value>, unit: &Value) {
    mount_get(
        server,
        &format!("/api/v10/channels/{THREAD_ID}"),
        json_response(200, THREAD),
    )
    .await;
    mount_get(
        server,
        &format!("/api/v10/channels/{PARENT_CHANNEL_ID}"),
        json_response(200, PARENT_CHANNEL),
    )
    .await;
    mount_get(
        server,
        &format!("/api/v10/channels/{PARENT_CHANNEL_ID}/messages/{THREAD_ID}"),
        json_response(200, THREAD_STARTER),
    )
    .await;
    mount_thread_messages(server, earlier, unit).await;
    mount_get(
        server,
        &format!("/api/v10/guilds/{GUILD_ID}/roles"),
        json_response(200, GUILD_ROLES),
    )
    .await;
    mount_get(
        server,
        &format!("/api/v10/guilds/{GUILD_ID}/emojis"),
        json_response(200, "[]"),
    )
    .await;
    Mock::given(method("GET"))
        .and(path_regex(format!(
            r"^/api/v10/guilds/{GUILD_ID}/members/\d+$"
        )))
        .respond_with(json_response(404, UNKNOWN_MEMBER))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v10/channels/{THREAD_ID}/typing")))
        .respond_with(ResponseTemplate::new(204))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v10/channels/{THREAD_ID}/messages")))
        .respond_with(json_response(
            200,
            &SENT_MESSAGE.replace("SENT_MESSAGE_ID", "1100000000000000007"),
        ))
        .mount(server)
        .await;
}

async fn mount_get(server: &MockServer, url_path: &str, response: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path(url_path))
        .respond_with(response)
        .mount(server)
        .await;
}

async fn mount_thread_messages(server: &MockServer, earlier: Vec<Value>, unit: &Value) {
    let messages_path = format!("/api/v10/channels/{THREAD_ID}/messages");
    let earlier_newest_first: Vec<Value> = earlier.into_iter().rev().collect();
    let newest_page: Vec<Value> = std::iter::once(unit.clone())
        .chain(earlier_newest_first.clone())
        .collect();
    Mock::given(method("GET"))
        .and(path(&messages_path))
        .and(query_param("before", UNIT_ID.to_string()))
        .respond_with(ResponseTemplate::new(200).set_body_json(earlier_newest_first))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(&messages_path))
        .and(query_param_is_missing("before"))
        .respond_with(ResponseTemplate::new(200).set_body_json(newest_page))
        .mount(server)
        .await;
}

pub(crate) fn reporter() -> Value {
    json!({ "id": REPORTER_ID.to_string(), "username": "bob", "global_name": "Bob" })
}

fn maintainer() -> Value {
    json!({ "id": "1300000000000000004", "username": "nukeop", "global_name": "nukeop" })
}

fn nuki() -> Value {
    json!({ "id": NUKI_USER_ID.to_string(), "username": "nuki-bot", "bot": true })
}

pub(crate) fn thread_message(id: u64, author: Value, content: &str) -> Value {
    json!({
        "id": id.to_string(),
        "channel_id": THREAD_ID.to_string(),
        "author": author,
        "content": content,
        "timestamp": "2026-10-19T12:40:00.000000+00:00",
        "tts": false,
        "mention_everyone": false,
        "mentions": [],
        "mention_roles": [],
        "attachments": [],
        "embeds": [],
        "pinned": false,
        "type": 0
    })
}

pub(crate) fn reply_to(id: u64, author: Value, content: &str, referenced: Value) -> Value {
    json!({
        "id": id.to_string(),
        "channel_id": THREAD_ID.to_string(),
        "author": author,
        "content": content,
        "timestamp": "2026-10-19T12:40:00.000000+00:00",
        "tts": false,
        "mention_everyone": false,
        "mentions": [],
        "mention_roles": [],
        "attachments": [],
        "embeds": [],
        "pinned": false,
        "type": 19,
        "message_reference": {
            "message_id": referenced["id"],
            "channel_id": THREAD_ID.to_string(),
            "guild_id": GUILD_ID.to_string()
        },
        "referenced_message": referenced
    })
}

pub(crate) fn mentioning_nuki(message: Value) -> Value {
    let mut mentioning = message;
    mentioning["mentions"] = json!([nuki()]);
    mentioning
}

pub(crate) fn gateway_message_in_server(message: &Value) -> Message {
    let mut message = message.clone();
    message["guild_id"] = json!(GUILD_ID.to_string());
    serde_json::from_value(message).unwrap()
}

pub(crate) fn nuki_question() -> Value {
    thread_message(
        1100000000000000004,
        nuki(),
        "Which Nuclear version do you use?",
    )
}

pub(crate) fn maintainer_finding() -> Value {
    thread_message(
        1100000000000000005,
        maintainer(),
        "when I add the track to the queue, it picks the right stream, then the stream disappears from the list and the next one is picked",
    )
}

pub(crate) fn unit_routed_data(probabilities: &[(&str, f64)], decision: &str) -> Value {
    json!({
        "message_count": 1,
        "probabilities": category_probabilities(probabilities),
        "decision": decision,
    })
}
