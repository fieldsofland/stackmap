use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::adapters::github::{self, GitHubError, OpenPullRequests, PrMatch};
use crate::model::BranchId;

// Historical PR lookup is deliberately a small demand-driven working set.
// Open PR discovery remains repository-wide in one provider call.
pub const MAX_TARGETS: usize = 8;
const RESULT_CAPACITY: usize = 40;
const RESULT_BATCH_SIZE: usize = 32;
const TARGETS_PER_CYCLE: usize = 4;
const TARGET_REQUESTS_PER_CYCLE: usize = TARGETS_PER_CYCLE * 2;
const TARGET_CYCLE_DEADLINE: Duration = Duration::from_secs(12);
const POSITIVE_TTL: Duration = Duration::from_secs(300);
const NEGATIVE_TTL: Duration = Duration::from_secs(120);
const ERROR_TTL: Duration = Duration::from_secs(15);
const OPEN_TTL: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GithubTarget {
    pub branch: BranchId,
    pub oid: Arc<str>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GithubRequest {
    pub generation: u64,
    pub targets: Arc<[GithubTarget]>,
    pub discovery: Arc<[GithubTarget]>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GithubOutcome {
    Ready(Arc<[PrMatch]>),
    NoMatch,
    Unavailable(GitHubError),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GithubResult {
    pub branch: BranchId,
    pub oid: Arc<str>,
    pub outcome: GithubOutcome,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GithubBatch {
    pub generation: u64,
    pub results: Arc<[GithubResult]>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GithubCommand {
    Request(GithubRequest),
    Cancel,
}

trait GithubProvider: Send + Sync {
    fn repository_slug(&self) -> Result<Arc<str>, GitHubError>;
    fn open(&self) -> Result<OpenPullRequests, GitHubError>;
    fn head(
        &self,
        repository: &str,
        owner: &str,
        branch: &BranchId,
        timeout: Duration,
    ) -> Result<Vec<PrMatch>, GitHubError>;
    fn commit(
        &self,
        repository: &str,
        oid: &str,
        timeout: Duration,
    ) -> Result<Vec<PrMatch>, GitHubError>;
}

struct CliProvider {
    cwd: PathBuf,
}

impl CliProvider {
    fn new(cwd: &Path) -> Self {
        Self {
            cwd: cwd.to_owned(),
        }
    }
}

impl GithubProvider for CliProvider {
    fn repository_slug(&self) -> Result<Arc<str>, GitHubError> {
        github::repository_slug(&self.cwd)
    }

    fn open(&self) -> Result<OpenPullRequests, GitHubError> {
        github::fetch_open(&self.cwd)
    }

    fn head(
        &self,
        repository: &str,
        owner: &str,
        branch: &BranchId,
        timeout: Duration,
    ) -> Result<Vec<PrMatch>, GitHubError> {
        github::fetch_head_with_timeout(&self.cwd, repository, owner, branch, timeout)
    }

    fn commit(
        &self,
        repository: &str,
        oid: &str,
        timeout: Duration,
    ) -> Result<Vec<PrMatch>, GitHubError> {
        github::fetch_commit_with_timeout(&self.cwd, repository, oid, timeout)
    }
}

#[derive(Default)]
struct CoordinatorState {
    latest: Option<GithubRequest>,
    updated: bool,
    shutdown: bool,
}

#[derive(Clone)]
struct CacheEntry {
    outcome: GithubOutcome,
    expires_at: Instant,
}

#[derive(Default)]
struct GithubCache {
    targets: HashMap<(BranchId, Arc<str>), CacheEntry>,
    open: Option<(Instant, OpenPullRequests)>,
    repository: Option<Arc<str>>,
}

pub struct GithubCoordinator {
    state: Arc<(Mutex<CoordinatorState>, Condvar)>,
    revision: Arc<AtomicU64>,
    results: Arc<Mutex<VecDeque<GithubBatch>>>,
    worker: Option<JoinHandle<()>>,
}

impl GithubCoordinator {
    pub fn start(cwd: &Path) -> std::io::Result<Self> {
        Self::start_with(Arc::new(CliProvider::new(cwd)))
    }

    fn start_with(provider: Arc<dyn GithubProvider>) -> std::io::Result<Self> {
        let state = Arc::new((Mutex::new(CoordinatorState::default()), Condvar::new()));
        let revision = Arc::new(AtomicU64::new(0));
        let results = Arc::new(Mutex::new(VecDeque::with_capacity(RESULT_CAPACITY)));
        let worker = thread::Builder::new()
            .name("stackmap-github".into())
            .spawn({
                let state = state.clone();
                let revision = revision.clone();
                let results = results.clone();
                move || {
                    let mut cache = GithubCache::default();
                    loop {
                        let (request, request_revision) = {
                            let (state, wake) = &*state;
                            let Ok(mut state) = state.lock() else {
                                break;
                            };
                            while !state.updated && !state.shutdown {
                                let Ok(next) = wake.wait(state) else {
                                    return;
                                };
                                state = next;
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
                        let continuation = process_cycle(
                            request,
                            provider.as_ref(),
                            &mut cache,
                            request_revision,
                            &revision,
                            |batch| push_result(&results, batch),
                        );
                        if let Some(continuation) = continuation {
                            let (state, wake) = &*state;
                            let Ok(mut state) = state.lock() else {
                                break;
                            };
                            if !state.updated
                                && !state.shutdown
                                && revision.load(Ordering::Acquire) == request_revision
                            {
                                state.latest = Some(continuation);
                                state.updated = true;
                                wake.notify_one();
                            }
                        }
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

    pub fn submit(&self, command: GithubCommand) {
        self.revision.fetch_add(1, Ordering::AcqRel);
        let (state, wake) = &*self.state;
        let Ok(mut state) = state.lock() else {
            return;
        };
        state.latest = match command {
            GithubCommand::Request(mut request) => {
                if request.targets.len() > MAX_TARGETS {
                    request.targets = Arc::from(&request.targets[..MAX_TARGETS]);
                }
                Some(request)
            }
            GithubCommand::Cancel => None,
        };
        state.updated = true;
        wake.notify_one();
    }

    pub fn try_result(&self) -> Option<GithubBatch> {
        self.results.lock().ok()?.pop_front()
    }
}

impl Drop for GithubCoordinator {
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

fn process_cycle(
    request: GithubRequest,
    provider: &dyn GithubProvider,
    cache: &mut GithubCache,
    request_revision: u64,
    revision: &AtomicU64,
    mut publish: impl FnMut(GithubBatch),
) -> Option<GithubRequest> {
    let repository = match cache
        .repository
        .clone()
        .map(Ok)
        .unwrap_or_else(|| provider.repository_slug())
    {
        Ok(repository) => {
            cache.repository = Some(repository.clone());
            repository
        }
        Err(error) => {
            publish_results(
                request.generation,
                request.targets.iter().map(|target| GithubResult {
                    branch: target.branch.clone(),
                    oid: target.oid.clone(),
                    outcome: GithubOutcome::Unavailable(error.clone()),
                }),
                &mut publish,
            );
            return None;
        }
    };
    let owner = repository.split('/').next().unwrap_or_default();
    let now = Instant::now();
    let open_is_fresh = cache
        .open
        .as_ref()
        .is_some_and(|(expires_at, _)| *expires_at > now);
    if !open_is_fresh {
        if let Ok(open) = provider.open() {
            cache.open = Some((now + OPEN_TTL, open));
        }
    }
    let open = cache
        .open
        .as_ref()
        .filter(|(expires_at, _)| *expires_at > now)
        .map(|(_, open)| open);

    if let Some(open) = open {
        let discovered = request.discovery.iter().filter_map(|target| {
            let matches = open
                .matches
                .iter()
                .filter(|candidate| candidate.branch == target.branch)
                .cloned()
                .collect::<Vec<_>>();
            (!matches.is_empty()).then(|| GithubResult {
                branch: target.branch.clone(),
                oid: target.oid.clone(),
                outcome: GithubOutcome::Ready(Arc::from(matches)),
            })
        });
        publish_results(request.generation, discovered, &mut publish);
    }

    let cycle_deadline = Instant::now() + TARGET_CYCLE_DEADLINE;
    let mut completed = Vec::new();
    let mut targeted = Vec::new();
    for target in request.targets.iter() {
        if revision.load(Ordering::Acquire) != request_revision {
            return None;
        }
        let key = (target.branch.clone(), target.oid.clone());
        let outcome = if let Some(entry) = cache
            .targets
            .get(&key)
            .filter(|entry| entry.expires_at > now)
        {
            entry.outcome.clone()
        } else {
            let open_matches = open
                .as_ref()
                .map(|open| {
                    open.matches
                        .iter()
                        .filter(|candidate| candidate.branch == target.branch)
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if open_matches
                .iter()
                .any(|candidate| candidate.head_oid.as_deref() == Some(target.oid.as_ref()))
            {
                GithubOutcome::Ready(Arc::from(open_matches))
            } else {
                targeted.push((target.clone(), open_matches));
                continue;
            }
        };
        completed.push(GithubResult {
            branch: target.branch.clone(),
            oid: target.oid.clone(),
            outcome,
        });
    }
    publish_results(request.generation, completed.into_iter(), &mut publish);

    let mut remaining = Vec::new();
    let mut requests_used = 0usize;
    let mut targets_used = 0usize;
    let mut pending_results = Vec::with_capacity(RESULT_BATCH_SIZE);
    for (target, open_matches) in targeted {
        if revision.load(Ordering::Acquire) != request_revision {
            return None;
        }
        if targets_used >= TARGETS_PER_CYCLE
            || requests_used >= TARGET_REQUESTS_PER_CYCLE
            || Instant::now() >= cycle_deadline
        {
            remaining.push(target);
            continue;
        }
        let Some((outcome, used)) = targeted_outcome(
            provider,
            &repository,
            owner,
            &target,
            open_matches,
            cycle_deadline,
        ) else {
            remaining.push(target);
            continue;
        };
        targets_used += 1;
        requests_used += used;
        let ttl = match outcome {
            GithubOutcome::Ready(_) => POSITIVE_TTL,
            GithubOutcome::NoMatch => NEGATIVE_TTL,
            GithubOutcome::Unavailable(_) => ERROR_TTL,
        };
        cache.targets.insert(
            (target.branch.clone(), target.oid.clone()),
            CacheEntry {
                outcome: outcome.clone(),
                expires_at: now + ttl,
            },
        );
        pending_results.push(GithubResult {
            branch: target.branch.clone(),
            oid: target.oid.clone(),
            outcome,
        });
        if pending_results.len() == RESULT_BATCH_SIZE {
            publish(GithubBatch {
                generation: request.generation,
                results: Arc::from(std::mem::take(&mut pending_results)),
            });
        }
    }
    if !pending_results.is_empty() {
        publish(GithubBatch {
            generation: request.generation,
            results: Arc::from(pending_results),
        });
    }
    (!remaining.is_empty()).then(|| GithubRequest {
        generation: request.generation,
        targets: Arc::from(remaining),
        discovery: Arc::from([]),
    })
}

fn targeted_outcome(
    provider: &dyn GithubProvider,
    repository: &str,
    owner: &str,
    target: &GithubTarget,
    mut known: Vec<PrMatch>,
    deadline: Instant,
) -> Option<(GithubOutcome, usize)> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return None;
    }
    let head = provider.head(repository, owner, &target.branch, remaining);
    if let Ok(matches) = &head {
        known.extend(matches.iter().cloned());
    }
    if known
        .iter()
        .any(|candidate| candidate.head_oid.as_deref() == Some(target.oid.as_ref()))
    {
        return Some((GithubOutcome::Ready(Arc::from(known)), 1));
    }
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return None;
    }
    let commit = provider.commit(repository, &target.oid, remaining);
    if let Ok(matches) = &commit {
        known.extend(matches.iter().cloned());
    }
    known.sort_by_key(|candidate| candidate.pull_request.number);
    known.dedup_by_key(|candidate| candidate.pull_request.number);
    let outcome = if !known.is_empty() {
        GithubOutcome::Ready(Arc::from(known))
    } else {
        match (head, commit) {
            (Ok(_), Ok(_)) => GithubOutcome::NoMatch,
            (Err(error), _) | (_, Err(error)) => GithubOutcome::Unavailable(error),
        }
    };
    Some((outcome, 2))
}

fn publish_results(
    generation: u64,
    results: impl Iterator<Item = GithubResult>,
    publish: &mut impl FnMut(GithubBatch),
) {
    let mut batch = Vec::with_capacity(RESULT_BATCH_SIZE);
    for result in results {
        batch.push(result);
        if batch.len() == RESULT_BATCH_SIZE {
            publish(GithubBatch {
                generation,
                results: Arc::from(std::mem::take(&mut batch)),
            });
        }
    }
    if !batch.is_empty() {
        publish(GithubBatch {
            generation,
            results: Arc::from(batch),
        });
    }
}

fn push_result(results: &Mutex<VecDeque<GithubBatch>>, batch: GithubBatch) {
    let Ok(mut results) = results.lock() else {
        return;
    };
    if results.len() == RESULT_CAPACITY {
        results.pop_front();
    }
    results.push_back(batch);
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::model::{PullRequest, PullRequestMatch, PullRequestStatus};

    use super::*;

    struct FakeProvider {
        head_calls: AtomicUsize,
        commit_calls: AtomicUsize,
    }

    impl FakeProvider {
        fn new() -> Self {
            Self {
                head_calls: AtomicUsize::new(0),
                commit_calls: AtomicUsize::new(0),
            }
        }
    }

    impl GithubProvider for FakeProvider {
        fn repository_slug(&self) -> Result<Arc<str>, GitHubError> {
            Ok(Arc::from("owner/repo"))
        }

        fn open(&self) -> Result<OpenPullRequests, GitHubError> {
            Ok(OpenPullRequests {
                matches: Vec::new(),
                saturated: true,
            })
        }

        fn head(
            &self,
            _repository: &str,
            _owner: &str,
            _branch: &BranchId,
            _timeout: Duration,
        ) -> Result<Vec<PrMatch>, GitHubError> {
            self.head_calls.fetch_add(1, Ordering::Relaxed);
            Ok(Vec::new())
        }

        fn commit(
            &self,
            _repository: &str,
            _oid: &str,
            _timeout: Duration,
        ) -> Result<Vec<PrMatch>, GitHubError> {
            self.commit_calls.fetch_add(1, Ordering::Relaxed);
            Ok(Vec::new())
        }
    }

    fn target(index: usize) -> GithubTarget {
        GithubTarget {
            branch: BranchId::new(format!("branch-{index}")),
            oid: Arc::from(format!("{index:040x}")),
        }
    }

    #[test]
    fn open_sweep_publishes_prs_for_branches_outside_targeted_working_set() {
        struct OpenProvider;
        impl GithubProvider for OpenProvider {
            fn repository_slug(&self) -> Result<Arc<str>, GitHubError> {
                Ok(Arc::from("owner/repo"))
            }
            fn open(&self) -> Result<OpenPullRequests, GitHubError> {
                let head_oid = Some(Arc::from("outside-oid"));
                Ok(OpenPullRequests {
                    matches: vec![PrMatch {
                        branch: BranchId::new("outside"),
                        head_oid: head_oid.clone(),
                        updated_at: Arc::from("2026-08-24T00:00:00Z"),
                        pull_request: PullRequest {
                            number: 4999,
                            title: Arc::from("Outside current stack"),
                            url: Arc::from("https://example.invalid/4999"),
                            status: PullRequestStatus::Open,
                            head_oid,
                            match_quality: PullRequestMatch::StaleTip,
                        },
                    }],
                    saturated: false,
                })
            }
            fn head(
                &self,
                _repository: &str,
                _owner: &str,
                _branch: &BranchId,
                _timeout: Duration,
            ) -> Result<Vec<PrMatch>, GitHubError> {
                Ok(Vec::new())
            }
            fn commit(
                &self,
                _repository: &str,
                _oid: &str,
                _timeout: Duration,
            ) -> Result<Vec<PrMatch>, GitHubError> {
                Ok(Vec::new())
            }
        }

        let revision = AtomicU64::new(1);
        let mut cache = GithubCache::default();
        let mut batches = Vec::new();
        process_cycle(
            GithubRequest {
                generation: 1,
                targets: Arc::from([target(0)]),
                discovery: Arc::from([GithubTarget {
                    branch: BranchId::new("outside"),
                    oid: Arc::from("outside-oid"),
                }]),
            },
            &OpenProvider,
            &mut cache,
            1,
            &revision,
            |batch| batches.push(batch),
        );

        let result = batches
            .iter()
            .flat_map(|batch| batch.results.iter())
            .find(|result| result.branch == BranchId::new("outside"))
            .expect("open PR outside the targeted working set");
        assert_eq!(result.oid.as_ref(), "outside-oid");
        assert!(matches!(result.outcome, GithubOutcome::Ready(_)));
    }

    #[test]
    fn bounded_working_set_continues_fairly_across_cycles() {
        let provider = FakeProvider::new();
        let targets = (0..MAX_TARGETS).map(target).collect::<Vec<_>>();
        let revision = AtomicU64::new(1);
        let mut cache = GithubCache::default();
        let mut batches = Vec::new();
        let first_continuation = process_cycle(
            GithubRequest {
                generation: 9,
                targets: Arc::from(targets),
                discovery: Arc::from([]),
            },
            &provider,
            &mut cache,
            1,
            &revision,
            |batch| batches.push(batch),
        );

        assert_eq!(
            provider.head_calls.load(Ordering::Relaxed),
            TARGETS_PER_CYCLE
        );
        assert_eq!(
            provider.commit_calls.load(Ordering::Relaxed),
            TARGETS_PER_CYCLE
        );
        assert_eq!(first_continuation.as_ref().unwrap().targets.len(), 4);

        let mut continuation = first_continuation;
        while let Some(request) = continuation {
            continuation = process_cycle(request, &provider, &mut cache, 1, &revision, |batch| {
                batches.push(batch)
            });
        }
        assert!(
            batches
                .iter()
                .all(|batch| batch.results.len() <= RESULT_BATCH_SIZE)
        );
        assert!(
            batches
                .iter()
                .flat_map(|batch| batch.results.iter())
                .any(|result| { result.branch == BranchId::new("branch-7") })
        );
    }

    #[test]
    fn positive_cache_avoids_repeating_targeted_calls() {
        struct MatchingProvider(AtomicUsize);
        impl GithubProvider for MatchingProvider {
            fn repository_slug(&self) -> Result<Arc<str>, GitHubError> {
                Ok(Arc::from("owner/repo"))
            }
            fn open(&self) -> Result<OpenPullRequests, GitHubError> {
                Ok(OpenPullRequests {
                    matches: Vec::new(),
                    saturated: false,
                })
            }
            fn head(
                &self,
                _repository: &str,
                _owner: &str,
                branch: &BranchId,
                _timeout: Duration,
            ) -> Result<Vec<PrMatch>, GitHubError> {
                self.0.fetch_add(1, Ordering::Relaxed);
                Ok(vec![PrMatch {
                    branch: branch.clone(),
                    head_oid: Some(Arc::from("abcd")),
                    updated_at: Arc::from("2026-01-01T00:00:00Z"),
                    pull_request: PullRequest {
                        number: 1,
                        title: Arc::from("PR"),
                        url: Arc::from("https://example.invalid/1"),
                        status: PullRequestStatus::Open,
                        head_oid: Some(Arc::from("abcd")),
                        match_quality: PullRequestMatch::StaleTip,
                    },
                }])
            }
            fn commit(
                &self,
                _repository: &str,
                _oid: &str,
                _timeout: Duration,
            ) -> Result<Vec<PrMatch>, GitHubError> {
                Ok(Vec::new())
            }
        }
        let provider = MatchingProvider(AtomicUsize::new(0));
        let request = GithubRequest {
            generation: 1,
            targets: Arc::from([GithubTarget {
                branch: BranchId::new("topic"),
                oid: Arc::from("abcd"),
            }]),
            discovery: Arc::from([]),
        };
        let revision = AtomicU64::new(1);
        let mut cache = GithubCache::default();
        process_cycle(request.clone(), &provider, &mut cache, 1, &revision, |_| {});
        process_cycle(request, &provider, &mut cache, 1, &revision, |_| {});
        assert_eq!(provider.0.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn continuation_queue_retains_earliest_bounded_outcome_until_drained() {
        let queue = Mutex::new(VecDeque::new());
        let cycles = MAX_TARGETS.div_ceil(TARGETS_PER_CYCLE);
        for generation in 0..cycles as u64 {
            push_result(
                &queue,
                GithubBatch {
                    generation,
                    results: Arc::from([GithubResult {
                        branch: BranchId::new(format!("branch-{generation}")),
                        oid: Arc::from("abcd"),
                        outcome: GithubOutcome::NoMatch,
                    }]),
                },
            );
        }
        let queue = queue.lock().unwrap();
        assert_eq!(queue.len(), cycles);
        assert_eq!(queue.front().unwrap().generation, 0);
    }
}
