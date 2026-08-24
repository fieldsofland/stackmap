use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;

use super::command::{CommandError, CommandOutput, run_bounded};
use crate::model::{BranchId, PullRequest, PullRequestStatus};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPullRequest {
    number: u64,
    title: String,
    url: String,
    head_ref_name: String,
    head_ref_oid: Option<String>,
    state: String,
    review_decision: Option<String>,
    #[serde(default)]
    updated_at: String,
}

#[derive(Debug, Deserialize)]
struct GhRepository {
    #[serde(rename = "nameWithOwner")]
    name_with_owner: String,
}

#[derive(Debug, Deserialize)]
struct GhApiHead {
    #[serde(rename = "ref")]
    reference: String,
    sha: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GhApiPullRequest {
    number: u64,
    title: String,
    html_url: String,
    state: String,
    merged_at: Option<String>,
    #[serde(default)]
    updated_at: String,
    head: GhApiHead,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrMatch {
    pub branch: BranchId,
    pub head_oid: Option<Arc<str>>,
    pub updated_at: Arc<str>,
    pub pull_request: PullRequest,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenPullRequests {
    pub matches: Vec<PrMatch>,
    pub saturated: bool,
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
            CommandError::MissingPipe(pipe) => {
                Self::Io(Arc::from(format!("gh {pipe} pipe was unavailable")))
            }
        }
    }
}

pub fn fetch(cwd: &Path) -> Result<Vec<PrMatch>, GitHubError> {
    fetch_list(cwd, "all")
}

pub fn fetch_open(cwd: &Path) -> Result<OpenPullRequests, GitHubError> {
    const LIMIT: usize = 1000;
    let matches = fetch_list(cwd, "open")?;
    Ok(OpenPullRequests {
        saturated: matches.len() == LIMIT,
        matches,
    })
}

fn fetch_list(cwd: &Path, state: &str) -> Result<Vec<PrMatch>, GitHubError> {
    let args = [
        "pr",
        "list",
        "--state",
        state,
        "--limit",
        "1000",
        "--json",
        "number,title,url,headRefName,headRefOid,state,reviewDecision,updatedAt",
    ];
    let output = run_bounded(
        OsStr::new("gh"),
        args,
        cwd,
        Duration::from_secs(8),
        4 * 1024 * 1024,
    )
    .map_err(GitHubError::from)?;
    parse_output(output)
}

pub fn repository_slug(cwd: &Path) -> Result<Arc<str>, GitHubError> {
    let output = run_bounded(
        OsStr::new("gh"),
        ["repo", "view", "--json", "nameWithOwner"],
        cwd,
        Duration::from_secs(3),
        64 * 1024,
    )
    .map_err(GitHubError::from)?;
    let bytes = successful_stdout(output)?;
    let repository: GhRepository = serde_json::from_slice(&bytes)
        .map_err(|error| GitHubError::Malformed(Arc::from(error.to_string())))?;
    validate_slug(&repository.name_with_owner)?;
    Ok(Arc::from(repository.name_with_owner))
}

pub(crate) fn fetch_head_with_timeout(
    cwd: &Path,
    repository: &str,
    owner: &str,
    branch: &BranchId,
    timeout: Duration,
) -> Result<Vec<PrMatch>, GitHubError> {
    run_api(cwd, head_lookup_args(repository, owner, branch), timeout)
}

pub(crate) fn fetch_commit_with_timeout(
    cwd: &Path,
    repository: &str,
    oid: &str,
    timeout: Duration,
) -> Result<Vec<PrMatch>, GitHubError> {
    if oid.is_empty() || !oid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(GitHubError::Malformed(Arc::from(
            "local commit OID was not hexadecimal",
        )));
    }
    run_api(cwd, commit_lookup_args(repository, oid), timeout)
}

fn run_api(
    cwd: &Path,
    args: Vec<OsString>,
    timeout: Duration,
) -> Result<Vec<PrMatch>, GitHubError> {
    let output = run_bounded(OsStr::new("gh"), args, cwd, timeout, 1024 * 1024)
        .map_err(GitHubError::from)?;
    let bytes = successful_stdout(output)?;
    parse_api_json(&bytes)
}

pub(crate) fn head_lookup_args(repository: &str, owner: &str, branch: &BranchId) -> Vec<OsString> {
    vec![
        "api".into(),
        "--method".into(),
        "GET".into(),
        format!("repos/{repository}/pulls").into(),
        "--raw-field".into(),
        "state=all".into(),
        "--raw-field".into(),
        format!("head={owner}:{}", branch.0).into(),
        "--raw-field".into(),
        "per_page=100".into(),
    ]
}

pub(crate) fn commit_lookup_args(repository: &str, oid: &str) -> Vec<OsString> {
    vec![
        "api".into(),
        "--method".into(),
        "GET".into(),
        format!("repos/{repository}/commits/{oid}/pulls").into(),
        "--header".into(),
        "Accept: application/vnd.github+json".into(),
        "--raw-field".into(),
        "per_page=100".into(),
    ]
}

