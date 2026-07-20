pub mod builder;
pub mod diffstats;
pub mod upstream;
pub mod watcher;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

use crate::adapters::git::GitAdapter;
use crate::model::RepositorySnapshot;

use self::builder::SnapshotBuilder;
use self::diffstats::DiffCache;
use self::upstream::{UpstreamBatch, UpstreamCommand, UpstreamCoordinator};

#[derive(Debug)]
pub enum RefreshEvent {
    Structural {
        request_epoch: u64,
        snapshot: Arc<RepositorySnapshot>,
    },
    Enriched(Arc<RepositorySnapshot>),
    Upstream(UpstreamBatch),
    Failed(Arc<str>),
}

#[derive(Clone, Copy, Debug)]
enum Request {
    Refresh,
    Shutdown,
}

pub struct RefreshHandle {
    requester: RefreshRequester,
    events: Arc<Mutex<VecDeque<RefreshEvent>>>,
    workers: Vec<JoinHandle<()>>,
    diff_state: Arc<(Mutex<DiffCoordinator>, Condvar)>,
    upstream: UpstreamCoordinator,
    shutdown: Arc<AtomicBool>,
    watcher: Option<watcher::RepositoryWatcher>,
}

#[derive(Clone)]
pub(super) struct RefreshRequester {
    requests: SyncSender<Request>,
    next_epoch: Arc<AtomicU64>,
    pending_epoch: Arc<AtomicU64>,
}

impl RefreshRequester {
    fn request(&self) -> u64 {
        let epoch = self.next_epoch.fetch_add(1, Ordering::AcqRel) + 1;
        self.pending_epoch.fetch_max(epoch, Ordering::AcqRel);
        match self.requests.try_send(Request::Refresh) {
            Ok(()) | Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {}
        }
        epoch
    }
}

#[derive(Default)]
struct DiffCoordinator {
    latest: Option<Arc<RepositorySnapshot>>,
    shutdown: bool,
}

