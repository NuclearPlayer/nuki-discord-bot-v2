use std::ops::RangeInclusive;

use super::catalog::RepoId;
use super::git::Git;
use super::manager::{RepoError, RepoManager};
use super::types::{
    Commit, DirEntry, EntryKind, FileContent, GitLogParams, GitShowParams, GrepMatch, GrepParams,
    ListDirParams, ListTagsParams, ReadFileParams, RepoSummary, ShownRevision,
};

const DEFAULT_REF: &str = "HEAD";

const LOG_FORMAT: &str = "--format=%H%x1f%an%x1f%aI%x1f%s";

pub fn list_repos(manager: &RepoManager) -> Vec<RepoSummary> {
    manager
        .catalog()
        .entries()
        .iter()
        .map(|entry| RepoSummary {
            repo: entry.id.clone(),
            description: entry.description.clone(),
            cloned: manager.is_cloned(&entry.id),
        })
        .collect()
}

pub async fn grep(manager: &RepoManager, params: GrepParams) -> Result<Vec<GrepMatch>, RepoError> {
    let reference = ref_or_head(params.r#ref);
    let args: Vec<&str> = [
        "grep",
        "-z",
        "-n",
        "-I",
        "-E",
        "-e",
        &params.pattern,
        "--end-of-options",
        &reference,
    ]
    .into_iter()
    .chain(path_args(params.path.as_deref()))
    .collect();
    let stdout = run_in(manager, &params.repo, &args, &[1]).await?;
    let prefix = format!("{reference}:");
    parse_all(stdout.lines(), |line| {
        line.strip_prefix(&prefix).and_then(parse_grep_line)
    })
}

pub async fn read_file(
    manager: &RepoManager,
    params: ReadFileParams,
) -> Result<FileContent, RepoError> {
    let reference = ref_or_head(params.r#ref);
    let object = format!("{reference}:{}", params.path);
    let stdout = run_in(
        manager,
        &params.repo,
        &["show", "--end-of-options", &object],
        &[],
    )
    .await?;
    let lines = params.start_line.unwrap_or(1)..=params.end_line.unwrap_or(usize::MAX);
    Ok(FileContent {
        path: params.path,
        r#ref: reference,
        content: select_lines(&stdout, lines),
    })
}

pub async fn list_dir(
    manager: &RepoManager,
    params: ListDirParams,
) -> Result<Vec<DirEntry>, RepoError> {
    let reference = ref_or_head(params.r#ref);
    let directory = params
        .path
        .map(|path| path.trim_matches('/').to_string())
        .filter(|path| !path.is_empty())
        .map(|path| format!("{path}/"));
    let args: Vec<&str> = ["ls-tree", "-z", "--end-of-options", &reference]
        .into_iter()
        .chain(directory.as_deref())
        .collect();
    let stdout = run_in(manager, &params.repo, &args, &[]).await?;
    parse_all(stdout.split_terminator('\0'), parse_tree_entry)
}

pub async fn git_log(
    manager: &RepoManager,
    params: GitLogParams,
) -> Result<Vec<Commit>, RepoError> {
    let limit = params.limit.to_string();
    let grep = params.grep.map(|grep| format!("--grep={grep}"));
    let args: Vec<&str> = ["log", "-E", "-n", &limit, LOG_FORMAT]
        .into_iter()
        .chain(grep.as_deref())
        .chain(["--end-of-options"])
        .chain(params.range.as_deref())
        .chain(path_args(params.path.as_deref()))
        .collect();
    let stdout = run_in(manager, &params.repo, &args, &[]).await?;
    parse_all(stdout.lines(), parse_commit_line)
}

pub async fn git_show(
    manager: &RepoManager,
    params: GitShowParams,
) -> Result<ShownRevision, RepoError> {
    let text = run_in(
        manager,
        &params.repo,
        &["show", "--stat", "--patch", "--end-of-options", &params.rev],
        &[],
    )
    .await?;
    Ok(ShownRevision {
        rev: params.rev,
        text,
    })
}

pub async fn list_tags(
    manager: &RepoManager,
    params: ListTagsParams,
) -> Result<Vec<String>, RepoError> {
    let stdout = run_in(manager, &params.repo, &["tag", "--sort=-creatordate"], &[]).await?;
    Ok(stdout.lines().map(str::to_string).collect())
}

/// Clones the repository if necessary and runs git in it.
async fn run_in(
    manager: &RepoManager,
    repo: &RepoId,
    args: &[&str],
    allowed_codes: &[i32],
) -> Result<String, RepoError> {
    let path = manager.ensure_cloned(repo).await?;
    let output = Git::run_allowing(Some(&path), args, allowed_codes).await?;
    Ok(output.stdout)
}

fn ref_or_head(reference: Option<String>) -> String {
    reference.unwrap_or_else(|| DEFAULT_REF.to_string())
}

fn path_args(path: Option<&str>) -> impl Iterator<Item = &str> {
    path.into_iter().flat_map(|path| ["--", path])
}

/// Parses each item of the git output. The formats are ours, so an item that does
/// not parse is a bug and fails the whole operation.
fn parse_all<'a, T>(
    items: impl Iterator<Item = &'a str>,
    parse: impl Fn(&'a str) -> Option<T>,
) -> Result<Vec<T>, RepoError> {
    items
        .map(|item| parse(item).ok_or_else(|| RepoError::UnparsableOutput(item.to_string())))
        .collect()
}

/// Reads `path\0line\0text`, the output of `git grep -z -n` without the ref prefix.
fn parse_grep_line(line: &str) -> Option<GrepMatch> {
    match line.splitn(3, '\0').collect::<Vec<_>>()[..] {
        [path, number, text] => Some(GrepMatch {
            path: path.to_string(),
            line: number.parse().ok()?,
            text: text.to_string(),
        }),
        _ => None,
    }
}

fn select_lines(text: &str, lines: RangeInclusive<usize>) -> String {
    text.split_inclusive('\n')
        .zip(1..)
        .filter(|(_, number)| lines.contains(number))
        .map(|(line, _)| line)
        .collect()
}

/// Reads `<mode> <type> <object>\t<path>`, one entry of `git ls-tree -z`. `tree`
/// is a directory, and so is `commit`, which is a submodule.
fn parse_tree_entry(entry: &str) -> Option<DirEntry> {
    let (meta, path) = entry.split_once('\t')?;
    let kind = match meta.split(' ').nth(1)? {
        "blob" => EntryKind::File,
        _ => EntryKind::Dir,
    };
    let name = path.rsplit_once('/').map_or(path, |(_, name)| name);
    Some(DirEntry {
        name: name.to_string(),
        kind,
    })
}

fn parse_commit_line(line: &str) -> Option<Commit> {
    match line.splitn(4, '\u{1f}').collect::<Vec<_>>()[..] {
        [hash, author, date, subject] => Some(Commit {
            hash: hash.to_string(),
            author: author.to_string(),
            date: date.to_string(),
            subject: subject.to_string(),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde::Serialize;
    use serde::de::DeserializeOwned;
    use serde_json::{Value, json};

    use super::*;
    use crate::repos::catalog::CatalogEntry;
    use crate::repos::test_support::{
        Origin, TestRepos, commit_1, commit_2, commit_3, test_repo_id,
    };

    fn params<P: DeserializeOwned>(value: Value) -> P {
        serde_json::from_value(value).unwrap()
    }

    fn to_json(value: impl Serialize) -> Value {
        serde_json::to_value(value).unwrap()
    }

    #[tokio::test]
    async fn list_repos_returns_the_catalog_entries_with_their_clone_state() {
        let repos = TestRepos::with_entries(vec![CatalogEntry {
            id: RepoId::parse("test/absent").unwrap(),
            description: None,
            clone_url: "/nowhere/absent".to_string(),
        }]);
        repos.manager.ensure_cloned(&test_repo_id()).await.unwrap();

        let summaries = list_repos(&repos.manager);

        assert_eq!(
            to_json(summaries),
            json!([
                { "repo": "test/origin", "description": "The test repository", "cloned": true },
                { "repo": "test/absent", "description": null, "cloned": false },
            ])
        );
    }

    #[tokio::test]
    async fn grep_returns_the_matching_lines() {
        let repos = TestRepos::new();

        let matches = grep(
            &repos.manager,
            params(json!({ "repo": "test/origin", "pattern": "return '(playing|paused)'" })),
        )
        .await
        .unwrap();

        assert_eq!(
            to_json(matches),
            json!([
                { "path": "src/player.js", "line": 2, "text": "  return 'playing';" },
                { "path": "src/player.js", "line": 6, "text": "  return 'paused';" },
            ])
        );
    }

    #[tokio::test]
    async fn grep_returns_an_empty_list_when_nothing_matches() {
        let repos = TestRepos::new();

        let matches = grep(
            &repos.manager,
            params(json!({ "repo": "test/origin", "pattern": "rewind" })),
        )
        .await
        .unwrap();

        assert_eq!(to_json(matches), json!([]));
    }

    #[tokio::test]
    async fn grep_returns_a_path_that_contains_a_colon() {
        let origin = Origin::empty();
        origin.commit(
            &[("docs/time:12.md", "play at noon\n")],
            "Add the schedule",
            "2026-01-01T10:00:00Z",
        );
        let repos = TestRepos::with_origin(origin);

        let matches = grep(
            &repos.manager,
            params(json!({ "repo": "test/origin", "pattern": "noon" })),
        )
        .await
        .unwrap();

        assert_eq!(
            to_json(matches),
            json!([{ "path": "docs/time:12.md", "line": 1, "text": "play at noon" }])
        );
    }

    #[tokio::test]
    async fn read_file_returns_the_file_at_the_ref() {
        let repos = TestRepos::new();

        let file = read_file(
            &repos.manager,
            params(json!({ "repo": "test/origin", "path": "src/player.js", "ref": "v1.0.0" })),
        )
        .await
        .unwrap();

        assert_eq!(
            to_json(file),
            json!({
                "path": "src/player.js",
                "ref": "v1.0.0",
                "content": "export function play() {\n  return 'playing';\n}\n\nexport function pause() {\n  return 'paused';\n}\n",
            })
        );
    }

    #[tokio::test]
    async fn read_file_returns_only_the_requested_lines() {
        let repos = TestRepos::new();

        let file = read_file(
            &repos.manager,
            params(json!({
                "repo": "test/origin",
                "path": "src/player.js",
                "startLine": 9,
                "endLine": 10,
            })),
        )
        .await
        .unwrap();

        assert_eq!(
            to_json(file),
            json!({
                "path": "src/player.js",
                "ref": "HEAD",
                "content": "export function stop() {\n  return 'stopped';\n",
            })
        );
    }

    #[tokio::test]
    async fn list_dir_returns_the_entries_of_the_root() {
        let repos = TestRepos::new();

        let entries = list_dir(&repos.manager, params(json!({ "repo": "test/origin" })))
            .await
            .unwrap();

        assert_eq!(
            to_json(entries),
            json!([
                { "name": "README.md", "kind": "file" },
                { "name": "docs", "kind": "dir" },
                { "name": "src", "kind": "dir" },
            ])
        );
    }

    #[tokio::test]
    async fn list_dir_returns_the_entries_of_a_subdirectory() {
        let repos = TestRepos::new();

        let entries = list_dir(
            &repos.manager,
            params(json!({ "repo": "test/origin", "path": "src" })),
        )
        .await
        .unwrap();

        assert_eq!(
            to_json(entries),
            json!([
                { "name": "player.js", "kind": "file" },
                { "name": "ui", "kind": "dir" },
            ])
        );
    }

    #[tokio::test]
    async fn git_log_returns_the_commits_newest_first() {
        let repos = TestRepos::new();

        let commits = git_log(&repos.manager, params(json!({ "repo": "test/origin" })))
            .await
            .unwrap();

        assert_eq!(
            to_json(commits),
            json!([commit_3(), commit_2(), commit_1()])
        );
    }

    #[tokio::test]
    async fn git_log_returns_at_most_the_limit() {
        let repos = TestRepos::new();

        let commits = git_log(
            &repos.manager,
            params(json!({ "repo": "test/origin", "limit": 1 })),
        )
        .await
        .unwrap();

        assert_eq!(to_json(commits), json!([commit_3()]));
    }

    #[tokio::test]
    async fn git_log_returns_only_the_commits_of_the_range() {
        let repos = TestRepos::new();

        let commits = git_log(
            &repos.manager,
            params(json!({ "repo": "test/origin", "range": "v1.0.0..HEAD" })),
        )
        .await
        .unwrap();

        assert_eq!(to_json(commits), json!([commit_3(), commit_2()]));
    }

    #[tokio::test]
    async fn git_log_returns_only_the_commits_that_touch_the_path() {
        let repos = TestRepos::new();

        let commits = git_log(
            &repos.manager,
            params(json!({ "repo": "test/origin", "path": "src/player.js" })),
        )
        .await
        .unwrap();

        assert_eq!(to_json(commits), json!([commit_3(), commit_1()]));
    }

    #[tokio::test]
    async fn git_log_returns_only_the_commits_whose_message_matches_grep() {
        let repos = TestRepos::new();

        let commits = git_log(
            &repos.manager,
            params(json!({ "repo": "test/origin", "grep": "guide|stop" })),
        )
        .await
        .unwrap();

        assert_eq!(to_json(commits), json!([commit_3(), commit_2()]));
    }

    #[tokio::test]
    async fn git_log_rejects_a_range_that_is_an_option() {
        let repos = TestRepos::new();
        let output = repos.root.path().join("log.txt");

        let result = git_log(
            &repos.manager,
            params(json!({
                "repo": "test/origin",
                "range": format!("--output={}", output.display()),
            })),
        )
        .await;

        assert!(matches!(result, Err(RepoError::Git(_))));
        assert!(!output.exists());
    }

    #[tokio::test]
    async fn git_show_returns_the_stat_and_the_patch_of_the_commit() {
        let repos = TestRepos::new();

        let shown = git_show(
            &repos.manager,
            params(json!({ "repo": "test/origin", "rev": "HEAD~1" })),
        )
        .await
        .unwrap();

        assert_eq!(
            to_json(shown),
            json!({
                "rev": "HEAD~1",
                "text": "\
commit 9fc4168ad34755f9a57e44464bcd2fd4558642ff
Author: Test Author <author@example.com>
Date:   Fri Jan 2 10:00:00 2026 +0000

    Add the guide
---
 docs/guide.md | 3 +++
 1 file changed, 3 insertions(+)

diff --git a/docs/guide.md b/docs/guide.md
new file mode 100644
index 0000000..0651af7
--- /dev/null
+++ b/docs/guide.md
@@ -0,0 +1,3 @@
+# Guide
+
+Press play.
",
            })
        );
    }

    #[tokio::test]
    async fn list_tags_returns_the_tags_newest_first() {
        let repos = TestRepos::new();

        let tags = list_tags(&repos.manager, params(json!({ "repo": "test/origin" })))
            .await
            .unwrap();

        assert_eq!(tags, vec!["v1.1.0", "v1.0.0"]);
    }

    #[test]
    fn parse_all_returns_the_line_that_does_not_parse_as_an_error() {
        let lines = [
            "abc\u{1f}Test Author\u{1f}2026-01-01T10:00:00Z\u{1f}Add",
            "abc",
        ];

        let result = parse_all(lines.into_iter(), parse_commit_line);

        assert!(matches!(result, Err(RepoError::UnparsableOutput(line)) if line == "abc"));
    }
}
