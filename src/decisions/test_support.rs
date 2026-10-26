use serde_json::{Value, json};
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::client::DECISIONS_PATH;

pub(crate) fn jev_response(answers: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "model": "typesafe/jev-1.14-20261014",
        "answers": answers,
    }))
}

pub(crate) async fn mount_jev_answers(server: &MockServer, answers: Value) {
    Mock::given(method("POST"))
        .and(path(DECISIONS_PATH))
        .respond_with(jev_response(answers))
        .mount(server)
        .await;
}

pub(crate) async fn mount_jev_answers_for(server: &MockServer, question: &str, answers: Value) {
    Mock::given(method("POST"))
        .and(path(DECISIONS_PATH))
        .and(body_partial_json(json!({ "questions": { question: {} } })))
        .respond_with(jev_response(answers))
        .mount(server)
        .await;
}

/// Returns the Jev request `body` without the instructions of its questions, because the
/// instructions are prompt text.
pub(crate) fn without_instructions(mut body: Value) -> Value {
    for question in body["questions"].as_object_mut().unwrap().values_mut() {
        question.as_object_mut().unwrap().remove("instructions");
    }
    body
}

pub(crate) async fn jev_request_bodies(server: &MockServer) -> Vec<Value> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| request.url.path() == DECISIONS_PATH)
        .map(|request| request.body_json().unwrap())
        .collect()
}
