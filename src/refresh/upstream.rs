use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use crate::adapters::git::GitAdapter;
use crate::model::{BranchId, RemoteRefEvidence};

pub const MAX_TARGETS: usize = 64;
const CACHE_CAPACITY: usize = 1024;
const RESULT_CAPACITY: usize = 8;
// Remote-ref Git commands have a 250ms individual timeout. Including the
// before/after source-token checks, a non-cancelled request is bounded to
// roughly this deadline plus 750ms.
const REQUEST_DEADLINE: Duration = Duration::from_secs(4);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpstreamTarget {
    pub branch: BranchId,
    pub oid: Arc<str>,
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
    let same_generation_token = (cache.last_request_generation == Some(request.generation))
        .then_some(cache.last_source_token)
        .flatten();
    let observed_token = match same_generation_token {
        Some(token) => token,
        None => match adapter.remote_ref_token() {
            Ok(token) => token,
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
        },
    };
    let mut results = Vec::with_capacity(request.targets.len());
    let mut misses = Vec::new();
    for target in request.targets.iter() {
        let key = (target.oid.clone(), observed_token);
        if let Some(evidence) = cache.get(&key) {
            results.push(UpstreamResult {
                branch: target.branch.clone(),
                oid: target.oid.clone(),
                evidence,
            });
        } else {
            misses.push(target.clone());
        }
    }
    if misses.is_empty() {
        cache.last_request_generation = Some(request.generation);
        cache.last_source_token = Some(observed_token);
        return (!obsolete(request_revision, revision)).then(|| UpstreamBatch {
            generation: request.generation,
            remote_ref_token: Some(observed_token),
            results: Arc::from(results),
        });
    }

    if same_generation_token.is_some() {
        match adapter.remote_ref_token() {
            Ok(token) if token == observed_token => {}
            Ok(_) => {
                cache.last_request_generation = None;
                cache.last_source_token = None;
                return unavailable_batch(
                    &request,
                    "local remote-tracking refs changed while checking",
                    None,
                    checked_at,
                    request_revision,
                    revision,
                );
            }
            Err(error) => {
                cache.last_request_generation = None;
                cache.last_source_token = None;
                return unavailable_batch(
                    &request,
                    &format!("local remote refs unavailable: {error}"),
                    None,
                    checked_at,
                    request_revision,
                    revision,
                );
            }
        }
    }
    let deadline = Instant::now() + request_deadline;
    let mut computed = HashMap::<Arc<str>, RemoteRefEvidence>::new();
    for target in &misses {
        if obsolete(request_revision, revision) {
            return None;
        }
        let evidence = if Instant::now() >= deadline {
            RemoteRefEvidence::Unavailable {
                reason: Arc::from("local remote-ref check deadline exceeded"),
                source_token: Some(observed_token),
                checked_at,
            }
        } else if let Some(value) = computed.get(&target.oid) {
            value.clone()
        } else {
            let value = match adapter.containing_remote_ref(&target.oid) {
                Ok(Some(reference)) => RemoteRefEvidence::Contained {
                    reference,
                    source_token: observed_token,
                    checked_at,
                },
                Ok(None) => RemoteRefEvidence::LocalOnly {
                    source_token: observed_token,
                    checked_at,
                },
                Err(error) => RemoteRefEvidence::Unavailable {
                    reason: Arc::from(error.to_string()),
                    source_token: Some(observed_token),
                    checked_at,
                },
            };
            computed.insert(target.oid.clone(), value.clone());
            value
        };
        results.push(UpstreamResult {
            branch: target.branch.clone(),
            oid: target.oid.clone(),
            evidence,
        });
    }
    if obsolete(request_revision, revision) {
        return None;
    }
    match adapter.remote_ref_token() {
        Ok(token) if token == observed_token => {}
        Ok(_) => {
            cache.last_request_generation = None;
            cache.last_source_token = None;
            return unavailable_batch(
                &request,
                "local remote-tracking refs changed while checking",
                None,
                SystemTime::now(),
                request_revision,
                revision,
            );
        }
        Err(error) => {
            cache.last_request_generation = None;
            cache.last_source_token = None;
            return unavailable_batch(
                &request,
                &format!("could not revalidate local remote refs: {error}"),
                Some(observed_token),
                SystemTime::now(),
                request_revision,
                revision,
            );
        }
    }
    for result in &results {
        if !matches!(result.evidence, RemoteRefEvidence::Unavailable { .. }) {
            cache.insert(
                (result.oid.clone(), observed_token),
                result.evidence.clone(),
            );
        }
    }
    cache.last_request_generation = Some(request.generation);
    cache.last_source_token = Some(observed_token);
    results.sort_by(|left, right| left.branch.cmp(&right.branch));
    Some(UpstreamBatch {
        generation: request.generation,
        remote_ref_token: Some(observed_token),
        results: Arc::from(results),
    })
}

trait EvidenceGit: Send + Sync {
    fn remote_ref_token(&self) -> anyhow::Result<u64>;
    fn containing_remote_ref(&self, oid: &str) -> anyhow::Result<Option<Arc<str>>>;
}

impl EvidenceGit for GitAdapter {
    fn remote_ref_token(&self) -> anyhow::Result<u64> {
        GitAdapter::remote_ref_token(self)
    }

    fn containing_remote_ref(&self, oid: &str) -> anyhow::Result<Option<Arc<str>>> {
        GitAdapter::containing_remote_ref(self, oid)
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
        token_calls: AtomicUsize,
        containment_calls: AtomicUsize,
        delay: Duration,
    }

