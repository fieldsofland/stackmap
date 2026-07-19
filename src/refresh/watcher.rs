use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;

pub(super) fn watch_repository(
    git_dir: &Path,
    common_dir: &Path,
    requester: super::RefreshRequester,
) -> notify::Result<RecommendedWatcher> {
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if event.is_ok() {
            requester.request();
        }
    })?;
    watcher.watch(git_dir, RecursiveMode::Recursive)?;
    if common_dir != git_dir {
        watcher.watch(common_dir, RecursiveMode::Recursive)?;
    }
    Ok(watcher)
}
