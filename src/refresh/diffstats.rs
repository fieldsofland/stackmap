use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;

use crate::adapters::git::GitAdapter;
use crate::model::topology::TopologyIndex;
use crate::model::{BranchId, DiffStat, DiffState, RepositorySnapshot};

type Key = (Arc<str>, Arc<str>);

pub(super) struct DiffCache {
    capacity: usize,
    values: HashMap<Key, DiffStat>,
    order: VecDeque<Key>,
}

impl DiffCache {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            values: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn get(&self, key: &Key) -> Option<DiffStat> {
        self.values.get(key).copied()
    }

    fn insert(&mut self, key: Key, value: DiffStat) {
        if self.values.insert(key.clone(), value).is_none() {
            self.order.push_back(key);
        }
        while self.values.len() > self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.values.remove(&oldest);
            }
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.values.len()
    }
}

#[derive(Clone)]
struct Task {
    indexes: Vec<usize>,
    stack_ids: Vec<BranchId>,
    key: Key,
}

pub(super) fn enrich(
    snapshot: Arc<RepositorySnapshot>,
    git: &GitAdapter,
    cache: &mut DiffCache,
    worker_limit: usize,
    latest_generation: &Arc<AtomicU64>,
    publish_partial: &dyn Fn(Arc<RepositorySnapshot>),
) -> Option<Arc<RepositorySnapshot>> {
    let generation = snapshot.generation;
    let oid_by_id: HashMap<_, _> = snapshot
        .branches
        .iter()
        .map(|b| (b.id.clone(), b.oid.clone()))
        .collect();
    let mut branches = snapshot.branches.to_vec();
    let mut stack_diffs = (*snapshot.stack_diffs).clone();
    let mut tasks = Vec::new();
    let mut task_by_key = HashMap::new();
    for (index, branch) in branches.iter_mut().enumerate() {
        let Some(parent) = branch.diff_parent.as_ref() else {
            branch.diff = DiffState::Unavailable(Arc::from("no validated parent"));
            continue;
        };
        let Some(parent_oid) = oid_by_id.get(parent) else {
            branch.diff = DiffState::Unavailable(Arc::from("parent object unavailable"));
            continue;
        };
        let key = (parent_oid.clone(), branch.oid.clone());
        if let Some(value) = cache.get(&key) {
            branch.diff = DiffState::Ready(value);
        } else {
            push_branch_task(&mut tasks, &mut task_by_key, index, key);
        }
    }
    for endpoints in TopologyIndex::build(&snapshot).stack_diff_endpoints() {
        stack_diffs
            .entry(endpoints.stack_id.clone())
            .or_insert(DiffState::Loading);
        let Some(bottom) = snapshot.branch(&endpoints.bottom) else {
            stack_diffs.insert(
                endpoints.stack_id,
                DiffState::Unavailable(Arc::from("stack bottom unavailable")),
            );
            continue;
        };
        let Some(parent) = bottom.diff_parent.as_ref() else {
            stack_diffs.insert(
                endpoints.stack_id,
                DiffState::Unavailable(Arc::from("no validated stack parent")),
            );
            continue;
        };
        let (Some(parent_oid), Some(head_oid)) =
            (oid_by_id.get(parent), oid_by_id.get(&endpoints.head))
        else {
            stack_diffs.insert(
                endpoints.stack_id,
                DiffState::Unavailable(Arc::from("stack endpoint unavailable")),
            );
            continue;
        };
        let key = (parent_oid.clone(), head_oid.clone());
        if let Some(value) = cache.get(&key) {
            stack_diffs.insert(endpoints.stack_id, DiffState::Ready(value));
        } else {
            push_stack_task(&mut tasks, &mut task_by_key, endpoints.stack_id, key);
        }
    }
    if tasks.is_empty() {
        return (!is_obsolete(generation, latest_generation)).then(|| {
            Arc::new(RepositorySnapshot {
                branches: Arc::from(branches),
                stack_diffs: Arc::new(stack_diffs),
                ..(*snapshot).clone()
            })
        });
    }

    let aggregate_start = tasks.partition_point(|task| !task.indexes.is_empty());
    let aggregate_tasks = tasks.split_off(aggregate_start);
    let branch_error = execute_tasks(
        tasks,
        generation,
        git,
        cache,
        worker_limit,
        latest_generation,
        &mut branches,
        &mut stack_diffs,
    );
    if is_obsolete(generation, latest_generation) {
        return None;
    }
    let branch_unavailable =
        branch_error.unwrap_or_else(|| Arc::from("diff worker stopped unexpectedly"));
    for branch in &mut branches {
        if matches!(branch.diff, DiffState::Loading) {
            branch.diff = DiffState::Unavailable(branch_unavailable.clone());
        }
    }
    if !aggregate_tasks.is_empty() {
        publish_partial(Arc::new(RepositorySnapshot {
            branches: Arc::from(branches.clone()),
            stack_diffs: Arc::new(stack_diffs.clone()),
            ..(*snapshot).clone()
        }));
    }

    let aggregate_error = execute_tasks(
        aggregate_tasks,
        generation,
        git,
        cache,
        worker_limit,
        latest_generation,
        &mut branches,
        &mut stack_diffs,
    );
    if is_obsolete(generation, latest_generation) {
        return None;
    }
    let aggregate_unavailable =
        aggregate_error.unwrap_or_else(|| Arc::from("diff worker stopped unexpectedly"));
    for state in stack_diffs.values_mut() {
        if matches!(state, DiffState::Loading) {
            *state = DiffState::Unavailable(aggregate_unavailable.clone());
        }
    }
    Some(Arc::new(RepositorySnapshot {
        branches: Arc::from(branches),
        stack_diffs: Arc::new(stack_diffs),
        ..(*snapshot).clone()
    }))
}

