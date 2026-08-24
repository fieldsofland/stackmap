use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::adapters::git::GitAdapter;
use crate::model::{BranchId, GraphiteHealth};

pub const MAX_TARGETS: usize = 512;
const CACHE_CAPACITY: usize = 2048;
const RESULT_CAPACITY: usize = 8;
const WORKER_LIMIT: usize = 4;
const REQUEST_DEADLINE: Duration = Duration::from_secs(4);

type CacheKey = (Arc<str>, Arc<str>);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HealthTarget {
    pub branch: BranchId,
    pub oid: Arc<str>,
    pub parent: BranchId,
    pub parent_oid: Arc<str>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HealthRequest {
    pub generation: u64,
    pub targets: Arc<[HealthTarget]>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HealthResult {
    pub branch: BranchId,
    pub oid: Arc<str>,
    pub parent: BranchId,
    pub parent_oid: Arc<str>,
    pub health: GraphiteHealth,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HealthBatch {
    pub generation: u64,
    pub results: Arc<[HealthResult]>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HealthCommand {
    Request(HealthRequest),
    Cancel,
}

#[derive(Default)]
struct CoordinatorState {
    latest: Option<HealthRequest>,
    updated: bool,
    shutdown: bool,
}

struct HealthCache {
    capacity: usize,
    values: HashMap<CacheKey, bool>,
    order: VecDeque<CacheKey>,
}

impl HealthCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            values: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn get(&self, key: &CacheKey) -> Option<bool> {
        self.values.get(key).copied()
    }

    fn insert(&mut self, key: CacheKey, value: bool) {
        if self.values.insert(key.clone(), value).is_none() {
            self.order.push_back(key);
        }
        while self.values.len() > self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.values.remove(&oldest);
            }
        }
    }
}

pub struct HealthCoordinator {
    state: Arc<(Mutex<CoordinatorState>, Condvar)>,
    revision: Arc<AtomicU64>,
    results: Arc<Mutex<VecDeque<HealthBatch>>>,
    worker: Option<JoinHandle<()>>,
}

impl HealthCoordinator {
    pub fn start(adapter: GitAdapter) -> std::io::Result<Self> {
        Self::start_with(Arc::new(adapter), REQUEST_DEADLINE)
    }

    fn start_with(
        adapter: Arc<dyn HealthGit>,
        request_deadline: Duration,
    ) -> std::io::Result<Self> {
        let state = Arc::new((Mutex::new(CoordinatorState::default()), Condvar::new()));
        let revision = Arc::new(AtomicU64::new(0));
        let results = Arc::new(Mutex::new(VecDeque::with_capacity(RESULT_CAPACITY)));
        let worker = thread::Builder::new()
            .name("stackmap-graphite-health".into())
            .spawn({
                let state = state.clone();
                let revision = revision.clone();
                let results = results.clone();
                move || {
                    let mut cache = HealthCache::new(CACHE_CAPACITY);
                    'worker: loop {
                        let (request, request_revision) = {
                            let (state, wake) = &*state;
                            let mut state = match state.lock() {
                                Ok(state) => state,
                                Err(_) => break 'worker,
                            };
                            while !state.updated && !state.shutdown {
                                state = match wake.wait(state) {
                                    Ok(state) => state,
                                    Err(_) => break 'worker,
                                };
                            }
                            if state.shutdown {
                                break;
                            }
                            state.updated = false;
                            (state.latest.take(), revision.load(Ordering::Acquire))
                        };
                        let Some(request) = request else {
                            continue;
                        };
                        let Some(batch) = enrich(
                            request,
                            adapter.as_ref(),
                            &mut cache,
                            request_revision,
                            &revision,
                            request_deadline,
                        ) else {
                            continue;
                        };
                        let Ok(mut results) = results.lock() else {
                            break;
                        };
                        if results.len() == RESULT_CAPACITY {
                            results.pop_front();
                        }
                        results.push_back(batch);
                    }
                }
            })?;
        Ok(Self {
            state,
            revision,
            results,
            worker: Some(worker),
        })
    }

    pub fn submit(&self, command: HealthCommand) {
        self.revision.fetch_add(1, Ordering::AcqRel);
        let (state, wake) = &*self.state;
        let Ok(mut state) = state.lock() else {
            return;
        };
        state.latest = match command {
            HealthCommand::Request(mut request) => {
                if request.targets.len() > MAX_TARGETS {
                    request.targets = Arc::from(&request.targets[..MAX_TARGETS]);
                }
                Some(request)
            }
            HealthCommand::Cancel => None,
        };
        state.updated = true;
        wake.notify_one();
    }

    pub fn try_result(&self) -> Option<HealthBatch> {
        self.results.lock().ok()?.pop_front()
    }
}

