use futures::future::BoxFuture;
use reqwest::Url;
use serde_json::json;

use crate::web::json_rpc::call_tool;
use crate::web::provider::{ProviderError, SearchResult, WebProvider};

const SEARCH_TOOL: &str = "web_search_exa";
const FETCH_TOOL: &str = "web_fetch_exa";
/// The maximum length of the page text. Exa gives 3000 characters if Nuki does not ask for more.
const MAX_CHARACTERS: u32 = 50_000;
/// The text between two search results. A snippet can contain `---`, so the separator includes
/// the `Title: ` of the next result.
const RESULT_SEPARATOR: &str = "\n\n---\n\nTitle: ";
const NO_TITLE: &str = "N/A";
/// The text of Exa's answer to a search with zero results.
const NO_RESULTS: &str = "No search results found. Please try a different query.";

/// The client of Exa's MCP server. Exa gives keyless access only on this endpoint.
pub struct ExaClient {
    http: reqwest::Client,
    /// The MCP endpoint. In production, it is `https://mcp.exa.ai/mcp`.
    endpoint: Url,
}

impl WebProvider for ExaClient {
    fn name(&self) -> &'static str {
        "exa"
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

impl ExaClient {
    pub fn new(http: reqwest::Client, endpoint: Url) -> Self {
        Self { http, endpoint }
    }

    async fn search_results(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let arguments = json!({ "query": query, "objective": query, "numResults": limit });
        let result = call_tool(&self.http, &self.endpoint, SEARCH_TOOL, arguments).await?;
        parse_results(result.text()?)
    }

    async fn fetch_page(&self, url: &str) -> Result<String, ProviderError> {
        let arguments = json!({ "urls": [url], "maxCharacters": MAX_CHARACTERS });
        let result = call_tool(&self.http, &self.endpoint, FETCH_TOOL, arguments).await?;
        page_text(result.text()?, url)
    }
}

/// Reads the search results from Exa's text, and skips the results that do not parse. If no
/// result parses, returns an error.
fn parse_results(text: &str) -> Result<Vec<SearchResult>, ProviderError> {
    if text == NO_RESULTS {
        return Ok(vec![]);
    }
    let results: Vec<SearchResult> = text
        .strip_prefix("Title: ")
        .map(|blocks| {
            blocks
                .split(RESULT_SEPARATOR)
                .filter_map(parse_result)
                .collect()
        })
        .unwrap_or_default();
    if results.is_empty() {
        return Err(ProviderError::InvalidAnswer(
            "the search text has no result blocks".to_string(),
        ));
    }
    Ok(results)
}

/// Reads one search result. `block` starts after `Title: `. The lines `URL:`, `Published:` and
/// `Author:` come next, then `Highlights:` and the snippet on the next lines, or `Text: ` and
/// the snippet.
fn parse_result(block: &str) -> Option<SearchResult> {
    let (title, rest) = block.split_once('\n')?;
    let (url, rest) = line_value(rest, "URL: ")?;
    let (_published, rest) = line_value(rest, "Published: ")?;
    let (_author, rest) = line_value(rest, "Author: ")?;
    let content = rest
        .strip_prefix("Highlights:\n")
        .or_else(|| rest.strip_prefix("Text: "))?;
    Some(SearchResult {
        title: title_of(title),
        url: url.to_string(),
        content: content.to_string(),
    })
}

/// Returns `""` for Exa's `N/A`, which Exa writes when it has no title.
fn title_of(title: &str) -> String {
    if title == NO_TITLE {
        return String::new();
    }
    title.to_string()
}

/// Returns the value of the first line, if the line starts with `label`, and the text after
/// that line.
fn line_value<'a>(text: &'a str, label: &str) -> Option<(&'a str, &'a str)> {
    text.strip_prefix(label)?.split_once('\n')
}