    impl FakeGit {
        fn new(token: u64, delay: Duration) -> Self {
            Self {
                token: AtomicU64::new(token),
                token_calls: AtomicUsize::new(0),
                containment_calls: AtomicUsize::new(0),
                delay,
            }
        }
    }

    impl EvidenceGit for FakeGit {
        fn remote_ref_token(&self) -> anyhow::Result<u64> {
            self.token_calls.fetch_add(1, Ordering::AcqRel);
            Ok(self.token.load(Ordering::Acquire))
        }

        fn containing_remote_ref(&self, oid: &str) -> anyhow::Result<Option<Arc<str>>> {
            self.containment_calls.fetch_add(1, Ordering::AcqRel);
            thread::sleep(self.delay);
            Ok((oid != "local").then(|| Arc::from("origin/main")))
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
                RemoteRefEvidence::LocalOnly {
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
    fn same_generation_cache_hit_spawns_no_more_git_checks() {
        let git = Arc::new(FakeGit::new(7, Duration::ZERO));
        let coordinator =
            UpstreamCoordinator::start_with(git.clone(), Duration::from_millis(100)).unwrap();
        let request = request(3, &["contained"]);
        coordinator.submit(UpstreamCommand::Request(request.clone()));
        assert!(matches!(
            wait_for_result(&coordinator).results[0].evidence,
            RemoteRefEvidence::Contained { .. }
        ));
        let token_calls = git.token_calls.load(Ordering::Acquire);
        let containment_calls = git.containment_calls.load(Ordering::Acquire);
        coordinator.submit(UpstreamCommand::Request(request));
        let _ = wait_for_result(&coordinator);
        assert_eq!(git.token_calls.load(Ordering::Acquire), token_calls);
        assert_eq!(
            git.containment_calls.load(Ordering::Acquire),
            containment_calls
        );
    }

    #[test]
    fn new_generation_reuses_oid_and_token_without_containment_checks() {
        let git = Arc::new(FakeGit::new(7, Duration::ZERO));
        let coordinator =
            UpstreamCoordinator::start_with(git.clone(), Duration::from_millis(100)).unwrap();
        coordinator.submit(UpstreamCommand::Request(request(3, &["contained"])));
        let _ = wait_for_result(&coordinator);
        let containment_calls = git.containment_calls.load(Ordering::Acquire);
        coordinator.submit(UpstreamCommand::Request(request(4, &["contained"])));
        let batch = wait_for_result(&coordinator);
        assert_eq!(batch.generation, 4);
        assert_eq!(
            git.containment_calls.load(Ordering::Acquire),
            containment_calls
        );
    }

    #[test]
    fn cancel_discards_an_active_result() {
        let git = Arc::new(FakeGit::new(9, Duration::from_millis(60)));
        let coordinator =
            UpstreamCoordinator::start_with(git.clone(), Duration::from_millis(200)).unwrap();
        coordinator.submit(UpstreamCommand::Request(request(1, &["old"])));
        wait_for_calls(&git.containment_calls, 1);
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
        wait_for_calls(&git.containment_calls, 1);
        coordinator.submit(UpstreamCommand::Request(request(2, &["new"])));
        let batch = wait_for_result(&coordinator);
        assert_eq!(batch.generation, 2);
        assert!(coordinator.try_result().is_none());
    }

    #[test]
    fn source_token_change_during_work_never_reports_local_only() {
        let git = Arc::new(FakeGit::new(4, Duration::from_millis(50)));
        let coordinator =
            UpstreamCoordinator::start_with(git.clone(), Duration::from_millis(250)).unwrap();
        coordinator.submit(UpstreamCommand::Request(request(1, &["local"])));
        wait_for_calls(&git.containment_calls, 1);
        git.token.store(5, Ordering::Release);
        let batch = wait_for_result(&coordinator);
        assert!(matches!(
            batch.results[0].evidence,
            RemoteRefEvidence::Unavailable { .. }
        ));
    }

    #[test]
    fn working_set_and_deadline_bound_slow_checks() {
        let git = Arc::new(FakeGit::new(2, Duration::from_millis(15)));
        let coordinator =
            UpstreamCoordinator::start_with(git.clone(), Duration::from_millis(40)).unwrap();
        let request = UpstreamRequest {
            generation: 1,
            targets: (0..100)
                .map(|index| UpstreamTarget {
                    branch: BranchId::new(format!("branch-{index}")),
                    oid: Arc::from(format!("oid-{index}")),
                })
                .collect(),
        };
        coordinator.submit(UpstreamCommand::Request(request));
        let batch = wait_for_result(&coordinator);
        assert_eq!(batch.results.len(), MAX_TARGETS);
        assert!(git.containment_calls.load(Ordering::Acquire) <= 4);
    }

    #[test]
    fn result_queue_discards_oldest_batches_at_capacity() {
        let git = Arc::new(FakeGit::new(1, Duration::ZERO));
        let coordinator =
            UpstreamCoordinator::start_with(git.clone(), Duration::from_millis(100)).unwrap();
        for generation in 1..=(RESULT_CAPACITY as u64 + 3) {
            let oid = format!("oid-{generation}");
            coordinator.submit(UpstreamCommand::Request(request(generation, &[&oid])));
            wait_for_calls(&git.containment_calls, generation as usize);
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
