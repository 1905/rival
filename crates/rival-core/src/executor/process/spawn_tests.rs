//! The macOS fork lock (Go `syscall.ForkLock`). std's `io::pipe` there
//! runs `pipe(2)` and then sets close-on-exec on each end, so a fork in
//! between gives the child both ends. That window is a few instructions
//! wide; these tests hold it open with a raw `pipe(2)` that sets the flags
//! late, and watch for EOF on the read end while the child still runs: EOF
//! arrives only once no process holds the write end.
//!
//! The forced window blocks every Rival pipe and spawn of its process, and
//! other unit tests have deadlines of 100ms. So it runs in its own process:
//! this test binary, re-executed on an ignored helper.
//!
//! Every child is owned: it is killed and reaped on every path. Only the
//! child's own PID, or the helper's own process group before its leader is
//! reaped, is signalled. Every wait, cleanup included, is bounded.

use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::{PoisonError, mpsc};
use std::time::{Duration, Instant};

use super::*;

/// Set on the helper child only (on its `Command`). Names its report file.
const HELPER_OUT: &str = "RIVAL_FORK_LOCK_HELPER_OUT";
const HELPER_TEST: &str = "executor::process::spawn_tests::fork_lock_helper_child";

/// How long the guarded window stays open. A spawn that ignored the lock
/// reports back well inside it.
const WINDOW: Duration = Duration::from_millis(500);
/// How long a child that holds the write end must keep EOF away.
const NO_EOF: Duration = Duration::from_millis(300);
/// Upper bound of every wait that should end at once.
const BOUND: Duration = Duration::from_secs(10);

/// `pipe(2)` without close-on-exec: the window std leaves open on macOS.
fn open_window() -> (OwnedFd, OwnedFd) {
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: pipe writes two new descriptors into the array.
    assert_eq!(
        unsafe { libc::pipe(fds.as_mut_ptr()) },
        0,
        "pipe: {}",
        io::Error::last_os_error()
    );
    // SAFETY: both descriptors are new and owned by nothing else.
    unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) }
}

/// Ends the window: std's two close-on-exec calls.
fn close_window(fds: &(OwnedFd, OwnedFd)) {
    for fd in [&fds.0, &fds.1] {
        // SAFETY: flag update on a descriptor we own.
        let rc = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
        assert_ne!(rc, -1, "fcntl: {}", io::Error::last_os_error());
    }
}

/// A child that runs until killed and touches no stream.
fn sleeper() -> Command {
    let mut cmd = Command::new("/bin/sleep");
    cmd.arg("30")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd
}

/// A child this test started. Killed and reaped on drop, also on panic.
/// Only this child's PID is signalled, and only while it is unreaped (std's
/// `Child` never signals a reaped PID).
struct Owned(Child);

impl Owned {
    /// Still running: not exited, not reaped.
    fn running(&mut self) -> bool {
        matches!(self.0.try_wait(), Ok(None))
    }

    /// SIGKILL, then polls `try_wait` for at most BOUND. Returns whether
    /// the child was reaped.
    fn end(&mut self) -> bool {
        let _ = self.0.kill();
        let deadline = Instant::now() + BOUND;
        loop {
            match self.0.try_wait() {
                Ok(Some(_)) => return true,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                _ => return false,
            }
        }
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        self.end();
    }
}

/// The helper process, leader of its own process group. The group is
/// SIGKILLed before the leader is reaped, on every path, panic included.
/// An unreaped leader keeps its PID, and so the group ID, from reuse; after
/// the reap nothing is signalled.
struct HelperGroup {
    child: Child,
    reaped: bool,
}

impl HelperGroup {
    fn pid(&self) -> libc::pid_t {
        libc::pid_t::try_from(self.child.id()).unwrap()
    }

    /// Polls until the leader exits or `deadline` passes, without reaping
    /// it (`WNOWAIT`) or blocking (`WNOHANG`).
    fn exited_by(&self, deadline: Instant) -> bool {
        let pid = self.pid();
        loop {
            // SAFETY: info is a valid out buffer, zeroed before each call
            // because WNOHANG may return without writing it.
            let (rc, exited) = unsafe {
                let mut info: libc::siginfo_t = std::mem::zeroed();
                let rc = libc::waitid(
                    libc::P_PID,
                    pid as libc::id_t,
                    &mut info,
                    libc::WEXITED | libc::WNOWAIT | libc::WNOHANG,
                );
                (rc, info.si_pid() == pid)
            };
            assert_eq!(rc, 0, "waitid: {}", io::Error::last_os_error());
            if exited {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Kills what is left of the group, then reaps the leader within
    /// BOUND. `None` once reaped before, or when the reap timed out.
    fn finish(&mut self) -> Option<std::process::ExitStatus> {
        if self.reaped {
            return None;
        }
        // SAFETY: the leader is unreaped, so the group ID is still ours.
        unsafe { libc::kill(-self.pid(), libc::SIGKILL) };
        let deadline = Instant::now() + BOUND;
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    self.reaped = true;
                    return Some(status);
                }
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                _ => return None,
            }
        }
    }
}

impl Drop for HelperGroup {
    fn drop(&mut self) {
        self.finish();
    }
}

