use std::convert::Infallible;
use std::sync::Arc;

use serde::de::IgnoredAny;

use crate::repos::manager::RepoManager;
use crate::repos::ops;
use crate::repos::types::{
    GitLogParams, GitShowParams, GrepParams, ListDirParams, ListTagsParams, ReadFileParams,
};
use crate::sandbox::operation::{Operation, OperationTable, Param};

const NAMESPACE: &str = "repo";

/// The operations that read the local clones of the catalog repositories.
pub fn repo_table(manager: Arc<RepoManager>) -> OperationTable {
    OperationTable::new(vec![
        list_repos(&manager),
        grep(&manager),
        read_file(&manager),
        list_dir(&manager),
        git_log(&manager),
        git_show(&manager),
        list_tags(&manager),
    ])
}

fn list_repos(manager: &Arc<RepoManager>) -> Operation {
    Operation::with_handle(
        manager,
        NAMESPACE,
        "listRepos",
        "Get a list of `nukeop/nuclear` and the `NuclearPlayer` repositories.",
        vec![],
        "{ repo: string; description: string | null; cloned: boolean }[]",
        |manager, _: IgnoredAny| async move { Ok::<_, Infallible>(ops::list_repos(&manager)) },
    )
}

fn grep(manager: &Arc<RepoManager>) -> Operation {
    Operation::with_handle(
        manager,
        NAMESPACE,
        "grep",
        "Search the contents of files with a regular expression.",
        vec![
            repo_param(),
            Param {
                name: "pattern",
                ts_type: "string",
                optional: false,
                description: "An extended regular expression.",
            },
            Param {
                name: "path",
                ts_type: "string",
                optional: true,
                description: "A path or a glob that limits the search.",
            },
            ref_param(),
        ],
        "{ path: string; line: number; text: string }[]",
        |manager, params: GrepParams| async move { ops::grep(&manager, params).await },
    )
}

fn read_file(manager: &Arc<RepoManager>) -> Operation {
    Operation::with_handle(
        manager,
        NAMESPACE,
        "readFile",
        "Read a file.",
        vec![
            repo_param(),
            Param {
                name: "path",
                ts_type: "string",
                optional: false,
                description: "The path of the file.",
            },
            ref_param(),
            Param {
                name: "startLine",
                ts_type: "number",
                optional: true,
                description: "The first line to read, from 1. The default is the first line.",
            },
            Param {
                name: "endLine",
                ts_type: "number",
                optional: true,
                description: "The last line to read. The default is the last line.",
            },
        ],
        "{ path: string; ref: string; content: string }",
        |manager, params: ReadFileParams| async move { ops::read_file(&manager, params).await },
    )
}

fn list_dir(manager: &Arc<RepoManager>) -> Operation {
    Operation::with_handle(
        manager,
        NAMESPACE,
        "listDir",
        "Get a list of the contents of a directory.",
        vec![
            repo_param(),
            Param {
                name: "path",
                ts_type: "string",
                optional: true,
                description: "The path of the directory. The default is the root.",
            },
            ref_param(),
        ],
        "{ name: string; kind: \"file\" | \"dir\" }[]",
        |manager, params: ListDirParams| async move { ops::list_dir(&manager, params).await },
    )
}

fn git_log(manager: &Arc<RepoManager>) -> Operation {
    Operation::with_handle(
        manager,
        NAMESPACE,
        "gitLog",
        "Show commits.",
        vec![
            repo_param(),
            Param {
                name: "range",
                ts_type: "string",
                optional: true,
                description: "A revision range, for example `v1.4.0..HEAD`. The default is HEAD.",
            },
            Param {
                name: "path",
                ts_type: "string",
                optional: true,
                description: "Show only the commits that change this path.",
            },
            Param {
                name: "grep",
                ts_type: "string",
                optional: true,
                description: "An extended regular expression. Show only the commits whose message matches it.",
            },
            Param {
                name: "limit",
                ts_type: "number",
                optional: true,
                description: "The maximum number of commits. The default is 30.",
            },
        ],
        "{ hash: string; author: string; date: string; subject: string }[]",
        |manager, params: GitLogParams| async move { ops::git_log(&manager, params).await },
    )
}

fn git_show(manager: &Arc<RepoManager>) -> Operation {
    Operation::with_handle(
        manager,
        NAMESPACE,
        "gitShow",
        "Show a commit, or show a file at a commit.",
        vec![
            repo_param(),
            Param {
                name: "rev",
                ts_type: "string",
                optional: false,
                description: "A commit, or `<commit>:<path>` for a file at a commit.",
            },
        ],
        "{ rev: string; text: string }",
        |manager, params: GitShowParams| async move { ops::git_show(&manager, params).await },
    )
}

