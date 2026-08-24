use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use crate::adapters::git::{ExactRemoteRefTips, GitAdapter};
use crate::model::{BranchId, ConfiguredUpstream, RemoteRefEvidence};

pub const MAX_TARGETS: usize = 512;
const CACHE_CAPACITY: usize = 1024;
const RESULT_CAPACITY: usize = 8;
// Stop starting additional classification checks after this deadline. An
// already-running bounded Git command may finish after it.
const REQUEST_DEADLINE: Duration = Duration::from_secs(4);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpstreamTarget {
    pub branch: BranchId,
    pub oid: Arc<str>,
    pub configured_upstream: ConfiguredUpstream,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpstreamRequest {
    pub generation: u64,
    pub targets: Arc<[UpstreamTarget]>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpstreamResult {
    pub branch: BranchId,
    pub oid: Arc<str>,
    pub evidence: RemoteRefEvidence,
    pub configured_upstream: Option<ConfiguredUpstream>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpstreamBatch {
    pub generation: u64,
    pub remote_ref_token: Option<u64>,
    pub results: Arc<[UpstreamResult]>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UpstreamCommand {
    Request(UpstreamRequest),
    Cancel,
}

#[derive(Default)]
struct CoordinatorState {
    latest: Option<UpstreamRequest>,
    updated: bool,
    shutdown: bool,
}

type CacheKey = (Arc<str>, u64);
struct EvidenceCache {
    capacity: usize,
    values: HashMap<CacheKey, RemoteRefEvidence>,
    order: VecDeque<CacheKey>,
    last_request_generation: Option<u64>,
    last_source_token: Option<u64>,
}

impl EvidenceCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            values: HashMap::new(),
            order: VecDeque::new(),
            last_request_generation: None,
            last_source_token: None,
        }
    }

    fn get(&self, key: &CacheKey) -> Option<RemoteRefEvidence> {
        self.values.get(key).cloned()
    }

    fn insert(&mut self, key: CacheKey, value: RemoteRefEvidence) {
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

pub struct UpstreamCoordinator {
    state: Arc<(Mutex<CoordinatorState>, Condvar)>,
    revision: Arc<AtomicU64>,
    results: Arc<Mutex<VecDeque<UpstreamBatch>>>,
    worker: Option<JoinHandle<()>>,
}

impl UpstreamCoordinator {
    pub fn start(adapter: GitAdapter) -> std::io::Result<Self> {
        Self::start_with(Arc::new(adapter), REQUEST_DEADLINE)
    }

    fn start_with(
        adapter: Arc<dyn EvidenceGit>,
        request_deadline: Duration,
    ) -> std::io::Result<Self> {
        let state = Arc::new((Mutex::new(CoordinatorState::default()), Condvar::new()));
        let revision = Arc::new(AtomicU64::new(0));
        let results = Arc::new(Mutex::new(VecDeque::with_capacity(RESULT_CAPACITY)));
        let worker = thread::Builder::new()
            .name("stackmap-upstream".into())
            .spawn({
                let state = state.clone();
                let revision = revision.clone();
                let results = results.clone();
                move || {
                    let mut cache = EvidenceCache::new(CACHE_CAPACITY);
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

    pub fn submit(&self, command: UpstreamCommand) {
        self.revision.fetch_add(1, Ordering::AcqRel);
        let (state, wake) = &*self.state;
        let Ok(mut state) = state.lock() else {
            return;
        };
        state.latest = match command {
            UpstreamCommand::Request(mut request) => {
                if request.targets.len() > MAX_TARGETS {
                    request.targets = Arc::from(&request.targets[..MAX_TARGETS]);
                }
                Some(request)
            }
            UpstreamCommand::Cancel => None,
        };
        state.updated = true;
        wake.notify_one();
    }

    pub fn try_result(&self) -> Option<UpstreamBatch> {
        self.results.lock().ok()?.pop_front()
    }
}

impl Drop for UpstreamCoordinator {
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

fn enrich(
    request: UpstreamRequest,
    adapter: &dyn EvidenceGit,
    cache: &mut EvidenceCache,
    request_revision: u64,
    revision: &AtomicU64,
    request_deadline: Duration,
) -> Option<UpstreamBatch> {
    if obsolete(request_revision, revision) {
        return None;
    }
    let checked_at = SystemTime::now();
    if let (Some(generation), Some(observed_token)) =
        (cache.last_request_generation, cache.last_source_token)
    {
        if generation == request.generation {
            let cached = request
                .targets
                .iter()
                .map(|target| cache.get(&(target.oid.clone(), observed_token)))
                .collect::<Option<Vec<_>>>();
            if let Some(evidence) = cached {
                return Some(completed_batch(request, evidence, observed_token));
            }
        }
    }

    let deadline = Instant::now() + request_deadline;
    if Instant::now() >= deadline {
        return unavailable_batch(
            &request,
            "local remote-ref check deadline exceeded",
            None,
            checked_at,
            request_revision,
            revision,
        );
    }
    let (observed_token, exact_tips) = match adapter.exact_remote_ref_tips() {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return unavailable_batch(
                &request,
                &format!("local remote refs unavailable: {error}"),
                None,
                checked_at,
                request_revision,
                revision,
            );
        }
    };
    if obsolete(request_revision, revision) {
        return None;
    }
    let evidence = request
        .targets
        .iter()
        .map(|target| match exact_tips.get(&target.oid) {
            Some(reference) => RemoteRefEvidence::ExactTip {
                reference: reference.clone(),
                source_token: observed_token,
                checked_at,
            },
            None => RemoteRefEvidence::LocalOnly {
                source_token: observed_token,
                checked_at,
            },
        })
        .collect::<Vec<_>>();
    for (target, evidence) in request.targets.iter().zip(&evidence) {
        if !matches!(evidence, RemoteRefEvidence::Unavailable { .. }) {
            cache.insert((target.oid.clone(), observed_token), evidence.clone());
        }
    }
    cache.last_request_generation = Some(request.generation);
    cache.last_source_token = Some(observed_token);
    Some(completed_batch(request, evidence, observed_token))
}

fn completed_batch(
    request: UpstreamRequest,
    evidence: Vec<RemoteRefEvidence>,
    observed_token: u64,
) -> UpstreamBatch {
    let mut results = request
        .targets
        .iter()
        .zip(evidence)
        .map(|(target, evidence)| UpstreamResult {
            branch: target.branch.clone(),
            oid: target.oid.clone(),
            evidence,
            configured_upstream: None,
        })
        .collect::<Vec<_>>();
    results.sort_by(|left, right| left.branch.cmp(&right.branch));
    UpstreamBatch {
        generation: request.generation,
        remote_ref_token: Some(observed_token),
        results: Arc::from(results),
    }
}

trait EvidenceGit: Send + Sync {
    fn exact_remote_ref_tips(&self) -> anyhow::Result<(u64, ExactRemoteRefTips)>;
}

impl EvidenceGit for GitAdapter {
    fn exact_remote_ref_tips(&self) -> anyhow::Result<(u64, ExactRemoteRefTips)> {
        GitAdapter::exact_remote_ref_tips(self)
    }
}

fn unavailable_batch(
    request: &UpstreamRequest,
    reason: &str,
    source_token: Option<u64>,
    checked_at: SystemTime,
    request_revision: u64,
    revision: &AtomicU64,
) -> Option<UpstreamBatch> {
    (!obsolete(request_revision, revision)).then(|| UpstreamBatch {
        generation: request.generation,
        remote_ref_token: source_token,
        results: request
            .targets
            .iter()
            .map(|target| UpstreamResult {
                branch: target.branch.clone(),
                oid: target.oid.clone(),
                evidence: RemoteRefEvidence::Unavailable {
                    reason: Arc::from(reason),
                    source_token,
                    checked_at,
                },
                configured_upstream: None,
            })
            .collect(),
    })
}

fn obsolete(request_revision: u64, revision: &AtomicU64) -> bool {
    revision.load(Ordering::Acquire) != request_revision
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    struct FakeGit {
        token: AtomicU64,
        map_calls: AtomicUsize,
        containment_calls: AtomicUsize,
        patch_calls: AtomicUsize,
        delay: Duration,
    }

    impl FakeGit {
        fn new(token: u64, delay: Duration) -> Self {
            Self {
                token: AtomicU64::new(token),
                map_calls: AtomicUsize::new(0),
                containment_calls: AtomicUsize::new(0),
                patch_calls: AtomicUsize::new(0),
                delay,
            }
        }
    }

    impl EvidenceGit for FakeGit {
        fn exact_remote_ref_tips(&self) -> anyhow::Result<(u64, ExactRemoteRefTips)> {
            self.map_calls.fetch_add(1, Ordering::AcqRel);
            thread::sleep(self.delay);
            Ok((
                self.token.load(Ordering::Acquire),
                HashMap::from([
                    (Arc::from("exact"), Arc::from("refs/remotes/origin/exact")),
                    (Arc::from("old"), Arc::from("refs/remotes/origin/old")),
                    (Arc::from("new"), Arc::from("refs/remotes/origin/new")),
                    (
                        Arc::from("contained"),
                        Arc::from("refs/remotes/origin/main"),
                    ),
                ]),
            ))
        }
    }

    fn request(generation: u64, oids: &[&str]) -> UpstreamRequest {
        UpstreamRequest {
            generation,
            targets: oids
                .iter()
                .enumerate()
                .map(|(index, oid)| UpstreamTarget {
                    branch: BranchId::new(format!("branch-{index}")),
                    oid: Arc::from(*oid),
                    configured_upstream: ConfiguredUpstream::None,
                })
                .collect(),
        }
    }

    fn wait_for_calls(calls: &AtomicUsize, minimum: usize) {
        let deadline = Instant::now() + Duration::from_secs(1);
        while calls.load(Ordering::Acquire) < minimum && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
        assert!(calls.load(Ordering::Acquire) >= minimum);
    }

    fn wait_for_result(coordinator: &UpstreamCoordinator) -> UpstreamBatch {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(result) = coordinator.try_result() {
                return result;
            }
            assert!(Instant::now() < deadline, "coordinator result timed out");
            thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn cache_is_bounded() {
        let mut cache = EvidenceCache::new(4);
        for index in 0..20 {
            cache.insert(
                (Arc::from(format!("oid-{index}")), index),
                RemoteRefEvidence::ExactTip {
                    reference: Arc::from("refs/remotes/origin/branch"),
                    source_token: index,
                    checked_at: SystemTime::UNIX_EPOCH,
                },
            );
        }
        assert_eq!(cache.values.len(), 4);
    }

    #[test]
    fn cancellation_revision_obsoletes_active_work() {
        let revision = AtomicU64::new(4);
        assert!(obsolete(3, &revision));
        assert!(!obsolete(4, &revision));
    }

    #[test]
    fn exact_tip_map_is_called_once_and_same_generation_uses_cache() {
        let git = Arc::new(FakeGit::new(7, Duration::ZERO));
        let coordinator =
            UpstreamCoordinator::start_with(git.clone(), Duration::from_millis(100)).unwrap();
        let request = request(3, &["contained"]);
        coordinator.submit(UpstreamCommand::Request(request.clone()));
        assert!(matches!(
            wait_for_result(&coordinator).results[0].evidence,
            RemoteRefEvidence::ExactTip { .. }
        ));
        assert_eq!(git.map_calls.load(Ordering::Acquire), 1);
        coordinator.submit(UpstreamCommand::Request(request));
        let _ = wait_for_result(&coordinator);
        assert_eq!(git.map_calls.load(Ordering::Acquire), 1);
        assert_eq!(git.containment_calls.load(Ordering::Acquire), 0);
        assert_eq!(git.patch_calls.load(Ordering::Acquire), 0);
    }

    #[test]
    fn new_generation_takes_one_new_exact_tip_snapshot() {
        let git = Arc::new(FakeGit::new(7, Duration::ZERO));
        let coordinator =
            UpstreamCoordinator::start_with(git.clone(), Duration::from_millis(100)).unwrap();
        coordinator.submit(UpstreamCommand::Request(request(3, &["contained"])));
        let _ = wait_for_result(&coordinator);
        coordinator.submit(UpstreamCommand::Request(request(4, &["contained"])));
        let batch = wait_for_result(&coordinator);
        assert_eq!(batch.generation, 4);
        assert_eq!(git.map_calls.load(Ordering::Acquire), 2);
    }

    #[test]
    fn cancel_discards_an_active_result() {
        let git = Arc::new(FakeGit::new(9, Duration::from_millis(60)));
        let coordinator =
            UpstreamCoordinator::start_with(git.clone(), Duration::from_millis(200)).unwrap();
        coordinator.submit(UpstreamCommand::Request(request(1, &["old"])));
        wait_for_calls(&git.map_calls, 1);
        coordinator.submit(UpstreamCommand::Cancel);
        thread::sleep(Duration::from_millis(100));
        assert!(coordinator.try_result().is_none());
    }

    #[test]
    fn latest_pending_request_supersedes_active_work() {
        let git = Arc::new(FakeGit::new(9, Duration::from_millis(50)));
        let coordinator =
            UpstreamCoordinator::start_with(git.clone(), Duration::from_millis(250)).unwrap();
        coordinator.submit(UpstreamCommand::Request(request(1, &["old"])));
        wait_for_calls(&git.map_calls, 1);
        coordinator.submit(UpstreamCommand::Request(request(2, &["new"])));
        let batch = wait_for_result(&coordinator);
        assert_eq!(batch.generation, 2);
        assert!(coordinator.try_result().is_none());
    }

    #[test]
    fn only_exact_tip_equality_is_reported() {
        let git = Arc::new(FakeGit::new(4, Duration::ZERO));
        let coordinator =
            UpstreamCoordinator::start_with(git.clone(), Duration::from_millis(250)).unwrap();
        coordinator.submit(UpstreamCommand::Request(request(1, &["exact", "ancestor"])));
        let batch = wait_for_result(&coordinator);
        assert!(matches!(
            batch.results[0].evidence,
            RemoteRefEvidence::ExactTip { .. }
        ));
        assert!(matches!(
            batch.results[1].evidence,
            RemoteRefEvidence::LocalOnly { .. }
        ));
    }

    #[test]
    fn expired_request_deadline_starts_no_git_work() {
        let git = Arc::new(FakeGit::new(4, Duration::ZERO));
        let coordinator = UpstreamCoordinator::start_with(git.clone(), Duration::ZERO).unwrap();
        coordinator.submit(UpstreamCommand::Request(request(1, &["exact"])));
        let batch = wait_for_result(&coordinator);
        assert!(matches!(
            batch.results[0].evidence,
            RemoteRefEvidence::Unavailable { .. }
        ));
        assert_eq!(git.map_calls.load(Ordering::Acquire), 0);
    }

    #[test]
    fn working_set_is_truncated_and_uses_one_map_call_at_scale() {
        let git = Arc::new(FakeGit::new(2, Duration::ZERO));
        let coordinator =
            UpstreamCoordinator::start_with(git.clone(), Duration::from_millis(100)).unwrap();
        let request = UpstreamRequest {
            generation: 1,
            targets: (0..5_000)
                .map(|index| UpstreamTarget {
                    branch: BranchId::new(format!("branch-{index}")),
                    oid: Arc::from(format!("oid-{index}")),
                    configured_upstream: ConfiguredUpstream::None,
                })
                .collect(),
        };
        coordinator.submit(UpstreamCommand::Request(request));
        let batch = wait_for_result(&coordinator);
        assert_eq!(batch.results.len(), MAX_TARGETS);
        assert_eq!(git.map_calls.load(Ordering::Acquire), 1);
        assert_eq!(git.containment_calls.load(Ordering::Acquire), 0);
        assert_eq!(git.patch_calls.load(Ordering::Acquire), 0);
    }

    #[test]
    fn raw_two_sided_history_does_not_launch_patch_equivalence_work() {
        let git = Arc::new(FakeGit::new(2, Duration::ZERO));
        let coordinator =
            UpstreamCoordinator::start_with(git.clone(), Duration::from_millis(100)).unwrap();
        let request = UpstreamRequest {
            generation: 1,
            targets: Arc::from([UpstreamTarget {
                branch: BranchId::new("feature"),
                oid: Arc::from("local-tip"),
                configured_upstream: ConfiguredUpstream::Diverged {
                    reference: Arc::from("refs/remotes/origin/feature"),
                    ahead: 156,
                    behind: 7,
                },
            }]),
        };
        coordinator.submit(UpstreamCommand::Request(request));
        let batch = wait_for_result(&coordinator);
        assert!(batch.results[0].configured_upstream.is_none());
        assert_eq!(git.map_calls.load(Ordering::Acquire), 1);
        assert_eq!(git.containment_calls.load(Ordering::Acquire), 0);
        assert_eq!(git.patch_calls.load(Ordering::Acquire), 0);
    }

    #[test]
    fn configured_upstream_compatibility_field_is_always_none() {
        let git = Arc::new(FakeGit::new(2, Duration::ZERO));
        let coordinator =
            UpstreamCoordinator::start_with(git.clone(), Duration::from_millis(100)).unwrap();
        let target = UpstreamTarget {
            branch: BranchId::new("feature"),
            oid: Arc::from("local-tip"),
            configured_upstream: ConfiguredUpstream::Rewritten {
                reference: Arc::from("refs/remotes/origin/feature"),
                ahead: 156,
                behind: 7,
            },
        };

        coordinator.submit(UpstreamCommand::Request(UpstreamRequest {
            generation: 2,
            targets: Arc::from([target]),
        }));

        let batch = wait_for_result(&coordinator);
        assert!(batch.results[0].configured_upstream.is_none());
        assert_eq!(git.patch_calls.load(Ordering::Acquire), 0);
    }

    #[test]
    fn result_queue_discards_oldest_batches_at_capacity() {
        let git = Arc::new(FakeGit::new(1, Duration::ZERO));
        let coordinator =
            UpstreamCoordinator::start_with(git.clone(), Duration::from_millis(100)).unwrap();
        for generation in 1..=(RESULT_CAPACITY as u64 + 3) {
            let oid = format!("oid-{generation}");
            coordinator.submit(UpstreamCommand::Request(request(generation, &[&oid])));
            wait_for_calls(&git.map_calls, generation as usize);
            thread::sleep(Duration::from_millis(2));
        }
        thread::sleep(Duration::from_millis(20));
        let mut batches = Vec::new();
        while let Some(batch) = coordinator.try_result() {
            batches.push(batch);
        }
        assert_eq!(batches.len(), RESULT_CAPACITY);
        assert_eq!(
            batches.last().unwrap().generation,
            RESULT_CAPACITY as u64 + 3
        );
    }
}
