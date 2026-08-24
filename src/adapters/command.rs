use std::ffi::OsStr;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct CommandOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

#[derive(Debug)]
pub enum CommandError {
    Spawn(std::io::Error),
    Timeout(Duration),
    Read(std::io::Error),
    Write(std::io::Error),
    InputTooLarge(usize),
    MissingPipe(&'static str),
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for CommandError {}

pub fn run_bounded<I, S>(
    program: &OsStr,
    args: I,
    cwd: &Path,
    timeout: Duration,
    output_limit: usize,
) -> Result<CommandOutput, CommandError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_bounded_inner(program, args, cwd, timeout, output_limit, None, false)
}

pub fn run_bounded_read_only_git<I, S>(
    args: I,
    cwd: &Path,
    timeout: Duration,
    output_limit: usize,
) -> Result<CommandOutput, CommandError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_bounded_inner(
        OsStr::new("git"),
        args,
        cwd,
        timeout,
        output_limit,
        None,
        true,
    )
}

pub fn run_bounded_with_stdin<I, S>(
    program: &OsStr,
    args: I,
    cwd: &Path,
    timeout: Duration,
    output_limit: usize,
    stdin: &[u8],
) -> Result<CommandOutput, CommandError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    if stdin.len() > 64 * 1024 {
        return Err(CommandError::InputTooLarge(stdin.len()));
    }
    run_bounded_inner(
        program,
        args,
        cwd,
        timeout,
        output_limit,
        Some(stdin),
        false,
    )
}

fn run_bounded_inner<I, S>(
    program: &OsStr,
    args: I,
    cwd: &Path,
    timeout: Duration,
    output_limit: usize,
    stdin: Option<&[u8]>,
    disable_optional_locks: bool,
) -> Result<CommandOutput, CommandError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let started = Instant::now();
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if disable_optional_locks {
        command.env("GIT_OPTIONAL_LOCKS", "0");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().map_err(CommandError::Spawn)?;
    let active_child = ActiveChild::register(child.id());

    let stdin_result = if let Some(input) = stdin {
        let mut stream = child
            .stdin
            .take()
            .ok_or(CommandError::MissingPipe("stdin"))?;
        let input = input.to_vec();
        let (send, receive) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let result = stream.write_all(&input);
            let _ = send.send(result);
        });
        Some(receive)
    } else {
        None
    };

    let stdout = child
        .stdout
        .take()
        .ok_or(CommandError::MissingPipe("stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or(CommandError::MissingPipe("stderr"))?;
    let (send, receive) = mpsc::sync_channel(2);
    for (is_stdout, stream) in [
        (true, Box::new(stdout) as Box<dyn Read + Send>),
        (false, Box::new(stderr) as Box<dyn Read + Send>),
    ] {
        let send = send.clone();
        thread::spawn(move || {
            let result = read_limited(stream, output_limit);
            let _ = send.send((is_stdout, result));
        });
    }
    drop(send);

    let status = loop {
        match child.try_wait().map_err(CommandError::Read)? {
            Some(status) => break status,
            None if started.elapsed() >= timeout => {
                terminate(&mut child);
                let _ = child.wait();
                return Err(CommandError::Timeout(timeout));
            }
            None => thread::sleep(Duration::from_millis(10)),
        }
    };
    drop(active_child);

    let mut stdout = (Vec::new(), false);
    let mut stderr = (Vec::new(), false);
    for _ in 0..2 {
        let (is_stdout, value) = receive
            .recv_timeout(remaining(started, timeout).ok_or_else(|| {
                terminate(&mut child);
                CommandError::Timeout(timeout)
            })?)
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => {
                    terminate(&mut child);
                    CommandError::Timeout(timeout)
                }
                mpsc::RecvTimeoutError::Disconnected => {
                    CommandError::Read(std::io::Error::other("output reader exited"))
                }
            })?;
        let value = value.map_err(CommandError::Read)?;
        if is_stdout {
            stdout = value
        } else {
            stderr = value
        }
    }
    if let Some(receive) = stdin_result {
        receive
            .recv_timeout(remaining(started, timeout).ok_or_else(|| {
                terminate(&mut child);
                CommandError::Timeout(timeout)
            })?)
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => {
                    terminate(&mut child);
                    CommandError::Timeout(timeout)
                }
                mpsc::RecvTimeoutError::Disconnected => {
                    CommandError::Write(std::io::Error::other("stdin writer exited"))
                }
            })?
            .map_err(CommandError::Write)?;
    }
    Ok(CommandOutput {
        status,
        stdout: stdout.0,
        stderr: stderr.0,
        stdout_truncated: stdout.1,
        stderr_truncated: stderr.1,
    })
}

