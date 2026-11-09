use futures::future::BoxFuture;
use reqwest::Url;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::web::provider::{ProviderError, SearchResult, WebProvider, parse_json};

/// The client of the Firecrawl Search and Scrape APIs. Nuki sends no API key, so Firecrawl
/// uses its keyless free tier.
pub struct FirecrawlClient {
    http: reqwest::Client,
    /// The base of the API, with no path. In production, it is `https://api.firecrawl.dev`.
    api_base: Url,
}

/// The time in milliseconds that Firecrawl can use for one request. It is less than the timeout
/// of Nuki's HTTP client, so Firecrawl stops its work before Nuki stops waiting. Then the work
/// does not use the keyless quota.
const FIRECRAWL_TIMEOUT_MS: u32 = 10_000;

#[derive(Serialize)]
struct SearchRequest<'a> {
    query: &'a str,
    limit: u32,
    timeout: u32,
}

#[derive(Deserialize)]
struct SearchResponse {
    data: SearchData,
}

#[derive(Deserialize)]
struct SearchData {
    #[serde(default)]
    web: Vec<WebResult>,
}

#[derive(Deserialize)]
struct WebResult {
    #[serde(default)]
    title: String,
    url: String,
    #[serde(default)]
    description: String,
}

#[derive(Serialize)]
struct ScrapeRequest<'a> {
    url: &'a str,
    formats: [&'a str; 1],
    timeout: u32,
}

#[derive(Deserialize)]
struct ScrapeResponse {
    data: ScrapedPage,
}

#[derive(Deserialize)]
struct ScrapedPage {
    markdown: Option<String>,
    #[serde(default)]
    metadata: PageMetadata,
}

/// The status of the page that Firecrawl read. Firecrawl answers 200 also when the page is an
/// error page, and then gives the status of the page here.
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageMetadata {
    status_code: Option<u16>,
    error: Option<String>,
}

/// The body of a Firecrawl error, for example `{ "success": false, "error": "..." }`.
#[derive(Deserialize)]
struct ErrorBody {
    error: String,
}

impl WebProvider for FirecrawlClient {
    fn name(&self) -> &'static str {
        "firecrawl"
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

impl FirecrawlClient {
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
            limit,
            timeout: FIRECRAWL_TIMEOUT_MS,
        };
        let response: SearchResponse = self.post("v2/search", &body).await?;
        Ok(response.data.web.into_iter().map(search_result).collect())
    }

    async fn fetch_page(&self, url: &str) -> Result<String, ProviderError> {
        let body = ScrapeRequest {
            url,
            formats: ["markdown"],
            timeout: FIRECRAWL_TIMEOUT_MS,
        };
        let response: ScrapeResponse = self.post("v2/scrape", &body).await?;
        page_text(response.data, url)
    }

    async fn post<B: Serialize, T: DeserializeOwned>(
        &self,
        endpoint: &str,
        body: &B,
    ) -> Result<T, ProviderError> {
        let url = self
            .api_base
            .join(endpoint)
            .expect("an endpoint path is a valid relative URL");
        let response = self.http.post(url).json(body).send().await?;
        let status = response.status();
        if !status.is_success() {
            return Err(api_error(status.as_u16(), response.text().await?));
        }
        parse_json(&response.text().await?)
    }
}

fn search_result(result: WebResult) -> SearchResult {
    SearchResult {
        title: result.title,
        url: result.url,
        content: result.description,
    }
}

/// Returns the markdown of the page. If the page has an HTTP status of 400 or more, returns
/// Firecrawl's error, or the status if Firecrawl gives no error.
fn page_text(page: ScrapedPage, url: &str) -> Result<String, ProviderError> {
    if let Some(status) = page.metadata.status_code.filter(|status| *status >= 400) {
        let message = page
            .metadata
            .error
            .unwrap_or_else(|| format!("HTTP {status}"));
        return Err(ProviderError::Failed {
            url: url.to_string(),
            message,
        });
    }
    page.markdown
        .filter(|text| !text.is_empty())
        .ok_or_else(|| ProviderError::NoContent {
            url: url.to_string(),
        })
}

