use std::ffi::{OsStr, OsString};
use std::io::{self, Write, stdout};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result};
use crossterm::event::{
    self, Event, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::queue;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use stackmap::runtime::git::{DeleteOutcome, DeleteRequest, GitAdapter};
use stackmap::runtime::{
    Action, App, ArchiveMode, BranchId, Config, ConfigWriteRequest, Input, Key, RefreshEvent,
    RefreshHandle, github, platform, render,
};

const RECONCILE_INTERVAL: Duration = Duration::from_secs(300);
const GITHUB_TTL: Duration = Duration::from_secs(30);
const CONFIG_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

enum PlatformResult {
    Open(Result<()>),
    Copy(Result<()>),
}

struct ConfigPersistenceResult {
    sequence: u64,
    result: Result<()>,
}

#[derive(Default)]
struct ConfigPersistenceQueue {
    active: Option<ConfigWriteRequest>,
    pending: Option<ConfigWriteRequest>,
    last_request: Option<ConfigWriteRequest>,
}

impl ConfigPersistenceQueue {
    fn submit(&mut self, request: ConfigWriteRequest) -> Option<ConfigWriteRequest> {
        self.last_request = Some(request.clone());
        if self.active.is_some() {
            self.pending = Some(request);
            None
        } else {
            self.active = Some(request.clone());
            Some(request)
        }
    }

    fn complete(&mut self) -> Option<ConfigWriteRequest> {
        self.active = None;
        self.pending.take()
    }

    fn activate(&mut self, request: &ConfigWriteRequest) {
        self.active = Some(request.clone());
        self.last_request = Some(request.clone());
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("stackmap: {error:#}");
        std::process::exit(2);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum CliAction {
    Help,
    Version,
    Run {
        repository: Option<PathBuf>,
        current: bool,
    },
}

fn help_text() -> String {
    format!(
        "stackmap {}\n\nUSAGE:\n    stackmap [--current] [REPOSITORY]\n\nRun a live local branch and Graphite stack map. Press ? in the TUI for keys.",
        env!("CARGO_PKG_VERSION")
    )
}

fn version_text() -> String {
    format!("stackmap {}", env!("CARGO_PKG_VERSION"))
}

fn parse_cli<I, S>(args: I) -> std::result::Result<CliAction, String>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let mut repository = None;
    let mut current = false;
    let mut terminated = false;
    let mut meta = None;
    for value in args.into_iter().map(Into::into) {
        if !terminated && value == OsStr::new("--") {
            terminated = true;
            continue;
        }
        if !terminated {
            if value == OsStr::new("--current") {
                if current {
                    return Err("--current may be provided only once".into());
                }
                current = true;
                continue;
            }
            if value == OsStr::new("--help") || value == OsStr::new("-h") {
                if meta.replace(CliAction::Help).is_some() {
                    return Err("only one help/version action may be provided".into());
                }
                continue;
            }
            if value == OsStr::new("--version") || value == OsStr::new("-V") {
                if meta.replace(CliAction::Version).is_some() {
                    return Err("only one help/version action may be provided".into());
                }
                continue;
            }
            if value != OsStr::new("-") && value.to_string_lossy().starts_with('-') {
                return Err(format!("unknown option: {}", value.to_string_lossy()));
            }
        }
        if repository.replace(PathBuf::from(value)).is_some() {
            return Err("expected at most one repository path".into());
        }
    }
    if let Some(meta) = meta {
        if current || repository.is_some() {
            return Err("help/version cannot be combined with other arguments".into());
        }
        return Ok(meta);
    }
    Ok(CliAction::Run {
        repository,
        current,
    })
}

fn run() -> Result<()> {
    let cli = parse_cli(std::env::args_os().skip(1)).map_err(anyhow::Error::msg)?;
    let (repository, current) = match cli {
        CliAction::Help => {
            println!("{}", help_text());
            return Ok(());
        }
        CliAction::Version => {
            println!("{}", version_text());
            return Ok(());
        }
        CliAction::Run {
            repository,
            current,
        } => (repository, current),
    };
    let start_dir = repository.unwrap_or(std::env::current_dir()?);
    let adapter = GitAdapter::discover(&start_dir).with_context(|| {
        format!(
            "{} is not inside a readable Git repository",
            start_dir.display()
        )
    })?;
    let refresh = RefreshHandle::start(start_dir.clone())?;
    let (checkout_send, checkout_receive) = mpsc::sync_channel(1);
    let (delete_send, delete_receive) = mpsc::sync_channel(1);
    let (github_send, github_receive) = mpsc::sync_channel(1);
    let (platform_send, platform_receive) = mpsc::sync_channel(1);
    let (config_send, config_receive) = mpsc::sync_channel::<ConfigPersistenceResult>(1);
    let mut config_queue = ConfigPersistenceQueue::default();
    let mut github_in_flight = false;
    let mut next_github_fetch = Instant::now();
    let shutdown = Arc::new(AtomicBool::new(false));
    for signal in [
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
    ] {
        signal_hook::flag::register(signal, shutdown.clone())?;
    }

    let guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;
    let mut app = App::default();
    if current {
        app.request_current_startup();
    }
    let mut last_reconcile = Instant::now();
    let mut last_clock_tick = Instant::now();
    let mut redraw = true;

    loop {
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        while let Some(event) = refresh.try_event() {
            if shutdown.load(Ordering::Acquire) {
                break;
            }
            redraw = true;
            match event {
                RefreshEvent::Structural {
                    request_epoch,
                    snapshot,
                } => {
                    app.apply_structural_snapshot_at(snapshot, request_epoch, Instant::now());
                    if let Some(request) = app.take_config_write_request()
                        && let Some(start) = config_queue.submit(request)
                    {
                        spawn_config_persistence(start, config_send.clone())?;
                    }
                    if matches!(app.archive_mode, ArchiveMode::Active)
                        && !github_in_flight
                        && Instant::now() >= next_github_fetch
                    {
                        github_in_flight = true;
                        app.mark_github_loading();
                        let directory = start_dir.clone();
                        let sender = github_send.clone();
                        thread::Builder::new()
                            .name("stackmap-github".into())
                            .spawn(move || {
                                let result = github::fetch(&directory);
                                let _ = sender.try_send(result);
                            })?;
                    }
                }
                RefreshEvent::Enriched(snapshot) => app.apply_enriched_snapshot(snapshot),
                RefreshEvent::Upstream(batch) => app.apply_upstream_batch(batch),
                RefreshEvent::Failed(error) => app.mark_stale(error),
            }
        }
        while let Ok(result) = github_receive.try_recv() {
            github_in_flight = false;
            next_github_fetch = Instant::now() + GITHUB_TTL;
            match result {
                Ok(matches) => app.apply_prs(matches),
                Err(error) => app.mark_github_failed(error),
            }
            redraw = true;
        }
        while let Ok(result) = platform_receive.try_recv() {
            app.platform_running = false;
            match result {
                PlatformResult::Open(Ok(())) => app.message = Some(Arc::from("PR opened")),
                PlatformResult::Copy(Ok(())) => app.message = Some(Arc::from("PR URL copied")),
                PlatformResult::Open(Err(error)) => {
                    app.message = Some(Arc::from(format!("open failed: {error}")))
                }
                PlatformResult::Copy(Err(error)) => {
                    app.message = Some(Arc::from(format!("copy failed: {error}")))
                }
            }
            redraw = true;
        }
        while let Ok(result) = config_receive.try_recv() {
            app.finish_config_persistence(result.sequence, result.result);
            if let Some(next) = config_queue
                .complete()
                .and_then(|request| app.prepare_pending_config_persistence(request))
            {
                config_queue.activate(&next);
                spawn_config_persistence(next, config_send.clone())?;
            }
            redraw = true;
        }
        while let Ok(result) = checkout_receive.try_recv() {
            let request_epoch = refresh.request();
            app.finish_checkout_at(result, request_epoch, Instant::now());
            redraw = true;
        }
        while let Ok(result) = delete_receive.try_recv() {
            let request_epoch = refresh.request();
            app.finish_deletion_at(result, request_epoch, Instant::now());
            redraw = true;
        }

        if let Some(command) = app.take_upstream_command() {
            refresh.request_upstream(command);
            redraw = true;
        }

        if app.tick(Instant::now()) {
            redraw = true;
        }
        if last_clock_tick.elapsed() >= Duration::from_secs(30) {
            redraw = true;
            last_clock_tick = Instant::now();
        }
        if redraw {
            terminal.draw(|frame| render(frame, &mut app, SystemTime::now()))?;
            redraw = false;
        }
        if last_reconcile.elapsed() >= RECONCILE_INTERVAL {
            refresh.request();
            last_reconcile = Instant::now();
        }
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => {
                    let Some(input) = Input::from_event(key) else {
                        continue;
                    };
                    if input.key == Key::Ignored {
                        continue;
                    }
                    redraw = true;
                    match app.handle_input(input) {
                        Action::None => {}
                        Action::Quit => break,
                        Action::Refresh => {
                            refresh.request();
                        }
                        Action::PersistConfig(request) => {
                            if let Some(start) = config_queue.submit(request) {
                                spawn_config_persistence(start, config_send.clone())?;
                            }
                        }
                        Action::Checkout(branch) => {
                            spawn_checkout(adapter.clone(), branch, checkout_send.clone())?;
                        }
                        Action::Delete(request) => {
                            spawn_delete(adapter.clone(), request, delete_send.clone())?;
                        }
                        Action::OpenUrl(url) => {
                            spawn_platform_action(
                                &mut app,
                                PlatformAction::Open(url),
                                platform_send.clone(),
                            )?;
                        }
                        Action::CopyUrl(url) => {
                            spawn_platform_action(
                                &mut app,
                                PlatformAction::Copy(url),
                                platform_send.clone(),
                            )?;
                        }
                    }
                }
                Event::Resize(_, _) => redraw = true,
                _ => {}
            }
        }
    }
    drop(terminal);
    drop(guard);
    flush_config_persistence(&mut app, &mut config_queue, &config_send, &config_receive)?;
    Ok(())
}

