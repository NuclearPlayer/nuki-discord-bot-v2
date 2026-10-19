use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

pub(crate) const JEV_PATH: &str = "/api/alpha/decisions";

pub(crate) fn jev_response(answers: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "model": "typesafe/jev-1.14-20261014",
        "answers": answers,
    }))
}

pub(crate) async fn mount_jev_answers(server: &MockServer, answers: Value) {
    Mock::given(method("POST"))
        .and(path(JEV_PATH))
        .respond_with(jev_response(answers))
        .mount(server)
        .await;
}

pub(crate) async fn jev_request_bodies(server: &MockServer) -> Vec<Value> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| request.url.path() == JEV_PATH)
        .map(|request| request.body_json().unwrap())
        .collect()
}