/// Returns the page text without Exa's header. The header is the lines `# title`, `URL:`, and
/// `Published:` and `Author:` if Exa knows them, then an empty line. If the text has no header,
/// Exa could not read the page, and the text tells why.
fn page_text(text: &str, url: &str) -> Result<String, ProviderError> {
    if !text.starts_with("# ") {
        return Err(ProviderError::Failed {
            url: url.to_string(),
            message: text.to_string(),
        });
    }
    text.split_once("\n\n")
        .map(|(_header, body)| body)
        .filter(|body| !body.is_empty())
        .map(str::to_string)
        .ok_or_else(|| ProviderError::NoContent {
            url: url.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::matchers::{body_json, headers, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::test_support::json_response;

    const SEARCH: &str = include_str!("../../tests/fixtures/exa/search.sse");
    const FETCH: &str = include_str!("../../tests/fixtures/exa/fetch.sse");
    const FETCH_ERROR: &str = include_str!("../../tests/fixtures/exa/fetch_error.sse");
    const RATE_LIMITED: &str = include_str!("../../tests/fixtures/exa/rate_limited.json");

    fn client_for(server: &MockServer) -> ExaClient {
        ExaClient::new(
            reqwest::Client::new(),
            format!("{}/mcp", server.uri()).parse().unwrap(),
        )
    }

    fn sse_response(body: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_raw(body, "text/event-stream")
    }

    /// Returns an SSE answer with one tool result, with `text` as its only content.
    fn sse_with_text(text: &str) -> String {
        let message = json!({
            "result": { "content": [{ "type": "text", "text": text }] },
            "jsonrpc": "2.0",
            "id": 1,
        });
        format!("event: message\ndata: {message}\n\n")
    }

    async fn mount(server: &MockServer, response: ResponseTemplate) {
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(response)
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn search_sends_a_tool_call_and_returns_the_results_of_the_sse_answer() {
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
                    "name": "web_search_exa",
                    "arguments": {
                        "query": "nuclear music player",
                        "objective": "nuclear music player",
                        "numResults": 2,
                    },
                },
            })))
            .respond_with(sse_response(SEARCH))
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
                    title: "Nuclear - Free Open-Source Music Player".to_string(),
                    url: "https://nuclearplayer.com/".to_string(),
                    content: "Nuclear is a free, open-source music player.\n\n---\n\nNo ads, no tracking."
                        .to_string(),
                },
                SearchResult {
                    title: "Getting started | Nuclear".to_string(),
                    url: "https://docs.nuclearplayer.com/nuclear/user-manual/getting-started"
                        .to_string(),
                    content: "Download Nuclear from the releases page.".to_string(),
                },
            ]
        );
    }

    #[tokio::test]
    async fn fetch_sends_a_tool_call_and_returns_the_page_text_without_the_header() {
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
                    "name": "web_fetch_exa",
                    "arguments": { "urls": ["https://example.com"], "maxCharacters": 50000 },
                },
            })))
            .respond_with(sse_response(FETCH))
            .mount(&server)
            .await;

        let text = client_for(&server)
            .fetch("https://example.com")
            .await
            .unwrap();

        assert_eq!(
            text,
            "Example Domain\n\nThis domain is for use in documentation examples without needing permission."
        );
    }

    #[tokio::test]
    async fn fetch_reads_an_sse_answer_with_a_content_type_in_other_letter_case() {
        let server = MockServer::start().await;
        mount(
            &server,
            ResponseTemplate::new(200).set_body_raw(FETCH, "Text/Event-Stream; charset=utf-8"),
        )
        .await;

        let text = client_for(&server)
            .fetch("https://example.com")
            .await
            .unwrap();

        assert_eq!(
            text,
            "Example Domain\n\nThis domain is for use in documentation examples without needing permission."
        );
    }

    #[tokio::test]
    async fn fetch_removes_the_published_and_author_lines_of_the_header() {
        let server = MockServer::start().await;
        let text = "# Getting started\nURL: https://docs.nuclearplayer.com/\nPublished: 2026-03-01\nAuthor: nukeop\n\nDownload Nuclear.";
        mount(&server, sse_response(&sse_with_text(text))).await;

        let text = client_for(&server)
            .fetch("https://docs.nuclearplayer.com/")
            .await
            .unwrap();

        assert_eq!(text, "Download Nuclear.");
    }

    #[tokio::test]
    async fn fetch_reads_the_first_text_item_and_ignores_the_other_item_types() {
        let server = MockServer::start().await;
        let message = json!({
            "result": {
                "content": [
                    { "type": "image", "data": "iVBORw0KGgo=", "mimeType": "image/png" },
                    { "type": "text", "text": "# Example Domain\nURL: https://example.com\n\nExample Domain" },
                ],
            },
            "jsonrpc": "2.0",
            "id": 1,
        });
        mount(
            &server,
            sse_response(&format!("event: message\ndata: {message}\n\n")),
        )
        .await;

        let text = client_for(&server)
            .fetch("https://example.com")
            .await
            .unwrap();

        assert_eq!(text, "Example Domain");
    }

    #[tokio::test]
    async fn fetch_returns_the_text_of_a_tool_error() {
        let server = MockServer::start().await;
        mount(&server, sse_response(FETCH_ERROR)).await;

        let error = client_for(&server)
            .fetch("https://example.invalid/x")
            .await
            .unwrap_err();

        assert_eq!(
            error.to_string(),
            "the tool failed: Error fetching URL(s): https://example.invalid/x: CRAWL_UNKNOWN_ERROR"
        );
    }

    #[tokio::test]
    async fn fetch_returns_the_text_of_exa_when_the_text_is_not_a_page() {
        let server = MockServer::start().await;
        mount(
            &server,
            sse_response(&sse_with_text("No content found for the provided URL(s).")),
        )
        .await;

        let error = client_for(&server)
            .fetch("https://example.com")
            .await
            .unwrap_err();

        assert_eq!(
            error.to_string(),
            "could not read https://example.com: No content found for the provided URL(s)."
        );
    }

    #[tokio::test]
    async fn fetch_returns_no_content_when_the_page_has_no_text() {
        let server = MockServer::start().await;
        mount(
            &server,
            sse_response(&sse_with_text(
                "# Example Domain\nURL: https://example.com\n\n",
            )),
        )
        .await;

        let error = client_for(&server)
            .fetch("https://example.com")
            .await
            .unwrap_err();

        assert_eq!(error.to_string(), "no text for https://example.com");
    }

    #[tokio::test]
    async fn search_returns_the_message_of_a_rate_limit_error() {
        let server = MockServer::start().await;
        mount(&server, json_response(429, RATE_LIMITED)).await;

        let error = client_for(&server).search("nuclear", 5).await.unwrap_err();

        assert_eq!(
            error.to_string(),
            "HTTP 429: You've hit Exa's free MCP rate limit. To continue using without limits, create your own Exa API key."
        );
    }

    #[tokio::test]
    async fn search_keeps_the_results_that_parse_and_skips_the_others() {
        let server = MockServer::start().await;
        let text = "Title: Nuclear\nURL: https://nuclearplayer.com/\nPublished: N/A\nAuthor: N/A\nText: A music player.\n\n---\n\nTitle: Broken\nURL: https://example.com/\nText: No Published and Author lines.";
        mount(&server, sse_response(&sse_with_text(text))).await;

        let results = client_for(&server).search("nuclear", 5).await.unwrap();

        assert_eq!(
            results,
            vec![SearchResult {
                title: "Nuclear".to_string(),
                url: "https://nuclearplayer.com/".to_string(),
                content: "A music player.".to_string(),
            }]
        );
    }

    #[tokio::test]
    async fn search_gives_an_empty_title_when_exa_has_no_title() {
        let server = MockServer::start().await;
        let text = "Title: N/A\nURL: https://nuclearplayer.com/\nPublished: N/A\nAuthor: N/A\nText: A music player.";
        mount(&server, sse_response(&sse_with_text(text))).await;

        let results = client_for(&server).search("nuclear", 5).await.unwrap();

        assert_eq!(
            results,
            vec![SearchResult {
                title: "".to_string(),
                url: "https://nuclearplayer.com/".to_string(),
                content: "A music player.".to_string(),
            }]
        );
    }

    #[tokio::test]
    async fn search_fails_when_no_result_parses() {
        let server = MockServer::start().await;
        let text = "Title: Broken\nURL: https://example.com/\nText: No Published and Author lines.";
        mount(&server, sse_response(&sse_with_text(text))).await;

        let error = client_for(&server).search("nuclear", 5).await.unwrap_err();

        assert_eq!(
            error.to_string(),
            "the answer is not valid: the search text has no result blocks"
        );
    }

    #[tokio::test]
    async fn search_returns_zero_results_when_exa_found_none() {
        let server = MockServer::start().await;
        mount(
            &server,
            sse_response(&sse_with_text(
                "No search results found. Please try a different query.",
            )),
        )
        .await;

        let results = client_for(&server).search("nuclear", 5).await.unwrap();

        assert_eq!(results, vec![]);
    }
}
