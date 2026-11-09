use futures::future::BoxFuture;
use reqwest::Url;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::web::provider::{ProviderError, SearchResult, WebProvider, parse_json};

/// The header that selects Tavily's keyless access. Nuki sends no API key.
const ACCESS_MODE_HEADER: &str = "X-Tavily-Access-Mode";
const KEYLESS: &str = "keyless";

/// The client of the Tavily Search and Extract APIs.
pub struct TavilyClient {
    http: reqwest::Client,
    /// The base of the API, with no path. In production, it is `https://api.tavily.com`.
    api_base: Url,
}

#[derive(Serialize)]
struct SearchRequest<'a> {
    query: &'a str,
    max_results: u32,
}

/// Tavily sends its results in the shape of `SearchResult`.
#[derive(Deserialize)]
struct SearchResponse {
    results: Vec<SearchResult>,
}

#[derive(Serialize)]
struct ExtractRequest<'a> {
    urls: [&'a str; 1],
}

#[derive(Deserialize)]
struct ExtractResponse {
    #[serde(default)]
    results: Vec<ExtractedPage>,
    #[serde(default)]
    failed_results: Vec<FailedExtraction>,
}

#[derive(Deserialize)]
struct ExtractedPage {
    raw_content: Option<String>,
}

#[derive(Deserialize)]
struct FailedExtraction {
    error: String,
}

/// The body of a Tavily error, for example `{ "detail": { "error": "..." } }`.
#[derive(Deserialize)]
struct ApiErrorBody {
    detail: ApiErrorDetail,
}

#[derive(Deserialize)]
struct ApiErrorDetail {
    error: String,
}

impl WebProvider for TavilyClient {
    fn name(&self) -> &'static str {
        "tavily"
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

impl TavilyClient {
    pub fn new(http: reqwest::Client, api_base: Url) -> Self {
        Self { http, api_base }
    }

    async fn search_results(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let body = SearchRequest {
            query,
            max_results: limit,
        };
        let response: SearchResponse = self.post("search", &body).await?;
        Ok(response.results)
    }

    async fn fetch_page(&self, url: &str) -> Result<String, ProviderError> {
        let body = ExtractRequest { urls: [url] };
        let response: ExtractResponse = self.post("extract", &body).await?;
        page_text(response, url)
    }

    async fn post<B: Serialize, T: DeserializeOwned>(
        &self,
        endpoint: &str,
        body: &B,
    ) -> Result<T, ProviderError> {
        let url = self
            .api_base
            .join(endpoint)
            .expect("an endpoint name is a valid relative URL");
        let response = self
            .http
            .post(url)
            .header(ACCESS_MODE_HEADER, KEYLESS)
            .json(body)
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            return Err(api_error(status.as_u16(), response.text().await?));
        }
        parse_json(&response.text().await?)
    }
}

/// Returns the text of the one page that Nuki asked for. Tavily answers 200 also when it could
/// not read the page, and then gives the reason in `failed_results`.
fn page_text(response: ExtractResponse, url: &str) -> Result<String, ProviderError> {
    if let Some(extracted) = response.results.into_iter().next() {
        return text_of(extracted, url);
    }
    match response.failed_results.into_iter().next() {
        Some(failure) => Err(ProviderError::Failed {
            url: url.to_string(),
            message: failure.error,
        }),
        None => Err(ProviderError::NoContent {
            url: url.to_string(),
        }),
    }
}

/// Returns the text of the page. If the text is missing or empty, returns `NoContent`.
fn text_of(extracted: ExtractedPage, url: &str) -> Result<String, ProviderError> {
    extracted
        .raw_content
        .filter(|text| !text.is_empty())
        .ok_or_else(|| ProviderError::NoContent {
            url: url.to_string(),
        })
}

