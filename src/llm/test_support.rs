use rig_core::message::{
    AssistantContent, Message, ToolCall, ToolResult, ToolResultContent, UserContent,
};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

use crate::sandbox::declarations::tool_description;
use crate::sandbox::test_support::echo_table;
use crate::test_support::json_response;

/// Rig's fixed extraction preamble, which `append_preamble` keeps before the system prompt.
pub(crate) const RIG_EXTRACTION_PREAMBLE: &str = "You are an AI assistant whose purpose is to extract structured data from the provided text.\n\
    You will have access to a `submit` function that defines the structure of the data to extract from the provided text.\n\
    Use the `submit` function to submit the structured data.\n\
    Be sure to fill out every field and ALWAYS CALL THE `submit` function, even with default values!!!.\n                \n\n\
    =============== ADDITIONAL INSTRUCTIONS ===============\n";

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

pub(crate) fn messages(turn_messages: Vec<Value>) -> Vec<Message> {
    turn_messages
        .into_iter()
        .map(|json| serde_json::from_value(json).unwrap())
        .collect()
}

/// Asserts that the turn is one `run_script` call of `test.echo({ a: 1 })` and its result.
pub(crate) fn assert_echo_tool_turn(turn: Vec<Message>) {
    let [call, result]: [Message; 2] = turn.try_into().unwrap();
    let call = tool_call(call);
    assert_eq!(call.function.name.as_str(), "run_script");
    assert_eq!(
        Value::from(call.function.arguments),
        json!({ "code": "return await test.echo({ a: 1 })" })
    );
    assert_eq!(
        tool_result(result).content,
        vec![ToolResultContent::text(r#"{"value":{"a":1}}"#)]
    );
}

fn tool_call(message: Message) -> ToolCall {
    let Message::Assistant(assistant) = message else {
        panic!("expected an assistant message, got {message:?}");
    };
    let [AssistantContent::ToolCall(call)]: [AssistantContent; 1] =
        assistant.content.try_into().unwrap()
    else {
        panic!("expected one tool call");
    };
    call
}

fn tool_result(message: Message) -> ToolResult {
    let Message::User { content } = message else {
        panic!("expected a user message, got {message:?}");
    };
    let [UserContent::ToolResult(result)]: [UserContent; 1] = content.try_into().unwrap() else {
        panic!("expected one tool result");
    };
    result
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
