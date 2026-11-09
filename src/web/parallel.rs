use futures::future::BoxFuture;
use reqwest::Url;
use serde::Deserialize;
use serde_json::json;

use crate::web::json_rpc::call_tool;
use crate::web::provider::{ProviderError, SearchResult, WebProvider};

const SEARCH_TOOL: &str = "web_search";
const FETCH_TOOL: &str = "web_fetch";

/// The client of Parallel's Search MCP server. Parallel gives keyless access only on this
/// endpoint.
pub struct ParallelClient {
    http: reqwest::Client,
    /// The MCP endpoint. In production, it is `https://search.parallel.ai/mcp`.
    endpoint: Url,
}

#[derive(Deserialize)]
struct SearchOutput {
    results: Vec<WebResult>,
}

#[derive(Deserialize)]
struct WebResult {
    url: String,
    title: Option<String>,
    excerpts: Vec<String>,
}

#[derive(Deserialize)]
struct FetchOutput {
    #[serde(default)]
    results: Vec<FetchedPage>,
    #[serde(default)]
    errors: Vec<FetchError>,
}

#[derive(Deserialize)]
struct FetchedPage {
    full_content: Option<String>,
}

#[derive(Deserialize)]
struct FetchError {
    error_type: String,
}

impl WebProvider for ParallelClient {
    fn name(&self) -> &'static str {
        "parallel"
    }

    fn search<'a>(
        &'a self,
        query: &'a str,
        limit: u32,
    ) -> BoxFuture<'a, Result<Vec<SearchResult>, ProviderError>> {
        Box::pin(self.search_results(query, limit))
    }

    fn fetch<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<String, ProviderError>> {
        Box::pin(self.fetch_page(url))
    }
}

impl ParallelClient {
    pub fn new(http: reqwest::Client, endpoint: Url) -> Self {
        Self { http, endpoint }
    }

    /// Parallel has no parameter for the number of results, so Nuki keeps the first `limit`.
    async fn search_results(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let arguments = json!({ "objective": query, "search_queries": [query] });
        let result = call_tool(&self.http, &self.endpoint, SEARCH_TOOL, arguments).await?;
        let output: SearchOutput = result.structured()?;
        Ok(output
            .results
            .into_iter()
            .take(limit as usize)
            .map(search_result)
            .collect())
    }

    async fn fetch_page(&self, url: &str) -> Result<String, ProviderError> {
        let arguments = json!({ "urls": [url], "full_content": true });
        let result = call_tool(&self.http, &self.endpoint, FETCH_TOOL, arguments).await?;
        page_text(result.structured()?, url)
    }
}

fn search_result(result: WebResult) -> SearchResult {
    SearchResult {
        title: result.title.unwrap_or_default(),
        url: result.url,
        content: result.excerpts.join("\n"),
    }
}

