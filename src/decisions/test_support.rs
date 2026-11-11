use serde_json::{Value, json};
use wiremock::matchers::{method, path};
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

/// Mounts one response for each item of `answers`. Jev gives each response one time, in the
/// sequence of `answers`.
pub(crate) async fn mount_jev_answers_in_sequence(server: &MockServer, answers: &[Value]) {
    for answers in answers {
        Mock::given(method("POST"))
            .and(path(DECISIONS_PATH))
            .respond_with(jev_response(answers.clone()))
            .up_to_n_times(1)
            .mount(server)
            .await;
    }
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
