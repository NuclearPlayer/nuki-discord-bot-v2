use std::path::Path;

use tokio::process::Command;

#[derive(Debug, PartialEq)]
pub struct GitOutput {
    pub status_code: i32,
    pub stdout: String,
}

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("could not run git: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("git {} failed with exit code {code:?}: {stderr}", args.join(" "))]
    Failed {
        args: Vec<String>,
        code: Option<i32>,
        stderr: String,
    },
}

pub struct Git;

impl Git {
    /// Runs `git` in `dir`, or in the current directory if `dir` is `None`.
    pub async fn run(dir: Option<&Path>, args: &[&str]) -> Result<GitOutput, GitError> {
        Git::run_allowing(dir, args, &[]).await
    }

    /// Like `run`, but an exit code in `allowed_codes` is a success, as is 0.
    pub async fn run_allowing(
        dir: Option<&Path>,
        args: &[&str],
        allowed_codes: &[i32],
    ) -> Result<GitOutput, GitError> {
        let output = command(dir, args).output().await?;
        match output.status.code() {
            Some(code) if code == 0 || allowed_codes.contains(&code) => Ok(GitOutput {
                status_code: code,
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            }),
            code => Err(GitError::Failed {
                args: args.iter().map(|arg| arg.to_string()).collect(),
                code,
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            }),
        }
    }
}

/// Builds the command without the system and user configuration, so that the
/// configuration of the machine cannot change the output.
fn command(dir: Option<&Path>, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    command
        .args(args)
        .kill_on_drop(true)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null");
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    command
}