fn flush_config_persistence(
    app: &mut App,
    queue: &mut ConfigPersistenceQueue,
    sender: &std::sync::mpsc::SyncSender<ConfigPersistenceResult>,
    receiver: &std::sync::mpsc::Receiver<ConfigPersistenceResult>,
) -> Result<()> {
    let mut final_retry_used = false;
    if let Some(request) = app.take_config_write_request()
        && let Some(start) = queue.submit(request)
    {
        spawn_config_persistence(start, sender.clone())?;
    } else if queue.active.is_none()
        && let Some(retry) = queue
            .last_request
            .clone()
            .and_then(|request| app.prepare_pending_config_persistence(request))
    {
        final_retry_used = true;
        let start = queue
            .submit(retry)
            .context("configuration shutdown retry could not enter the idle queue")?;
        spawn_config_persistence(start, sender.clone())?;
    }
    let deadline = Instant::now() + CONFIG_SHUTDOWN_TIMEOUT;
    while queue.active.is_some() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            anyhow::bail!("timed out flushing stackmap configuration after terminal restore");
        }
        let result = receiver
            .recv_timeout(remaining)
            .map_err(|error| match error {
                std::sync::mpsc::RecvTimeoutError::Timeout => anyhow::anyhow!(
                    "timed out flushing stackmap configuration after terminal restore"
                ),
                std::sync::mpsc::RecvTimeoutError::Disconnected => {
                    anyhow::anyhow!("configuration persistence worker disconnected during shutdown")
                }
            })?;
        let failed = result.result.is_err();
        let active = queue.active.clone();
        let had_pending = queue.pending.is_some();
        app.finish_config_persistence(result.sequence, result.result);
        let pending = queue.complete();
        let retry = if failed && !had_pending && !final_retry_used {
            final_retry_used = true;
            active
        } else {
            None
        };
        let next = pending
            .or(retry)
            .and_then(|request| app.prepare_pending_config_persistence(request));
        if let Some(next) = next {
            queue.activate(&next);
            spawn_config_persistence(next, sender.clone())?;
        } else if failed {
            anyhow::bail!("configuration could not be saved during shutdown after one retry");
        }
    }
    Ok(())
}