impl Drop for HealthCoordinator {
    fn drop(&mut self) {
        let (state, wake) = &*self.state;
        if let Ok(mut state) = state.lock() {
            state.shutdown = true;
        }
        self.revision.fetch_add(1, Ordering::AcqRel);
        wake.notify_all();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[derive(Clone)]
struct Task {
    key: CacheKey,
    targets: Vec<HealthTarget>,
}

fn enrich(
    request: HealthRequest,
    adapter: &dyn HealthGit,
    cache: &mut HealthCache,
    request_revision: u64,
    revision: &AtomicU64,
    request_deadline: Duration,
) -> Option<HealthBatch> {
    if obsolete(request_revision, revision) {
        return None;
    }
    let mut results = Vec::with_capacity(request.targets.len());
    let mut tasks = Vec::<Task>::new();
    let mut task_by_key = HashMap::<CacheKey, usize>::new();
    for target in request.targets.iter() {
        let key = (target.parent_oid.clone(), target.oid.clone());
        if let Some(healthy) = cache.get(&key) {
            results.push(result_for(target, Ok::<bool, String>(healthy)));
        } else if let Some(index) = task_by_key.get(&key).copied() {
            tasks[index].targets.push(target.clone());
        } else {
            task_by_key.insert(key.clone(), tasks.len());
            tasks.push(Task {
                key,
                targets: vec![target.clone()],
            });
        }
    }

    if !tasks.is_empty() {
        let tasks = Arc::new(tasks);
        let cursor = Arc::new(AtomicUsize::new(0));
        let deadline = Instant::now() + request_deadline;
        let (send, receive) = mpsc::sync_channel(WORKER_LIMIT * 2);
        thread::scope(|scope| {
            for _ in 0..WORKER_LIMIT.min(tasks.len()) {
                let tasks = tasks.clone();
                let cursor = cursor.clone();
                let send = send.clone();
                scope.spawn(move || {
                    loop {
                        if obsolete(request_revision, revision) || Instant::now() >= deadline {
                            break;
                        }
                        let index = cursor.fetch_add(1, Ordering::Relaxed);
                        let Some(task) = tasks.get(index) else {
                            break;
                        };
                        let value = adapter
                            .is_ancestor(&task.key.0, &task.key.1)
                            .map_err(|error| error.to_string());
                        if send.send((index, value)).is_err() {
                            break;
                        }
                    }
                });
            }
            drop(send);
            while let Ok((index, value)) = receive.recv() {
                let task = &tasks[index];
                if let Ok(healthy) = &value {
                    cache.insert(task.key.clone(), *healthy);
                }
                for target in &task.targets {
                    results.push(result_for(target, value.clone()));
                }
            }
        });
        if obsolete(request_revision, revision) {
            return None;
        }
        for task in tasks.iter() {
            for target in &task.targets {
                if !results.iter().any(|result| result.branch == target.branch) {
                    results.push(HealthResult {
                        branch: target.branch.clone(),
                        oid: target.oid.clone(),
                        parent: target.parent.clone(),
                        parent_oid: target.parent_oid.clone(),
                        health: GraphiteHealth::Unavailable(Arc::from(
                            "Graphite ancestry check deadline exceeded",
                        )),
                    });
                }
            }
        }
    }
    results.sort_by(|left, right| left.branch.cmp(&right.branch));
    Some(HealthBatch {
        generation: request.generation,
        results: Arc::from(results),
    })
}

fn result_for(target: &HealthTarget, value: Result<bool, String>) -> HealthResult {
    let health = match value {
        Ok(true) => GraphiteHealth::Healthy,
        Ok(false) => GraphiteHealth::NeedsRestack {
            recorded_parent: target.parent.clone(),
        },
        Err(error) => GraphiteHealth::Unavailable(Arc::from(error.to_string())),
    };
    HealthResult {
        branch: target.branch.clone(),
        oid: target.oid.clone(),
        parent: target.parent.clone(),
        parent_oid: target.parent_oid.clone(),
        health,
    }
}

fn obsolete(request_revision: u64, revision: &AtomicU64) -> bool {
    revision.load(Ordering::Acquire) != request_revision
}

trait HealthGit: Send + Sync {
    fn is_ancestor(&self, ancestor: &str, descendant: &str) -> anyhow::Result<bool>;
}

impl HealthGit for GitAdapter {
    fn is_ancestor(&self, ancestor: &str, descendant: &str) -> anyhow::Result<bool> {
        GitAdapter::is_ancestor(self, ancestor, descendant)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeGit {
        calls: AtomicUsize,
    }

    impl HealthGit for FakeGit {
        fn is_ancestor(&self, ancestor: &str, descendant: &str) -> anyhow::Result<bool> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            Ok(ancestor != descendant)
        }
    }

    fn request(generation: u64, parent_oid: &str, oid: &str) -> HealthRequest {
        HealthRequest {
            generation,
            targets: Arc::from([HealthTarget {
                branch: BranchId::new("feature"),
                oid: Arc::from(oid),
                parent: BranchId::new("main"),
                parent_oid: Arc::from(parent_oid),
            }]),
        }
    }

    fn many_targets(generation: u64, count: usize) -> HealthRequest {
        HealthRequest {
            generation,
            targets: (0..count)
                .map(|index| HealthTarget {
                    branch: BranchId::new(format!("branch-{index}")),
                    oid: Arc::from(format!("tip-{index}")),
                    parent: BranchId::new("main"),
                    parent_oid: Arc::from(format!("parent-{index}")),
                })
                .collect(),
        }
    }

    fn wait_for_result(coordinator: &HealthCoordinator) -> HealthBatch {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(batch) = coordinator.try_result() {
                return batch;
            }
            assert!(Instant::now() < deadline, "health result timed out");
            thread::sleep(Duration::from_millis(2));
        }
    }

    struct BoundedGit {
        calls: AtomicUsize,
        active: AtomicUsize,
        max_active: AtomicUsize,
        delay: Duration,
    }

    impl HealthGit for BoundedGit {
        fn is_ancestor(&self, _ancestor: &str, _descendant: &str) -> anyhow::Result<bool> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            let active = self.active.fetch_add(1, Ordering::AcqRel) + 1;
            self.max_active.fetch_max(active, Ordering::AcqRel);
            thread::sleep(self.delay);
            self.active.fetch_sub(1, Ordering::AcqRel);
            Ok(true)
        }
    }

