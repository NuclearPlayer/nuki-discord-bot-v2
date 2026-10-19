use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use futures::future::join_all;
use tokio::sync::OwnedMutexGuard;

use super::catalog::{RepoCatalog, RepoId};
use super::git::{Git, GitError};

#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    #[error("unknown repository {0}")]
    UnknownRepo(RepoId),
    #[error(transparent)]
    Git(#[from] GitError),
    #[error("could not parse the git output line {0:?}")]
    UnparsableOutput(String),
    #[error("could not remove the leftover partial clone {}: {source}", path.display())]
    RemoveLeftover {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not move the clone into place at {}: {source}", path.display())]
    MoveIntoPlace {
        path: PathBuf,
        source: std::io::Error,
    },
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
        let guard = self.clone_lock(id).lock_owned().await;
        let path = self.path_of(id);
        if !path.exists() {
            clone_in_task(entry.clone_url.clone(), path.clone(), guard).await?;
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

/// Clones in a task of its own that holds the clone lock until the clone ends. A
/// caller that stops waiting does not stop the clone, and the next caller waits for it.
async fn clone_in_task(
    clone_url: String,
    path: PathBuf,
    guard: OwnedMutexGuard<()>,
) -> Result<(), RepoError> {
    let task = tokio::spawn(async move {
        let result = clone_bare(&clone_url, &path).await;
        drop(guard);
        result
    });
    task.await.expect("the clone task panicked")
}

/// Clones into a sibling partial path and then renames it to `path`, so that `path`
/// only exists as a complete clone. A clone that stopped before the rename leaves the
/// partial path, and the next call removes it.
async fn clone_bare(clone_url: &str, path: &Path) -> Result<(), RepoError> {
    let partial = path.with_extension("git.partial");
    remove_leftover(&partial)
        .await
        .map_err(|source| RepoError::RemoveLeftover {
            path: partial.clone(),
            source,
        })?;
    clone_and_configure(clone_url, &partial).await?;
    tokio::fs::rename(&partial, path)
        .await
        .map_err(|source| RepoError::MoveIntoPlace {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(())
}

async fn remove_leftover(path: &Path) -> std::io::Result<()> {
    match tokio::fs::remove_dir_all(path).await {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

async fn clone_and_configure(clone_url: &str, path: &Path) -> Result<(), GitError> {
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
    use std::time::Duration;

    use tempfile::TempDir;
    use tokio::time::{sleep, timeout};

    use super::*;
    use crate::repos::catalog::CatalogEntry;
    use crate::repos::test_support::{TestRepos, rev_parse, test_repo_id};

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
    async fn ensure_cloned_replaces_a_leftover_partial_clone() {
        let repos = TestRepos::new();
        let partial = repos.root.path().join("test/origin.git.partial");
        std::fs::create_dir_all(&partial).unwrap();
        std::fs::write(partial.join("HEAD"), "garbage").unwrap();

        let path = repos.manager.ensure_cloned(&test_repo_id()).await.unwrap();

        assert_eq!(rev_parse(&path, "HEAD"), repos.origin.rev_parse("HEAD"));
        assert!(!partial.exists());
    }

    #[tokio::test]
    async fn a_failed_clone_leaves_no_repository() {
        let missing_origin = TempDir::new().unwrap().path().join("missing");
        let clone_url = missing_origin.to_str().unwrap().to_string();
        let id = RepoId {
            owner: "test".to_string(),
            name: "missing".to_string(),
        };
        let repos = TestRepos::with_entries(vec![CatalogEntry {
            id: id.clone(),
            description: None,
            clone_url: clone_url.clone(),
        }]);
        let path = repos.root.path().join("test/missing.git");
        let partial = repos.root.path().join("test/missing.git.partial");

        let result = repos.manager.ensure_cloned(&id).await;

        let expected_args = vec![
            "clone".to_string(),
            "--bare".to_string(),
            clone_url.clone(),
            partial.to_str().unwrap().to_string(),
        ];
        let expected_stderr = format!("fatal: repository '{clone_url}' does not exist\n");
        assert!(matches!(
            result,
            Err(RepoError::Git(GitError::Failed { args, code: Some(128), stderr }))
                if args == expected_args && stderr == expected_stderr
        ));
        assert!(!path.exists());
        assert!(!partial.exists());
    }

    #[tokio::test]
    async fn a_clone_finishes_after_the_caller_stops_waiting() {
        let repos = TestRepos::new();
        let id = test_repo_id();

        let stopped = timeout(Duration::ZERO, repos.manager.ensure_cloned(&id)).await;
        timeout(Duration::from_secs(10), async {
            while !repos.manager.is_cloned(&id) {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();

        assert!(stopped.is_err());
        let path = repos.root.path().join("test/origin.git");
        assert_eq!(rev_parse(&path, "HEAD"), repos.origin.rev_parse("HEAD"));
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