fn spawn_config_persistence(
    request: ConfigWriteRequest,
    sender: std::sync::mpsc::SyncSender<ConfigPersistenceResult>,
) -> io::Result<()> {
    thread::Builder::new()
        .name("stackmap-config".into())
        .spawn(move || {
            let result =
                Config::persist_mutation(&request.common_dir, &request.mutation, &request.fallback);
            let _ = sender.try_send(ConfigPersistenceResult {
                sequence: request.sequence,
                result,
            });
        })?;
    Ok(())
}

enum PlatformAction {
    Open(Arc<str>),
    Copy(Arc<str>),
}

fn spawn_platform_action(
    app: &mut App,
    action: PlatformAction,
    sender: std::sync::mpsc::SyncSender<PlatformResult>,
) -> io::Result<()> {
    if app.platform_running {
        app.message = Some(Arc::from("open/copy action already running"));
        return Ok(());
    }
    app.platform_running = true;
    thread::Builder::new()
        .name("stackmap-platform".into())
        .spawn(move || {
            let result = match action {
                PlatformAction::Open(url) => PlatformResult::Open(platform::open_url(&url)),
                PlatformAction::Copy(url) => PlatformResult::Copy(platform::copy_text(&url)),
            };
            let _ = sender.try_send(result);
        })?;
    Ok(())
}

