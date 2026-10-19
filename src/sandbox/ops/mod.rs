pub mod discord;
pub mod github;
pub mod repo;

use std::sync::Arc;

use crate::github::client::GitHubClient;
use crate::repos::manager::RepoManager;
use crate::sandbox::operation::OperationTable;

/// All operations of `run_script`: `discord.*`, then `repo.*`, then `github.*`.
pub fn full_table(
    discord: discord::DiscordOps,
    repos: Arc<RepoManager>,
    github: Arc<GitHubClient>,
) -> OperationTable {
    OperationTable::merge([
        discord::discord_table(discord),
        repo::repo_table(repos),
        github::github_table(github),
    ])
}