/// Makes the error of a Firecrawl answer that is not a success. Takes the message from `error`.
/// If the body has no such field, uses the full body as the message.
fn api_error(status: u16, body: String) -> ProviderError {
    let message = serde_json::from_str::<ErrorBody>(&body)
        .map(|error| error.error)
        .unwrap_or(body);
    ProviderError::Status { status, message }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer};

    use super::*;
    use crate::test_support::json_response;

    const SEARCH: &str = include_str!("../../tests/fixtures/firecrawl/search.json");
    const SCRAPE: &str = include_str!("../../tests/fixtures/firecrawl/scrape.json");
    const BLOCKED: &str = include_str!("../../tests/fixtures/firecrawl/blocked.json");
    const RATE_LIMITED: &str = include_str!("../../tests/fixtures/firecrawl/rate_limited.json");

    fn client_for(server: &MockServer) -> FirecrawlClient {
        FirecrawlClient::new(reqwest::Client::new(), server.uri().parse().unwrap())
    }

    async fn mount(server: &MockServer, endpoint: &str, status: u16, body: &str) {
        Mock::given(method("POST"))
            .and(path(endpoint))
            .respond_with(json_response(status, body))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn search_sends_the_query_and_the_limit_and_returns_the_results() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v2/search"))
            .and(body_json(
                json!({ "query": "nuclear music player", "limit": 2, "timeout": 10000 }),
            ))
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
                    title: "Nuclear - Free Open-Source Music Player".to_string(),
                    url: "https://nuclearplayer.com/".to_string(),
                    content: "Nuclear is a free, open-source music player without ads or tracking."
                        .to_string(),
                },
                SearchResult {
                    title: "GitHub - nukeop/nuclear: Streaming music player that finds free music for you".to_string(),
                    url: "https://github.com/nukeop/nuclear".to_string(),
                    content: "Streaming music player that finds free music for you.".to_string(),
                },
            ]
        );
    }

    #[tokio::test]
    async fn fetch_asks_for_markdown_and_returns_the_markdown_of_the_page() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v2/scrape"))
            .and(body_json(
                json!({ "url": "https://example.com", "formats": ["markdown"], "timeout": 10000 }),
            ))
            .respond_with(json_response(200, SCRAPE))
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
    async fn search_returns_the_message_when_firecrawl_refuses_the_ip_address() {
        let server = MockServer::start().await;
        mount(&server, "/v2/search", 403, BLOCKED).await;

        let error = client_for(&server).search("nuclear", 5).await.unwrap_err();

        assert_eq!(
            error.to_string(),
            "HTTP 403: Unfortunately, your IP address looks like a VPN or proxy, so Firecrawl can't be used without an API key from here. Sign up with Google or GitHub at https://firecrawl.dev/k/pkfggerayywt for a free API "
        );
    }

    #[tokio::test]
    async fn fetch_returns_the_message_of_a_rate_limit_error() {
        let server = MockServer::start().await;
        mount(&server, "/v2/scrape", 429, RATE_LIMITED).await;

        let error = client_for(&server)
            .fetch("https://example.com")
            .await
            .unwrap_err();

        assert_eq!(
            error.to_string(),
            "HTTP 429: You've hit Firecrawl's keyless free tier rate limit. To continue now, create a free API key at https://firecrawl.dev/k/pkfggerayywt"
        );
    }

    async fn mount_scrape_of_status(server: &MockServer, status_code: u16, error: Option<&str>) {
        let answer = json!({
            "success": true,
            "data": {
                "markdown": "# Not Found",
                "metadata": { "statusCode": status_code, "error": error },
            },
        });
        mount(server, "/v2/scrape", 200, &answer.to_string()).await;
    }

    #[tokio::test]
    async fn fetch_fails_with_the_status_when_the_page_has_status_404_and_no_error() {
        let server = MockServer::start().await;
        mount_scrape_of_status(&server, 404, None).await;

        let error = client_for(&server)
            .fetch("https://example.com/missing")
            .await
            .unwrap_err();

        assert_eq!(
            error.to_string(),
            "could not read https://example.com/missing: HTTP 404"
        );
    }

    #[tokio::test]
    async fn fetch_fails_with_the_error_of_firecrawl_when_the_page_has_status_500() {
        let server = MockServer::start().await;
        mount_scrape_of_status(&server, 500, Some("The page returned an error")).await;

        let error = client_for(&server)
            .fetch("https://example.com/broken")
            .await
            .unwrap_err();

        assert_eq!(
            error.to_string(),
            "could not read https://example.com/broken: The page returned an error"
        );
    }

    #[tokio::test]
    async fn search_returns_an_invalid_answer_when_the_answer_has_no_data() {
        let server = MockServer::start().await;
        mount(
            &server,
            "/v2/search",
            200,
            &json!({ "success": true }).to_string(),
        )
        .await;

        let error = client_for(&server).search("nuclear", 5).await.unwrap_err();

        assert_eq!(
            error.to_string(),
            "the answer is not valid: missing field `data` at line 1 column 16"
        );
    }

    #[tokio::test]
    async fn fetch_returns_no_content_when_the_answer_has_no_markdown() {
        let server = MockServer::start().await;
        mount(
            &server,
            "/v2/scrape",
            200,
            &json!({ "success": true, "data": { "metadata": {} } }).to_string(),
        )
        .await;

        let error = client_for(&server)
            .fetch("https://example.com")
            .await
            .unwrap_err();

        assert_eq!(error.to_string(), "no text for https://example.com");
    }
}