fn list_tags(manager: &Arc<RepoManager>) -> Operation {
    Operation::with_handle(
        manager,
        NAMESPACE,
        "listTags",
        "Get a list of the tags.",
        vec![repo_param()],
        "string[]",
        |manager, params: ListTagsParams| async move { ops::list_tags(&manager, params).await },
    )
}

fn repo_param() -> Param {
    Param {
        name: "repo",
        ts_type: "string",
        optional: false,
        description: "The repository, as `owner/name`.",
    }
}

fn ref_param() -> Param {
    Param {
        name: "ref",
        ts_type: "string",
        optional: true,
        description: "A branch, a tag, or a commit. The default is HEAD.",
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;
    use tempfile::TempDir;

    use super::*;
    use crate::repos::catalog::RepoCatalog;
    use crate::repos::test_support::{Origin, TestRepos, commit_2, commit_3};
    use crate::sandbox::runtime::{Sandbox, SandboxLimits};
    use crate::sandbox::test_support::value;

    struct RepoSandbox {
        sandbox: Sandbox,
        _origin: Origin,
        _root: TempDir,
    }

    impl RepoSandbox {
        fn new() -> RepoSandbox {
            let TestRepos {
                origin,
                root,
                manager,
            } = TestRepos::new();
            let table = repo_table(Arc::new(manager));
            RepoSandbox {
                sandbox: Sandbox::new(Arc::new(table), SandboxLimits::production()),
                _origin: origin,
                _root: root,
            }
        }
    }

    #[test]
    fn repo_table_has_the_seven_operations_in_sequence() {
        let manager = RepoManager::new(PathBuf::new(), RepoCatalog::new(vec![]));

        let table = repo_table(Arc::new(manager));

        let names: Vec<String> = table
            .operations()
            .iter()
            .map(|operation| format!("{}.{}", operation.namespace, operation.name))
            .collect();
        assert_eq!(
            names,
            vec![
                "repo.listRepos",
                "repo.grep",
                "repo.readFile",
                "repo.listDir",
                "repo.gitLog",
                "repo.gitShow",
                "repo.listTags",
            ]
        );
    }

    #[tokio::test]
    async fn scripts_can_grep_and_read_a_file() {
        let repos = RepoSandbox::new();

        let outcome = repos
            .sandbox
            .run(
                "const matches = await repo.grep({ repo: 'test/origin', pattern: \"return '(playing|paused)'\" });
                const file = await repo.readFile({ repo: 'test/origin', path: 'src/player.js', startLine: 1, endLine: 3 });
                return { matches, file };",
            )
            .await;

        assert_eq!(
            outcome,
            value(json!({
                "matches": [
                    { "path": "src/player.js", "line": 2, "text": "  return 'playing';" },
                    { "path": "src/player.js", "line": 6, "text": "  return 'paused';" },
                ],
                "file": {
                    "path": "src/player.js",
                    "ref": "HEAD",
                    "content": "export function play() {\n  return 'playing';\n}\n",
                },
            }))
        );
    }

    #[tokio::test]
    async fn scripts_can_list_repos_dirs_tags_and_show_a_commit() {
        let repos = RepoSandbox::new();

        let outcome = repos
            .sandbox
            .run(
                "const repos = await repo.listRepos();
                const entries = await repo.listDir({ repo: 'test/origin', path: 'src' });
                const tags = await repo.listTags({ repo: 'test/origin' });
                const shown = await repo.gitShow({ repo: 'test/origin', rev: 'v1.0.0:README.md' });
                return { repos, entries, tags, shown };",
            )
            .await;

        assert_eq!(
            outcome,
            value(json!({
                "repos": [
                    { "repo": "test/origin", "description": "The test repository", "cloned": false },
                ],
                "entries": [
                    { "name": "player.js", "kind": "file" },
                    { "name": "ui", "kind": "dir" },
                ],
                "tags": ["v1.1.0", "v1.0.0"],
                "shown": { "rev": "v1.0.0:README.md", "text": "# Player\n" },
            }))
        );
    }

    #[tokio::test]
    async fn scripts_can_read_the_log_of_a_range() {
        let repos = RepoSandbox::new();

        let outcome = repos
            .sandbox
            .run("return await repo.gitLog({ repo: 'test/origin', range: 'v1.0.0..HEAD' });")
            .await;

        assert_eq!(outcome, value(json!([commit_3(), commit_2()])));
    }

    #[tokio::test]
    async fn an_unknown_repository_is_an_operation_error() {
        let repos = RepoSandbox::new();

        let outcome = repos
            .sandbox
            .run(
                "try { await repo.listTags({ repo: 'test/unknown' }); } catch (e) { return e.message; }",
            )
            .await;

        assert_eq!(outcome, value(json!("unknown repository test/unknown")));
    }
}
