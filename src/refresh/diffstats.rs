use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;

use crate::adapters::git::GitAdapter;
use crate::model::{DiffStat, DiffState, RepositorySnapshot};

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
    key: Key,
}

pub(super) fn enrich(
    snapshot: Arc<RepositorySnapshot>,
    git: &GitAdapter,
    cache: &mut DiffCache,
    worker_limit: usize,
    latest_generation: &Arc<AtomicU64>,
) -> Option<Arc<RepositorySnapshot>> {
    let generation = snapshot.generation;
    let oid_by_id: HashMap<_, _> = snapshot
        .branches
        .iter()
        .map(|b| (b.id.clone(), b.oid.clone()))
        .collect();
    let mut branches = snapshot.branches.to_vec();
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
            push_task(&mut tasks, &mut task_by_key, index, key);
        }
    }
    if tasks.is_empty() {
        return (!is_obsolete(generation, latest_generation)).then(|| {
            Arc::new(RepositorySnapshot {
                branches: Arc::from(branches),
                ..(*snapshot).clone()
            })
        });
    }

    let tasks = Arc::new(tasks);
    let cursor = Arc::new(AtomicUsize::new(0));
    let workers = worker_limit.clamp(1, 4).min(tasks.len());
    let (send, receive) = mpsc::sync_channel(workers * 2);
    let mut handles = Vec::with_capacity(workers);
    for number in 0..workers {
        let tasks = tasks.clone();
        let cursor = cursor.clone();
        let send = send.clone();
        let git = git.clone();
        let latest_generation = latest_generation.clone();
        handles.push(
            thread::Builder::new()
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
                })
                .expect("spawn bounded diff worker"),
        );
    }
    drop(send);
    while let Ok((task, result)) = receive.recv() {
        match result {
            Ok(stat) => {
                cache.insert(task.key, stat);
                for index in task.indexes {
                    branches[index].diff = DiffState::Ready(stat);
                }
            }
            Err(error) => {
                let error: Arc<str> = Arc::from(error);
                for index in task.indexes {
                    branches[index].diff = DiffState::Unavailable(error.clone());
                }
            }
        }
    }
    for handle in handles {
        let _ = handle.join();
    }
    if is_obsolete(generation, latest_generation) {
        return None;
    }
    Some(Arc::new(RepositorySnapshot {
        branches: Arc::from(branches),
        ..(*snapshot).clone()
    }))
}

fn push_task(tasks: &mut Vec<Task>, task_by_key: &mut HashMap<Key, usize>, index: usize, key: Key) {
    if let Some(task) = task_by_key.get(&key).copied() {
        tasks[task].indexes.push(index);
    } else {
        task_by_key.insert(key.clone(), tasks.len());
        tasks.push(Task {
            indexes: vec![index],
            key,
        });
    }
}

fn is_obsolete(generation: u64, latest_generation: &AtomicU64) -> bool {
    latest_generation.load(Ordering::Acquire) != generation
}

#[cfg(test)]
mod tests {
    use super::*;

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
        push_task(&mut tasks, &mut by_key, 2, key.clone());
        push_task(&mut tasks, &mut by_key, 7, key);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].indexes, [2, 7]);
    }

    #[test]
    fn newer_structural_generation_cancels_old_work() {
        let latest = AtomicU64::new(4);
        assert!(is_obsolete(3, &latest));
        assert!(!is_obsolete(4, &latest));
    }
}
