use std::ffi::{OsStr, OsString};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};

#[cfg(test)]
use super::command::CommandError;
use super::command::{CommandOutput, run_bounded, run_bounded_read_only_git};
#[cfg(test)]
use super::graphite::raw_branch_metadata_presence;
use super::graphite::{raw_branch_metadata_has_child, read_topology};
use crate::model::{BranchId, ConfiguredUpstream, DiffStat, GraphiteProvenance, RepositoryState};

const GIT_TIMEOUT: Duration = Duration::from_secs(3);
const GIT_MUTATION_TIMEOUT: Duration = Duration::from_secs(30);
const UPSTREAM_GIT_TIMEOUT: Duration = Duration::from_millis(250);
const OUTPUT_LIMIT: usize = 16 * 1024 * 1024;
const UPSTREAM_OUTPUT_LIMIT: usize = 256 * 1024;

pub type ExactRemoteRefTips = std::collections::HashMap<Arc<str>, Arc<str>>;

mod inventory;
mod mutation;

pub use inventory::{GitBranch, GitInventory};
use mutation::{
    DeleteCommandRunner, DeleteContractCache, DeletePostStateReader, GraphiteContractCache,
    SystemDeleteCommandRunner, SystemDeletePostStateReader,
};
pub use mutation::{
    DeleteOutcome, DeleteRequest, GraphiteBranchExpectation, GraphiteEdgeExpectation,
    GraphiteMutationOutcome, MoveRequest, RestackRequest,
};

