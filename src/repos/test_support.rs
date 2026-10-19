use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};
use tempfile::TempDir;

use crate::repos::catalog::{CatalogEntry, RepoCatalog, RepoId};
use crate::repos::git::isolated_git;
use crate::repos::manager::RepoManager;

pub(crate) fn test_repo_id() -> RepoId {
    RepoId {
        owner: "test".to_string(),
        name: "origin".to_string(),
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    output_of(command(dir, args))
}

fn git_at(dir: &Path, args: &[&str], date: &str) -> String {
    let mut command = command(dir, args);
    command
        .env("GIT_AUTHOR_NAME", "Test Author")
        .env("GIT_AUTHOR_EMAIL", "author@example.com")
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_NAME", "Test Committer")
        .env("GIT_COMMITTER_EMAIL", "committer@example.com")
        .env("GIT_COMMITTER_DATE", date);
    output_of(command)
}

fn command(dir: &Path, args: &[&str]) -> Command {
    let mut command = isolated_git();
    command.current_dir(dir).args(args);
    command
}

fn output_of(mut command: Command) -> String {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

pub(crate) fn rev_parse(dir: &Path, rev: &str) -> String {
    git(dir, &["rev-parse", rev])
}

pub(crate) struct Origin {
    dir: TempDir,
}

impl Origin {
    pub(crate) fn empty() -> Origin {
        let origin = Origin {
            dir: TempDir::new().unwrap(),
        };
        git(origin.path(), &["init", "-b", "main"]);
        origin
    }

    pub(crate) fn create() -> Origin {
        let origin = Origin::empty();
        origin.commit(
            &[
                ("README.md", "# Player\n"),
                (
                    "src/player.js",
                    "export function play() {\n  return 'playing';\n}\n\nexport function pause() {\n  return 'paused';\n}\n",
                ),
            ],
            "Add the player",
            "2026-01-01T10:00:00Z",
        );
        origin.tag("v1.0.0");
        origin.commit(
            &[("docs/guide.md", "# Guide\n\nPress play.\n")],
            "Add the guide",
            "2026-01-02T10:00:00Z",
        );
        origin.commit(
            &[
                (
                    "src/player.js",
                    "export function play() {\n  return 'playing';\n}\n\nexport function pause() {\n  return 'paused';\n}\n\nexport function stop() {\n  return 'stopped';\n}\n",
                ),
                ("src/ui/button.js", "export const button = 'play';\n"),
            ],
            "Add stop to the player",
            "2026-01-03T10:00:00Z",
        );
        origin.tag("v1.1.0");
        origin
    }

    pub(crate) fn path(&self) -> &Path {
        self.dir.path()
    }

    pub(crate) fn catalog_entry(&self) -> CatalogEntry {
        CatalogEntry {
            id: test_repo_id(),
            description: Some("The test repository".to_string()),
            clone_url: self.path().to_str().unwrap().to_string(),
        }
    }

    pub(crate) fn commit(&self, files: &[(&str, &str)], message: &str, date: &str) {
        for (path, content) in files {
            let file = self.path().join(path);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, content).unwrap();
        }
        git(self.path(), &["add", "--all"]);
        git_at(self.path(), &["commit", "--message", message], date);
    }

    fn tag(&self, name: &str) {
        git(self.path(), &["tag", name]);
    }

    pub(crate) fn rev_parse(&self, rev: &str) -> String {
        rev_parse(self.path(), rev)
    }
}

/// A commit of `Origin::create`, as the JSON of a `Commit`.
fn commit_json(hash: &str, date: &str, subject: &str) -> Value {
    json!({ "hash": hash, "author": "Test Author", "date": date, "subject": subject })
}

pub(crate) fn commit_1() -> Value {
    commit_json(
        "321254eb140ee36a0943df0de59adff028cebaac",
        "2026-01-01T10:00:00Z",
        "Add the player",
    )
}

pub(crate) fn commit_2() -> Value {
    commit_json(
        "9fc4168ad34755f9a57e44464bcd2fd4558642ff",
        "2026-01-02T10:00:00Z",
        "Add the guide",
    )
}

pub(crate) fn commit_3() -> Value {
    commit_json(
        "aec79b59c2b59a6335b1b17f2dfe5d78ff92764f",
        "2026-01-03T10:00:00Z",
        "Add stop to the player",
    )
}

pub(crate) struct TestRepos {
    pub(crate) origin: Origin,
    pub(crate) root: TempDir,
    pub(crate) manager: RepoManager,
}

impl TestRepos {
    pub(crate) fn new() -> TestRepos {
        TestRepos::with_entries(vec![])
    }

    pub(crate) fn with_entries(extra: Vec<CatalogEntry>) -> TestRepos {
        TestRepos::build(Origin::create(), extra)
    }

    pub(crate) fn with_origin(origin: Origin) -> TestRepos {
        TestRepos::build(origin, vec![])
    }

    fn build(origin: Origin, extra: Vec<CatalogEntry>) -> TestRepos {
        let root = TempDir::new().unwrap();
        let entries = [origin.catalog_entry()].into_iter().chain(extra).collect();
        let manager = RepoManager::new(root.path().to_path_buf(), RepoCatalog::new(entries));
        TestRepos {
            origin,
            root,
            manager,
        }
    }
}