fn spawn_checkout(
    adapter: GitAdapter,
    branch: BranchId,
    sender: std::sync::mpsc::SyncSender<Result<()>>,
) -> io::Result<()> {
    thread::Builder::new()
        .name("stackmap-checkout".into())
        .spawn(move || {
            let _ = sender.try_send(adapter.checkout(&branch));
        })?;
    Ok(())
}

fn spawn_delete(
    adapter: GitAdapter,
    request: DeleteRequest,
    sender: std::sync::mpsc::SyncSender<Result<DeleteOutcome>>,
) -> io::Result<()> {
    thread::Builder::new()
        .name("stackmap-delete".into())
        .spawn(move || {
            let _ = sender.try_send(adapter.delete_branch(&request));
        })?;
    Ok(())
}

struct TerminalGuard {
    keyboard_enhanced: bool,
}

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        if let Err(error) = execute!(stdout(), EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(error.into());
        }
        let keyboard_enhanced = matches!(
            crossterm::terminal::supports_keyboard_enhancement(),
            Ok(true)
        );
        if keyboard_enhanced
            && let Err(error) = execute!(
                stdout(),
                PushKeyboardEnhancementFlags(keyboard_enhancement_flags())
            )
        {
            let _ = execute!(stdout(), LeaveAlternateScreen);
            let _ = disable_raw_mode();
            return Err(error.into());
        }
        Ok(Self { keyboard_enhanced })
    }
}

fn keyboard_enhancement_flags() -> KeyboardEnhancementFlags {
    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
        | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
        | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
        | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = restore_terminal(&mut stdout(), self.keyboard_enhanced);
    }
}