#[derive(Clone, Debug)]
pub struct GitAdapter {
    start_dir: PathBuf,
    root: PathBuf,
    git_dir: PathBuf,
    common_dir: PathBuf,
    gt_delete_contract: DeleteContractCache,
    gt_restack_contract: GraphiteContractCache,
    gt_move_contract: GraphiteContractCache,
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
            gt_restack_contract: Arc::new(Mutex::new(None)),
            gt_move_contract: Arc::new(Mutex::new(None)),
        })
    }

    pub fn inventory(&self) -> Result<GitInventory> {
        self.inventory_inner()
    }

    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    pub fn common_dir(&self) -> &Path {
        &self.common_dir
    }

    fn mutation_inventory(&self) -> Result<GitInventory> {
        self.inventory_inner()
    }

    fn inventory_inner(&self) -> Result<GitInventory> {
        let root = self.root.clone();
        let git_dir = self.git_dir.clone();
        let common_dir = self.common_dir.clone();
        let (configured_upstreams, output, current_output, head_exists, dirty_output) =
            thread::scope(|scope| {
                let configured = scope.spawn(|| self.configured_upstream_branches());
                let branches = scope.spawn(|| {
                    self.git(
                        &[
                            "for-each-ref",
                            "--sort=refname",
                            "--format=%(refname)%00%(objectname)%00%(committerdate:unix)%00%(worktreepath)%00%(upstream)%00%(upstream:track,nobracket)%00",
                            "refs/heads",
                        ],
                        OUTPUT_LIMIT,
                    )
                });
                let current =
                    scope.spawn(|| self.git(&["symbolic-ref", "--quiet", "--short", "HEAD"], 4096));
                let head = scope.spawn(|| {
                    self.git(&["rev-parse", "--verify", "HEAD"], 4096)
                        .map(|output| output.status.success())
                });
                let dirty = scope.spawn(|| {
                    self.git(
                        &["status", "--porcelain=v2", "-z", "--untracked-files=normal"],
                        OUTPUT_LIMIT,
                    )
                });
                Ok::<_, anyhow::Error>((
                    configured
                        .join()
                        .map_err(|_| anyhow!("configured-upstream scan worker panicked"))?,
                    branches
                        .join()
                        .map_err(|_| anyhow!("branch inventory worker panicked"))??,
                    current
                        .join()
                        .map_err(|_| anyhow!("current-branch worker panicked"))??,
                    head.join()
                        .map_err(|_| anyhow!("HEAD verification worker panicked"))??,
                    dirty
                        .join()
                        .map_err(|_| anyhow!("worktree status worker panicked"))??,
                ))
            })?;
        ensure_success(&output, "enumerate local branches")?;
        let fields: Vec<&[u8]> = output.stdout.split(|byte| *byte == 0).collect();
        let mut branches = Vec::with_capacity(fields.len() / 6);
        for chunk in fields.chunks(6) {
            if chunk.len() < 6 || chunk[0].is_empty() {
                continue;
            }
            let reference = text(strip_record_separator(chunk[0]), "ref name")?;
            let oid = text(chunk[1], "object id")?;
            let Some(name) = reference.strip_prefix("refs/heads/") else {
                continue;
            };
            let committed_at = text(chunk[2], "committer timestamp")?
                .parse::<i64>()
                .with_context(|| format!("invalid committer timestamp for {name}"))?;
            let worktree_text = text(chunk[3], "worktree path")?;
            let upstream = text(chunk[4], "configured upstream")?;
            let tracking = text(chunk[5], "configured upstream tracking")?;
            branches.push(GitBranch {
                id: BranchId::new(name),
                oid: Arc::from(oid),
                committed_at,
                worktree: (!worktree_text.is_empty()).then(|| PathBuf::from(worktree_text)),
                configured_upstream: parse_configured_upstream(
                    upstream,
                    tracking,
                    configured_upstreams
                        .as_ref()
                        .map(|configured| configured.contains(name)),
                ),
            });
        }
        if current_output.stdout_truncated {
            bail!("could not read current branch: Git output exceeded the bounded output limit");
        }
        let current = current_output.status.success().then(|| {
            BranchId::new(
                String::from_utf8_lossy(strip_line_ending(&current_output.stdout)).to_string(),
            )
        });
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

    pub fn exact_remote_ref_tips(&self) -> Result<(u64, ExactRemoteRefTips)> {
        let output = self.git_upstream(&[
            "for-each-ref",
            "--sort=refname",
            "--format=%(refname)%00%(objectname)%00",
            "refs/remotes",
        ])?;
        ensure_success(&output, "read local remote-tracking refs")?;
        let fields: Vec<&[u8]> = output.stdout.split(|byte| *byte == 0).collect();
        let mut refs = Vec::with_capacity(fields.len() / 2);
        let mut exact_tips = std::collections::HashMap::with_capacity(fields.len() / 2);
        for chunk in fields.chunks(2) {
            if chunk.len() < 2 || chunk[0].is_empty() {
                continue;
            }
            let reference = text(strip_record_separator(chunk[0]), "remote ref name")?;
            let oid = text(chunk[1], "remote ref object id")?;
            refs.push((reference, oid));
            if !reference.ends_with("/HEAD") {
                exact_tips
                    .entry(Arc::from(oid))
                    .or_insert_with(|| Arc::from(reference));
            }
        }
        Ok((remote_ref_token(refs), exact_tips))
    }

    pub fn remote_ref_token(&self) -> Result<u64> {
        self.exact_remote_ref_tips().map(|(token, _)| token)
    }

    pub fn containing_remote_refs(
        &self,
        oids: &[Arc<str>],
    ) -> Result<std::collections::HashMap<Arc<str>, Arc<str>>> {
        if oids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        let mut args = vec![
            OsString::from("name-rev"),
            OsString::from("--name-only"),
            OsString::from("--refs=refs/remotes/*"),
            OsString::from("--always"),
        ];
        args.extend(oids.iter().map(|oid| OsString::from(oid.as_ref())));
        let output = run_bounded_read_only_git(
            args.iter(),
            &self.start_dir,
            GIT_TIMEOUT,
            UPSTREAM_OUTPUT_LIMIT,
        )
        .map_err(|error| anyhow!(error))?;
        ensure_success(&output, "check batched local remote-ref containment")?;
        let names = output
            .stdout
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>();
        if names.len() != oids.len() {
            bail!(
                "batched local remote-ref containment returned {} rows for {} object IDs",
                names.len(),
                oids.len()
            );
        }
        let mut containing = std::collections::HashMap::new();
        for (oid, name) in oids.iter().zip(names) {
            let name = text(name, "containing remote ref")?;
            if name == "undefined" {
                continue;
            }
            let name = name.strip_prefix("remotes/").unwrap_or(name);
            let end = name.find(['~', '^']).unwrap_or(name.len());
            containing.insert(oid.clone(), Arc::from(&name[..end]));
        }
        Ok(containing)
    }

    pub fn patch_distinct_counts(&self, branch: &BranchId, upstream: &str) -> Result<(u64, u64)> {
        let range = format!("refs/heads/{}...{upstream}", branch.0);
        let output = self.git(
            &[
                "rev-list",
                "--left-right",
                "--cherry-pick",
                "--count",
                "--end-of-options",
                &range,
            ],
            4096,
        )?;
        ensure_success(&output, "classify patch-distinct upstream history")?;
        let counts = text(strip_line_ending(&output.stdout), "patch-distinct counts")?;
        let mut counts = counts.split_whitespace();
        let local = counts
            .next()
            .context("patch-distinct comparison omitted the local count")?
            .parse::<u64>()
            .context("patch-distinct local count was invalid")?;
        let remote = counts
            .next()
            .context("patch-distinct comparison omitted the remote count")?
            .parse::<u64>()
            .context("patch-distinct remote count was invalid")?;
        if counts.next().is_some() {
            bail!("patch-distinct comparison returned unexpected extra fields");
        }
        Ok((local, remote))
    }

    pub fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool> {
        let output = self.git_upstream(&["merge-base", "--is-ancestor", ancestor, descendant])?;
        if output.stdout_truncated || output.stderr_truncated {
            bail!("Git ancestry output exceeded the bounded output limit");
        }
        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => bail!(
                "Git ancestry check failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        }
    }

    fn configured_upstream_branches(&self) -> Option<std::collections::HashSet<String>> {
        let output = self
            .git(
                &[
                    "config",
                    "--null",
                    "--name-only",
                    "--get-regexp",
                    "^branch\\..*\\.(remote|merge)$",
                ],
                256 * 1024,
            )
            .ok()?;
        if output.stdout_truncated || (!output.status.success() && output.status.code() != Some(1))
        {
            return None;
        }
        let mut branches = std::collections::HashSet::new();
        for key in output.stdout.split(|byte| *byte == 0) {
            let key = std::str::from_utf8(key).ok()?;
            let Some(key) = key.strip_prefix("branch.") else {
                continue;
            };
            if let Some(branch) = key
                .strip_suffix(".remote")
                .or_else(|| key.strip_suffix(".merge"))
            {
                branches.insert(branch.to_owned());
            }
        }
        Some(branches)
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
        let live = self.mutation_inventory()?;
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
            return self.transfer_linked_worktree(branch, path, &live);
        }
        let output = self.git_mutation(
            &[
                OsStr::new("switch"),
                OsStr::new("--"),
                OsStr::new(branch.0.as_ref()),
            ],
            OUTPUT_LIMIT,
        )?;
        ensure_success(&output, "switch branch")
    }

    fn transfer_linked_worktree(
        &self,
        branch: &BranchId,
        linked_path: &Path,
        primary: &GitInventory,
    ) -> Result<()> {
        if primary.dirty {
            bail!("moving a linked worktree requires a clean primary checkout");
        }
        let linked_adapter = GitAdapter::discover(linked_path)?;
        let linked = linked_adapter.inventory()?;
        if linked.current.as_ref() != Some(branch) {
            bail!(
                "linked worktree ownership changed before moving {branch} to the primary checkout"
            );
        }
        if !matches!(linked.state, RepositoryState::Ready) {
            bail!(
                "linked worktree is unsafe while repository state is {:?}",
                linked.state
            );
        }
        if linked.dirty {
            bail!(
                "linked worktree has uncommitted or untracked changes; preserving {}",
                linked_path.display()
            );
        }
        if linked_adapter.has_ignored_worktree_files()? {
            bail!(
                "linked worktree contains ignored files; preserving {}",
                linked_path.display()
            );
        }

        let remove = self.git_mutation(
            &[
                OsStr::new("worktree"),
                OsStr::new("remove"),
                OsStr::new("--"),
                linked_path.as_os_str(),
            ],
            OUTPUT_LIMIT,
        )?;
        ensure_success(&remove, "remove clean linked worktree")?;

        let switch = self.git_mutation(
            &[
                OsStr::new("switch"),
                OsStr::new("--"),
                OsStr::new(branch.0.as_ref()),
            ],
            OUTPUT_LIMIT,
        )?;
        if let Err(switch_error) = ensure_success(&switch, "switch branch") {
            let restore = self.git_mutation(
                &[
                    OsStr::new("worktree"),
                    OsStr::new("add"),
                    OsStr::new("--"),
                    linked_path.as_os_str(),
                    OsStr::new(branch.0.as_ref()),
                ],
                OUTPUT_LIMIT,
            );
            return match restore.and_then(|output| {
                ensure_success(&output, "restore linked worktree after failed primary switch")
            }) {
                Ok(()) => Err(switch_error.context(
                    "primary switch failed; the original linked worktree was restored",
                )),
                Err(restore_error) => Err(switch_error.context(format!(
                    "primary switch failed and the linked worktree could not be restored: {restore_error}"
                ))),
            };
        }
        Ok(())
    }

    fn has_ignored_worktree_files(&self) -> Result<bool> {
        let output = self.git(
            &[
                "status",
                "--porcelain=v1",
                "--ignored=matching",
                "--untracked-files=all",
            ],
            OUTPUT_LIMIT,
        )?;
        ensure_success(&output, "check linked worktree ignored files")?;
        Ok(output
            .stdout
            .split(|byte| *byte == b'\n')
            .any(|line| line.starts_with(b"!! ")))
    }

    pub fn delete_branch(&self, request: &DeleteRequest) -> Result<DeleteOutcome> {
        self.delete_branch_with(request, &SystemDeleteCommandRunner)
    }

    pub fn graphite_restack(&self, request: &RestackRequest) -> Result<GraphiteMutationOutcome> {
        let runner = SystemDeleteCommandRunner;
        self.ensure_gt_action_contract(
            &runner,
            &self.gt_restack_contract,
            "restack",
            &["Ensure each branch", "--branch", "--upstack", "--only"],
        )?;
        let before = self.graphite_action_preflight(
            &request.repository_id,
            &request.source,
            None,
            &request.affected,
            &request.topology,
        )?;
        if before.parents.get(&request.source.branch) != Some(&request.expected_parent) {
            bail!("Graphite parent changed after restack confirmation");
        }
        let invocation = runner.run(
            &[
                OsStr::new("--cwd"),
                self.root.as_os_str(),
                OsStr::new("--no-interactive"),
                OsStr::new("restack"),
                OsStr::new("--branch"),
                OsStr::new(request.source.branch.0.as_ref()),
                OsStr::new("--upstack"),
            ],
            &self.root,
            GIT_MUTATION_TIMEOUT,
            OUTPUT_LIMIT,
        );
        self.classify_restack(request, invocation)
    }

    pub fn graphite_move(&self, request: &MoveRequest) -> Result<GraphiteMutationOutcome> {
        let runner = SystemDeleteCommandRunner;
        self.ensure_gt_action_contract(
            &runner,
            &self.gt_move_contract,
            "move",
            &["Rebase the current branch", "--onto", "--source", "--only"],
        )?;
        let before = self.graphite_action_preflight(
            &request.repository_id,
            &request.source,
            Some(&request.target),
            &request.affected,
            &request.topology,
        )?;
        if before.parents.get(&request.source.branch) != request.expected_parent.as_ref() {
            bail!("Graphite parent changed after move confirmation");
        }
        if request.source.branch == request.target.branch
            || request
                .affected
                .iter()
                .any(|affected| affected.branch == request.target.branch)
        {
            bail!("move target would create a Graphite cycle");
        }
        let mut args = vec![
            OsStr::new("--cwd"),
            self.root.as_os_str(),
            OsStr::new("--no-interactive"),
            OsStr::new("move"),
            OsStr::new("--source"),
            OsStr::new(request.source.branch.0.as_ref()),
            OsStr::new("--onto"),
            OsStr::new(request.target.branch.0.as_ref()),
        ];
        if request.only {
            args.push(OsStr::new("--only"));
        }
        let invocation = runner.run(&args, &self.root, GIT_MUTATION_TIMEOUT, OUTPUT_LIMIT);
        self.classify_move(request, invocation)
    }

    fn graphite_action_preflight(
        &self,
        repository_id: &Arc<str>,
        source: &GraphiteBranchExpectation,
        target: Option<&GraphiteBranchExpectation>,
        affected: &[GraphiteBranchExpectation],
        topology: &[GraphiteEdgeExpectation],
    ) -> Result<super::graphite::GraphiteTopology> {
        if source.branch.0.starts_with('-')
            || target.is_some_and(|target| target.branch.0.starts_with('-'))
        {
            bail!("cannot safely mutate option-shaped branch names");
        }
        let live = self.mutation_inventory()?;
        if &live.repository_id != repository_id {
            bail!("repository identity changed after confirmation");
        }
        if !matches!(live.state, RepositoryState::Ready) {
            bail!(
                "Graphite action disabled while repository is {:?}",
                live.state
            );
        }
        if live.dirty {
            bail!("Graphite action requires a clean startup worktree");
        }
        let local: std::collections::HashSet<_> = live
            .branches
            .iter()
            .map(|branch| branch.id.clone())
            .collect();
        let graphite = read_topology(&live.common_dir, &local);
        let verify = |expectation: &GraphiteBranchExpectation, rewritten: bool| -> Result<()> {
            let branch = live
                .branches
                .iter()
                .find(|candidate| candidate.id == expectation.branch)
                .ok_or_else(|| anyhow!("branch {} no longer exists", expectation.branch))?;
            if branch.oid != expectation.oid {
                bail!("branch {} changed after confirmation", expectation.branch);
            }
            if rewritten
                && branch
                    .worktree
                    .as_ref()
                    .is_some_and(|path| path != &live.root)
            {
                bail!(
                    "branch {} is checked out in another worktree",
                    expectation.branch
                );
            }
            Ok(())
        };
        verify(source, true)?;
        if let Some(target) = target {
            verify(target, false)?;
        }
        for branch in affected {
            verify(branch, true)?;
        }
        for edge in topology {
            if graphite.parents.get(&edge.branch) != edge.parent.as_ref() {
                bail!("Graphite topology changed after confirmation");
            }
        }
        if graphite.configured_trunks.contains(&source.branch) {
            bail!("configured trunks cannot be moved or restacked");
        }
        if graphite.provenance(&source.branch) != GraphiteProvenance::Tracked {
            bail!("Graphite action requires a tracked source branch");
        }
        if let Some(target) = target {
            if graphite.configured_trunks.contains(&target.branch)
                || graphite.provenance(&target.branch) != GraphiteProvenance::Tracked
            {
                bail!("move target must be a characterized tracked non-trunk branch");
            }
            let source_trunk = graphite
                .trunk_by_branch
                .get(&source.branch)
                .ok_or_else(|| anyhow!("source branch has no characterized Graphite trunk"))?;
            if graphite.trunk_by_branch.get(&target.branch) != Some(source_trunk) {
                bail!("cross-trunk Graphite moves are not characterized");
            }
        }
        Ok(graphite)
    }

    fn ensure_gt_action_contract(
        &self,
        runner: &impl DeleteCommandRunner,
        cache: &GraphiteContractCache,
        command: &str,
        required: &[&str],
    ) -> Result<()> {
        let mut cached = cache
            .lock()
            .map_err(|_| anyhow!("Graphite action contract cache is poisoned"))?;
        if let Some(result) = cached.as_ref() {
            return result.clone().map_err(|error| anyhow!(error.to_string()));
        }
        let version_output = runner
            .run(&[OsStr::new("--version")], &self.root, GIT_TIMEOUT, 4096)
            .map_err(|error| anyhow!(error.to_string()))?;
        let version = String::from_utf8_lossy(&version_output.stdout)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let version_supported = version_output.status.success()
            && !version_output.stdout_truncated
            && !version_output.stderr_truncated
            && characterized_gt_version(&version);
        let result = if version_supported {
            let output = runner
                .run(
                    &[OsStr::new(command), OsStr::new("--help")],
                    &self.root,
                    GIT_TIMEOUT,
                    256 * 1024,
                )
                .map_err(|error| anyhow!(error.to_string()))?;
            if !output.status.success() || output.stdout_truncated || output.stderr_truncated {
                Err(Arc::from("installed Graphite CLI contract is unsupported"))
            } else {
                let mut bytes = output.stdout;
                bytes.extend_from_slice(&output.stderr);
                let text = String::from_utf8_lossy(&bytes)
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                graphite_help_contract_matches(&text, required)
                    .then_some(())
                    .ok_or_else(|| Arc::from("installed Graphite CLI contract is unsupported"))
            }
        } else {
            Err(Arc::from("Graphite actions require characterized gt 1.8.6"))
        };
        *cached = Some(result.clone());
        result.map_err(|error| anyhow!(error.to_string()))
    }

    fn classify_restack(
        &self,
        request: &RestackRequest,
        invocation: std::result::Result<CommandOutput, super::command::CommandError>,
    ) -> Result<GraphiteMutationOutcome> {
        let live = match self.mutation_inventory() {
            Ok(live) => live,
            Err(_) => return Ok(GraphiteMutationOutcome::Inconsistent),
        };
        if !matches!(live.state, RepositoryState::Ready) {
            return Ok(GraphiteMutationOutcome::Inconsistent);
        }
        let local: std::collections::HashSet<_> = live
            .branches
            .iter()
            .map(|branch| branch.id.clone())
            .collect();
        let graphite = read_topology(&live.common_dir, &local);
        let topology_ok = request.topology.iter().all(|edge| {
            graphite.parents.get(&edge.branch) == edge.parent.as_ref()
                && graphite.provenance(&edge.branch) == GraphiteProvenance::Tracked
        });
        let affected_present = request.affected.iter().all(|expected| {
            live.branches
                .iter()
                .any(|branch| branch.id == expected.branch)
        });
        let expected_affected: std::collections::HashSet<_> = request
            .affected
            .iter()
            .map(|expected| expected.branch.clone())
            .collect();
        let mut live_affected = std::collections::HashSet::from([request.source.branch.clone()]);
        let mut pending = vec![request.source.branch.clone()];
        while let Some(parent) = pending.pop() {
            for child in graphite
                .parents
                .iter()
                .filter(|(_, candidate_parent)| *candidate_parent == &parent)
                .map(|(child, _)| child.clone())
            {
                if live_affected.insert(child.clone()) {
                    pending.push(child);
                }
            }
        }
        let healthy = match self.validate_restack_poststate(&live, request) {
            Ok(ancestry_ok) => {
                topology_ok && affected_present && live_affected == expected_affected && ancestry_ok
            }
            Err(_) => return Ok(GraphiteMutationOutcome::Inconsistent),
        };
        if healthy {
            Ok(GraphiteMutationOutcome::Applied)
        } else if command_failed(&invocation)
            && self.graphite_state_is_unchanged(&live, &request.affected, &request.topology)
        {
            Ok(GraphiteMutationOutcome::Unchanged)
        } else {
            Ok(GraphiteMutationOutcome::Inconsistent)
        }
    }

    fn classify_move(
        &self,
        request: &MoveRequest,
        invocation: std::result::Result<CommandOutput, super::command::CommandError>,
    ) -> Result<GraphiteMutationOutcome> {
        let live = match self.mutation_inventory() {
            Ok(live) => live,
            Err(_) => return Ok(GraphiteMutationOutcome::Inconsistent),
        };
        if !matches!(live.state, RepositoryState::Ready) {
            return Ok(GraphiteMutationOutcome::Inconsistent);
        }
        let local: std::collections::HashSet<_> = live
            .branches
            .iter()
            .map(|branch| branch.id.clone())
            .collect();
        let graphite = read_topology(&live.common_dir, &local);
        let target_oid_ok = live
            .branches
            .iter()
            .any(|branch| branch.id == request.target.branch && branch.oid == request.target.oid);
        let topology_ok = target_oid_ok
            && graphite.parents.get(&request.source.branch) == Some(&request.target.branch)
            && request.topology.iter().all(|edge| {
                if edge.branch == request.source.branch {
                    return true;
                }
                let expected =
                    if request.only && edge.parent.as_ref() == Some(&request.source.branch) {
                        request.expected_parent.as_ref()
                    } else {
                        edge.parent.as_ref()
                    };
                graphite.parents.get(&edge.branch) == expected
            });
        let ancestry_ok = request.affected.iter().try_fold(true, |healthy, branch| {
            let Some(parent) = graphite.parents.get(&branch.branch) else {
                return Ok(healthy);
            };
            let Some(parent_oid) = live
                .branches
                .iter()
                .find(|item| &item.id == parent)
                .map(|b| &b.oid)
            else {
                return Ok(false);
            };
            let Some(child_oid) = live
                .branches
                .iter()
                .find(|item| item.id == branch.branch)
                .map(|b| &b.oid)
            else {
                return Ok(false);
            };
            self.is_ancestor(parent_oid, child_oid)
                .map(|is_ancestor| healthy && is_ancestor)
        });
        let ancestry_ok = match ancestry_ok {
            Ok(ancestry_ok) => ancestry_ok,
            Err(_) => return Ok(GraphiteMutationOutcome::Inconsistent),
        };
        if topology_ok && ancestry_ok {
            Ok(GraphiteMutationOutcome::Applied)
        } else if command_failed(&invocation)
            && target_oid_ok
            && self.graphite_state_is_unchanged(&live, &request.affected, &request.topology)
        {
            Ok(GraphiteMutationOutcome::Unchanged)
        } else {
            Ok(GraphiteMutationOutcome::Inconsistent)
        }
    }

    fn validate_restack_poststate(
        &self,
        live: &GitInventory,
        request: &RestackRequest,
    ) -> Result<bool> {
        request
            .affected
            .iter()
            .try_fold(true, |healthy, expectation| {
                let parent = request
                    .topology
                    .iter()
                    .find(|edge| edge.branch == expectation.branch)
                    .and_then(|edge| edge.parent.as_ref());
                let Some(parent) = parent else {
                    return Ok(healthy);
                };
                let Some(parent_oid) = live
                    .branches
                    .iter()
                    .find(|b| &b.id == parent)
                    .map(|b| &b.oid)
                else {
                    return Ok(false);
                };
                let Some(child_oid) = live
                    .branches
                    .iter()
                    .find(|b| b.id == expectation.branch)
                    .map(|b| &b.oid)
                else {
                    return Ok(false);
                };
                self.is_ancestor(parent_oid, child_oid)
                    .map(|is_ancestor| healthy && is_ancestor)
            })
    }

    fn graphite_state_is_unchanged(
        &self,
        live: &GitInventory,
        branches: &[GraphiteBranchExpectation],
        topology: &[GraphiteEdgeExpectation],
    ) -> bool {
        let local: std::collections::HashSet<_> = live
            .branches
            .iter()
            .map(|branch| branch.id.clone())
            .collect();
        let graphite = read_topology(&live.common_dir, &local);
        branches.iter().all(|expected| {
            live.branches
                .iter()
                .any(|branch| branch.id == expected.branch && branch.oid == expected.oid)
        }) && topology
            .iter()
            .all(|edge| graphite.parents.get(&edge.branch) == edge.parent.as_ref())
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
        let output = self.git_mutation(
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
        let help = runner
            .run(
                &[OsStr::new("delete"), OsStr::new("--help")],
                &self.root,
                GIT_TIMEOUT,
                256 * 1024,
            )
            .map_err(|error| anyhow!(error.to_string()))?;
        let result = (|| {
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
        })();
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

    fn git_upstream(&self, args: &[&str]) -> Result<CommandOutput> {
        run_bounded_read_only_git(
            args.iter().map(OsStr::new),
            &self.start_dir,
            UPSTREAM_GIT_TIMEOUT,
            UPSTREAM_OUTPUT_LIMIT,
        )
        .map_err(|error| anyhow!(error))
    }

    fn git_os(&self, args: &[&OsStr], limit: usize) -> Result<CommandOutput> {
        run_bounded_read_only_git(args, &self.start_dir, GIT_TIMEOUT, limit)
            .map_err(|error| anyhow!(error))
    }

    fn git_mutation(&self, args: &[&OsStr], limit: usize) -> Result<CommandOutput> {
        run_bounded(
            OsStr::new("git"),
            args,
            &self.start_dir,
            GIT_MUTATION_TIMEOUT,
            limit,
        )
        .map_err(|error| anyhow!(error))
    }
}

fn characterized_gt_version(normalized: &str) -> bool {
    normalized == "1.8.6"
}

fn graphite_help_contract_matches(normalized: &str, required: &[&str]) -> bool {
    required.iter().all(|required| {
        let required = required.split_whitespace().collect::<Vec<_>>().join(" ");
        normalized.contains(&required)
    })
}

fn command_failed(
    invocation: &std::result::Result<CommandOutput, super::command::CommandError>,
) -> bool {
    invocation
        .as_ref()
        .map_or(true, |output| !output.status.success())
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
        let output = run_bounded_read_only_git(args, start_dir, GIT_TIMEOUT, 64 * 1024)
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
        branch.configured_upstream.hash(&mut hasher);
    }
    current.hash(&mut hasher);
    std::mem::discriminant(state).hash(&mut hasher);
    hasher.finish()
}

fn remote_ref_token<'a>(refs: impl IntoIterator<Item = (&'a str, &'a str)>) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for (reference, oid) in refs {
        reference.hash(&mut hasher);
        oid.hash(&mut hasher);
    }
    hasher.finish()
}

