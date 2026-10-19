use wiremock::{MockServer, ResponseTemplate};

use crate::github::client::GitHubClient;

pub(crate) const RELEASES: &str = include_str!("../../tests/fixtures/github/releases.json");
pub(crate) const PULL_REQUESTS: &str =
    include_str!("../../tests/fixtures/github/pull_requests.json");
pub(crate) const PLUGIN_REGISTRY_PULL_REQUESTS: &str =
    include_str!("../../tests/fixtures/github/plugin_registry_pull_requests.json");
pub(crate) const DISCUSSIONS: &str = include_str!("../../tests/fixtures/github/discussions.json");

/// A client whose REST and GraphQL requests go to `server`.
pub(crate) fn client_for(server: &MockServer) -> GitHubClient {
    GitHubClient::new(
        "test-token".to_string(),
        server.uri(),
        format!("{}/graphql", server.uri()),
    )
}

pub(crate) fn json_response(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body, "application/json")
}