#[allow(clippy::too_many_arguments)]
fn execute_tasks(
    tasks: Vec<Task>,
    generation: u64,
    git: &GitAdapter,
    cache: &mut DiffCache,
    worker_limit: usize,
    latest_generation: &Arc<AtomicU64>,
    branches: &mut [crate::model::Branch],
    stack_diffs: &mut HashMap<BranchId, DiffState>,
) -> Option<Arc<str>> {
    if tasks.is_empty() {
        return None;
    }
    let tasks = Arc::new(tasks);
    let cursor = Arc::new(AtomicUsize::new(0));
    let workers = worker_count(tasks.len(), worker_limit);
    let (send, receive) = mpsc::sync_channel(workers * 2);
    let mut handles = Vec::with_capacity(workers);
    let mut spawn_error = None;
    for number in 0..workers {
        let tasks = tasks.clone();
        let cursor = cursor.clone();
        let send = send.clone();
        let git = git.clone();
        let latest_generation = latest_generation.clone();
        match thread::Builder::new()
            .name(format!("stackmap-diff-{number}"))
            .spawn(move || {
                loop {
                    if is_obsolete(generation, &latest_generation) {
                        break;
                    }
                    let index = cursor.fetch_add(1, Ordering::Relaxed);
                    let Some(task) = tasks.get(index) else {
                        break;
                    };
                    let value = git
                        .diffstat(&task.key.0, &task.key.1)
                        .map_err(|error| error.to_string());
                    if send.send((task.clone(), value)).is_err() {
                        break;
                    }
                }
            }) {
            Ok(handle) => handles.push(handle),
            Err(error) => {
                spawn_error = Some(Arc::<str>::from(format!(
                    "could not start bounded diff worker: {error}"
                )));
                break;
            }
        }
    }
    drop(send);
    while let Ok((task, result)) = receive.recv() {
        match result {
            Ok(stat) => {
                cache.insert(task.key, stat);
                for index in task.indexes {
                    branches[index].diff = DiffState::Ready(stat);
                }
                for stack_id in task.stack_ids {
                    stack_diffs.insert(stack_id, DiffState::Ready(stat));
                }
            }
            Err(error) => {
                let error: Arc<str> = Arc::from(error);
                for index in task.indexes {
                    branches[index].diff = DiffState::Unavailable(error.clone());
                }
                for stack_id in task.stack_ids {
                    stack_diffs.insert(stack_id, DiffState::Unavailable(error.clone()));
                }
            }
        }
    }
    for handle in handles {
        let _ = handle.join();
    }
    spawn_error
}

fn worker_count(task_count: usize, worker_limit: usize) -> usize {
    worker_limit.clamp(1, 4).min(task_count)
}

