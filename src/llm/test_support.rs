use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

use crate::sandbox::declarations::tool_description;
use crate::sandbox::test_support::echo_table;
use crate::test_support::json_response;

pub(crate) async fn mount_llm_replies(server: &MockServer, replies: &[&str]) {
    for body in replies {
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(json_response(200, body))
            .up_to_n_times(1)
            .mount(server)
            .await;
    }
}

pub(crate) async fn llm_request_bodies(server: &MockServer) -> Vec<Value> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| request.url.path() == "/chat/completions")
        .map(|request| request.body_json().unwrap())
        .collect()
}

pub(crate) fn run_script_tools() -> Value {
    json!([{
        "type": "function",
        "function": {
            "name": "run_script",
            "description": tool_description(&echo_table()),
            "parameters": {
                "type": "object",
                "properties": {
                    "code": {
                        "type": "string",
                        "description": "The body of an async JS function."
                    }
                },
                "required": ["code"]
            }
        }
    }])
}
