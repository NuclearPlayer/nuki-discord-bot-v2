use std::sync::Arc;

use serde::Deserialize;

use crate::github::client::GitHubClient;
use crate::repos::catalog::NUCLEAR_REPO;
use crate::sandbox::operation::{Operation, OperationTable, Param};

const NAMESPACE: &str = "github";
const DEFAULT_LIMIT: u32 = 10;

const RELEASES_TYPE: &str =
    "{ tag: string; name: string | null; publishedAt: string; url: string; body: string | null }[]";
const PULL_REQUESTS_TYPE: &str = "{ number: number; title: string; state: \"open\" | \"closed\"; url: string; createdAt: string; mergedAt: string | null }[]";
const DISCUSSIONS_TYPE: &str = "{ number: number; title: string; url: string; createdAt: string; isAnswered: boolean | null; body: string }[]";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListReleasesParams {
    #[serde(default = "default_repo")]
    repo: String,
    #[serde(default = "default_limit")]
    limit: u32,
}

/// The parameters of `searchPullRequests` and `searchDiscussions`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchParams {
    query: String,
    #[serde(default = "default_repo")]
    repo: String,
    #[serde(default = "default_limit")]
    limit: u32,
}

fn default_repo() -> String {
    NUCLEAR_REPO.to_owned()
}

fn default_limit() -> u32 {
    DEFAULT_LIMIT
}

pub fn github_table(client: Arc<GitHubClient>) -> OperationTable {
    OperationTable::new(vec![
        list_releases(&client),
        search_pull_requests(&client),
        search_discussions(&client),
    ])
}

fn list_releases(client: &Arc<GitHubClient>) -> Operation {
    Operation::with_handle(
        client,
        NAMESPACE,
        "listReleases",
        "Get the releases, with their notes and their dates.",
        vec![repo_param(), limit_param()],
        RELEASES_TYPE,
        |client, params: ListReleasesParams| async move {
            client.list_releases(&params.repo, params.limit).await
        },
    )
}

fn search_pull_requests(client: &Arc<GitHubClient>) -> Operation {
    Operation::with_handle(
        client,
        NAMESPACE,
        "searchPullRequests",
        "Get the pull requests that match a query.",
        vec![query_param(), repo_param(), limit_param()],
        PULL_REQUESTS_TYPE,
        |client, params: SearchParams| async move {
            client
                .search_pull_requests(&params.repo, &params.query, params.limit)
                .await
        },
    )
}

fn search_discussions(client: &Arc<GitHubClient>) -> Operation {
    Operation::with_handle(
        client,
        NAMESPACE,
        "searchDiscussions",
        "Get the Discussions that match a query.",
        vec![query_param(), repo_param(), limit_param()],
        DISCUSSIONS_TYPE,
        |client, params: SearchParams| async move {
            client
                .search_discussions(&params.repo, &params.query, params.limit)
                .await
        },
    )
}

fn query_param() -> Param {
    Param {
        name: "query",
        ts_type: "string",
        optional: false,
        description: "GitHub search text. The repository qualifier is added for you.",
    }
}

fn repo_param() -> Param {
    Param {
        name: "repo",
        ts_type: "string",
        optional: true,
        description: "The repository as owner/name. The default is nukeop/nuclear.",
    }
}

