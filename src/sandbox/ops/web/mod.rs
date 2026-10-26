mod client;

use std::sync::Arc;

pub use client::WebOps;
use client::{FetchParams, SearchParams};

use crate::sandbox::operation::{Operation, OperationTable, Param};

const NAMESPACE: &str = "web";

const SEARCH_TYPE: &str = "{ title: string; url: string; content: string }[]";

/// Returns the operations that search the web and read web pages through Tavily.
pub fn web_table(ops: WebOps) -> OperationTable {
    let ops = Arc::new(ops);
    OperationTable::new(vec![search(&ops), fetch(&ops)])
}

fn search(ops: &Arc<WebOps>) -> Operation {
    Operation::with_handle(
        ops,
        NAMESPACE,
        "search",
        "Search the web.",
        vec![
            Param {
                name: "query",
                ts_type: "string",
                optional: false,
                description: "The search text.",
            },
            Param {
                name: "limit",
                ts_type: "number",
                optional: true,
                description: "The maximum number of results, at most 20. The default is 5.",
            },
        ],
        SEARCH_TYPE,
        |ops, params: SearchParams| async move { ops.search(params).await },
    )
}

fn fetch(ops: &Arc<WebOps>) -> Operation {
    Operation::with_handle(
        ops,
        NAMESPACE,
        "fetch",
        "Get the text of a web page.",
        vec![Param {
            name: "url",
            ts_type: "string",
            optional: false,
            description: "The URL of the page to read.",
        }],
        "string",
        |ops, params: FetchParams| async move { ops.fetch(params).await },
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer};

    use super::*;
    use crate::sandbox::runtime::{Sandbox, SandboxLimits, ScriptOutcome};
    use crate::sandbox::test_support::value;
    use crate::test_support::json_response;
    use crate::web::test_support::{
        EXTRACT, EXTRACT_TEXT, SEARCH, client_for, mount_extract, search_results,
    };

    async fn run(server: &MockServer, code: &str) -> ScriptOutcome {
        let table = web_table(WebOps::new(Arc::new(client_for(server))));
        Sandbox::new(Arc::new(table), SandboxLimits::production())
            .run(code)
            .await
            .outcome
    }

    async fn mount_search(server: &MockServer, max_results: u32) {
        Mock::given(method("POST"))
            .and(path("/search"))
            .and(body_json(
                json!({ "query": "nuclear music player", "max_results": max_results }),
            ))
            .respond_with(json_response(200, SEARCH))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn scripts_get_the_title_url_and_content_of_each_search_result() {
        let server = MockServer::start().await;
        mount_search(&server, 5).await;

        let outcome = run(
            &server,
            "return await web.search({ query: 'nuclear music player' });",
        )
        .await;

        assert_eq!(
            outcome,
            value(json!([
                {
                    "title": "Nuclear Music Player - Download",
                    "url": "https://nuclear-music-player.en.softonic.com",
                    "content": "Nuclear Music Player is an innovative audio software that redefines how users experience music across platforms. It combines streaming convenience with local pl",
                },
                {
                    "title": "GitHub - nukeop/nuclear: Streaming music player that finds free music for you · GitHub",
                    "url": "https://github.com/nukeop/nuclear",
                    "content": "## Repository files navigation\n\nNuclear Music Player\n\n# Nuclear\n\nNuclear is a free, open-source music player without ads or tracking. Search for any song or art",
                },
            ]))
        );
    }

    #[tokio::test]
    async fn scripts_search_with_at_most_20_results_when_the_limit_is_higher() {
        let server = MockServer::start().await;
        mount_search(&server, 20).await;

        let outcome = run(
            &server,
            "return await web.search({ query: 'nuclear music player', limit: 50 });",
        )
        .await;

        assert_eq!(outcome, value(json!(search_results())));
    }

    #[tokio::test]
    async fn scripts_search_with_at_least_1_result_when_the_limit_is_0() {
        let server = MockServer::start().await;
        mount_search(&server, 1).await;

        let outcome = run(
            &server,
            "return await web.search({ query: 'nuclear music player', limit: 0 });",
        )
        .await;

        assert_eq!(outcome, value(json!(search_results())));
    }

    #[tokio::test]
    async fn scripts_can_fetch_the_text_of_a_page() {
        let server = MockServer::start().await;
        mount_extract(&server, "https://example.com", EXTRACT).await;

        let outcome = run(
            &server,
            "return await web.fetch({ url: 'https://example.com' });",
        )
        .await;

        assert_eq!(outcome, value(json!(EXTRACT_TEXT)));
    }
}
