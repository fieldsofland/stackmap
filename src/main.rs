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

use stackmap::adapters::{git::DeleteRequest, git::GitAdapter, github, platform};
use stackmap::app::{Action, App, ColorWriteRequest};
use stackmap::config::Config;
use stackmap::events::{Input, Key};
use stackmap::refresh::{RefreshEvent, RefreshHandle};

const RECONCILE_INTERVAL: Duration = Duration::from_secs(30);
const GITHUB_TTL: Duration = Duration::from_secs(30);

enum PlatformResult {
    Open(Result<()>),
    Copy(Result<()>),
}

struct ColorPersistenceResult {
    sequence: u64,
    result: Result<()>,
}

#[derive(Default)]
struct ColorPersistenceQueue {
    active: bool,
    pending: Option<ColorWriteRequest>,
}

impl ColorPersistenceQueue {
    fn submit(&mut self, request: ColorWriteRequest) -> Option<ColorWriteRequest> {
        if self.active {
            self.pending = Some(request);
            None
        } else {
            self.active = true;
            Some(request)
        }
    }

    fn complete(&mut self) -> Option<ColorWriteRequest> {
        self.active = false;
        self.pending.take()
    }

    fn activate(&mut self) {
        self.active = true;
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
            println!(
                "stackmap 0.0.0\n\nUSAGE:\n    stackmap [--current] [REPOSITORY]\n\nRun a live local branch and Graphite stack map. Press ? in the TUI for keys."
            );
            return Ok(());
        }
        CliAction::Version => {
            println!("stackmap {}", env!("CARGO_PKG_VERSION"));
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
    let (color_send, color_receive) = mpsc::sync_channel::<ColorPersistenceResult>(1);
    let mut color_queue = ColorPersistenceQueue::default();
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

    let _guard = TerminalGuard::enter()?;
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
        while let Some(event) = refresh.try_event() {
            redraw = true;
            match event {
                RefreshEvent::Structural {
                    request_epoch,
                    snapshot,
                } => {
                    app.apply_structural_snapshot(snapshot, request_epoch);
                    if !github_in_flight && Instant::now() >= next_github_fetch {
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
        while let Ok(result) = color_receive.try_recv() {
            app.finish_color_persistence(result.sequence, result.result);
            if let Some(next) = color_queue
                .complete()
                .and_then(|request| app.prepare_pending_color_persistence(request))
            {
                color_queue.activate();
                spawn_color_persistence(next, color_send.clone())?;
            }
            redraw = true;
        }
        while let Ok(result) = checkout_receive.try_recv() {
            let request_epoch = refresh.request();
            app.finish_checkout(result, request_epoch);
            redraw = true;
        }
        while let Ok(result) = delete_receive.try_recv() {
            let request_epoch = refresh.request();
            app.finish_deletion(result, request_epoch);
            redraw = true;
        }

        if last_clock_tick.elapsed() >= Duration::from_secs(30) {
            redraw = true;
            last_clock_tick = Instant::now();
        }
        if redraw {
            terminal.draw(|frame| stackmap::ui::render(frame, &mut app, SystemTime::now()))?;
            redraw = false;
        }
        if last_reconcile.elapsed() >= RECONCILE_INTERVAL {
            refresh.request();
            last_reconcile = Instant::now();
        }
        if shutdown.load(Ordering::Acquire) {
            break;
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
                        Action::PersistColor(request) => {
                            if let Some(start) = color_queue.submit(request) {
                                spawn_color_persistence(start, color_send.clone())?;
                            }
                        }
                        Action::Checkout(branch) => {
                            app.message = Some(Arc::from(format!("switching to {branch}…")));
                            spawn_checkout(adapter.clone(), branch, checkout_send.clone())?;
                        }
                        Action::Delete(request) => {
                            app.message =
                                Some(Arc::from(format!("deleting {} locally…", request.branch)));
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
    Ok(())
}

fn spawn_color_persistence(
    request: ColorWriteRequest,
    sender: std::sync::mpsc::SyncSender<ColorPersistenceResult>,
) -> io::Result<()> {
    thread::Builder::new()
        .name("stackmap-color".into())
        .spawn(move || {
            let result =
                Config::persist_colors(&request.common_dir, &request.updates, &request.fallback);
            let _ = sender.try_send(ColorPersistenceResult {
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
    branch: stackmap::model::BranchId,
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
    sender: std::sync::mpsc::SyncSender<Result<stackmap::adapters::git::DeleteOutcome>>,
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
                PushKeyboardEnhancementFlags(
                    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                        | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                )
            )
        {
            let _ = execute!(stdout(), LeaveAlternateScreen);
            let _ = disable_raw_mode();
            return Err(error.into());
        }
        Ok(Self { keyboard_enhanced })
    }
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

    use stackmap::model::BranchId;

    use super::*;

    fn request(sequence: u64) -> ColorWriteRequest {
        ColorWriteRequest {
            sequence,
            common_dir: PathBuf::from("/tmp/stackmap-color-test"),
            updates: std::collections::BTreeMap::from([(BranchId::new("stack"), None)]),
            fallback: Config::default(),
        }
    }

    #[test]
    fn color_queue_keeps_one_active_and_only_the_latest_pending_write() {
        let mut queue = ColorPersistenceQueue::default();
        assert_eq!(queue.submit(request(1)).unwrap().sequence, 1);
        assert!(queue.submit(request(2)).is_none());
        assert!(queue.submit(request(3)).is_none());
        assert_eq!(queue.complete().unwrap().sequence, 3);
        assert!(queue.complete().is_none());
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
}
