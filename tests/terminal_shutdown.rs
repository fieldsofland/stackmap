#![cfg(unix)]

use std::ffi::CString;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

fn git(repository: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(repository)
        .args(args)
        .status()
        .expect("git should start");
    assert!(status.success(), "git {args:?} failed with {status}");
}

fn wait_for_exit(pid: libc::pid_t, timeout: Duration) -> Option<libc::c_int> {
    let deadline = Instant::now() + timeout;
    loop {
        let mut status = 0;
        let result = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
        if result == pid {
            return Some(status);
        }
        assert_eq!(
            result,
            0,
            "waitpid failed: {}",
            std::io::Error::last_os_error()
        );
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn initialize_repository(repository: &Path) {
    git(repository, &["init", "-q"]);
    git(
        repository,
        &["config", "user.email", "stackmap@example.com"],
    );
    git(repository, &["config", "user.name", "Stackmap Test"]);
    std::fs::write(repository.join("README.md"), "test\n").unwrap();
    git(repository, &["add", "README.md"]);
    git(repository, &["commit", "-qm", "initial"]);
}

fn spawn_stackmap(repository: &Path) -> (libc::pid_t, std::fs::File, Vec<u8>) {
    let binary = CString::new(env!("CARGO_BIN_EXE_stackmap")).unwrap();
    let directory = CString::new(repository.to_str().unwrap()).unwrap();
    let mut master = -1;
    let mut size = libc::winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let pid = unsafe {
        libc::forkpty(
            &mut master,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    };
    assert!(
        pid >= 0,
        "forkpty failed: {}",
        std::io::Error::last_os_error()
    );
    if pid == 0 {
        unsafe {
            if libc::chdir(directory.as_ptr()) != 0 {
                libc::_exit(126);
            }
            libc::execl(
                binary.as_ptr(),
                binary.as_ptr(),
                std::ptr::null::<libc::c_char>(),
            );
            libc::_exit(127);
        }
    }

    let master = unsafe { OwnedFd::from_raw_fd(master) };
    let flags = unsafe { libc::fcntl(master.as_raw_fd(), libc::F_GETFL) };
    assert!(flags >= 0);
    assert_eq!(
        unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) },
        0
    );
    let mut master = std::fs::File::from(master);
    let mut output = Vec::new();
    let query = b"\x1b[?u\x1b[c";
    let deadline = Instant::now() + Duration::from_secs(5);
    while !output.windows(query.len()).any(|bytes| bytes == query) {
        let mut buffer = [0_u8; 1024];
        match master.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => output.extend_from_slice(&buffer[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("could not read PTY output: {error}"),
        }
        assert!(
            Instant::now() < deadline,
            "Stackmap did not query terminal capabilities"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert!(output.windows(query.len()).any(|bytes| bytes == query));
    master.write_all(b"\x1b[?0u\x1b[?1;2c").unwrap();
    master.flush().unwrap();
    let rendered_deadline = Instant::now() + Duration::from_secs(5);
    while output.len() < 100 {
        let mut buffer = [0_u8; 1024];
        match master.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => output.extend_from_slice(&buffer[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("could not read rendered PTY output: {error}"),
        }
        assert!(
            Instant::now() < rendered_deadline,
            "Stackmap did not render after terminal capability negotiation: {output:?}"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        output.len() >= 100,
        "Stackmap did not reach its rendered UI: {output:?}"
    );
    (pid, master, output)
}

fn kill_and_reap(pid: libc::pid_t) {
    unsafe {
        libc::kill(pid, libc::SIGKILL);
        libc::waitpid(pid, std::ptr::null_mut(), 0);
    }
}

#[test]
fn closing_the_terminal_exits_stackmap_instead_of_leaving_it_running() {
    let repository = tempfile::tempdir().unwrap();
    initialize_repository(repository.path());
    let (pid, master, _) = spawn_stackmap(repository.path());
    drop(master);

    let status = wait_for_exit(pid, Duration::from_secs(3));
    if status.is_none() {
        kill_and_reap(pid);
    }
    let status = status.expect("Stackmap remained alive after its terminal closed");
    assert!(
        libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
        "Stackmap did not exit cleanly after terminal closure: {status}"
    );
}

#[test]
fn control_c_exits_stackmap_and_restores_the_terminal() {
    let repository = tempfile::tempdir().unwrap();
    initialize_repository(repository.path());
    let (pid, mut master, mut output) = spawn_stackmap(repository.path());
    master.write_all(b"\x03").unwrap();
    master.flush().unwrap();
    let flags = unsafe { libc::fcntl(master.as_raw_fd(), libc::F_GETFL) };
    assert!(flags >= 0);
    assert_eq!(
        unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags & !libc::O_NONBLOCK) },
        0
    );
    let reader = thread::spawn(move || {
        let mut buffer = [0_u8; 1024];
        loop {
            match master.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => output.extend_from_slice(&buffer[..count]),
            }
        }
        output
    });

    let status = wait_for_exit(pid, Duration::from_secs(3));
    if status.is_none() {
        kill_and_reap(pid);
    }
    let output = reader.join().unwrap();
    let status = status.expect("Stackmap remained alive after Ctrl-C");
    assert!(
        libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
        "Stackmap did not exit cleanly after Ctrl-C: {status}"
    );
    assert!(
        output.windows(8).any(|bytes| bytes == b"\x1b[?1049l"),
        "Stackmap did not restore the terminal after Ctrl-C: {output:?}"
    );
}
