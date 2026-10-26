use reqwest::Url;
use serde::Serialize;
use serde::de::DeserializeOwned;
use thiserror::Error;

use crate::web::types::{
    ApiErrorBody, ExtractRequest, ExtractResponse, ExtractedPage, SearchRequest, SearchResponse,
    SearchResult,
};

/// The header that selects Tavily's keyless access. Nuki sends no API key.
const ACCESS_MODE_HEADER: &str = "X-Tavily-Access-Mode";
const KEYLESS: &str = "keyless";

#[derive(Debug, Error)]
pub enum TavilyError {
    #[error("Tavily request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Tavily returned {status}: {message}")]
    Api { status: u16, message: String },
    #[error("Tavily could not read {url}: {message}")]
    Failed { url: String, message: String },
    #[error("Tavily returned no text for {url}")]
    NoContent { url: String },
}

/// The client of the Tavily Search and Extract APIs.
pub struct TavilyClient {
    http: reqwest::Client,
    /// The base of the API, with no path. In production, it is `https://api.tavily.com`.
    api_base: Url,
}

impl TavilyClient {
    pub fn new(api_base: Url) -> Self {
        Self {
            http: reqwest::Client::new(),
            api_base,
        }
    }

    /// Returns the results of a web search for `query`, at most `limit` of them.
    pub async fn search(&self, query: &str, limit: u32) -> Result<Vec<SearchResult>, TavilyError> {
        let body = SearchRequest {
            query,
            max_results: limit,
        };
        let response: SearchResponse = self.post("search", &body).await?;
        Ok(response.results)
    }

    /// Returns the text of the page at `url`. Tavily downloads the page, not Nuki.
    pub async fn extract(&self, url: &str) -> Result<String, TavilyError> {
        let body = ExtractRequest { urls: [url] };
        let response: ExtractResponse = self.post("extract", &body).await?;
        page_text(response, url)
    }

    async fn post<B: Serialize, T: DeserializeOwned>(
        &self,
        endpoint: &str,
        body: &B,
    ) -> Result<T, TavilyError> {
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
        Ok(response.json().await?)
    }
}

/// Returns the text of the one page that Nuki asked for. Tavily answers 200 also when it could
/// not read the page, and then gives the reason in `failed_results`.
fn page_text(response: ExtractResponse, url: &str) -> Result<String, TavilyError> {
    if let Some(extracted) = response.results.into_iter().next() {
        return text_of(extracted, url);
    }
    match response.failed_results.into_iter().next() {
        Some(failure) => Err(TavilyError::Failed {
            url: url.to_string(),
            message: failure.error,
        }),
        None => Err(TavilyError::NoContent {
            url: url.to_string(),
        }),
    }
}

/// Returns the text of the page. If the text is missing or empty, returns `NoContent`.
fn text_of(extracted: ExtractedPage, url: &str) -> Result<String, TavilyError> {
    extracted
        .raw_content
        .filter(|text| !text.is_empty())
        .ok_or_else(|| TavilyError::NoContent {
            url: url.to_string(),
        })
}

/// Makes the error of a Tavily answer that is not a success. Takes the message from
/// `detail.error`. If the body has no such field, uses the full body as the message.
fn api_error(status: u16, body: String) -> TavilyError {
    let message = serde_json::from_str::<ApiErrorBody>(&body)
        .map(|error| error.detail.error)
        .unwrap_or(body);
    TavilyError::Api { status, message }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::test_support::json_response;
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

    async fn extract_error(server: &MockServer) -> String {
        client_for(server)
            .extract("https://example.com")
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
    async fn extract_sends_a_keyless_request_and_returns_the_text_of_the_page() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/extract"))
            .and(header("X-Tavily-Access-Mode", "keyless"))
            .and(body_json(json!({ "urls": ["https://example.com"] })))
            .respond_with(json_response(200, EXTRACT))
            .mount(&server)
            .await;

        let text = client_for(&server)
            .extract("https://example.com")
            .await
            .unwrap();

        assert_eq!(text, EXTRACT_TEXT);
    }

    #[tokio::test]
    async fn extract_returns_the_reason_when_tavily_could_not_read_the_page() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/extract"))
            .respond_with(json_response(200, EXTRACT_FAILED))
            .mount(&server)
            .await;

        let error = client_for(&server)
            .extract("https://thisdoesnotexist-xyz-123.example")
            .await
            .unwrap_err();

        assert_eq!(
            error.to_string(),
            "Tavily could not read https://thisdoesnotexist-xyz-123.example: Failed to fetch url"
        );
    }

    #[tokio::test]
    async fn extract_returns_no_content_when_the_text_of_the_page_is_null() {
        let server = MockServer::start().await;
        mount_extract_response(
            &server,
            json!({ "results": [{ "url": "https://example.com/", "raw_content": null }] }),
        )
        .await;

        let error = extract_error(&server).await;

        assert_eq!(error, "Tavily returned no text for https://example.com");
    }

    #[tokio::test]
    async fn extract_returns_no_content_when_the_text_of_the_page_is_empty() {
        let server = MockServer::start().await;
        mount_extract_response(
            &server,
            json!({ "results": [{ "url": "https://example.com/", "raw_content": "" }] }),
        )
        .await;

        let error = extract_error(&server).await;

        assert_eq!(error, "Tavily returned no text for https://example.com");
    }

    #[tokio::test]
    async fn extract_returns_no_content_when_tavily_gives_no_result_and_no_failure() {
        let server = MockServer::start().await;
        mount_extract_response(&server, json!({})).await;

        let error = extract_error(&server).await;

        assert_eq!(error, "Tavily returned no text for https://example.com");
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
            "Tavily returned 429: Your request has been blocked due to excessive requests. Please reduce the rate of requests."
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

        assert_eq!(error.to_string(), "Tavily returned 502: Bad Gateway");
    }
}