/// Makes the error of a Tavily answer that is not a success. Takes the message from
/// `detail.error`. If the body has no such field, uses the full body as the message.
fn api_error(status: u16, body: String) -> ProviderError {
    let message = serde_json::from_str::<ApiErrorBody>(&body)
        .map(|error| error.detail.error)
        .unwrap_or(body);
    ProviderError::Status { status, message }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::test_support::json_response;
    use crate::web::provider::WebProvider;
    use crate::web::test_support::{
        EXTRACT, EXTRACT_FAILED, EXTRACT_TEXT, RATE_LIMITED, SEARCH, client_for, search_results,
    };

    async fn mount_extract_response(server: &MockServer, response: serde_json::Value) {
        Mock::given(method("POST"))
            .and(path("/extract"))
            .respond_with(json_response(200, &response.to_string()))
            .mount(server)
            .await;
    }

    async fn fetch_error(server: &MockServer) -> String {
        client_for(server)
            .fetch("https://example.com")
            .await
            .unwrap_err()
            .to_string()
    }

    #[tokio::test]
    async fn search_sends_a_keyless_request_and_returns_the_results() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .and(header("X-Tavily-Access-Mode", "keyless"))
            .and(body_json(
                json!({ "query": "nuclear music player", "max_results": 2 }),
            ))
            .respond_with(json_response(200, SEARCH))
            .mount(&server)
            .await;

        let results = client_for(&server)
            .search("nuclear music player", 2)
            .await
            .unwrap();

        assert_eq!(results, search_results());
    }

    #[tokio::test]
    async fn fetch_sends_a_keyless_request_and_returns_the_text_of_the_page() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/extract"))
            .and(header("X-Tavily-Access-Mode", "keyless"))
            .and(body_json(json!({ "urls": ["https://example.com"] })))
            .respond_with(json_response(200, EXTRACT))
            .mount(&server)
            .await;

        let text = client_for(&server)
            .fetch("https://example.com")
            .await
            .unwrap();

        assert_eq!(text, EXTRACT_TEXT);
    }

    #[tokio::test]
    async fn fetch_returns_the_reason_when_tavily_could_not_read_the_page() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/extract"))
            .respond_with(json_response(200, EXTRACT_FAILED))
            .mount(&server)
            .await;

        let error = client_for(&server)
            .fetch("https://thisdoesnotexist-xyz-123.example")
            .await
            .unwrap_err();

        assert_eq!(
            error.to_string(),
            "could not read https://thisdoesnotexist-xyz-123.example: Failed to fetch url"
        );
    }

    #[tokio::test]
    async fn fetch_returns_no_content_when_the_text_of_the_page_is_null() {
        let server = MockServer::start().await;
        mount_extract_response(
            &server,
            json!({ "results": [{ "url": "https://example.com/", "raw_content": null }] }),
        )
        .await;

        let error = fetch_error(&server).await;

        assert_eq!(error, "no text for https://example.com");
    }

    #[tokio::test]
    async fn fetch_returns_no_content_when_the_text_of_the_page_is_empty() {
        let server = MockServer::start().await;
        mount_extract_response(
            &server,
            json!({ "results": [{ "url": "https://example.com/", "raw_content": "" }] }),
        )
        .await;

        let error = fetch_error(&server).await;

        assert_eq!(error, "no text for https://example.com");
    }

    #[tokio::test]
    async fn fetch_returns_no_content_when_tavily_gives_no_result_and_no_failure() {
        let server = MockServer::start().await;
        mount_extract_response(&server, json!({})).await;

        let error = fetch_error(&server).await;

        assert_eq!(error, "no text for https://example.com");
    }

    #[tokio::test]
    async fn search_returns_the_message_of_a_rate_limit_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(json_response(429, RATE_LIMITED))
            .mount(&server)
            .await;

        let error = client_for(&server).search("nuclear", 5).await.unwrap_err();

        assert_eq!(
            error.to_string(),
            "HTTP 429: Your request has been blocked due to excessive requests. Please reduce the rate of requests."
        );
    }

    #[tokio::test]
    async fn search_returns_an_invalid_answer_when_the_body_is_not_json() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>Maintenance</html>"))
            .mount(&server)
            .await;

        let error = client_for(&server).search("nuclear", 5).await.unwrap_err();

        assert_eq!(
            error.to_string(),
            "the answer is not valid: expected value at line 1 column 1"
        );
    }

    #[tokio::test]
    async fn search_returns_the_full_body_of_an_error_without_a_detail_field() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(502).set_body_string("Bad Gateway"))
            .mount(&server)
            .await;

        let error = client_for(&server).search("nuclear", 5).await.unwrap_err();

        assert_eq!(error.to_string(), "HTTP 502: Bad Gateway");
    }
}