fn remaining(started: Instant, timeout: Duration) -> Option<Duration> {
    timeout.checked_sub(started.elapsed())
}

fn terminate(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        // Each subprocess owns its process group, so a timeout also stops hooks
        // or helpers that inherited its pipes instead of leaving reader threads
        // waiting for descendants after the direct child exits.
        signal_process_group(child.id(), "-TERM");
        let grace_started = Instant::now();
        while grace_started.elapsed() < Duration::from_millis(250) {
            if child.try_wait().ok().flatten().is_some() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        signal_process_group(child.id(), "-KILL");
    }
    let _ = child.kill();
}

fn active_children() -> &'static Mutex<std::collections::HashSet<u32>> {
    static ACTIVE: OnceLock<Mutex<std::collections::HashSet<u32>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

struct ActiveChild(u32);

impl ActiveChild {
    fn register(pid: u32) -> Self {
        if let Ok(mut active) = active_children().lock() {
            active.insert(pid);
        }
        Self(pid)
    }
}

impl Drop for ActiveChild {
    fn drop(&mut self) {
        if let Ok(mut active) = active_children().lock() {
            active.remove(&self.0);
        }
    }
}

pub fn terminate_active_commands() {
    let pids = active_children()
        .lock()
        .map(|active| active.iter().copied().collect::<Vec<_>>())
        .unwrap_or_default();
    for &pid in &pids {
        signal_process_group(pid, "-TERM");
    }
    thread::sleep(Duration::from_millis(100));
    for pid in pids {
        signal_process_group(pid, "-KILL");
    }
}

#[cfg(unix)]
fn signal_process_group(pid: u32, signal: &str) {
    let group = format!("-{pid}");
    let _ = Command::new("/bin/kill")
        .args([signal, &group])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(not(unix))]
fn signal_process_group(_pid: u32, _signal: &str) {}

fn read_limited(
    mut stream: Box<dyn Read + Send>,
    limit: usize,
) -> std::io::Result<(Vec<u8>, bool)> {
    let mut kept = Vec::with_capacity(limit.min(8192));
    let mut buffer = [0_u8; 8192];
    let mut truncated = false;
    loop {
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let remaining = limit.saturating_sub(kept.len());
        kept.extend_from_slice(&buffer[..count.min(remaining)]);
        truncated |= count > remaining;
    }
    Ok((kept, truncated))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_is_bounded_without_losing_exit_status() {
        let output = run_bounded(
            OsStr::new("/bin/sh"),
            [
                "-c",
                "i=0; while [ $i -lt 1000 ]; do printf x; i=$((i+1)); done",
            ],
            Path::new("/"),
            Duration::from_secs(1),
            64,
        )
        .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout.len(), 64);
        assert!(output.stdout_truncated);
    }

    #[test]
    fn timeout_terminates_child() {
        let error = run_bounded(
            OsStr::new("/bin/sh"),
            ["-c", "sleep 2"],
            Path::new("/"),
            Duration::from_millis(30),
            64,
        )
        .unwrap_err();
        assert!(matches!(error, CommandError::Timeout(_)));
    }

    #[test]
    fn small_stdin_is_delivered_with_the_same_timeout_bound() {
        let output = run_bounded_with_stdin(
            OsStr::new("/bin/sh"),
            ["-c", "cat"],
            Path::new("/"),
            Duration::from_secs(1),
            64,
            b"clipboard value",
        )
        .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"clipboard value");
    }

    #[test]
    fn oversized_stdin_is_rejected_before_spawning() {
        let error = run_bounded_with_stdin(
            OsStr::new("/bin/sh"),
            ["-c", "cat"],
            Path::new("/"),
            Duration::from_secs(1),
            64,
            &vec![b'x'; 64 * 1024 + 1],
        )
        .unwrap_err();
        assert!(matches!(error, CommandError::InputTooLarge(65_537)));
    }

    #[test]
    fn descendants_cannot_hold_output_pipes_past_the_wall_clock_deadline() {
        let started = Instant::now();
        let error = run_bounded(
            OsStr::new("/bin/sh"),
            ["-c", "sleep 5 &"],
            Path::new("/"),
            Duration::from_millis(80),
            64,
        )
        .unwrap_err();
        assert!(matches!(error, CommandError::Timeout(_)));
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
