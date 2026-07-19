use std::ffi::OsStr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;

use super::command::{CommandError, CommandOutput, run_bounded};
use crate::model::{BranchId, PullRequest};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPullRequest {
    number: u64,
    title: String,
    url: String,
    head_ref_name: String,
    head_ref_oid: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrMatch {
    pub branch: BranchId,
    pub oid: Arc<str>,
    pub pull_request: PullRequest,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GitHubError {
    Spawn(Arc<str>),
    Timeout(Duration),
    NonZero(Arc<str>),
    Truncated,
    Malformed(Arc<str>),
    Io(Arc<str>),
}

impl std::fmt::Display for GitHubError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(error) => write!(formatter, "could not start gh: {error}"),
            Self::Timeout(duration) => write!(formatter, "gh timed out after {duration:?}"),
            Self::NonZero(error) => write!(formatter, "gh failed: {error}"),
            Self::Truncated => formatter.write_str("gh response exceeded the output limit"),
            Self::Malformed(error) => write!(formatter, "gh returned malformed JSON: {error}"),
            Self::Io(error) => write!(formatter, "gh I/O failed: {error}"),
        }
    }
}

impl std::error::Error for GitHubError {}

impl From<CommandError> for GitHubError {
    fn from(error: CommandError) -> Self {
        match error {
            CommandError::Spawn(error) => Self::Spawn(Arc::from(error.to_string())),
            CommandError::Timeout(duration) => Self::Timeout(duration),
            CommandError::Read(error) | CommandError::Write(error) => {
                Self::Io(Arc::from(error.to_string()))
            }
            CommandError::InputTooLarge(size) => {
                Self::Io(Arc::from(format!("input exceeded limit ({size} bytes)")))
            }
        }
    }
}

pub fn fetch(cwd: &Path) -> Result<Vec<PrMatch>, GitHubError> {
    let args = [
        "pr",
        "list",
        "--state",
        "open",
        "--limit",
        "1000",
        "--json",
        "number,title,url,headRefName,headRefOid",
    ];
    let output = run_bounded(
        OsStr::new("gh"),
        args,
        cwd,
        Duration::from_secs(3),
        4 * 1024 * 1024,
    )
    .map_err(GitHubError::from)?;
    parse_output(output)
}

fn parse_output(output: CommandOutput) -> Result<Vec<PrMatch>, GitHubError> {
    if !output.status.success() {
        return Err(GitHubError::NonZero(Arc::from(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        )));
    }
    if output.stdout_truncated {
        return Err(GitHubError::Truncated);
    }
    parse_json(&output.stdout)
}

pub fn parse_json(bytes: &[u8]) -> Result<Vec<PrMatch>, GitHubError> {
    let values: Vec<GhPullRequest> = serde_json::from_slice(bytes)
        .map_err(|error| GitHubError::Malformed(Arc::from(error.to_string())))?;
    Ok(values
        .into_iter()
        .filter_map(|value| {
            Some(PrMatch {
                branch: BranchId::new(value.head_ref_name),
                oid: Arc::from(value.head_ref_oid?),
                pull_request: PullRequest {
                    number: value.number,
                    title: Arc::from(value.title),
                    url: Arc::from(value.url),
                },
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;

    use super::*;

    fn output(status: i32, truncated: bool) -> CommandOutput {
        CommandOutput {
            status: ExitStatus::from_raw(status),
            stdout: b"[]".to_vec(),
            stderr: b"auth failed".to_vec(),
            stdout_truncated: truncated,
            stderr_truncated: false,
        }
    }

    #[test]
    fn provider_output_failures_remain_typed() {
        assert!(matches!(
            parse_output(output(1 << 8, false)),
            Err(GitHubError::NonZero(_))
        ));
        assert!(matches!(
            parse_output(output(0, true)),
            Err(GitHubError::Truncated)
        ));
    }

    #[test]
    fn command_failures_remain_typed() {
        let spawn = GitHubError::from(CommandError::Spawn(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "missing",
        )));
        let timeout = GitHubError::from(CommandError::Timeout(Duration::from_secs(3)));
        assert!(matches!(spawn, GitHubError::Spawn(_)));
        assert!(matches!(timeout, GitHubError::Timeout(_)));
    }
}
