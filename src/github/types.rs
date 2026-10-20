use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrgRepo {
    pub full_name: String,
    pub description: Option<String>,
    pub clone_url: String,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub tag: String,
    pub name: Option<String>,
    pub published_at: DateTime<Utc>,
    pub url: String,
    pub body: Option<String>,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub url: String,
    pub created_at: DateTime<Utc>,
    pub merged_at: Option<DateTime<Utc>>,
}

/// This type deserializes directly from the GraphQL response, because GraphQL uses the same
/// field names as this type.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Discussion {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub created_at: DateTime<Utc>,
    pub is_answered: Option<bool>,
    pub body: String,
}

#[derive(Deserialize)]
pub(super) struct RepoResponse {
    full_name: String,
    description: Option<String>,
    clone_url: String,
    pub(super) archived: bool,
}

impl From<RepoResponse> for OrgRepo {
    fn from(repo: RepoResponse) -> Self {
        Self {
            full_name: repo.full_name,
            description: repo.description,
            clone_url: repo.clone_url,
        }
    }
}

#[derive(Deserialize)]
pub(super) struct ReleaseResponse {
    tag_name: String,
    name: Option<String>,
    published_at: DateTime<Utc>,
    html_url: String,
    body: Option<String>,
}

impl From<ReleaseResponse> for Release {
    fn from(release: ReleaseResponse) -> Self {
        Self {
            tag: release.tag_name,
            name: release.name,
            published_at: release.published_at,
            url: release.html_url,
            body: release.body,
        }
    }
}

#[derive(Deserialize)]
pub(super) struct SearchResponse<T> {
    pub(super) items: Vec<T>,
}

/// A pull request in the form that the issue search returns. Only the nested `pull_request`
/// object contains the merge time.
#[derive(Deserialize)]
pub(super) struct IssueResponse {
    number: u64,
    title: String,
    state: String,
    html_url: String,
    created_at: DateTime<Utc>,
    pull_request: PullRequestLink,
}

#[derive(Deserialize)]
struct PullRequestLink {
    merged_at: Option<DateTime<Utc>>,
}

impl From<IssueResponse> for PullRequest {
    fn from(issue: IssueResponse) -> Self {
        Self {
            number: issue.number,
            title: issue.title,
            state: issue.state,
            url: issue.html_url,
            created_at: issue.created_at,
            merged_at: issue.pull_request.merged_at,
        }
    }
}

#[derive(Deserialize)]
pub(super) struct GraphQlResponse<T> {
    pub(super) data: Option<T>,
    #[serde(default)]
    pub(super) errors: Vec<GraphQlErrorMessage>,
}

#[derive(Deserialize)]
pub(super) struct GraphQlErrorMessage {
    pub(super) message: String,
}

#[derive(Deserialize)]
pub(super) struct DiscussionSearchData {
    search: DiscussionNodes,
}

#[derive(Deserialize)]
struct DiscussionNodes {
    nodes: Vec<Discussion>,
}

impl DiscussionSearchData {
    pub(super) fn into_discussions(self) -> Vec<Discussion> {
        self.search.nodes
    }
}

#[derive(Deserialize)]
pub(super) struct ApiErrorBody {
    pub(super) message: String,
}