    #[test]
    fn unchanged_oid_pair_uses_cached_health() {
        let git = FakeGit {
            calls: AtomicUsize::new(0),
        };
        let revision = AtomicU64::new(1);
        let mut cache = HealthCache::new(8);

        let first = enrich(
            request(1, "parent", "tip"),
            &git,
            &mut cache,
            1,
            &revision,
            Duration::from_secs(1),
        )
        .unwrap();
        let second = enrich(
            request(2, "parent", "tip"),
            &git,
            &mut cache,
            1,
            &revision,
            Duration::from_secs(1),
        )
        .unwrap();

        assert_eq!(git.calls.load(Ordering::Acquire), 1);
        assert_eq!(first.results[0].health, GraphiteHealth::Healthy);
        assert_eq!(second.results[0].health, GraphiteHealth::Healthy);
    }

    #[test]
    fn changed_oid_pair_is_rechecked() {
        let git = FakeGit {
            calls: AtomicUsize::new(0),
        };
        let revision = AtomicU64::new(1);
        let mut cache = HealthCache::new(8);

        enrich(
            request(1, "parent", "tip-1"),
            &git,
            &mut cache,
            1,
            &revision,
            Duration::from_secs(1),
        )
        .unwrap();
        enrich(
            request(2, "parent", "tip-2"),
            &git,
            &mut cache,
            1,
            &revision,
            Duration::from_secs(1),
        )
        .unwrap();

        assert_eq!(git.calls.load(Ordering::Acquire), 2);
    }

    #[test]
    fn deadline_worker_and_target_bounds_hold_for_large_requests() {
        let git = Arc::new(BoundedGit {
            calls: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
            delay: Duration::from_millis(20),
        });
        let coordinator =
            HealthCoordinator::start_with(git.clone(), Duration::from_millis(35)).unwrap();

        coordinator.submit(HealthCommand::Request(many_targets(1, 5_000)));
        let batch = wait_for_result(&coordinator);

        assert_eq!(batch.results.len(), MAX_TARGETS);
        assert!(git.calls.load(Ordering::Acquire) <= WORKER_LIMIT * 2);
        assert!(git.max_active.load(Ordering::Acquire) <= WORKER_LIMIT);
    }

    #[test]
    fn cancel_discards_active_health_results_without_clearing_the_worker() {
        let git = Arc::new(BoundedGit {
            calls: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
            delay: Duration::from_millis(40),
        });
        let coordinator =
            HealthCoordinator::start_with(git.clone(), Duration::from_millis(200)).unwrap();
        coordinator.submit(HealthCommand::Request(request(1, "parent", "tip")));
        let deadline = Instant::now() + Duration::from_secs(1);
        while git.calls.load(Ordering::Acquire) == 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
        coordinator.submit(HealthCommand::Cancel);
        thread::sleep(Duration::from_millis(80));
        assert!(coordinator.try_result().is_none());
    }

    #[test]
    fn health_result_queue_discards_oldest_batches_at_capacity() {
        let git = Arc::new(BoundedGit {
            calls: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
            delay: Duration::ZERO,
        });
        let coordinator =
            HealthCoordinator::start_with(git.clone(), Duration::from_secs(1)).unwrap();
        for generation in 1..=(RESULT_CAPACITY as u64 + 2) {
            coordinator.submit(HealthCommand::Request(request(
                generation,
                &format!("parent-{generation}"),
                &format!("tip-{generation}"),
            )));
            let deadline = Instant::now() + Duration::from_secs(1);
            while git.calls.load(Ordering::Acquire) < generation as usize
                && Instant::now() < deadline
            {
                thread::sleep(Duration::from_millis(2));
            }
            thread::sleep(Duration::from_millis(2));
        }
        let mut batches = Vec::new();
        while let Some(batch) = coordinator.try_result() {
            batches.push(batch);
        }
        assert_eq!(batches.len(), RESULT_CAPACITY);
        assert_eq!(
            batches.last().unwrap().generation,
            RESULT_CAPACITY as u64 + 2
        );
    }
}
