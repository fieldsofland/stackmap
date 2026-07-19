use std::sync::Arc;

use std::path::PathBuf;

use stackmap::model::{
    Branch, BranchId, DiffState, GraphiteProvenance, RepositorySnapshot, RepositoryState,
};

pub fn branches(count: usize, stack_size: usize) -> Vec<Branch> {
    let mut values = Vec::with_capacity(count);
    for index in 0..count {
        let root_index = index - index % stack_size;
        let id = BranchId::new(format!("feature-{index:04}"));
        let root = BranchId::new(format!("feature-{root_index:04}"));
        let parent =
            (index % stack_size != 0).then(|| BranchId::new(format!("feature-{:04}", index - 1)));
        values.push(Branch {
            id,
            oid: Arc::from(format!("{index:040x}")),
            parent: parent.clone(),
            diff_parent: parent,
            stack_root: root,
            trunk: None,
            graphite: GraphiteProvenance::DefinitelyUntracked,
            committed_at: 1_700_000_000 + index as i64,
            current: index == 0,
            dirty: false,
            worktree: None,
            diff: DiffState::Loading,
            pr: None,
        });
    }
    values
}

pub fn snapshot(count: usize, stack_size: usize) -> RepositorySnapshot {
    let branches = branches(count, stack_size);
    let branch_index = RepositorySnapshot::index_branches(&branches);
    RepositorySnapshot {
        generation: 1,
        root: PathBuf::from("/bench"),
        git_dir: PathBuf::from("/bench/.git"),
        common_dir: PathBuf::from("/bench/.git"),
        repository_id: Arc::from("bench"),
        default_trunk: None,
        configured_trunks: Arc::from([]),
        trunks: Arc::from([]),
        graphite_children: Arc::from([]),
        branches: Arc::from(branches),
        branch_index,
        state: RepositoryState::Ready,
        graphite_status: Arc::from("bench"),
        stale_error: None,
    }
}