/// Returns the full content of the one page that Nuki asked for. Parallel answers without a
/// tool error also when it could not read the page, and then gives the reason in `errors`.
fn page_text(output: FetchOutput, url: &str) -> Result<String, ProviderError> {
    let text = output
        .results
        .into_iter()
        .next()
        .and_then(|page| page.full_content)
        .filter(|text| !text.is_empty());
    if let Some(text) = text {
        return Ok(text);
    }
    match output.errors.into_iter().next() {
        Some(error) => Err(ProviderError::Failed {
            url: url.to_string(),
            message: error.error_type,
        }),
        None => Err(ProviderError::NoContent {
            url: url.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::matchers::{body_json, headers, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::test_support::json_response;

    const SEARCH: &str = include_str!("../../tests/fixtures/parallel/search.json");
    const FETCH: &str = include_str!("../../tests/fixtures/parallel/fetch.json");
    const FETCH_FAILED: &str = include_str!("../../tests/fixtures/parallel/fetch_failed.json");
    const RPC_ERROR: &str = include_str!("../../tests/fixtures/parallel/rpc_error.json");

    fn client_for(server: &MockServer) -> ParallelClient {
        ParallelClient::new(
            reqwest::Client::new(),
            format!("{}/mcp", server.uri()).parse().unwrap(),
        )
    }

    async fn mount(server: &MockServer, status: u16, body: &str) {
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(json_response(status, body))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn search_sends_a_tool_call_and_returns_the_first_results_up_to_the_limit() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .and(headers(
                "Accept",
                vec!["application/json", "text/event-stream"],
            ))
            .and(body_json(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "web_search",
                    "arguments": {
                        "objective": "nuclear music player",
                        "search_queries": ["nuclear music player"],
                    },
                },
            })))
            .respond_with(json_response(200, SEARCH))
            .mount(&server)
            .await;

        let results = client_for(&server)
            .search("nuclear music player", 2)
            .await
            .unwrap();

        assert_eq!(
            results,
            vec![
                SearchResult {
                    title: "Nuclear Documentation | Nuclear".to_string(),
                    url: "https://docs.nuclearplayer.com/nuclear".to_string(),
                    content:
                        "# Nuclear Documentation\nNuclear is a free, open-source music player."
                            .to_string(),
                },
                SearchResult {
                    title: "".to_string(),
                    url: "https://github.com/nukeop/nuclear".to_string(),
                    content: "# nukeop/nuclear\n\n- Page: GitHub repository".to_string(),
                },
            ]
        );
    }

    #[tokio::test]
    async fn fetch_asks_for_the_full_content_and_returns_it() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .and(headers(
                "Accept",
                vec!["application/json", "text/event-stream"],
            ))
            .and(body_json(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "web_fetch",
                    "arguments": { "urls": ["https://example.com"], "full_content": true },
                },
            })))
            .respond_with(json_response(200, FETCH))
            .mount(&server)
            .await;

        let text = client_for(&server)
            .fetch("https://example.com")
            .await
            .unwrap();

        assert_eq!(
            text,
            "# Example Domain\n\nThis domain is for use in documentation examples without needing permission."
        );
    }

    #[tokio::test]
    async fn fetch_returns_the_error_type_when_the_answer_has_only_errors() {
        let server = MockServer::start().await;
        mount(&server, 200, FETCH_FAILED).await;

        let error = client_for(&server)
            .fetch("https://example.invalid/x")
            .await
            .unwrap_err();

        assert_eq!(
            error.to_string(),
            "could not read https://example.invalid/x: connect_error"
        );
    }

    #[tokio::test]
    async fn search_returns_the_json_rpc_error() {
        let server = MockServer::start().await;
        mount(&server, 200, RPC_ERROR).await;

        let error = client_for(&server).search("nuclear", 5).await.unwrap_err();

        assert_eq!(error.to_string(), "JSON-RPC error -32601: Method not found");
    }

    #[tokio::test]
    async fn search_returns_a_tool_error_when_the_tool_failed_with_no_text() {
        let server = MockServer::start().await;
        let answer = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": { "content": [], "isError": true },
        });
        mount(&server, 200, &answer.to_string()).await;

        let error = client_for(&server).search("nuclear", 5).await.unwrap_err();

        assert_eq!(error.to_string(), "the tool failed: no message");
    }

    #[tokio::test]
    async fn search_returns_the_body_of_a_rate_limit_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(ResponseTemplate::new(429).set_body_string("Too Many Requests"))
            .mount(&server)
            .await;

        let error = client_for(&server).search("nuclear", 5).await.unwrap_err();

        assert_eq!(error.to_string(), "HTTP 429: Too Many Requests");
    }

    #[tokio::test]
    async fn fetch_returns_no_content_when_the_full_content_is_empty() {
        let server = MockServer::start().await;
        let answer = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "content": [{ "type": "text", "text": "{}" }],
                "structuredContent": {
                    "results": [{ "url": "https://example.com", "full_content": "" }],
                    "errors": [],
                },
            },
        });
        mount(&server, 200, &answer.to_string()).await;

        let error = client_for(&server)
            .fetch("https://example.com")
            .await
            .unwrap_err();

        assert_eq!(error.to_string(), "no text for https://example.com");
    }
}
