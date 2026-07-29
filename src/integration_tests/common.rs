#![allow(dead_code)]

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use crate::model::{
    Branch, BranchId, ConfiguredUpstream, DiffState, GraphiteProvenance, RemoteRefEvidence,
    RepositorySnapshot, RepositoryState,
};
use tempfile::TempDir;

pub fn git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

pub fn init_repo() -> TempDir {
    let directory = tempfile::tempdir().expect("temp repository");
    git(directory.path(), &["init", "-b", "main"]);
    git(directory.path(), &["config", "user.name", "Stackmap Tests"]);
    git(
        directory.path(),
        &["config", "user.email", "stackmap@example.invalid"],
    );
    fs::write(directory.path().join("file.txt"), "base\n").unwrap();
    git(directory.path(), &["add", "file.txt"]);
    git(directory.path(), &["commit", "-m", "base"]);
    directory
}

pub fn branch(name: &str, parent: Option<&str>, root: &str, current: bool) -> Branch {
    Branch {
        id: BranchId::new(name.to_owned()),
        oid: Arc::from(format!("oid-{name}")),
        parent: parent.map(|value| BranchId::new(value.to_owned())),
        diff_parent: parent.map(|value| BranchId::new(value.to_owned())),
        stack_root: BranchId::new(root.to_owned()),
        trunk: None,
        graphite: GraphiteProvenance::DefinitelyUntracked,
        committed_at: 1_700_000_000,
        current,
        dirty: false,
        worktree: None,
        configured_upstream: ConfiguredUpstream::None,
        remote_ref: RemoteRefEvidence::NotRequested,
        diff: DiffState::Loading,
        pr: None,
    }
}

pub fn snapshot(branches: Vec<Branch>) -> Arc<RepositorySnapshot> {
    let branch_index = RepositorySnapshot::index_branches(&branches);
    Arc::new(RepositorySnapshot {
        generation: 1,
        root: PathBuf::from("/repo"),
        git_dir: PathBuf::from("/repo/.git"),
        common_dir: PathBuf::from("/tmp/stackmap-tests-common"),
        repository_id: Arc::from("test-repository"),
        default_trunk: Some(BranchId::new("main")),
        configured_trunks: Arc::from([BranchId::new("main")]),
        trunks: Arc::from([BranchId::new("main")]),
        graphite_children: Arc::from([]),
        branches: Arc::from(branches),
        branch_index,
        stack_diffs: Arc::new(HashMap::new()),
        state: RepositoryState::Ready,
        graphite_status: Arc::from("fixture"),
        stale_error: None,
    })
}
