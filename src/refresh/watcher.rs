use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

const QUIET_PERIOD: Duration = Duration::from_millis(200);

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
        .spawn(move || {
            loop {
                if stop_receive.try_recv().is_ok() {
                    break;
                }
                if events_receive
                    .recv_timeout(Duration::from_millis(100))
                    .is_err()
                {
                    continue;
                }
                while events_receive.recv_timeout(QUIET_PERIOD).is_ok() {}
                if stop_receive.try_recv().is_ok() {
                    break;
                }
                requester.request();
            }
        })
        .map_err(notify::Error::io)?;
    Ok(RepositoryWatcher {
        watcher: Some(watcher),
        stop: stop_send,
        worker: Some(worker),
    })
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
    use super::*;

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
}
