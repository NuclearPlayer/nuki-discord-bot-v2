use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use futures::future::join_all;

use super::catalog::{RepoCatalog, RepoId};
use super::git::{Git, GitError};

#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    #[error("unknown repository {0}")]
    UnknownRepo(RepoId),
    #[error(transparent)]
    Git(#[from] GitError),
}

/// The clones fetch the branches and the tags of the origin, and no other refs such
/// as GitHub's `refs/pull/*`.
const BRANCHES_REFSPEC: &str = "+refs/heads/*:refs/heads/*";
const TAGS_REFSPEC: &str = "+refs/tags/*:refs/tags/*";

/// Keeps bare clones of the catalog repositories under `root`.
pub struct RepoManager {
    root: PathBuf,
    catalog: RepoCatalog,
    clone_locks: Mutex<HashMap<RepoId, Arc<tokio::sync::Mutex<()>>>>,
}

impl RepoManager {
    pub fn new(root: PathBuf, catalog: RepoCatalog) -> RepoManager {
        RepoManager {
            root,
            catalog,
            clone_locks: Mutex::new(HashMap::new()),
        }
    }

    pub fn catalog(&self) -> &RepoCatalog {
        &self.catalog
    }

    pub fn path_of(&self, id: &RepoId) -> PathBuf {
        self.root.join(&id.owner).join(format!("{}.git", id.name))
    }

    pub fn is_cloned(&self, id: &RepoId) -> bool {
        self.path_of(id).exists()
    }

    pub async fn ensure_cloned(&self, id: &RepoId) -> Result<PathBuf, RepoError> {
        let entry = self
            .catalog
            .get(id)
            .ok_or_else(|| RepoError::UnknownRepo(id.clone()))?;
        let lock = self.clone_lock(id);
        let _guard = lock.lock().await;
        let path = self.path_of(id);
        if !path.exists() {
            clone_bare(&entry.clone_url, &path).await?;
        }
        Ok(path)
    }

    /// The lock that lets only one call at a time clone `id`.
    fn clone_lock(&self, id: &RepoId) -> Arc<tokio::sync::Mutex<()>> {
        self.clone_locks
            .lock()
            .unwrap()
            .entry(id.clone())
            .or_default()
            .clone()
    }

    /// Updates each cloned repository and returns the failures.
    pub async fn fetch_all(&self) -> Vec<(RepoId, RepoError)> {
        let fetches = self
            .catalog
            .entries()
            .iter()
            .map(|entry| &entry.id)
            .filter(|id| self.is_cloned(id))
            .map(|id| async move { (id.clone(), self.fetch(id).await) });
        join_all(fetches)
            .await
            .into_iter()
            .filter_map(|(id, result)| result.err().map(|error| (id, error)))
            .collect()
    }

    async fn fetch(&self, id: &RepoId) -> Result<(), RepoError> {
        Git::run(Some(&self.path_of(id)), &["remote", "update", "--prune"]).await?;
        Ok(())
    }
}

async fn clone_bare(clone_url: &str, path: &Path) -> Result<(), GitError> {
    Git::run(
        None,
        &["clone", "--bare", clone_url, &path.to_string_lossy()],
    )
    .await?;
    Git::run(
        Some(path),
        &[
            "config",
            "--replace-all",
            "remote.origin.fetch",
            BRANCHES_REFSPEC,
        ],
    )
    .await?;
    Git::run(
        Some(path),
        &["config", "--add", "remote.origin.fetch", TAGS_REFSPEC],
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repos::ops::test_support::{TestRepos, rev_parse, test_repo_id};

    #[tokio::test]
    async fn ensure_cloned_clones_a_repository_that_is_not_cloned() {
        let repos = TestRepos::new();

        let path = repos.manager.ensure_cloned(&test_repo_id()).await.unwrap();

        assert_eq!(path, repos.root.path().join("test/origin.git"));
        assert_eq!(rev_parse(&path, "HEAD"), repos.origin.rev_parse("HEAD"));
    }

    #[tokio::test]
    async fn concurrent_ensure_cloned_calls_clone_once() {
        let repos = TestRepos::new();
        let id = test_repo_id();

        let (first, second) = tokio::join!(
            repos.manager.ensure_cloned(&id),
            repos.manager.ensure_cloned(&id)
        );

        let expected = repos.root.path().join("test/origin.git");
        assert_eq!(first.unwrap(), expected);
        assert_eq!(second.unwrap(), expected);
    }

    #[tokio::test]
    async fn ensure_cloned_returns_unknown_repo_for_a_repository_outside_the_catalog() {
        let repos = TestRepos::new();
        let unknown = RepoId {
            owner: "test".to_string(),
            name: "unknown".to_string(),
        };

        let result = repos.manager.ensure_cloned(&unknown).await;

        assert!(matches!(result, Err(RepoError::UnknownRepo(id)) if id == unknown));
    }

    #[tokio::test]
    async fn fetch_all_gets_new_commits_of_the_origin() {
        let repos = TestRepos::new();
        let path = repos.manager.ensure_cloned(&test_repo_id()).await.unwrap();
        repos.origin.commit(
            &[("NEWS.md", "# News\n")],
            "Add the news",
            "2026-01-04T10:00:00Z",
        );

        let failures: Vec<String> = repos
            .manager
            .fetch_all()
            .await
            .into_iter()
            .map(|(id, error)| format!("{id}: {error}"))
            .collect();

        assert_eq!(failures, Vec::<String>::new());
        assert_eq!(rev_parse(&path, "HEAD"), repos.origin.rev_parse("HEAD"));
    }
}