fn restore_terminal(writer: &mut impl Write, keyboard_enhanced: bool) -> io::Result<()> {
    if keyboard_enhanced {
        queue!(writer, PopKeyboardEnhancementFlags)?;
    }
    queue!(writer, LeaveAlternateScreen)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use stackmap::runtime::{
        Branch, BranchId, ConfiguredUpstream, DiffState, GraphiteProvenance, RemoteRefEvidence,
        RepositorySnapshot, RepositoryState,
    };

    use super::*;

    fn request(sequence: u64) -> ConfigWriteRequest {
        let mut mutation = stackmap::runtime::ConfigMutation::default();
        mutation.set_color(BranchId::new("stack"), None);
        ConfigWriteRequest {
            sequence,
            common_dir: PathBuf::from("/tmp/stackmap-color-test"),
            mutation,
            fallback: Config::default(),
        }
    }

    fn config_snapshot(common_dir: PathBuf) -> Arc<RepositorySnapshot> {
        let branch = |name: &str, current: bool| Branch {
            id: BranchId::new(name),
            oid: Arc::from(format!("oid-{name}")),
            parent: None,
            diff_parent: None,
            stack_root: BranchId::new(name),
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
        };
        let branches = vec![branch("main", true), branch("alpha", false)];
        Arc::new(RepositorySnapshot {
            generation: 1,
            root: PathBuf::from("/repo"),
            git_dir: common_dir.clone(),
            common_dir,
            repository_id: Arc::from("config-shutdown-test"),
            default_trunk: Some(BranchId::new("main")),
            configured_trunks: Arc::from([BranchId::new("main")]),
            trunks: Arc::from([BranchId::new("main")]),
            graphite_children: Arc::from([(
                BranchId::new("main"),
                Arc::from([BranchId::new("alpha")]),
            )]),
            branch_index: RepositorySnapshot::index_branches(&branches),
            branches: branches.into(),
            state: RepositoryState::Ready,
            graphite_status: Arc::from("fixture"),
            stale_error: None,
        })
    }

    #[test]
    fn color_queue_keeps_one_active_and_only_the_latest_pending_write() {
        let mut queue = ConfigPersistenceQueue::default();
        assert_eq!(queue.submit(request(1)).unwrap().sequence, 1);
        assert!(queue.submit(request(2)).is_none());
        assert!(queue.submit(request(3)).is_none());
        assert_eq!(queue.complete().unwrap().sequence, 3);
        assert!(queue.complete().is_none());
    }

    #[test]
    fn immediate_quit_flushes_active_and_latest_pending_config_mutations() {
        let directory = tempfile::tempdir().unwrap();
        let mut app = App::default();
        app.apply_snapshot(config_snapshot(directory.path().to_owned()));
        app.selected = Some(BranchId::new("alpha"));
        let Action::PersistConfig(color) = app.handle_key(Key::Character('c')) else {
            panic!("color mutation");
        };
        let Action::PersistConfig(archive) = app.handle_key(Key::Character('x')) else {
            panic!("archive mutation");
        };
        let (sender, receiver) = mpsc::sync_channel(1);
        let mut queue = ConfigPersistenceQueue::default();
        let start = queue.submit(color).expect("first write starts");
        spawn_config_persistence(start, sender.clone()).unwrap();
        assert!(queue.submit(archive).is_none());

        flush_config_persistence(&mut app, &mut queue, &sender, &receiver).unwrap();

        let saved = Config::load(directory.path()).unwrap();
        assert_eq!(saved.color(&BranchId::new("alpha")), Some("#7aa2f7"));
        assert!(saved.is_archived(&BranchId::new("alpha")));
    }

    #[test]
    fn final_shutdown_write_failure_retries_once_then_returns_visible_error() {
        let directory = tempfile::tempdir().unwrap();
        let invalid_common_dir = directory.path().join("not-a-directory");
        std::fs::write(&invalid_common_dir, "file blocks config directory creation").unwrap();
        let mut app = App::default();
        app.apply_snapshot(config_snapshot(invalid_common_dir));
        app.selected = Some(BranchId::new("alpha"));
        let Action::PersistConfig(request) = app.handle_key(Key::Character('c')) else {
            panic!("color mutation");
        };
        let (sender, receiver) = mpsc::sync_channel(1);
        let mut queue = ConfigPersistenceQueue::default();
        let start = queue.submit(request).expect("write starts");
        spawn_config_persistence(start, sender.clone()).unwrap();
        let initial = receiver.recv().unwrap();
        assert!(initial.result.is_err());
        app.finish_config_persistence(initial.sequence, initial.result);
        assert!(queue.complete().is_none());
        assert!(queue.active.is_none());

        let error = flush_config_persistence(&mut app, &mut queue, &sender, &receiver)
            .expect_err("the deterministic shutdown retry must surface its failure");

        assert!(
            error
                .to_string()
                .contains("could not be saved during shutdown after one retry")
        );
        assert!(queue.active.is_none());
    }

    #[test]
    fn terminal_cleanup_always_leaves_alt_screen_and_pops_enabled_keyboard_flags() {
        let mut enhanced = Vec::new();
        restore_terminal(&mut enhanced, true).unwrap();
        assert_eq!(enhanced, b"\x1b[<1u\x1b[?1049l");

        let mut fallback = Vec::new();
        restore_terminal(&mut fallback, false).unwrap();
        assert_eq!(fallback, b"\x1b[?1049l");
    }

    #[test]
    fn enhanced_terminals_request_complete_modifier_reporting() {
        let flags = keyboard_enhancement_flags();
        assert!(flags.contains(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES));
        assert!(flags.contains(KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES));
        assert!(flags.contains(KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS));
        assert!(flags.contains(KeyboardEnhancementFlags::REPORT_EVENT_TYPES));
    }

    #[test]
    fn cli_parser_accepts_current_on_either_side_of_repository_and_honors_terminator() {
        assert_eq!(
            parse_cli(["--current", "/repo"]),
            Ok(CliAction::Run {
                repository: Some(PathBuf::from("/repo")),
                current: true,
            })
        );
        assert_eq!(
            parse_cli(["/repo", "--current"]),
            Ok(CliAction::Run {
                repository: Some(PathBuf::from("/repo")),
                current: true,
            })
        );
        assert_eq!(
            parse_cli(["--", "--current"]),
            Ok(CliAction::Run {
                repository: Some(PathBuf::from("--current")),
                current: false,
            })
        );
    }

    #[test]
    fn cli_parser_handles_meta_actions_and_rejects_ambiguous_input() {
        assert_eq!(parse_cli(["--help"]), Ok(CliAction::Help));
        assert_eq!(parse_cli(["-V"]), Ok(CliAction::Version));
        for args in [
            vec!["--current", "--current"],
            vec!["--unknown"],
            vec!["one", "two"],
            vec!["--help", "one"],
            vec!["--version", "--current"],
        ] {
            assert!(parse_cli(args).is_err(), "accepted ambiguous args");
        }
    }

    #[test]
    fn help_and_version_share_the_cargo_package_version() {
        let expected = format!("stackmap {}", env!("CARGO_PKG_VERSION"));
        assert!(help_text().starts_with(&expected));
        assert_eq!(version_text(), expected);
    }
}