fn parse_output(output: CommandOutput) -> Result<Vec<PrMatch>, GitHubError> {
    parse_json(&successful_stdout(output)?)
}

fn successful_stdout(output: CommandOutput) -> Result<Vec<u8>, GitHubError> {
    if !output.status.success() {
        return Err(GitHubError::NonZero(clean_text(&String::from_utf8_lossy(
            &output.stderr,
        ))));
    }
    if output.stdout_truncated {
        return Err(GitHubError::Truncated);
    }
    Ok(output.stdout)
}

pub fn parse_json(bytes: &[u8]) -> Result<Vec<PrMatch>, GitHubError> {
    let values: Vec<GhPullRequest> = serde_json::from_slice(bytes)
        .map_err(|error| GitHubError::Malformed(Arc::from(error.to_string())))?;
    Ok(values
        .into_iter()
        .map(|value| {
            let head_oid = value.head_ref_oid.map(Arc::from);
            PrMatch {
                branch: BranchId::new(value.head_ref_name),
                head_oid: head_oid.clone(),
                updated_at: Arc::from(value.updated_at),
                pull_request: PullRequest {
                    number: value.number,
                    title: clean_text(&value.title),
                    url: Arc::from(value.url),
                    status: match value.state.as_str() {
                        "MERGED" => PullRequestStatus::Merged,
                        "CLOSED" => PullRequestStatus::Closed,
                        _ if value.review_decision.as_deref() == Some("APPROVED") => {
                            PullRequestStatus::Approved
                        }
                        _ => PullRequestStatus::Open,
                    },
                    head_oid,
                    match_quality: crate::model::PullRequestMatch::StaleTip,
                },
            }
        })
        .collect())
}

fn parse_api_json(bytes: &[u8]) -> Result<Vec<PrMatch>, GitHubError> {
    let values: Vec<GhApiPullRequest> = serde_json::from_slice(bytes)
        .map_err(|error| GitHubError::Malformed(Arc::from(error.to_string())))?;
    Ok(values
        .into_iter()
        .map(|value| {
            let head_oid = value.head.sha.map(Arc::from);
            PrMatch {
                branch: BranchId::new(value.head.reference),
                head_oid: head_oid.clone(),
                updated_at: Arc::from(value.updated_at),
                pull_request: PullRequest {
                    number: value.number,
                    title: clean_text(&value.title),
                    url: Arc::from(value.html_url),
                    status: if value.merged_at.is_some() {
                        PullRequestStatus::Merged
                    } else if value.state.eq_ignore_ascii_case("closed") {
                        PullRequestStatus::Closed
                    } else {
                        PullRequestStatus::Open
                    },
                    head_oid,
                    match_quality: crate::model::PullRequestMatch::StaleTip,
                },
            }
        })
        .collect())
}

fn validate_slug(slug: &str) -> Result<(), GitHubError> {
    let mut components = slug.split('/');
    let valid_component = |value: &str| {
        !value.is_empty()
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    };
    if components.next().is_some_and(valid_component)
        && components.next().is_some_and(valid_component)
        && components.next().is_none()
    {
        Ok(())
    } else {
        Err(GitHubError::Malformed(Arc::from(
            "repository nameWithOwner was invalid",
        )))
    }
}

fn clean_text(value: &str) -> Arc<str> {
    Arc::from(
        value
            .chars()
            .map(|character| {
                if character.is_control() {
                    ' '
                } else {
                    character
                }
            })
            .take(512)
            .collect::<String>()
            .trim(),
    )
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

    #[test]
    fn targeted_calls_force_get_and_keep_hostile_branch_in_one_value() {
        let branch = BranchId::new("-topic/%# unicode");
        let args = head_lookup_args("owner/repo", "owner", &branch);
        let rendered = args
            .iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(&rendered[..3], ["api", "--method", "GET"]);
        assert!(
            rendered
                .iter()
                .any(|value| value == "head=owner:-topic/%# unicode")
        );
        assert!(!rendered.iter().any(|value| value == "POST"));
    }

    #[test]
    fn commit_calls_force_get_and_use_a_fixed_endpoint_argument() {
        let args = commit_lookup_args("owner/repo", "abcdef");
        let rendered = args
            .iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(&rendered[..3], ["api", "--method", "GET"]);
        assert_eq!(rendered[3], "repos/owner/repo/commits/abcdef/pulls");
    }

    #[test]
    fn rest_pull_requests_parse_merged_and_sanitize_titles() {
        let matches = parse_api_json(
            br#"[{"number":7,"title":"line\n\u001b[31m","html_url":"https://example.invalid/7","state":"closed","merged_at":"2026-08-01T00:00:00Z","head":{"ref":"topic","sha":"abc"}}]"#,
        )
        .unwrap();
        assert_eq!(matches[0].pull_request.status, PullRequestStatus::Merged);
        assert!(!matches[0].pull_request.title.chars().any(char::is_control));
    }
}
