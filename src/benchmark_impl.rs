use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use crate::model::topology::{OrderMode, ProjectionOptions, TopologyIndex};
use crate::model::{
    Branch, BranchId, ConfiguredUpstream, DiffState, GraphiteProvenance, RemoteRefEvidence,
    RepositorySnapshot, RepositoryState,
};

fn branches(count: usize, stack_size: usize) -> Vec<Branch> {
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
            configured_upstream: ConfiguredUpstream::None,
            remote_ref: RemoteRefEvidence::NotRequested,
            diff: DiffState::Loading,
            pr: None,
        });
    }
    values
}

fn snapshot(count: usize, stack_size: usize) -> RepositorySnapshot {
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
        stack_diffs: Arc::new(HashMap::new()),
        state: RepositoryState::Ready,
        graphite_status: Arc::from("bench"),
        stale_error: None,
    }
}

pub(crate) fn run() {
    for (count, iterations) in [(500, 1_000), (5_000, 100)] {
        let snapshot = snapshot(count, 10);
        let started = Instant::now();
        for _ in 0..iterations {
            std::hint::black_box(TopologyIndex::build(&snapshot));
        }
        println!(
            "{iterations} structural indexes of {count} branches: {:?}",
            started.elapsed()
        );

        let index = TopologyIndex::build(&snapshot);
        for (label, options) in [
            ("recent", ProjectionOptions::default()),
            (
                "alphabetical",
                ProjectionOptions {
                    order: OrderMode::Alphabetical,
                    ..ProjectionOptions::default()
                },
            ),
            (
                "graphite",
                ProjectionOptions {
                    order: OrderMode::Graphite,
                    ..ProjectionOptions::default()
                },
            ),
            (
                "compact",
                ProjectionOptions {
                    separators: false,
                    ..ProjectionOptions::default()
                },
            ),
        ] {
            let started = Instant::now();
            for _ in 0..iterations {
                std::hint::black_box(index.project(&options));
            }
            println!(
                "{iterations} {label} projections of {count} branches: {:?}",
                started.elapsed()
            );
        }
    }

    let snapshot = snapshot(5_000, 5_000);
    let index = TopologyIndex::build(&snapshot);
    let options = ProjectionOptions::default();
    let iterations = 10;
    let started = Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(index.project(&options));
    }
    println!(
        "{iterations} recent projections of one 5000-branch deep stack: {:?}",
        started.elapsed()
    );
}
