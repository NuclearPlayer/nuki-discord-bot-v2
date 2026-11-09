use reqwest::Url;
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::web::provider::{ProviderError, parse_json};

/// The MCP servers refuse a request that does not accept both JSON and SSE.
const ACCEPTED_TYPES: &str = "application/json, text/event-stream";
const EVENT_STREAM: &str = "text/event-stream";
/// The message of a tool error that has no text item.
const NO_TOOL_MESSAGE: &str = "no message";

/// One item of `result.content`. Nuki reads only text items, and ignores the other types.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Content {
    Text {
        text: String,
    },
    #[serde(other)]
    Other,
}

/// The `result` of a successful `tools/call`.
#[derive(Debug, Deserialize)]
pub(crate) struct ToolResult {
    #[serde(default)]
    content: Vec<Content>,
    #[serde(rename = "structuredContent")]
    structured_content: Option<Value>,
    #[serde(default, rename = "isError")]
    is_error: bool,
}

impl ToolResult {
    /// Returns the text of the first text item.
    pub fn text(&self) -> Result<&str, ProviderError> {
        self.first_text()
            .ok_or_else(|| ProviderError::InvalidAnswer("the result has no text".to_string()))
    }

    fn first_text(&self) -> Option<&str> {
        self.content.iter().find_map(|content| match content {
            Content::Text { text } => Some(text.as_str()),
            Content::Other => None,
        })
    }

    /// Reads `structuredContent` as a `T`.
    pub fn structured<T: DeserializeOwned>(self) -> Result<T, ProviderError> {
        let content = self.structured_content.ok_or_else(|| {
            ProviderError::InvalidAnswer("the result has no structured content".to_string())
        })?;
        serde_json::from_value(content)
            .map_err(|error| ProviderError::InvalidAnswer(error.to_string()))
    }
}

#[derive(Debug, Deserialize)]
struct Response {
    result: Option<ToolResult>,
    error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
struct RpcError {
    code: i64,
    message: String,
}

/// Sends a `tools/call` of `tool` with `arguments` to the MCP endpoint, and returns the result.
/// Sends no `initialize` request and no session ID.
pub(crate) async fn call_tool(
    http: &reqwest::Client,
    endpoint: &Url,
    tool: &str,
    arguments: Value,
) -> Result<ToolResult, ProviderError> {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": tool, "arguments": arguments },
    });
    let response = http
        .post(endpoint.clone())
        .header(ACCEPT, ACCEPTED_TYPES)
        .json(&body)
        .send()
        .await?;
    let status = response.status();
    let answers_with_sse = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(is_event_stream);
    let text = response.text().await?;
    if !status.is_success() {
        return Err(status_error(status.as_u16(), text));
    }
    let message = if answers_with_sse {
        sse_data(&text)?
    } else {
        text.as_str()
    };
    result_of(parse_json(message)?)
}

/// Tells if `content_type` is SSE. Media types do not use letter case, and can have parameters
/// after `;`.
fn is_event_stream(content_type: &str) -> bool {
    let media_type = content_type.split(';').next().unwrap_or_default().trim();
    media_type.eq_ignore_ascii_case(EVENT_STREAM)
}

/// Returns the JSON of the first `data:` line. Exa sends one event with one `data:` line.
/// If a server sends more events or splits the JSON on more lines, change this function.
fn sse_data(body: &str) -> Result<&str, ProviderError> {
    body.lines()
        .find_map(|line| line.strip_prefix("data:"))
        .map(str::trim_start)
        .ok_or_else(|| ProviderError::InvalidAnswer("the SSE answer has no data".to_string()))
}

/// Returns the result. If the answer has a JSON-RPC error, or if the tool failed, returns the
/// error.
fn result_of(response: Response) -> Result<ToolResult, ProviderError> {
    if let Some(error) = response.error {
        return Err(ProviderError::Rpc {
            code: error.code,
            message: error.message,
        });
    }
    let result = response
        .result
        .ok_or_else(|| ProviderError::InvalidAnswer("the answer has no result".to_string()))?;
    if result.is_error {
        let message = result.first_text().unwrap_or(NO_TOOL_MESSAGE);
        return Err(ProviderError::Tool(message.to_string()));
    }
    Ok(result)
}

/// Makes the error of an answer that is not a success. Takes the message from the JSON-RPC
/// error. If the body has no such error, uses the full body as the message.
fn status_error(status: u16, body: String) -> ProviderError {
    let message = serde_json::from_str::<Response>(&body)
        .ok()
        .and_then(|response| response.error)
        .map(|error| error.message)
        .unwrap_or(body);
    ProviderError::Status { status, message }
}
