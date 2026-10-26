pub mod discord;
pub mod github;
pub mod repo;
pub mod web;

use std::sync::Arc;

use crate::github::client::GitHubClient;
use crate::repos::manager::RepoManager;
use crate::sandbox::operation::OperationTable;

/// Returns all operations of `run_script`: `discord.*`, then `repo.*`, then `github.*`, then
/// `web.*`.
pub fn full_table(
    discord: discord::DiscordOps,
    repos: Arc<RepoManager>,
    github: Arc<GitHubClient>,
    web: web::WebOps,
) -> OperationTable {
    OperationTable::merge([
        discord::discord_table(discord),
        repo::repo_table(repos),
        github::github_table(github),
        web::web_table(web),
    ])
}