fn push_branch_task(
    tasks: &mut Vec<Task>,
    task_by_key: &mut HashMap<Key, usize>,
    index: usize,
    key: Key,
) {
    if let Some(task) = task_by_key.get(&key).copied() {
        tasks[task].indexes.push(index);
    } else {
        task_by_key.insert(key.clone(), tasks.len());
        tasks.push(Task {
            indexes: vec![index],
            stack_ids: Vec::new(),
            key,
        });
    }
}

fn push_stack_task(
    tasks: &mut Vec<Task>,
    task_by_key: &mut HashMap<Key, usize>,
    stack_id: BranchId,
    key: Key,
) {
    if let Some(task) = task_by_key.get(&key).copied() {
        tasks[task].stack_ids.push(stack_id);
    } else {
        task_by_key.insert(key.clone(), tasks.len());
        tasks.push(Task {
            indexes: Vec::new(),
            stack_ids: vec![stack_id],
            key,
        });
    }
}

fn is_obsolete(generation: u64, latest_generation: &AtomicU64) -> bool {
    latest_generation.load(Ordering::Acquire) != generation
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::Command;

    use super::*;
    use crate::model::{
        Branch, ConfiguredUpstream, GraphiteProvenance, RemoteRefEvidence, RepositoryState,
    };

    #[test]
    fn cache_evicts_instead_of_growing_forever() {
        let mut cache = DiffCache::new(8);
        for index in 0..100 {
            cache.insert(
                (
                    Arc::from(format!("parent-{index}")),
                    Arc::from(format!("child-{index}")),
                ),
                DiffStat {
                    insertions: index,
                    ..DiffStat::default()
                },
            );
        }
        assert_eq!(cache.len(), 8);
    }

    #[test]
    fn identical_oid_pairs_share_one_task() {
        let key = (Arc::from("parent"), Arc::from("child"));
        let mut tasks = Vec::new();
        let mut by_key = HashMap::new();
        push_branch_task(&mut tasks, &mut by_key, 2, key.clone());
        push_branch_task(&mut tasks, &mut by_key, 7, key);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].indexes, [2, 7]);
    }

    #[test]
    fn branch_and_stack_targets_share_one_task() {
        let key = (Arc::from("parent"), Arc::from("child"));
        let mut tasks = Vec::new();
        let mut by_key = HashMap::new();
        push_branch_task(&mut tasks, &mut by_key, 2, key.clone());
        push_stack_task(&mut tasks, &mut by_key, BranchId::new("stack"), key);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].indexes, [2]);
        assert_eq!(tasks[0].stack_ids, [BranchId::new("stack")]);
    }

    #[test]
    fn eager_summary_scheduling_is_bounded_and_branch_first_at_scale() {
        for (branches, stack_size) in [(500, 1), (5_000, 10)] {
            let mut tasks = Vec::new();
            let mut by_key = HashMap::new();
            for index in 0..branches {
                push_branch_task(
                    &mut tasks,
                    &mut by_key,
                    index,
                    (
                        Arc::from(format!("p-{index}")),
                        Arc::from(format!("b-{index}")),
                    ),
                );
            }
            for stack in 0..branches / stack_size {
                let bottom = stack * stack_size;
                let head = bottom + stack_size - 1;
                push_stack_task(
                    &mut tasks,
                    &mut by_key,
                    BranchId::new(format!("stack-{stack}")),
                    (
                        Arc::from(format!("p-{bottom}")),
                        Arc::from(format!("b-{head}")),
                    ),
                );
            }

            let aggregate_start = tasks.partition_point(|task| !task.indexes.is_empty());
            assert_eq!(worker_count(tasks.len(), usize::MAX), 4);
            assert_eq!(aggregate_start, branches);
            assert_eq!(
                tasks.len(),
                branches + usize::from(stack_size > 1) * (branches / stack_size)
            );
            assert!(
                tasks[..aggregate_start]
                    .iter()
                    .all(|task| !task.indexes.is_empty())
            );
            assert!(
                tasks[aggregate_start..]
                    .iter()
                    .all(|task| task.indexes.is_empty())
            );
        }
    }

    #[test]
    fn newer_structural_generation_cancels_old_work() {
        let latest = AtomicU64::new(4);
        assert!(is_obsolete(3, &latest));
        assert!(!is_obsolete(4, &latest));
    }

    #[test]
    fn stack_summary_is_direct_base_to_tip_while_branches_remain_relative() {
        let directory = tempfile::tempdir().unwrap();
        git(directory.path(), &["init", "-b", "main"]);
        git(directory.path(), &["config", "user.name", "Stackmap Tests"]);
        git(
            directory.path(),
            &["config", "user.email", "stackmap@example.invalid"],
        );
        std::fs::write(directory.path().join("file.txt"), "base\n").unwrap();
        git(directory.path(), &["add", "file.txt"]);
        git(directory.path(), &["commit", "-m", "base"]);
        let main_oid = git(directory.path(), &["rev-parse", "HEAD"]);
        git(directory.path(), &["checkout", "-b", "a"]);
        std::fs::write(directory.path().join("file.txt"), "base\nadded\n").unwrap();
        git(directory.path(), &["commit", "-am", "add"]);
        let a_oid = git(directory.path(), &["rev-parse", "HEAD"]);
        git(directory.path(), &["checkout", "-b", "b"]);
        std::fs::write(directory.path().join("file.txt"), "base\n").unwrap();
        git(directory.path(), &["commit", "-am", "revert"]);
        let b_oid = git(directory.path(), &["rev-parse", "HEAD"]);

        let branches = vec![
            test_branch("main", &main_oid, None, None, "main", true),
            test_branch("a", &a_oid, None, Some("main"), "a", false),
            test_branch("b", &b_oid, Some("a"), Some("a"), "a", false),
        ];
        let adapter = GitAdapter::discover(directory.path()).unwrap();
        let snapshot = Arc::new(RepositorySnapshot {
            generation: 1,
            root: directory.path().to_path_buf(),
            git_dir: directory.path().join(".git"),
            common_dir: directory.path().join(".git"),
            repository_id: Arc::from("diff-test"),
            default_trunk: Some(BranchId::new("main")),
            configured_trunks: Arc::from([BranchId::new("main")]),
            trunks: Arc::from([BranchId::new("main")]),
            graphite_children: Arc::from([
                (BranchId::new("main"), Arc::from([BranchId::new("a")])),
                (BranchId::new("a"), Arc::from([BranchId::new("b")])),
            ]),
            branch_index: RepositorySnapshot::index_branches(&branches),
            branches: branches.into(),
            stack_diffs: Arc::new(HashMap::from([(BranchId::new("a"), DiffState::Loading)])),
            state: RepositoryState::Ready,
            graphite_status: Arc::from("fixture"),
            stale_error: None,
        });
        let latest = Arc::new(AtomicU64::new(1));
        let partials = std::sync::Mutex::new(Vec::new());
        let enriched = enrich(
            snapshot,
            &adapter,
            &mut DiffCache::new(8),
            4,
            &latest,
            &|partial| partials.lock().unwrap().push(partial),
        )
        .unwrap();

        let partials = partials.into_inner().unwrap();
        assert_eq!(partials.len(), 1);
        assert!(matches!(
            partials[0].branch(&BranchId::new("a")).unwrap().diff,
            DiffState::Ready(_)
        ));
        assert_eq!(
            partials[0].stack_diffs[&BranchId::new("a")],
            DiffState::Loading
        );

        assert_eq!(
            enriched.stack_diffs[&BranchId::new("a")],
            DiffState::Ready(DiffStat::default())
        );
        assert_eq!(
            enriched.branch(&BranchId::new("a")).unwrap().diff,
            DiffState::Ready(DiffStat {
                insertions: 1,
                deletions: 0,
                files: 1,
                binary_files: 0,
            })
        );
        assert_eq!(
            enriched.branch(&BranchId::new("b")).unwrap().diff,
            DiffState::Ready(DiffStat {
                insertions: 0,
                deletions: 1,
                files: 1,
                binary_files: 0,
            })
        );
    }

    fn git(cwd: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn test_branch(
        name: &str,
        oid: &str,
        parent: Option<&str>,
        diff_parent: Option<&str>,
        root: &str,
        current: bool,
    ) -> Branch {
        Branch {
            id: BranchId::new(name),
            oid: Arc::from(oid),
            parent: parent.map(BranchId::new),
            diff_parent: diff_parent.map(BranchId::new),
            stack_root: BranchId::new(root),
            trunk: Some(BranchId::new("main")),
            graphite: GraphiteProvenance::Tracked,
            committed_at: 1,
            current,
            dirty: false,
            worktree: None,
            configured_upstream: ConfiguredUpstream::None,
            remote_ref: RemoteRefEvidence::NotRequested,
            diff: DiffState::Loading,
            pr: None,
        }
    }
}