fn parse_configured_upstream(
    reference: &str,
    tracking: &str,
    configured: Option<bool>,
) -> ConfiguredUpstream {
    if reference.is_empty() {
        return if configured == Some(true) {
            ConfiguredUpstream::Unavailable {
                reference: None,
                reason: Arc::from("configured upstream does not resolve to a local ref"),
            }
        } else if configured.is_none() {
            ConfiguredUpstream::Unavailable {
                reference: None,
                reason: Arc::from("configured upstream scan unavailable"),
            }
        } else {
            ConfiguredUpstream::None
        };
    }
    let reference: Arc<str> = Arc::from(reference);
    let tracking = tracking.trim();
    if tracking.is_empty() {
        return ConfiguredUpstream::Equal { reference };
    }
    if tracking == "gone" {
        return ConfiguredUpstream::Gone { reference };
    }
    let mut ahead = None;
    let mut behind = None;
    for part in tracking.split(',').map(str::trim) {
        if let Some(value) = part.strip_prefix("ahead ") {
            ahead = value.parse().ok();
        } else if let Some(value) = part.strip_prefix("behind ") {
            behind = value.parse().ok();
        }
    }
    match (ahead, behind) {
        (Some(ahead), Some(behind)) => ConfiguredUpstream::Diverged {
            reference,
            ahead,
            behind,
        },
        (Some(ahead), None) => ConfiguredUpstream::Ahead { reference, ahead },
        (None, Some(behind)) => ConfiguredUpstream::Behind { reference, behind },
        _ => ConfiguredUpstream::Unavailable {
            reference: Some(reference),
            reason: Arc::from(format!("unrecognized tracking state: {tracking}")),
        },
    }
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

    #[derive(Clone, Copy)]
    enum ProbeKind {
        Action,
        Delete,
    }

    struct FlakyContractRunner {
        kind: ProbeKind,
        calls: Mutex<usize>,
    }

    impl FlakyContractRunner {
        fn new(kind: ProbeKind) -> Self {
            Self {
                kind,
                calls: Mutex::new(0),
            }
        }

        fn calls(&self) -> usize {
            *self.calls.lock().unwrap()
        }
    }

    impl DeleteCommandRunner for FlakyContractRunner {
        fn run(
            &self,
            args: &[&OsStr],
            _cwd: &Path,
            _timeout: Duration,
            _output_limit: usize,
        ) -> std::result::Result<CommandOutput, CommandError> {
            let mut calls = self.calls.lock().unwrap();
            *calls += 1;
            if *calls == 1 {
                return Err(CommandError::Timeout(Duration::from_secs(3)));
            }
            if args == [OsStr::new("--version")] {
                return Ok(output(true, b"1.8.6", false));
            }
            match self.kind {
                ProbeKind::Action => Ok(output(true, b"--no-interactive", false)),
                ProbeKind::Delete => Ok(output(
                    true,
                    b"Delete a branch and its Graphite metadata (local-only) --no-interactive --force --upstack --downstack --close",
                    false,
                )),
            }
        }
    }

    #[test]
    fn transient_action_contract_probe_errors_are_not_cached() {
        let (_directory, adapter, _) = tracked_repository();
        let runner = FlakyContractRunner::new(ProbeKind::Action);
        assert!(
            adapter
                .ensure_gt_action_contract(
                    &runner,
                    &adapter.gt_restack_contract,
                    "restack",
                    &["--no-interactive"],
                )
                .is_err()
        );
        assert!(
            adapter
                .ensure_gt_action_contract(
                    &runner,
                    &adapter.gt_restack_contract,
                    "restack",
                    &["--no-interactive"],
                )
                .is_ok()
        );
        assert_eq!(runner.calls(), 3);
    }

    #[test]
    fn transient_delete_contract_probe_errors_are_not_cached() {
        let (_directory, adapter, _) = tracked_repository();
        let runner = FlakyContractRunner::new(ProbeKind::Delete);
        assert!(adapter.ensure_gt_delete_contract(&runner).is_err());
        assert!(adapter.ensure_gt_delete_contract(&runner).is_ok());
        assert_eq!(runner.calls(), 2);
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
    fn exact_remote_ref_tips_maps_only_remote_branch_tips() {
        let (directory, adapter, _request) = tracked_repository();
        let tip = git(directory.path(), &["rev-parse", "refs/heads/feature"]);
        git(
            directory.path(),
            &["update-ref", "refs/remotes/origin/feature", &tip],
        );
        git(
            directory.path(),
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/feature",
            ],
        );
        git(
            directory.path(),
            &["update-ref", "refs/remotes/upstream/feature", &tip],
        );

        let (_token, exact_tips) = adapter.exact_remote_ref_tips().unwrap();

        assert_eq!(
            exact_tips.get(tip.as_str()).map(Arc::as_ref),
            Some("refs/remotes/origin/feature")
        );
        assert_eq!(exact_tips.len(), 1);
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

    #[test]
    fn graphite_mutations_require_the_exact_characterized_version() {
        assert!(characterized_gt_version("1.8.6"));
        assert!(!characterized_gt_version("gt 1.8.6"));
        assert!(!characterized_gt_version("1.8.7"));
    }

    #[test]
    fn graphite_help_contract_ignores_formatting_but_not_missing_semantics() {
        let help = "Ensure each branch\n  --branch VALUE   --upstack   --only";
        let normalized = help.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(graphite_help_contract_matches(
            &normalized,
            &["Ensure each branch", "--branch", "--upstack", "--only"]
        ));
        assert!(!graphite_help_contract_matches(
            &normalized,
            &["--branch", "--source"]
        ));
    }
}
