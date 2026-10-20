use reqwest::header::{ACCEPT, USER_AGENT};
use reqwest::{Method, Request, RequestBuilder, Url};
use serde::de::DeserializeOwned;
use serde_json::json;
use thiserror::Error;

use crate::github::types::{
    ApiErrorBody, Discussion, DiscussionSearchData, GraphQlResponse, IssueResponse, OrgRepo,
    PullRequest, Release, ReleaseResponse, RepoResponse, SearchResponse,
};
use crate::repos::catalog::RepoId;

const GITHUB_MEDIA_TYPE: &str = "application/vnd.github+json";
const BOT_USER_AGENT: &str = "nuki-discord-bot";
const DISCUSSIONS_QUERY: &str = "query($query: String!, $first: Int!) { search(query: $query, type: DISCUSSION, first: $first) { nodes { ... on Discussion { number title url createdAt isAnswered body } } } }";

#[derive(Debug, Error)]
pub enum GitHubError {
    #[error("GitHub request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("GitHub API returned {status}: {message}")]
    Api { status: u16, message: String },
    #[error("GitHub GraphQL error: {message}")]
    GraphQl { message: String },
}

pub struct GitHubClient {
    http: reqwest::Client,
    token: String,
    api_base: Url,
    graphql_url: Url,
}

impl GitHubClient {
    pub fn new(token: String, api_base: Url, graphql_url: Url) -> Self {
        Self {
            http: reqwest::Client::new(),
            token,
            api_base,
            graphql_url,
        }
    }

    // One page of 100 is enough for NuclearPlayer today. Follow the `Link` header if the organization grows past it.
    pub async fn list_org_repos(&self, org: &str) -> Result<Vec<OrgRepo>, GitHubError> {
        let repos: Vec<RepoResponse> = self
            .get(
                &["orgs", org, "repos"],
                &[("per_page", "100"), ("type", "public")],
            )
            .await?;
        Ok(repos
            .into_iter()
            .filter(|repo| !repo.archived)
            .map(OrgRepo::from)
            .collect())
    }

    pub async fn list_releases(
        &self,
        repo: &RepoId,
        limit: u32,
    ) -> Result<Vec<Release>, GitHubError> {
        let per_page = limit.to_string();
        let releases: Vec<ReleaseResponse> = self
            .get(
                &["repos", repo.owner(), repo.name(), "releases"],
                &[("per_page", &per_page)],
            )
            .await?;
        Ok(releases.into_iter().map(Release::from).collect())
    }

    pub async fn search_pull_requests(
        &self,
        repo: &RepoId,
        query: &str,
        limit: u32,
    ) -> Result<Vec<PullRequest>, GitHubError> {
        let search_text = format!("repo:{repo} is:pr {query}");
        let per_page = limit.to_string();
        let search: SearchResponse<IssueResponse> = self
            .get(
                &["search", "issues"],
                &[("q", &search_text), ("per_page", &per_page)],
            )
            .await?;
        Ok(search.items.into_iter().map(PullRequest::from).collect())
    }

    pub async fn search_discussions(
        &self,
        repo: &RepoId,
        query: &str,
        limit: u32,
    ) -> Result<Vec<Discussion>, GitHubError> {
        let body = json!({
            "query": DISCUSSIONS_QUERY,
            "variables": {
                "query": format!("repo:{repo} {query}"),
                "first": limit,
            },
        });
        let request = self
            .request(Method::POST, self.graphql_url.clone())
            .json(&body)
            .build()?;
        let response: GraphQlResponse<DiscussionSearchData> = self.execute(request).await?;
        graphql_data(response).map(DiscussionSearchData::into_discussions)
    }

    async fn get<T: DeserializeOwned>(
        &self,
        segments: &[&str],
        query: &[(&str, &str)],
    ) -> Result<T, GitHubError> {
        let request = self
            .request(Method::GET, self.api_url(segments))
            .query(query)
            .build()?;
        self.execute(request).await
    }

    /// The API base with `segments` added. Each segment is escaped, so a `/` or a `?`
    /// in it cannot change the path or the query. `path_segments_mut().extend` drops
    /// a `.` or `..` segment instead of escaping it; `RepoId` rejects both.
    fn api_url(&self, segments: &[&str]) -> Url {
        let mut url = self.api_base.clone();
        url.path_segments_mut()
            .expect("the GitHub API base is an http or https URL")
            .pop_if_empty()
            .extend(segments);
        url
    }

    fn request(&self, method: Method, url: Url) -> RequestBuilder {
        self.http
            .request(method, url)
            .bearer_auth(&self.token)
            .header(ACCEPT, GITHUB_MEDIA_TYPE)
            .header(USER_AGENT, BOT_USER_AGENT)
    }

    async fn execute<T: DeserializeOwned>(&self, request: Request) -> Result<T, GitHubError> {
        let response = self.http.execute(request).await?;
        let status = response.status();
        if !status.is_success() {
            return Err(api_error(status.as_u16(), response.text().await?));
        }
        Ok(response.json().await?)
    }
}

