use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const QUIET_PERIOD: Duration = Duration::from_millis(75);

pub(super) struct RepositoryWatcher {
    watcher: Option<RecommendedWatcher>,
    stop: mpsc::SyncSender<()>,
    worker: Option<JoinHandle<()>>,
}

pub(super) fn watch_repository(
    git_dir: &Path,
    common_dir: &Path,
    requester: super::RefreshRequester,
) -> notify::Result<RepositoryWatcher> {
    let git_dir = git_dir.to_path_buf();
    let common_dir = common_dir.to_path_buf();
    let (events_send, events_receive) = mpsc::sync_channel(1);
    let (stop_send, stop_receive) = mpsc::sync_channel(1);
    let callback_git_dir = git_dir.clone();
    let callback_common_dir = common_dir.clone();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if event.as_ref().is_ok_and(|event| {
            event
                .paths
                .iter()
                .any(|path| relevant(path, &callback_git_dir, &callback_common_dir))
        }) {
            let _ = events_send.try_send(());
        }
    })?;
    watcher.watch(&git_dir, RecursiveMode::Recursive)?;
    if common_dir != git_dir {
        watcher.watch(&common_dir, RecursiveMode::Recursive)?;
    }
    let worker = thread::Builder::new()
        .name("stackmap-watcher".into())
        .spawn(move || run_worker(events_receive, stop_receive, requester))
        .map_err(notify::Error::io)?;
    Ok(RepositoryWatcher {
        watcher: Some(watcher),
        stop: stop_send,
        worker: Some(worker),
    })
}

fn run_worker(
    events_receive: mpsc::Receiver<()>,
    stop_receive: mpsc::Receiver<()>,
    requester: super::RefreshRequester,
) {
    loop {
        match stop_receive.try_recv() {
            Ok(()) | Err(TryRecvError::Disconnected) => break,
            Err(TryRecvError::Empty) => {}
        }
        match events_receive.recv_timeout(Duration::from_millis(100)) {
            Ok(()) => {}
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        }
        // Refresh on the leading edge. Further events update the
        // coalesced request epoch while this burst is settling.
        requester.request();
        loop {
            match events_receive.recv_timeout(QUIET_PERIOD) {
                Ok(()) => {
                    requester.request();
                }
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        match stop_receive.try_recv() {
            Ok(()) | Err(TryRecvError::Disconnected) => break,
            Err(TryRecvError::Empty) => {}
        }
    }
}

fn relevant(path: &Path, git_dir: &Path, common_dir: &Path) -> bool {
    let relative = path
        .strip_prefix(git_dir)
        .or_else(|_| path.strip_prefix(common_dir))
        .unwrap_or(path);
    let components: Vec<_> = relative.iter().collect();
    let first = components.first().and_then(|part| part.to_str());
    let file = relative.file_name().and_then(|part| part.to_str());

    if file.is_some_and(|name| name.ends_with(".lock") || name.ends_with(".tmp")) {
        return false;
    }
    matches!(
        first,
        Some("refs" | "worktrees" | "rebase-apply" | "rebase-merge")
    ) || matches!(
        file,
        Some(
            "HEAD"
                | "ORIG_HEAD"
                | "MERGE_HEAD"
                | "CHERRY_PICK_HEAD"
                | "REVERT_HEAD"
                | "BISECT_LOG"
                | "packed-refs"
                | "index"
                | "config"
                | ".graphite_repo_config"
                | ".graphite_metadata.db"
                | ".graphite_metadata.db-wal"
                | ".graphite_metadata.db-shm"
        )
    ) || is_stackmap_config(relative)
}

fn is_stackmap_config(path: &Path) -> bool {
    path == Path::new("stackmap/config.toml")
}

impl Drop for RepositoryWatcher {
    fn drop(&mut self) {
        self.watcher.take();
        let _ = self.stop.try_send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU64;

    use super::*;

    fn requester() -> (
        super::super::RefreshRequester,
        mpsc::Receiver<super::super::Request>,
    ) {
        let (send, receive) = mpsc::sync_channel(1);
        (
            super::super::RefreshRequester {
                requests: send,
                next_epoch: std::sync::Arc::new(AtomicU64::new(0)),
                pending_epoch: std::sync::Arc::new(AtomicU64::new(0)),
            },
            receive,
        )
    }

    #[test]
    fn ignores_transient_and_object_store_activity() {
        let git = Path::new("/repo/.git");
        assert!(!relevant(Path::new("/repo/.git/index.lock"), git, git));
        assert!(!relevant(Path::new("/repo/.git/objects/ab/cdef"), git, git));
        assert!(relevant(Path::new("/repo/.git/index"), git, git));
        assert!(relevant(
            Path::new("/repo/.git/refs/heads/feature"),
            git,
            git
        ));
    }

    #[test]
    fn worker_exits_when_the_filesystem_event_channel_disconnects() {
        let (events_send, events_receive) = mpsc::sync_channel(1);
        let (stop_send, stop_receive) = mpsc::sync_channel(1);
        let (done_send, done_receive) = mpsc::sync_channel(1);
        let (requester, _requests) = requester();
        drop(events_send);

        let worker = thread::spawn(move || {
            run_worker(events_receive, stop_receive, requester);
            let _ = done_send.send(());
        });

        let result = done_receive.recv_timeout(Duration::from_millis(250));
        let _ = stop_send.send(());
        worker.join().unwrap();
        assert_eq!(result, Ok(()));
    }
}
