use std::ffi::OsStr;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};

use super::command::{CommandError, CommandOutput, run_bounded};
use super::graphite::{raw_branch_metadata_has_child, raw_branch_metadata_presence, read_topology};
use crate::model::{BranchId, DiffStat, GraphiteProvenance, RepositoryState};

const GIT_TIMEOUT: Duration = Duration::from_secs(3);
const OUTPUT_LIMIT: usize = 16 * 1024 * 1024;
type DeleteContractCache = Arc<Mutex<Option<Result<(), Arc<str>>>>>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeleteRequest {
    pub branch: BranchId,
    pub expected_oid: Arc<str>,
    pub expected_provenance: GraphiteProvenance,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeleteOutcome {
    Deleted,
    Unchanged,
    Inconsistent,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitBranch {
    pub id: BranchId,
    pub oid: Arc<str>,
    pub committed_at: i64,
    pub worktree: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitInventory {
    pub root: PathBuf,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
    pub repository_id: Arc<str>,
    pub branches: Vec<GitBranch>,
    pub current: Option<BranchId>,
    pub dirty: bool,
    pub state: RepositoryState,
    pub source_token: u64,
}

#[derive(Clone, Debug)]
pub struct GitAdapter {
    start_dir: PathBuf,
    root: PathBuf,
    git_dir: PathBuf,
    common_dir: PathBuf,
    gt_delete_contract: DeleteContractCache,
}

trait DeleteCommandRunner {
    fn run(
        &self,
        args: &[&OsStr],
        cwd: &Path,
        timeout: Duration,
        output_limit: usize,
    ) -> std::result::Result<CommandOutput, CommandError>;
}

struct SystemDeleteCommandRunner;

trait DeletePostStateReader {
    fn exact_local_ref(&self, adapter: &GitAdapter, branch: &BranchId) -> Result<Option<Arc<str>>>;
    fn metadata_present(&self, common_dir: &Path, branch: &BranchId) -> Result<bool>;
}

struct SystemDeletePostStateReader;

impl DeletePostStateReader for SystemDeletePostStateReader {
    fn exact_local_ref(&self, adapter: &GitAdapter, branch: &BranchId) -> Result<Option<Arc<str>>> {
        adapter.exact_local_ref(branch)
    }

    fn metadata_present(&self, common_dir: &Path, branch: &BranchId) -> Result<bool> {
        raw_branch_metadata_presence(common_dir, branch)
    }
}

impl DeleteCommandRunner for SystemDeleteCommandRunner {
    fn run(
        &self,
        args: &[&OsStr],
        cwd: &Path,
        timeout: Duration,
        output_limit: usize,
    ) -> std::result::Result<CommandOutput, CommandError> {
        run_bounded(
            OsStr::new("gt"),
            args.iter().copied(),
            cwd,
            timeout,
            output_limit,
        )
    }
}

impl GitAdapter {
    pub fn discover(start_dir: impl Into<PathBuf>) -> Result<Self> {
        let start_dir = start_dir.into();
        let (root, git_dir, common_dir) = discover_paths(&start_dir)?;
        Ok(Self {
            start_dir,
            root,
            git_dir,
            common_dir,
            gt_delete_contract: Arc::new(Mutex::new(None)),
        })
    }

    pub fn inventory(&self) -> Result<GitInventory> {
        let root = self.root.clone();
        let git_dir = self.git_dir.clone();
        let common_dir = self.common_dir.clone();
        let output = self.git(
            &[
                "for-each-ref",
                "--sort=refname",
                "--format=%(refname:short)%00%(objectname)%00%(committerdate:unix)%00%(worktreepath)%00",
                "refs/heads",
            ],
            OUTPUT_LIMIT,
        )?;
        ensure_success(&output, "enumerate local branches")?;
        let fields: Vec<&[u8]> = output.stdout.split(|byte| *byte == 0).collect();
        let mut branches = Vec::with_capacity(fields.len() / 4);
        for chunk in fields.chunks(4) {
            if chunk.len() < 4 || chunk[0].is_empty() {
                continue;
            }
            let name_bytes = strip_record_separator(chunk[0]);
            let name = text(name_bytes, "branch name")?;
            let oid = text(chunk[1], "object id")?;
            let committed_at = text(chunk[2], "committer timestamp")?
                .parse::<i64>()
                .with_context(|| format!("invalid committer timestamp for {name}"))?;
            let worktree_text = text(chunk[3], "worktree path")?;
            branches.push(GitBranch {
                id: BranchId::new(name),
                oid: Arc::from(oid),
                committed_at,
                worktree: (!worktree_text.is_empty()).then(|| PathBuf::from(worktree_text)),
            });
        }

        let current_output = self.git(&["symbolic-ref", "--quiet", "--short", "HEAD"], 4096)?;
        if current_output.stdout_truncated {
            bail!("could not read current branch: Git output exceeded the bounded output limit");
        }
        let current = current_output.status.success().then(|| {
            BranchId::new(
                String::from_utf8_lossy(strip_line_ending(&current_output.stdout)).to_string(),
            )
        });
        let head_exists = self
            .git(&["rev-parse", "--verify", "HEAD"], 4096)?
            .status
            .success();
        let dirty_output = self.git(
            &["status", "--porcelain=v2", "-z", "--untracked-files=normal"],
            OUTPUT_LIMIT,
        )?;
        ensure_success(&dirty_output, "read worktree status")?;
        let operation = operation_state(&git_dir);
        let state = if let Some(operation) = operation {
            RepositoryState::OperationInProgress(Arc::from(operation))
        } else if !head_exists {
            RepositoryState::Unborn
        } else if current.is_none() {
            RepositoryState::Detached
        } else {
            RepositoryState::Ready
        };

        let repository_id = repository_id(&common_dir);
        let source_token = token_for(&branches, current.as_ref(), &state);
        Ok(GitInventory {
            root,
            git_dir,
            common_dir,
            repository_id,
            branches,
            current,
            dirty: !dirty_output.stdout.is_empty(),
            state,
            source_token,
        })
    }

    pub fn diffstat(&self, parent_oid: &str, child_oid: &str) -> Result<DiffStat> {
        let output = self.git(
            &["diff", "--numstat", parent_oid, child_oid, "--"],
            OUTPUT_LIMIT,
        )?;
        ensure_success(&output, "compute parent-relative diffstat")?;
        let mut stat = DiffStat::default();
        for line in output.stdout.split(|byte| *byte == b'\n') {
            if line.is_empty() {
                continue;
            }
            let mut columns = line.splitn(3, |byte| *byte == b'\t');
            let insertions = columns.next().unwrap_or_default();
            let deletions = columns.next().unwrap_or_default();
            if insertions == b"-" || deletions == b"-" {
                stat.binary_files = stat.binary_files.saturating_add(1);
            } else {
                stat.insertions = stat.insertions.saturating_add(parse_u64(insertions)?);
                stat.deletions = stat.deletions.saturating_add(parse_u64(deletions)?);
            }
            stat.files = stat.files.saturating_add(1);
        }
        Ok(stat)
    }

    pub fn checkout(&self, branch: &BranchId) -> Result<()> {
        let live = self.inventory()?;
        if live.current.as_ref() == Some(branch) {
            bail!("{branch} is already current");
        }
        if !matches!(live.state, RepositoryState::Ready) {
            bail!(
                "checkout disabled while repository state is {:?}",
                live.state
            );
        }
        let target = live
            .branches
            .iter()
            .find(|candidate| &candidate.id == branch)
            .ok_or_else(|| anyhow!("branch {branch} no longer exists"))?;
        if let Some(path) = &target.worktree
            && path != &live.root
        {
            bail!("branch {branch} is checked out at {}", path.display());
        }
        let output = self.git_os(
            &[
                OsStr::new("switch"),
                OsStr::new("--"),
                OsStr::new(branch.0.as_ref()),
            ],
            OUTPUT_LIMIT,
        )?;
        ensure_success(&output, "switch branch")
    }

    pub fn delete_branch(&self, request: &DeleteRequest) -> Result<DeleteOutcome> {
        self.delete_branch_with(request, &SystemDeleteCommandRunner)
    }

    fn delete_branch_with(
        &self,
        request: &DeleteRequest,
        runner: &impl DeleteCommandRunner,
    ) -> Result<DeleteOutcome> {
        self.delete_branch_with_state(request, runner, &SystemDeletePostStateReader)
    }

    fn delete_branch_with_state(
        &self,
        request: &DeleteRequest,
        runner: &impl DeleteCommandRunner,
        post_state: &impl DeletePostStateReader,
    ) -> Result<DeleteOutcome> {
        if request.expected_provenance == GraphiteProvenance::Tracked
            && request.branch.0.starts_with('-')
        {
            bail!(
                "cannot safely delete option-shaped branch {}",
                request.branch
            );
        }
        if request.expected_provenance == GraphiteProvenance::Tracked {
            self.ensure_gt_delete_contract(runner)?;
        }

        // This is the final live safety preflight. For tracked branches it
        // intentionally runs after the installed CLI contract check and
        // immediately before the provider invocation.
        let live = self.inventory()?;
        if !matches!(live.state, RepositoryState::Ready) {
            bail!(
                "deletion disabled while repository state is {:?}",
                live.state
            );
        }
        if live.current.as_ref() == Some(&request.branch) {
            bail!("cannot delete the current branch {}", request.branch);
        }
        let branch = live
            .branches
            .iter()
            .find(|branch| branch.id == request.branch)
            .ok_or_else(|| anyhow!("branch {} no longer exists", request.branch))?;
        if branch.oid != request.expected_oid {
            bail!("branch {} changed after confirmation", request.branch);
        }
        if branch.worktree.is_some() {
            bail!("branch {} is checked out in a worktree", request.branch);
        }
        let local: std::collections::HashSet<_> = live
            .branches
            .iter()
            .map(|branch| branch.id.clone())
            .collect();
        let graphite = read_topology(&live.common_dir, &local);
        if graphite.configured_trunks.contains(&request.branch) {
            bail!("cannot delete configured trunk {}", request.branch);
        }
        let provenance = graphite.provenance(&request.branch);
        if provenance != request.expected_provenance {
            bail!("Graphite tracking changed after confirmation");
        }

        match provenance {
            GraphiteProvenance::DefinitelyUntracked => self.delete_untracked(request),
            GraphiteProvenance::Tracked => {
                if request.branch.0.starts_with('-') {
                    bail!(
                        "cannot safely delete option-shaped branch {}",
                        request.branch
                    );
                }
                if graphite
                    .parents
                    .values()
                    .any(|parent| parent == &request.branch)
                {
                    bail!(
                        "tracked branch {} has children; refusing a restacking deletion",
                        request.branch
                    );
                }
                if raw_branch_metadata_has_child(&live.common_dir, &request.branch)? {
                    bail!(
                        "tracked branch {} has children; refusing a restacking deletion",
                        request.branch
                    );
                }
                self.delete_tracked(request, runner, post_state)
            }
            GraphiteProvenance::Degraded => {
                bail!("Graphite tracking is degraded; deletion is disabled")
            }
        }
    }

    fn delete_untracked(&self, request: &DeleteRequest) -> Result<DeleteOutcome> {
        let merged = self.git(
            &["merge-base", "--is-ancestor", &request.expected_oid, "HEAD"],
            4096,
        )?;
        if !merged.status.success() {
            bail!(
                "branch {} is not merged into the current HEAD",
                request.branch
            );
        }
        let reference = format!("refs/heads/{}", request.branch);
        let output = self.git_os(
            &[
                OsStr::new("update-ref"),
                OsStr::new("-d"),
                OsStr::new(&reference),
                OsStr::new(request.expected_oid.as_ref()),
            ],
            OUTPUT_LIMIT,
        )?;
        if !output.status.success() {
            let inventory = self.inventory()?;
            let current = inventory
                .branches
                .iter()
                .find(|branch| branch.id == request.branch);
            return Ok(match current {
                Some(branch) if branch.oid == request.expected_oid => DeleteOutcome::Unchanged,
                Some(_) => DeleteOutcome::Inconsistent,
                None => DeleteOutcome::Inconsistent,
            });
        }
        Ok(
            if self
                .inventory()?
                .branches
                .iter()
                .any(|branch| branch.id == request.branch)
            {
                DeleteOutcome::Inconsistent
            } else {
                DeleteOutcome::Deleted
            },
        )
    }

    fn ensure_gt_delete_contract(&self, runner: &impl DeleteCommandRunner) -> Result<()> {
        let mut cached = self
            .gt_delete_contract
            .lock()
            .map_err(|_| anyhow!("Graphite deletion contract cache is poisoned"))?;
        if let Some(result) = cached.as_ref() {
            return result.clone().map_err(|error| anyhow!(error.to_string()));
        }
        let result = runner
            .run(
                &[OsStr::new("delete"), OsStr::new("--help")],
                &self.root,
                GIT_TIMEOUT,
                256 * 1024,
            )
            .map_err(|error| Arc::<str>::from(error.to_string()))
            .and_then(|help| {
                if !help.status.success() || help.stdout_truncated || help.stderr_truncated {
                    return Err(Arc::from(
                        "installed Graphite CLI deletion contract is unsupported",
                    ));
                }
                let mut help_bytes = help.stdout;
                help_bytes.extend_from_slice(&help.stderr);
                let help_text = String::from_utf8_lossy(&help_bytes);
                [
                    "Delete a branch and its Graphite metadata (local-only)",
                    "--no-interactive",
                    "--force",
                    "--upstack",
                    "--downstack",
                    "--close",
                ]
                .into_iter()
                .all(|required| help_text.contains(required))
                .then_some(())
                .ok_or_else(|| Arc::from("installed Graphite CLI deletion contract is unsupported"))
            });
        *cached = Some(result.clone());
        result.map_err(|error| anyhow!(error.to_string()))
    }

    fn delete_tracked(
        &self,
        request: &DeleteRequest,
        runner: &impl DeleteCommandRunner,
        post_state: &impl DeletePostStateReader,
    ) -> Result<DeleteOutcome> {
        let invocation = runner.run(
            &[
                OsStr::new("--cwd"),
                self.root.as_os_str(),
                OsStr::new("--no-interactive"),
                OsStr::new("delete"),
                OsStr::new(request.branch.0.as_ref()),
            ],
            &self.root,
            GIT_TIMEOUT,
            OUTPUT_LIMIT,
        );

        // Once the provider invocation starts, its exit status is not the
        // authority. Re-read both state stores even after timeout/truncation.
        let current_ref = match post_state.exact_local_ref(self, &request.branch) {
            Ok(current_ref) => current_ref,
            Err(_) => return Ok(DeleteOutcome::Inconsistent),
        };
        let metadata_present = match post_state.metadata_present(&self.common_dir, &request.branch)
        {
            Ok(metadata_present) => metadata_present,
            Err(_) => return Ok(DeleteOutcome::Inconsistent),
        };
        let command_succeeded = invocation.as_ref().is_ok_and(|output| {
            output.status.success() && !output.stdout_truncated && !output.stderr_truncated
        });
        Ok(
            match (current_ref.as_deref(), metadata_present, command_succeeded) {
                (None, false, _) => DeleteOutcome::Deleted,
                (Some(oid), true, false) if oid == request.expected_oid.as_ref() => {
                    DeleteOutcome::Unchanged
                }
                _ => DeleteOutcome::Inconsistent,
            },
        )
    }

    fn exact_local_ref(&self, branch: &BranchId) -> Result<Option<Arc<str>>> {
        let reference = format!("refs/heads/{branch}");
        let output = self.git_os(
            &[
                OsStr::new("rev-parse"),
                OsStr::new("--verify"),
                OsStr::new("--quiet"),
                OsStr::new("--end-of-options"),
                OsStr::new(&reference),
            ],
            4096,
        )?;
        if output.stdout_truncated {
            bail!("could not verify deletion ref: Git output exceeded its limit");
        }
        if output.status.success() {
            let oid = text(
                strip_line_ending(&output.stdout),
                "deleted branch object id",
            )?;
            if oid.is_empty() {
                bail!("could not verify deletion ref: Git returned an empty object id");
            }
            return Ok(Some(Arc::from(oid)));
        }
        if output.status.code() == Some(1) {
            return Ok(None);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("could not verify deletion ref: {}", stderr.trim())
    }

    pub fn remote_identity(&self) -> Option<String> {
        let output = self.git(&["remote", "get-url", "origin"], 16 * 1024).ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    fn git(&self, args: &[&str], limit: usize) -> Result<CommandOutput> {
        self.git_os(&args.iter().map(OsStr::new).collect::<Vec<_>>(), limit)
    }

    fn git_os(&self, args: &[&OsStr], limit: usize) -> Result<CommandOutput> {
        run_bounded(OsStr::new("git"), args, &self.start_dir, GIT_TIMEOUT, limit)
            .map_err(|error| anyhow!(error))
    }
}

fn ensure_success(output: &CommandOutput, action: &str) -> Result<()> {
    if output.stdout_truncated {
        bail!("could not {action}: Git output exceeded the bounded output limit");
    }
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!("could not {action}: {}", stderr.trim())
}

fn discover_paths(start_dir: &Path) -> Result<(PathBuf, PathBuf, PathBuf)> {
    let run = |args: &[&str], action| -> Result<CommandOutput> {
        let output = run_bounded(OsStr::new("git"), args, start_dir, GIT_TIMEOUT, 64 * 1024)
            .map_err(|error| anyhow!(error))?;
        ensure_success(&output, action)?;
        Ok(output)
    };
    let root_output = run(
        &["rev-parse", "--show-toplevel"],
        "discover repository root",
    )?;
    let root = PathBuf::from(String::from_utf8(
        strip_line_ending(&root_output.stdout).to_vec(),
    )?);
    let git_output = run(
        &["rev-parse", "--absolute-git-dir"],
        "discover Git directory",
    )?;
    let git_dir = PathBuf::from(String::from_utf8(
        strip_line_ending(&git_output.stdout).to_vec(),
    )?);
    let common_output = run(
        &["rev-parse", "--git-common-dir"],
        "discover Git common directory",
    )?;
    let mut common_dir = PathBuf::from(String::from_utf8(
        strip_line_ending(&common_output.stdout).to_vec(),
    )?);
    if common_dir.is_relative() {
        common_dir = root.join(common_dir);
    }
    Ok((root, git_dir, common_dir))
}

fn strip_line_ending(mut bytes: &[u8]) -> &[u8] {
    if bytes.ends_with(b"\n") {
        bytes = &bytes[..bytes.len() - 1];
        if bytes.ends_with(b"\r") {
            bytes = &bytes[..bytes.len() - 1];
        }
    }
    bytes
}

fn strip_record_separator(bytes: &[u8]) -> &[u8] {
    bytes
        .strip_prefix(b"\n")
        .or_else(|| bytes.strip_prefix(b"\r\n"))
        .unwrap_or(bytes)
}

fn text<'a>(bytes: &'a [u8], label: &str) -> Result<&'a str> {
    std::str::from_utf8(bytes).with_context(|| format!("invalid UTF-8 in {label}"))
}

fn parse_u64(bytes: &[u8]) -> Result<u64> {
    Ok(text(bytes, "diffstat")?.parse()?)
}

fn operation_state(git_dir: &Path) -> Option<&'static str> {
    [
        ("rebase-merge", "rebase"),
        ("rebase-apply", "rebase"),
        ("MERGE_HEAD", "merge"),
        ("CHERRY_PICK_HEAD", "cherry-pick"),
        ("REVERT_HEAD", "revert"),
        ("BISECT_LOG", "bisect"),
    ]
    .into_iter()
    .find_map(|(path, label)| git_dir.join(path).exists().then_some(label))
}

fn repository_id(path: &Path) -> Arc<str> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    Arc::from(format!("{:016x}", hasher.finish()))
}

fn token_for(branches: &[GitBranch], current: Option<&BranchId>, state: &RepositoryState) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for branch in branches {
        branch.id.hash(&mut hasher);
        branch.oid.hash(&mut hasher);
        branch.worktree.hash(&mut hasher);
    }
    current.hash(&mut hasher);
    std::mem::discriminant(state).hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Command, ExitStatus};

    use rusqlite::Connection;

    use super::*;

    #[derive(Clone, Copy)]
    enum Effect {
        KeepNonZero,
        KeepTimeout,
        KeepTruncated,
        DeleteBothNonZero,
        DeleteBothRemoveDatabase,
        DeleteRefOnlySuccess,
        DeleteDuringHelp,
    }

    struct FakeDeleteRunner {
        root: PathBuf,
        common_dir: PathBuf,
        effect: Effect,
        calls: Mutex<Vec<Vec<String>>>,
    }

    impl FakeDeleteRunner {
        fn new(adapter: &GitAdapter, effect: Effect) -> Self {
            Self {
                root: adapter.root.clone(),
                common_dir: adapter.common_dir.clone(),
                effect,
                calls: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<Vec<String>> {
            self.calls.lock().unwrap().clone()
        }

        fn delete_ref(&self) {
            git(&self.root, &["update-ref", "-d", "refs/heads/feature"]);
        }

        fn delete_metadata(&self) {
            Connection::open(self.common_dir.join(".graphite_metadata.db"))
                .unwrap()
                .execute(
                    "DELETE FROM branch_metadata WHERE branch_name = 'feature'",
                    [],
                )
                .unwrap();
        }
    }

    impl DeleteCommandRunner for FakeDeleteRunner {
        fn run(
            &self,
            args: &[&OsStr],
            _cwd: &Path,
            _timeout: Duration,
            _output_limit: usize,
        ) -> std::result::Result<CommandOutput, CommandError> {
            let args: Vec<_> = args
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();
            self.calls.lock().unwrap().push(args.clone());
            if args == ["delete", "--help"] {
                if matches!(self.effect, Effect::DeleteDuringHelp) {
                    self.delete_ref();
                }
                return Ok(output(
                    true,
                    b"Delete a branch and its Graphite metadata (local-only) --no-interactive --force --upstack --downstack --close",
                    false,
                ));
            }
            match self.effect {
                Effect::KeepNonZero => Ok(output(false, b"", false)),
                Effect::KeepTimeout => Err(CommandError::Timeout(Duration::from_secs(3))),
                Effect::KeepTruncated => Ok(output(true, b"partial", true)),
                Effect::DeleteBothNonZero => {
                    self.delete_ref();
                    self.delete_metadata();
                    Ok(output(false, b"", false))
                }
                Effect::DeleteBothRemoveDatabase => {
                    self.delete_ref();
                    fs::remove_file(self.common_dir.join(".graphite_metadata.db")).unwrap();
                    Ok(output(true, b"", false))
                }
                Effect::DeleteRefOnlySuccess => {
                    self.delete_ref();
                    Ok(output(true, b"", false))
                }
                Effect::DeleteDuringHelp => panic!("preflight should stop provider invocation"),
            }
        }
    }

    struct FailingPostState {
        ref_error: bool,
        metadata_error: bool,
    }

    impl DeletePostStateReader for FailingPostState {
        fn exact_local_ref(
            &self,
            adapter: &GitAdapter,
            branch: &BranchId,
        ) -> Result<Option<Arc<str>>> {
            if self.ref_error {
                bail!("simulated local ref read failure");
            }
            adapter.exact_local_ref(branch)
        }

        fn metadata_present(&self, common_dir: &Path, branch: &BranchId) -> Result<bool> {
            if self.metadata_error {
                bail!("simulated metadata read failure");
            }
            raw_branch_metadata_presence(common_dir, branch)
        }
    }

    fn output(success: bool, stdout: &[u8], truncated: bool) -> CommandOutput {
        CommandOutput {
            status: ExitStatus::from_raw(if success { 0 } else { 1 << 8 }),
            stdout: stdout.to_vec(),
            stderr: Vec::new(),
            stdout_truncated: truncated,
            stderr_truncated: false,
        }
    }

    fn git(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn tracked_repository() -> (tempfile::TempDir, GitAdapter, DeleteRequest) {
        let directory = tempfile::tempdir().unwrap();
        git(directory.path(), &["init", "-b", "main"]);
        git(directory.path(), &["config", "user.name", "Stackmap Tests"]);
        git(
            directory.path(),
            &["config", "user.email", "stackmap@example.invalid"],
        );
        fs::write(directory.path().join("file.txt"), "base\n").unwrap();
        git(directory.path(), &["add", "file.txt"]);
        git(directory.path(), &["commit", "-m", "base"]);
        git(directory.path(), &["branch", "feature"]);
        let common_dir = directory.path().join(".git");
        fs::write(
            common_dir.join(".graphite_repo_config"),
            r#"{"trunk":"main","trunks":[{"name":"main"}]}"#,
        )
        .unwrap();
        let connection = Connection::open(common_dir.join(".graphite_metadata.db")).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE branch_metadata (branch_name TEXT PRIMARY KEY, parent_branch_name TEXT);\
                 INSERT INTO branch_metadata VALUES ('main', NULL);\
                 INSERT INTO branch_metadata VALUES ('feature', 'main');",
            )
            .unwrap();
        drop(connection);
        let adapter = GitAdapter::discover(directory.path()).unwrap();
        let oid = git(directory.path(), &["rev-parse", "refs/heads/feature"]);
        let request = DeleteRequest {
            branch: BranchId::new("feature"),
            expected_oid: Arc::from(oid),
            expected_provenance: GraphiteProvenance::Tracked,
        };
        (directory, adapter, request)
    }

    #[test]
    fn tracked_delete_contract_is_cached_and_argv_is_exact() {
        let (_directory, adapter, request) = tracked_repository();
        let runner = FakeDeleteRunner::new(&adapter, Effect::KeepNonZero);
        assert_eq!(
            adapter.delete_branch_with(&request, &runner).unwrap(),
            DeleteOutcome::Unchanged
        );
        assert_eq!(
            adapter.delete_branch_with(&request, &runner).unwrap(),
            DeleteOutcome::Unchanged
        );
        let root = adapter.root.to_string_lossy().into_owned();
        assert_eq!(
            runner.calls(),
            [
                vec!["delete".into(), "--help".into()],
                vec![
                    "--cwd".into(),
                    root.clone(),
                    "--no-interactive".into(),
                    "delete".into(),
                    "feature".into(),
                ],
                vec![
                    "--cwd".into(),
                    root,
                    "--no-interactive".into(),
                    "delete".into(),
                    "feature".into(),
                ],
            ]
        );
    }

    #[test]
    fn contract_check_precedes_the_final_live_preflight() {
        let (_directory, adapter, request) = tracked_repository();
        let runner = FakeDeleteRunner::new(&adapter, Effect::DeleteDuringHelp);
        assert!(adapter.delete_branch_with(&request, &runner).is_err());
        assert_eq!(
            runner.calls(),
            [vec![String::from("delete"), String::from("--help")]]
        );
    }

    #[test]
    fn tracked_delete_classifies_both_postconditions_after_provider_failures() {
        for (effect, expected) in [
            (Effect::KeepNonZero, DeleteOutcome::Unchanged),
            (Effect::KeepTimeout, DeleteOutcome::Unchanged),
            (Effect::KeepTruncated, DeleteOutcome::Unchanged),
            (Effect::DeleteBothNonZero, DeleteOutcome::Deleted),
            (Effect::DeleteRefOnlySuccess, DeleteOutcome::Inconsistent),
        ] {
            let (_directory, adapter, request) = tracked_repository();
            let runner = FakeDeleteRunner::new(&adapter, effect);
            assert_eq!(
                adapter.delete_branch_with(&request, &runner).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn option_shaped_tracked_names_are_rejected_before_provider_calls() {
        let (_directory, adapter, request) = tracked_repository();
        let runner = FakeDeleteRunner::new(&adapter, Effect::KeepNonZero);
        let request = DeleteRequest {
            branch: BranchId::new("--force"),
            ..request
        };
        assert!(adapter.delete_branch_with(&request, &runner).is_err());
        assert!(runner.calls().is_empty());
    }

    #[test]
    fn option_shaped_untracked_names_use_the_exact_git_ref() {
        let (_directory, adapter, tracked_request) = tracked_repository();
        let oid = git(&adapter.root, &["rev-parse", "HEAD"]);
        git(
            &adapter.root,
            &["update-ref", "refs/heads/--force", oid.as_str()],
        );
        let runner = FakeDeleteRunner::new(&adapter, Effect::KeepNonZero);
        let request = DeleteRequest {
            branch: BranchId::new("--force"),
            expected_oid: Arc::from(oid),
            expected_provenance: GraphiteProvenance::DefinitelyUntracked,
        };
        assert_eq!(
            adapter.delete_branch_with(&request, &runner).unwrap(),
            DeleteOutcome::Deleted
        );
        assert!(runner.calls().is_empty());
        assert_ne!(request.branch, tracked_request.branch);
    }

    #[test]
    fn tracked_delete_returns_inconsistent_when_raw_metadata_cannot_be_read() {
        let (_directory, adapter, request) = tracked_repository();
        let runner = FakeDeleteRunner::new(&adapter, Effect::DeleteBothRemoveDatabase);
        assert_eq!(
            adapter.delete_branch_with(&request, &runner).unwrap(),
            DeleteOutcome::Inconsistent
        );
    }

    #[test]
    fn tracked_delete_returns_inconsistent_when_local_ref_cannot_be_read_after_partial_mutation() {
        let (_directory, adapter, request) = tracked_repository();
        let runner = FakeDeleteRunner::new(&adapter, Effect::DeleteRefOnlySuccess);
        let state = FailingPostState {
            ref_error: true,
            metadata_error: false,
        };
        assert_eq!(
            adapter
                .delete_branch_with_state(&request, &runner, &state)
                .unwrap(),
            DeleteOutcome::Inconsistent
        );
        assert_eq!(runner.calls().len(), 2);
    }

    #[test]
    fn tracked_delete_refuses_a_nonlocal_raw_metadata_child() {
        let (_directory, adapter, request) = tracked_repository();
        Connection::open(adapter.common_dir.join(".graphite_metadata.db"))
            .unwrap()
            .execute(
                "INSERT INTO branch_metadata VALUES ('stale-child', 'feature')",
                [],
            )
            .unwrap();
        let runner = FakeDeleteRunner::new(&adapter, Effect::KeepNonZero);
        assert!(adapter.delete_branch_with(&request, &runner).is_err());
        assert_eq!(
            runner.calls(),
            [vec![String::from("delete"), String::from("--help")]]
        );
    }
}
