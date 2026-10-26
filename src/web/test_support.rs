use serde_json::json;
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer};

use crate::test_support::json_response;
use crate::web::client::TavilyClient;
use crate::web::types::SearchResult;

pub(crate) const SEARCH: &str = include_str!("../../tests/fixtures/tavily/search.json");
pub(crate) const EXTRACT: &str = include_str!("../../tests/fixtures/tavily/extract.json");
pub(crate) const EXTRACT_FAILED: &str =
    include_str!("../../tests/fixtures/tavily/extract_failed.json");
pub(crate) const RATE_LIMITED: &str = include_str!("../../tests/fixtures/tavily/rate_limited.json");

pub(crate) const EXTRACT_TEXT: &str = "This domain is for use in documentation examples without needing permission. This is not a service; avoid relying on it for testing and monitoring purposes\n\n[Learn more](https://iana.org/help/example-domains)";

pub(crate) fn search_results() -> Vec<SearchResult> {
    vec![
        SearchResult {
            title: "Nuclear Music Player - Download".to_string(),
            url: "https://nuclear-music-player.en.softonic.com".to_string(),
            content: "Nuclear Music Player is an innovative audio software that redefines how users experience music across platforms. It combines streaming convenience with local pl".to_string(),
        },
        SearchResult {
            title: "GitHub - nukeop/nuclear: Streaming music player that finds free music for you · GitHub".to_string(),
            url: "https://github.com/nukeop/nuclear".to_string(),
            content: "## Repository files navigation\n\nNuclear Music Player\n\n# Nuclear\n\nNuclear is a free, open-source music player without ads or tracking. Search for any song or art".to_string(),
        },
    ]
}

pub(crate) fn client_for(server: &MockServer) -> TavilyClient {
    TavilyClient::new(server.uri().parse().unwrap())
}

pub(crate) async fn mount_extract(server: &MockServer, url: &str, response: &str) {
    Mock::given(method("POST"))
        .and(path("/extract"))
        .and(body_json(json!({ "urls": [url] })))
        .respond_with(json_response(200, response))
        .mount(server)
        .await;
}