impl RefreshHandle {
    pub fn start(start_dir: PathBuf) -> anyhow::Result<Self> {
        let adapter = GitAdapter::discover(&start_dir)?;
        let initial = adapter.inventory()?;
        let (request_send, request_receive) = mpsc::sync_channel(1);
        let pending_epoch = Arc::new(AtomicU64::new(0));
        let requester = RefreshRequester {
            requests: request_send,
            next_epoch: Arc::new(AtomicU64::new(0)),
            pending_epoch: pending_epoch.clone(),
        };
        let events = Arc::new(Mutex::new(VecDeque::with_capacity(8)));
        let diff_state = Arc::new((Mutex::new(DiffCoordinator::default()), Condvar::new()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let latest_generation = Arc::new(AtomicU64::new(0));
        let upstream = UpstreamCoordinator::start(adapter.clone())?;
        let structural_worker = thread::Builder::new()
            .name("stackmap-refresh".into())
            .spawn({
                let adapter = adapter.clone();
                let events = events.clone();
                let diff_state = diff_state.clone();
                let shutdown = shutdown.clone();
                let latest_generation = latest_generation.clone();
                move || {
                    let mut builder = SnapshotBuilder::new(adapter);
                    while let Ok(Request::Refresh) = request_receive.recv() {
                        if shutdown.load(Ordering::Acquire) {
                            break;
                        }
                        let request_epoch = pending_epoch.swap(0, Ordering::AcqRel);
                        if request_epoch == 0 {
                            continue;
                        }
                        match builder.build() {
                            Ok(structural) => {
                                latest_generation.store(structural.generation, Ordering::Release);
                                push_event(
                                    &events,
                                    RefreshEvent::Structural {
                                        request_epoch,
                                        snapshot: structural.clone(),
                                    },
                                );
                                let (state, wake) = &*diff_state;
                                match state.lock() {
                                    Ok(mut state) => {
                                        state.latest = Some(structural);
                                        wake.notify_one();
                                    }
                                    Err(_) => push_event(
                                        &events,
                                        RefreshEvent::Failed(Arc::from(
                                            "diff coordinator stopped after internal state failure",
                                        )),
                                    ),
                                }
                            }
                            Err(error) => {
                                push_event(
                                    &events,
                                    RefreshEvent::Failed(Arc::from(error.to_string())),
                                );
                            }
                        }
                    }
                }
            })?;
        let diff_worker = thread::Builder::new()
            .name("stackmap-diff-coordinator".into())
            .spawn({
                let adapter = adapter.clone();
                let events = events.clone();
                let diff_state = diff_state.clone();
                let latest_generation = latest_generation.clone();
                move || {
                    let mut cache = DiffCache::new(2048);
                    'worker: loop {
                        let snapshot = {
                            let (state, wake) = &*diff_state;
                            let mut state = match state.lock() {
                                Ok(state) => state,
                                Err(_) => {
                                    push_event(
                                        &events,
                                        RefreshEvent::Failed(Arc::from(
                                            "diff coordinator stopped after internal state failure",
                                        )),
                                    );
                                    break 'worker;
                                }
                            };
                            while state.latest.is_none() && !state.shutdown {
                                state = match wake.wait(state) {
                                    Ok(state) => state,
                                    Err(_) => {
                                        push_event(
                                            &events,
                                            RefreshEvent::Failed(Arc::from(
                                                "diff coordinator stopped after internal state failure",
                                            )),
                                        );
                                        break 'worker;
                                    }
                                };
                            }
                            if state.shutdown {
                                break 'worker;
                            }
                            let Some(snapshot) = state.latest.take() else {
                                continue;
                            };
                            snapshot
                        };
                        if let Some(enriched) =
                            diffstats::enrich(snapshot, &adapter, &mut cache, 4, &latest_generation)
                        {
                            push_event(&events, RefreshEvent::Enriched(enriched));
                        }
                    }
                }
            })?;
        let watcher =
            watcher::watch_repository(&initial.git_dir, &initial.common_dir, requester.clone())
                .ok();
        let handle = Self {
            requester,
            events,
            workers: vec![structural_worker, diff_worker],
            diff_state,
            upstream,
            shutdown,
            watcher,
        };
        handle.request();
        Ok(handle)
    }

    pub fn request(&self) -> u64 {
        self.requester.request()
    }

    pub fn try_event(&self) -> Option<RefreshEvent> {
        self.events
            .lock()
            .ok()?
            .pop_front()
            .or_else(|| self.upstream.try_result().map(RefreshEvent::Upstream))
    }

    pub fn request_upstream(&self, command: UpstreamCommand) {
        self.upstream.submit(command);
    }
}

fn push_event(events: &Mutex<VecDeque<RefreshEvent>>, event: RefreshEvent) {
    let Ok(mut events) = events.lock() else {
        return;
    };
    if events.len() == 8 {
        events.pop_front();
    }
    events.push_back(event);
}

impl Drop for RefreshHandle {
    fn drop(&mut self) {
        self.watcher.take();
        self.shutdown.store(true, Ordering::Release);
        let (state, wake) = &*self.diff_state;
        if let Ok(mut state) = state.lock() {
            state.shutdown = true;
        }
        wake.notify_all();
        let _ = self.requester.requests.try_send(Request::Shutdown);
        crate::adapters::command::terminate_active_commands();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalesced_requests_retain_the_latest_causal_epoch() {
        let (requests, receiver) = mpsc::sync_channel(1);
        let pending_epoch = Arc::new(AtomicU64::new(0));
        let requester = RefreshRequester {
            requests,
            next_epoch: Arc::new(AtomicU64::new(0)),
            pending_epoch: pending_epoch.clone(),
        };

        assert_eq!(requester.request(), 1);
        assert_eq!(requester.request(), 2);
        assert!(matches!(receiver.recv().unwrap(), Request::Refresh));
        assert_eq!(pending_epoch.swap(0, Ordering::AcqRel), 2);
    }
}