/// Whether the read end reaches EOF within `wait`.
fn eof_within(read: &File, wait: Duration) -> bool {
    let end = Instant::now() + wait;
    let mut buf = [0u8; 16];
    loop {
        match (&*read).read(&mut buf) {
            Ok(0) => return true,
            Ok(n) => panic!("{n} unexpected bytes: nobody writes to this pipe"),
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock
                    || e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => panic!("read: {e}"),
        }
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        let mut pfd = libc::pollfd {
            fd: read.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let ms = left.as_millis().clamp(1, 1000) as libc::c_int;
        // SAFETY: pfd is one valid pollfd. EINTR or a timeout just loops.
        unsafe { libc::poll(&mut pfd, 1, ms) };
    }
}

/// What the read end saw.
#[derive(Debug, PartialEq, Eq)]
struct Seen {
    /// EOF before the kill: no process held the write end.
    eof_before_kill: bool,
    /// The child still ran after that check, so an EOF there is not just
    /// an early exit of a child that held the write end.
    child_ran_through_check: bool,
    /// EOF once the child was killed and reaped.
    eof_after_reap: bool,
}

/// Drops this process's write end, then checks EOF while the child runs
/// and after it is reaped.
fn observe(fds: (OwnedFd, OwnedFd), mut child: Owned) -> Seen {
    let (read, write) = fds;
    drop(write);
    let read = File::from(read);
    set_nonblocking(&read).unwrap();
    let eof_before_kill = eof_within(&read, NO_EOF);
    let child_ran_through_check = child.running();
    assert!(
        child.end(),
        "child {} not reaped within {BOUND:?}",
        child.0.id()
    );
    Seen {
        eof_before_kill,
        child_ran_through_check,
        eof_after_reap: eof_within(&read, BOUND),
    }
}

/// Runs inside the helper process only (see
/// `spawn_waits_for_an_open_pipe_window`): the control, then the guarded
/// case. Writes what it saw to the report file, then exits before libtest
/// prints a summary.
#[test]
#[ignore = "helper process for the fork lock test"]
fn fork_lock_helper_child() {
    let Some(out) = std::env::var_os(HELPER_OUT) else {
        return;
    };
    // Control: a spawn that skips the guard inside the window inherits the
    // write end, and the reader gets no EOF until that child is gone. The
    // exclusive lock keeps every other Rival pipe and spawn of this process
    // out of the window, so this child can only take this pipe.
    let control = {
        let lock = FORK_LOCK.write().unwrap_or_else(PoisonError::into_inner);
        let fds = open_window();
        let child = Owned(sleeper().spawn().unwrap());
        close_window(&fds);
        drop(lock);
        observe(fds, child)
    };

    // Guarded: the window is opened under the shared lock, as `pipe` does,
    // and stays open until a spawn reports back or WINDOW passes. `spawn`
    // must wait for it to close.
    let (spawned_in_window, guarded) = std::thread::scope(|s| {
        let (opened_tx, opened_rx) = mpsc::channel();
        let (spawned_tx, spawned_rx) = mpsc::channel::<()>();
        let window = s.spawn(move || {
            with_pipe_lock(|| {
                let fds = open_window();
                opened_tx.send(()).unwrap();
                let spawned_in_window = spawned_rx.recv_timeout(WINDOW).is_ok();
                close_window(&fds);
                (fds, spawned_in_window)
            })
        });
        opened_rx
            .recv_timeout(BOUND)
            .expect("the window thread never opened its pipe");
        let child = Owned(spawn(&mut sleeper()).unwrap());
        let _ = spawned_tx.send(());
        let (fds, spawned_in_window) = window.join().unwrap();
        (spawned_in_window, observe(fds, child))
    });
    let report =
        format!("control={control:?}\nspawned_in_window={spawned_in_window}\nguarded={guarded:?}");
    std::fs::write(&out, report).unwrap();
    // SAFETY: ends the helper without libtest's summary.
    unsafe { libc::_exit(0) };
}

#[test]
fn spawn_waits_for_an_open_pipe_window() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("report");
    let log = dir.path().join("helper.log");
    let log_file = File::create(&log).unwrap();
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args([
        "--exact",
        HELPER_TEST,
        "--ignored",
        "--quiet",
        "--test-threads=1",
    ])
    .env(HELPER_OUT, &out)
    .env("HOME", dir.path())
    .env("USERPROFILE", dir.path())
    .env("RIVAL_HOME", dir.path().join(".rival"))
    .stdin(Stdio::null())
    .stdout(log_file.try_clone().unwrap())
    .stderr(log_file)
    // The helper's sleepers stay in this group, so the parent can end
    // them even after a SIGKILL skipped the helper's own cleanup.
    .process_group(0);
    let mut helper = HelperGroup {
        child: spawn(&mut cmd).unwrap(),
        reaped: false,
    };
    drop(cmd);
    // At most 4096 chars of the helper's output go into a failure message.
    let helper_log = || -> String {
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        text.chars().take(4096).collect()
    };
    // The helper needs about WINDOW + NO_EOF; this bounds a hung one.
    if !helper.exited_by(Instant::now() + 3 * BOUND) {
        let diagnostics = helper_log();
        helper.finish();
        panic!("fork lock helper still running; its output:\n{diagnostics}");
    }
    let status = helper.finish().expect("helper not reaped");
    assert!(status.success(), "helper {status}:\n{}", helper_log());

    let control = Seen {
        eof_before_kill: false,
        child_ran_through_check: true,
        eof_after_reap: true,
    };
    let guarded = Seen {
        eof_before_kill: true,
        child_ran_through_check: true,
        eof_after_reap: true,
    };
    // Control: the unguarded child held the write end until it died.
    // Guarded: the spawn waited for the window to close, and its child
    // never held the write end.
    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        format!("control={control:?}\nspawned_in_window=false\nguarded={guarded:?}")
    );
}

#[test]
fn pipe_ends_are_close_on_exec() {
    let (read, write) = pipe().unwrap();
    for fd in [read.as_raw_fd(), write.as_raw_fd()] {
        // SAFETY: reads the flags of a descriptor we own.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        assert_ne!(flags, -1, "fcntl: {}", io::Error::last_os_error());
        assert_ne!(flags & libc::FD_CLOEXEC, 0, "fd {fd} is inheritable");
    }
}