/// GitHub error bodies have a `message` field. A body without it is the message as a whole.
fn api_error(status: u16, body: String) -> GitHubError {
    let message = serde_json::from_str::<ApiErrorBody>(&body)
        .map(|error| error.message)
        .unwrap_or(body);
    GitHubError::Api { status, message }
}

fn graphql_data<T>(response: GraphQlResponse<T>) -> Result<T, GitHubError> {
    match (response.errors.into_iter().next(), response.data) {
        (Some(error), _) => Err(GitHubError::GraphQl {
            message: error.message,
        }),
        (None, Some(data)) => Ok(data),
        (None, None) => Err(GitHubError::GraphQl {
            message: "the response has neither data nor errors".to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};
    use serde_json::json;
    use wiremock::matchers::{body_json, header, method, path, query_param};
    use wiremock::{Mock, MockServer};

    use super::*;
    use crate::github::test_support::{DISCUSSIONS, PULL_REQUESTS, RELEASES, client_for};
    use crate::test_support::json_response;

    fn time(text: &str) -> DateTime<Utc> {
        text.parse().unwrap()
    }

    fn nuclear() -> RepoId {
        RepoId::parse("nukeop/nuclear").unwrap()
    }

    #[tokio::test]
    async fn list_org_repos_returns_only_repositories_that_are_not_archived() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/orgs/NuclearPlayer/repos"))
            .and(query_param("per_page", "100"))
            .and(query_param("type", "public"))
            .respond_with(json_response(
                200,
                include_str!("../../tests/fixtures/github/org_repos.json"),
            ))
            .mount(&server)
            .await;

        let repos = client_for(&server)
            .list_org_repos("NuclearPlayer")
            .await
            .unwrap();

        assert_eq!(
            repos,
            vec![
                OrgRepo {
                    full_name: "NuclearPlayer/plugin-registry".to_string(),
                    description: None,
                    clone_url: "https://github.com/NuclearPlayer/plugin-registry.git".to_string(),
                },
                OrgRepo {
                    full_name: "NuclearPlayer/nuclear-plugin-lastfm".to_string(),
                    description: Some(
                        "Last.fm scrobbling and now playing integration for Nuclear music player"
                            .to_string()
                    ),
                    clone_url: "https://github.com/NuclearPlayer/nuclear-plugin-lastfm.git"
                        .to_string(),
                },
            ]
        );
    }

    #[tokio::test]
    async fn a_question_mark_in_a_path_segment_is_escaped() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/orgs/Nuclear%3Fx=/repos"))
            .and(query_param("per_page", "100"))
            .and(query_param("type", "public"))
            .respond_with(json_response(200, "[]"))
            .expect(1)
            .mount(&server)
            .await;

        let repos = client_for(&server)
            .list_org_repos("Nuclear?x=")
            .await
            .unwrap();

        assert_eq!(repos, vec![]);
    }

    #[tokio::test]
    async fn list_releases_returns_the_releases() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/nukeop/nuclear/releases"))
            .and(query_param("per_page", "2"))
            .respond_with(json_response(200, RELEASES))
            .mount(&server)
            .await;

        let releases = client_for(&server)
            .list_releases(&nuclear(), 2)
            .await
            .unwrap();

        assert_eq!(
            releases,
            vec![
                Release {
                    tag: "player@1.51.2".to_string(),
                    name: Some("Nuclear Player v1.51.2".to_string()),
                    published_at: time("2026-10-16T00:52:44Z"),
                    url: "https://github.com/nukeop/nuclear/releases/tag/player%401.51.2"
                        .to_string(),
                    body: Some(
                        "## What's New in v1.51.2\n\n- **Fix**: Fixed silent YouTube playback on Linux. `Linux` - @nukeop\n"
                            .to_string()
                    ),
                },
                Release {
                    tag: "player@1.51.1".to_string(),
                    name: Some("Nuclear Player v1.51.1".to_string()),
                    published_at: time("2026-10-14T01:24:31Z"),
                    url: "https://github.com/nukeop/nuclear/releases/tag/player%401.51.1"
                        .to_string(),
                    body: Some(
                        "Nuclear Player release v1.51.1\n\n> **macOS:** If the app won't open, run `sudo xattr -r -d com.apple.quarantine /Applications/Nuclear.app` in Terminal.\n"
                            .to_string()
                    ),
                },
            ]
        );
    }

    #[tokio::test]
    async fn search_pull_requests_sends_the_repository_and_the_query() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search/issues"))
            .and(query_param("q", "repo:nukeop/nuclear is:pr playlists"))
            .and(query_param("per_page", "2"))
            .respond_with(json_response(200, PULL_REQUESTS))
            .expect(1)
            .mount(&server)
            .await;

        client_for(&server)
            .search_pull_requests(&nuclear(), "playlists", 2)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn search_pull_requests_returns_the_pull_requests() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search/issues"))
            .respond_with(json_response(200, PULL_REQUESTS))
            .mount(&server)
            .await;

        let pull_requests = client_for(&server)
            .search_pull_requests(&nuclear(), "playlists", 2)
            .await
            .unwrap();

        assert_eq!(
            pull_requests,
            vec![
                PullRequest {
                    number: 2230,
                    title: "Playlists upgrade".to_string(),
                    state: "open".to_string(),
                    url: "https://github.com/nukeop/nuclear/pull/2230".to_string(),
                    created_at: time("2026-10-19T00:58:02Z"),
                    merged_at: None,
                },
                PullRequest {
                    number: 2220,
                    title: "Jam lyrics".to_string(),
                    state: "closed".to_string(),
                    url: "https://github.com/nukeop/nuclear/pull/2220".to_string(),
                    created_at: time("2026-10-13T20:50:28Z"),
                    merged_at: Some(time("2026-10-13T23:01:47Z")),
                },
            ]
        );
    }

    #[tokio::test]
    async fn search_discussions_sends_the_search_text_as_a_variable() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_json(json!({
                "query": "query($query: String!, $first: Int!) { search(query: $query, type: DISCUSSION, first: $first) { nodes { ... on Discussion { number title url createdAt isAnswered body } } } }",
                "variables": {
                    "query": "repo:nukeop/nuclear \"previous\" button",
                    "first": 2,
                },
            })))
            .respond_with(json_response(200, DISCUSSIONS))
            .expect(1)
            .mount(&server)
            .await;

        client_for(&server)
            .search_discussions(&nuclear(), "\"previous\" button", 2)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn search_discussions_returns_the_discussions() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(json_response(200, DISCUSSIONS))
            .mount(&server)
            .await;

        let discussions = client_for(&server)
            .search_discussions(&nuclear(), "spotify", 2)
            .await
            .unwrap();

        assert_eq!(
            discussions,
            vec![
                Discussion {
                    number: 2178,
                    title: "Previous button skips current track instead of restarting it"
                        .to_string(),
                    url: "https://github.com/nukeop/nuclear/discussions/2178".to_string(),
                    created_at: time("2026-09-19T11:09:00Z"),
                    is_answered: None,
                    body: "### Description\r\n\r\nWhen pressing the **Previous** button while a track is already playing, Nuclear immediately skips to the previous track.\r\n\r\nThis means there "
                        .to_string(),
                },
                Discussion {
                    number: 2161,
                    title: "I can't hear music".to_string(),
                    url: "https://github.com/nukeop/nuclear/discussions/2161".to_string(),
                    created_at: time("2026-09-03T21:29:44Z"),
                    is_answered: Some(false),
                    body: "This might be a me thing but I run linux specifically parrot os, and when I tried to press play on a song it didn't play, I installed Spotify, Youtube, SoundClo"
                        .to_string(),
                },
            ]
        );
    }

    #[tokio::test]
    async fn a_graphql_error_becomes_a_graphql_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(json_response(
                200,
                include_str!("../../tests/fixtures/github/graphql_error.json"),
            ))
            .mount(&server)
            .await;

        let error = client_for(&server)
            .search_discussions(&nuclear(), "spotify", 101)
            .await
            .unwrap_err();

        let GitHubError::GraphQl { message } = error else {
            panic!("expected a GraphQL error, got {error:?}");
        };
        assert_eq!(
            message,
            "Requesting 101 records on the `search` connection exceeds the `first` limit of 100 records."
        );
    }

    #[tokio::test]
    async fn an_error_status_becomes_an_api_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/nukeop/nuclear/releases"))
            .respond_with(json_response(
                403,
                r#"{"message":"API rate limit exceeded"}"#,
            ))
            .mount(&server)
            .await;

        let error = client_for(&server)
            .list_releases(&nuclear(), 2)
            .await
            .unwrap_err();

        assert!(
            matches!(
                &error,
                GitHubError::Api { status: 403, message } if message == "API rate limit exceeded"
            ),
            "unexpected error: {error:?}"
        );
    }

    #[tokio::test]
    async fn requests_have_the_github_headers() {
        let server = MockServer::start().await;
        let with_headers = |mock: wiremock::MockBuilder| {
            mock.and(header("authorization", "Bearer test-token"))
                .and(header("accept", "application/vnd.github+json"))
                .and(header("user-agent", "nuki-discord-bot"))
        };
        with_headers(Mock::given(method("GET")).and(path("/repos/nukeop/nuclear/releases")))
            .respond_with(json_response(200, RELEASES))
            .expect(1)
            .mount(&server)
            .await;
        with_headers(Mock::given(method("POST")).and(path("/graphql")))
            .respond_with(json_response(200, DISCUSSIONS))
            .expect(1)
            .mount(&server)
            .await;
        let client = client_for(&server);

        client.list_releases(&nuclear(), 2).await.unwrap();
        client
            .search_discussions(&nuclear(), "spotify", 2)
            .await
            .unwrap();
    }
}