fn limit_param() -> Param {
    Param {
        name: "limit",
        ts_type: "number",
        optional: true,
        description: "The maximum number of results, at most 100. The default is 10.",
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::matchers::{body_partial_json, method, path, query_param};
    use wiremock::{Mock, MockServer};

    use super::*;
    use crate::github::test_support::{
        DISCUSSIONS, PLUGIN_REGISTRY_PULL_REQUESTS, RELEASES, client_for,
    };
    use crate::sandbox::runtime::{Sandbox, SandboxLimits, ScriptOutcome};
    use crate::sandbox::test_support::value;
    use crate::test_support::json_response;

    async fn run(server: &MockServer, code: &str) -> ScriptOutcome {
        let table = github_table(Arc::new(client_for(server)));
        Sandbox::new(Arc::new(table), SandboxLimits::production())
            .run(code)
            .await
            .outcome
    }

    #[tokio::test]
    async fn scripts_can_list_releases_with_the_default_repository() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/nukeop/nuclear/releases"))
            .and(query_param("per_page", "10"))
            .respond_with(json_response(200, RELEASES))
            .mount(&server)
            .await;

        let outcome = run(&server, "return await github.listReleases();").await;

        assert_eq!(
            outcome,
            value(json!([
                {
                    "tag": "player@1.51.2",
                    "name": "Nuclear Player v1.51.2",
                    "publishedAt": "2026-10-16T00:52:44Z",
                    "url": "https://github.com/nukeop/nuclear/releases/tag/player%401.51.2",
                    "body": "## What's New in v1.51.2\n\n- **Fix**: Fixed silent YouTube playback on Linux. `Linux` - @nukeop\n",
                },
                {
                    "tag": "player@1.51.1",
                    "name": "Nuclear Player v1.51.1",
                    "publishedAt": "2026-10-14T01:24:31Z",
                    "url": "https://github.com/nukeop/nuclear/releases/tag/player%401.51.1",
                    "body": "Nuclear Player release v1.51.1\n\n> **macOS:** If the app won't open, run `sudo xattr -r -d com.apple.quarantine /Applications/Nuclear.app` in Terminal.\n",
                },
            ]))
        );
    }

    #[tokio::test]
    async fn scripts_can_search_pull_requests_of_a_given_repository() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search/issues"))
            .and(query_param(
                "q",
                "repo:NuclearPlayer/plugin-registry is:pr playlists",
            ))
            .and(query_param("per_page", "2"))
            .respond_with(json_response(200, PLUGIN_REGISTRY_PULL_REQUESTS))
            .mount(&server)
            .await;

        let outcome = run(
            &server,
            "return await github.searchPullRequests({ query: 'playlists', repo: 'NuclearPlayer/plugin-registry', limit: 2 });",
        )
        .await;

        assert_eq!(
            outcome,
            value(json!([
                {
                    "number": 19,
                    "title": "Update plugins.json",
                    "state": "closed",
                    "url": "https://github.com/NuclearPlayer/plugin-registry/pull/19",
                    "createdAt": "2026-10-05T14:07:17Z",
                    "mergedAt": "2026-10-05T22:22:59Z",
                },
                {
                    "number": 18,
                    "title": "Add nuclear-playlist-tags plugin",
                    "state": "closed",
                    "url": "https://github.com/NuclearPlayer/plugin-registry/pull/18",
                    "createdAt": "2026-10-03T17:45:25Z",
                    "mergedAt": null,
                },
            ]))
        );
    }

    #[tokio::test]
    async fn scripts_can_search_discussions() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_partial_json(json!({
                "variables": { "query": "repo:nukeop/nuclear previous button", "first": 10 },
            })))
            .respond_with(json_response(200, DISCUSSIONS))
            .mount(&server)
            .await;

        let outcome = run(
            &server,
            "return await github.searchDiscussions({ query: 'previous button' });",
        )
        .await;

        assert_eq!(
            outcome,
            value(json!([
                {
                    "number": 2178,
                    "title": "Previous button skips current track instead of restarting it",
                    "url": "https://github.com/nukeop/nuclear/discussions/2178",
                    "createdAt": "2026-09-19T11:09:00Z",
                    "isAnswered": null,
                    "body": "### Description\r\n\r\nWhen pressing the **Previous** button while a track is already playing, Nuclear immediately skips to the previous track.\r\n\r\nThis means there ",
                },
                {
                    "number": 2161,
                    "title": "I can't hear music",
                    "url": "https://github.com/nukeop/nuclear/discussions/2161",
                    "createdAt": "2026-09-03T21:29:44Z",
                    "isAnswered": false,
                    "body": "This might be a me thing but I run linux specifically parrot os, and when I tried to press play on a song it didn't play, I installed Spotify, Youtube, SoundClo",
                },
            ]))
        );
    }
}
