//! `--detach`: re-exec rival into its own process session.
//!
//! Go: `cmd/detach.go`, `cmd/detach_unix.go`, `cmd/detach_other.go`.
//!
//! Claude Code skills launch rival from shells they tear down with a
//! process-group kill. A setsid'd child lives in its own session and process
//! group, so that teardown cannot reach it. The launching shell returns at
//! once and callers poll the printed PID.
//!
//! Stdin, stdout and stderr are inherited as they are, so redirects like
//! `< prompt > out 2> err` keep working in the child. A TTY-attached child
//! would lose its controlling terminal; this is for non-interactive use.
//! Windows has no setsid: see `set_detach_attr` for its process group and
//! console rules.

use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};

use rival_core::executor::process;

/// Marks the re-exec'd child so it does not detach again. Same name as Go.
pub const DETACHED_ENV: &str = "RIVAL_DETACHED";

/// What the caller does after [`detach_if_requested`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetachOutcome {
    /// Not requested, or this is already the detached child: run the command.
    Continue,
    /// The parent is done: exit the process with this code (Go: `os.Exit`).
    Exit(i32),
}

/// Go: `detachIfRequested`. Called from the root pre-run before any session
/// or queue side effects. Reads `RIVAL_DETACHED`, the executable path and the
/// process args, and prints to the real stderr. Never exits the process.
pub fn detach_if_requested(detach: bool) -> DetachOutcome {
    let marker = std::env::var_os(DETACHED_ENV);
    if !requested(detach, marker.as_deref()) {
        return DetachOutcome::Continue;
    }
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let mut stderr = io::stderr();
    match std::env::current_exe() {
        Ok(exe) => spawn_unless_std_fd_closed(
            &exe,
            &args,
            &mut stderr,
            crate::startup_fds::any_closed_at_start(),
        ),
        Err(err) => {
            let _ = writeln!(stderr, "rival: detach failed: {}", err);
            DetachOutcome::Exit(1)
        }
    }
}

/// Go hands fds 0-2 to the child as they are. When one was closed at
/// startup, `StartProcess` fails with EBADF before any child runs, and the
/// parent exits 1. Rust reopened that fd on /dev/null before `main`, so
/// `std_fd_closed` is the state the loader constructor recorded.
pub fn spawn_unless_std_fd_closed(
    exe: &Path,
    args: &[OsString],
    stderr: &mut dyn Write,
    std_fd_closed: bool,
) -> DetachOutcome {
    if std_fd_closed {
        let err = io::Error::from_raw_os_error(libc::EBADF);
        let _ = writeln!(
            stderr,
            "rival: detach failed: {}",
            start_error_text(exe.as_os_str(), &err)
        );
        return DetachOutcome::Exit(1);
    }
    spawn_detached(exe, args, stderr)
}

/// Go: `!detach || os.Getenv(detachedEnv) == "1"` negated. `marker` is the
/// current `RIVAL_DETACHED` value.
pub fn requested(detach: bool, marker: Option<&OsStr>) -> bool {
    detach && marker != Some(OsStr::new("1"))
}

/// The child command: `exe` with `args`, the current environment plus
/// `RIVAL_DETACHED=1` (overriding any inherited value), inherited stdio, the
/// current directory, and a new session on Unix.
pub fn detach_command(exe: &Path, args: &[OsString]) -> Command {
    let mut child = Command::new(exe);
    child
        .args(args)
        .env(DETACHED_ENV, "1")
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    set_detach_attr(&mut child);
    child
}

/// Starts [`detach_command`] and reports the child PID on `stderr`.
pub fn spawn_detached(exe: &Path, args: &[OsString], stderr: &mut dyn Write) -> DetachOutcome {
    start_and_report(detach_command(exe, args), stderr)
}

/// Spawns `child` and prints `rival: detached pid=N`. If that line cannot be
/// written, the caller could never learn the PID, so the child is killed
/// rather than left untrackable.
pub fn start_and_report(mut child: Command, stderr: &mut dyn Write) -> DetachOutcome {
    let mut spawned: Child = match process::spawn(&mut child) {
        Ok(spawned) => spawned,
        Err(err) => {
            let _ = writeln!(
                stderr,
                "rival: detach failed: {}",
                start_error_text(child.get_program(), &err)
            );
            return DetachOutcome::Exit(1);
        }
    };
    let line = format!("rival: detached pid={}\n", spawned.id());
    if stderr.write_all(line.as_bytes()).is_err() {
        let _ = spawned.kill();
        return DetachOutcome::Exit(1);
    }
    DetachOutcome::Exit(0)
}

/// The error text of a failed start: `start <path>: <io error>`.
fn start_error_text(program: &OsStr, err: &io::Error) -> String {
    format!("start {}: {err}", program.to_string_lossy())
}

/// Go `detach_unix.go`: `SysProcAttr{Setsid: true}` — own session and
/// process group, so a process-group kill of the launching shell cannot
/// reach the child.
#[cfg(unix)]
fn set_detach_attr(child: &mut Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: the hook runs in the forked child before exec and calls only
    // the async-signal-safe setsid and errno read.
    unsafe {
        child.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

/// Windows has no session to leave. The child gets its own process group,
/// so the launcher's group cannot reach it with a console control event
/// (Ctrl+C is off in a new group), and it never joins the parent's owner
/// Job: this short-lived parent creates none (only a provider spawn does).
///
/// The console is chosen from the actual standard handles. If any of them
/// is a console (`GetConsoleMode` succeeds), the child shares that console,
/// so its console input and output keep working after the parent exits.
/// Otherwise every stream is a file, pipe or device, and `DETACHED_PROCESS`
/// gives the child no console at all. `CREATE_NO_WINDOW` is never used: it
/// would drop the inherited console, and beside `DETACHED_PROCESS` it does
/// nothing. Redirected handles are inherited as they are, never reopened.
///
/// Limit: closing the shared console window ends that console; a child
/// still using it then loses its console I/O and gets the close event.
#[cfg(windows)]
fn set_detach_attr(child: &mut Command) {
    use std::os::windows::process::CommandExt;
    child.creation_flags(detach_creation_flags(any_std_handle_is_console()));
}

/// The creation flags of the detached child: `CREATE_NEW_PROCESS_GROUP`,
/// plus `DETACHED_PROCESS` when no standard handle is a console.
#[cfg(windows)]
pub fn detach_creation_flags(any_console: bool) -> u32 {
    use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS};
    if any_console {
        CREATE_NEW_PROCESS_GROUP
    } else {
        CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS
    }
}

/// Whether stdin, stdout or stderr of this process is a console handle.
#[cfg(windows)]
pub fn any_std_handle_is_console() -> bool {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{
        GetConsoleMode, GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE]
        .into_iter()
        .any(|which| {
            // SAFETY: plain queries; a NULL or invalid handle just fails.
            unsafe {
                let handle = GetStdHandle(which);
                let mut mode = 0;
                !handle.is_null()
                    && handle != INVALID_HANDLE_VALUE
                    && GetConsoleMode(handle, &mut mode) != 0
            }
        })
}

/// Platform-neutral checks of the guard and the command shape.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requested_follows_the_go_guard() {
        assert!(requested(true, None));
        assert!(requested(true, Some(OsStr::new(""))));
        assert!(requested(true, Some(OsStr::new("0"))));
        assert!(requested(true, Some(OsStr::new(" 1"))));
        assert!(!requested(true, Some(OsStr::new("1"))));
        assert!(!requested(false, None));
        assert!(!requested(false, Some(OsStr::new("1"))));
    }

    #[test]
    fn detach_if_requested_without_flag_continues() {
        assert_eq!(detach_if_requested(false), DetachOutcome::Continue);
    }

    #[test]
    fn command_keeps_args_and_overrides_the_marker() {
        let args = [OsString::from("command"), OsString::from("--detach")];
        let cmd = detach_command(Path::new("/x/rival"), &args);
        assert_eq!(cmd.get_program(), OsStr::new("/x/rival"));
        assert_eq!(cmd.get_args().collect::<Vec<_>>(), ["command", "--detach"]);
        let envs: Vec<_> = cmd.get_envs().collect();
        assert_eq!(envs, [(OsStr::new(DETACHED_ENV), Some(OsStr::new("1")))]);
        assert_eq!(cmd.get_current_dir(), None, "never changes directory");
    }

    #[test]
    fn spawn_failure_prints_go_path_error() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("missing-rival");
        let mut err = Vec::new();
        let out = spawn_detached(&exe, &[], &mut err);
        assert_eq!(out, DetachOutcome::Exit(1));
        let text = crate::testutil::NO_SUCH_FILE;
        assert_eq!(
            String::from_utf8(err).unwrap(),
            format!("rival: detach failed: start {}: {text}\n", exe.display())
        );
    }
}

#[cfg(all(test, windows))]
mod windows_tests;

#[cfg(all(test, unix))]
mod unix_tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Set on the helper child only (on its `Command`, never the test
    /// process env). Names the file the helper writes its report to.
    const HELPER_OUT: &str = "RIVAL_DETACH_TEST_HELPER_OUT";
    const HELPER_TEST: &str = "detach::unix_tests::detach_helper_child";

    #[test]
    fn closed_std_fd_at_startup_fails_like_go_start_without_a_child() {
        let mut err = Vec::new();
        // /bin/sleep would print a pid line if anything were spawned.
        let out = spawn_unless_std_fd_closed(
            Path::new("/bin/sleep"),
            &[OsString::from("30")],
            &mut err,
            true,
        );
        assert_eq!(out, DetachOutcome::Exit(1));
        assert_eq!(
            String::from_utf8(err).unwrap(),
            "rival: detach failed: start /bin/sleep: Bad file descriptor (os error 9)\n"
        );
    }

    /// A writer that captures what it was given, then fails.
    struct FailingWriter(Vec<u8>);

    impl Write for FailingWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.extend_from_slice(buf);
            Err(io::Error::from_raw_os_error(libc::EPIPE))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn pid_from_line(line: &[u8]) -> libc::pid_t {
        let text = std::str::from_utf8(line).unwrap();
        let digits = text
            .strip_prefix("rival: detached pid=")
            .and_then(|s| s.strip_suffix('\n'))
            .unwrap_or_else(|| panic!("unexpected line {text:?}"));
        digits.parse().unwrap()
    }

    /// A detached child this test spawned and has not reaped yet. Dropping
    /// it unreaped (timeout or failed assertion) kills and reaps that child
    /// only. A reaped PID is never signalled again.
    struct OwnedChild {
        pid: libc::pid_t,
        reaped: bool,
    }

    impl OwnedChild {
        fn new(pid: libc::pid_t) -> OwnedChild {
            assert!(pid > 0, "bad pid {pid}");
            OwnedChild { pid, reaped: false }
        }

        /// One `waitpid(WNOHANG)`, retried on EINTR: `Some(status)` once the
        /// child exited, `None` while it runs.
        fn poll(&mut self) -> io::Result<Option<libc::c_int>> {
            let pid = self.pid;
            self.poll_with(|status| {
                // SAFETY: pid is our own unreaped child; status is a valid pointer.
                match unsafe { libc::waitpid(pid, status, libc::WNOHANG) } {
                    -1 => Err(io::Error::last_os_error()),
                    got => Ok(got),
                }
            })
        }

        /// Only a reap or ECHILD ends ownership. EINTR retries; any other
        /// error keeps the child owned, so Drop still kills and reaps it.
        fn poll_with(
            &mut self,
            mut waitpid: impl FnMut(&mut libc::c_int) -> io::Result<libc::pid_t>,
        ) -> io::Result<Option<libc::c_int>> {
            loop {
                let mut status = 0;
                match waitpid(&mut status) {
                    Ok(0) => return Ok(None),
                    Ok(_) => {
                        self.reaped = true;
                        return Ok(Some(status));
                    }
                    Err(err) if err.raw_os_error() == Some(libc::EINTR) => continue,
                    Err(err) => {
                        // ECHILD: not ours any more; never signal it.
                        self.reaped |= err.raw_os_error() == Some(libc::ECHILD);
                        return Err(err);
                    }
                }
            }
        }

        fn try_reap(&mut self) -> Option<libc::c_int> {
            self.poll()
                .unwrap_or_else(|err| panic!("waitpid({}): {err}", self.pid))
        }

        /// Polls until the child exits; panics after `timeout`.
        fn wait_exit(&mut self, timeout: Duration) -> libc::c_int {
            let deadline = Instant::now() + timeout;
            loop {
                if let Some(status) = self.try_reap() {
                    return status;
                }
                assert!(
                    Instant::now() < deadline,
                    "child {} still running after {timeout:?}",
                    self.pid
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    impl Drop for OwnedChild {
        fn drop(&mut self) {
            if self.reaped {
                return;
            }
            // SAFETY: pid is still our unreaped child, so it cannot be reused.
            unsafe { libc::kill(self.pid, libc::SIGKILL) };
            let deadline = Instant::now() + Duration::from_secs(10);
            while Instant::now() < deadline {
                // Reaped or ECHILD: done. Other errors: keep trying.
                let _ = self.poll();
                if self.reaped {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    #[test]
    fn owned_child_ownership_ends_only_on_reap_or_echild() {
        fn errno(code: i32) -> io::Result<libc::pid_t> {
            Err(io::Error::from_raw_os_error(code))
        }
        // No process has this pid, so a Drop that fires on failure is harmless.
        let pid = libc::pid_t::MAX;
        let script = |steps: Vec<io::Result<libc::pid_t>>| {
            let mut steps = steps.into_iter();
            move |status: &mut libc::c_int| {
                *status = 9;
                steps.next().expect("waitpid called too often")
            }
        };

        let mut child = OwnedChild::new(pid);
        let got = child.poll_with(script(vec![errno(libc::EINTR), errno(libc::EINTR), Ok(0)]));
        assert_eq!(got.unwrap(), None);
        assert!(!child.reaped, "EINTR must not end ownership");

        let got = child.poll_with(script(vec![errno(libc::EINTR), Ok(pid)]));
        assert_eq!(got.unwrap(), Some(9));
        assert!(child.reaped);

        let mut child = OwnedChild::new(pid);
        let got = child.poll_with(script(vec![errno(libc::EINVAL)]));
        assert_eq!(got.unwrap_err().raw_os_error(), Some(libc::EINVAL));
        assert!(!child.reaped, "other errors keep cleanup");

        let got = child.poll_with(script(vec![errno(libc::EINTR), errno(libc::ECHILD)]));
        assert_eq!(got.unwrap_err().raw_os_error(), Some(libc::ECHILD));
        assert!(child.reaped, "ECHILD ends ownership");
    }

    const REAP_TIMEOUT: Duration = Duration::from_secs(10);

    /// Variable names that look like credentials; kept out of the helper.
    fn is_secret_name(name: &OsStr) -> bool {
        let name = name.to_string_lossy().to_ascii_uppercase();
        ["KEY", "TOKEN", "SECRET", "PASSWORD", "CREDENTIAL"]
            .iter()
            .any(|w| name.contains(w))
    }

    #[test]
    fn print_failure_kills_only_the_new_child() {
        let mut err = FailingWriter(Vec::new());
        let out = spawn_detached(Path::new("/bin/sleep"), &[OsString::from("30")], &mut err);
        assert_eq!(out, DetachOutcome::Exit(1));
        let mut child = OwnedChild::new(pid_from_line(&err.0));
        let status = child.wait_exit(REAP_TIMEOUT);
        assert!(libc::WIFSIGNALED(status), "status {status:#x}");
        assert_eq!(libc::WTERMSIG(status), libc::SIGKILL);
    }

    fn fd_identity(fd: i32) -> String {
        // SAFETY: fstat writes into a zeroed stat buffer we own.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(fd, &mut st) } != 0 {
            return "closed".to_string();
        }
        format!("{}:{}", st.st_dev, st.st_ino)
    }

    /// Runs only inside the detached helper process (see
    /// `child_gets_new_session_marker_args_cwd_and_stdio`). It records what
    /// the child sees, then exits before libtest prints a summary.
    #[test]
    #[ignore = "helper process for the detach spawn test"]
    fn detach_helper_child() {
        let Some(out) = std::env::var_os(HELPER_OUT) else {
            return;
        };
        // SAFETY: plain process-identity queries.
        let (pid, sid, pgrp) = unsafe { (libc::getpid(), libc::getsid(0), libc::getpgrp()) };
        let report = [
            format!("pid={pid}"),
            format!("sid={sid}"),
            format!("pgrp={pgrp}"),
            format!(
                "marker={}",
                std::env::var(DETACHED_ENV).unwrap_or_else(|_| "<unset>".into())
            ),
            format!(
                "args={}",
                std::env::args().skip(1).collect::<Vec<_>>().join("\u{1f}")
            ),
            format!("cwd={}", std::env::current_dir().unwrap().display()),
            format!("home={}", std::env::var("HOME").unwrap_or_default()),
            format!(
                "rival_home={}",
                std::env::var("RIVAL_HOME").unwrap_or_default()
            ),
            format!(
                "secrets={}",
                std::env::vars_os()
                    .filter(|(k, _)| is_secret_name(k))
                    .count()
            ),
            format!("fd0={}", fd_identity(0)),
            format!("fd1={}", fd_identity(1)),
            format!("fd2={}", fd_identity(2)),
        ]
        .join("\n");
        let tmp = Path::new(&out).with_extension("tmp");
        std::fs::write(&tmp, report).unwrap();
        std::fs::rename(&tmp, &out).unwrap();
        // SAFETY: ends the helper without running libtest's summary.
        unsafe { libc::_exit(0) };
    }

    #[test]
    fn child_gets_new_session_marker_args_cwd_and_stdio() {
        let dir = tempfile::tempdir().unwrap();
        let report_path = dir.path().join("report.txt");
        let exe = std::env::current_exe().unwrap();
        let args: Vec<OsString> = ["--exact", HELPER_TEST, "--ignored", "--quiet"]
            .iter()
            .map(OsString::from)
            .collect();
        let home = dir.path().join("home");
        let rival_home = home.join(".rival");
        let mut cmd = detach_command(&exe, &args);
        cmd.env(HELPER_OUT, &report_path)
            .env("HOME", &home)
            .env("USERPROFILE", &home)
            .env("RIVAL_HOME", &rival_home);
        for (key, _) in std::env::vars_os().filter(|(k, _)| is_secret_name(k)) {
            cmd.env_remove(key);
        }
        let mut err = Vec::new();
        assert_eq!(start_and_report(cmd, &mut err), DetachOutcome::Exit(0));
        let pid = pid_from_line(&err);
        let mut child = OwnedChild::new(pid);

        let status = child.wait_exit(REAP_TIMEOUT);
        assert!(
            libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
            "status {status:#x}"
        );
        let report = std::fs::read_to_string(&report_path).unwrap();
        let field = |key: &str| {
            report
                .lines()
                .find_map(|l| l.strip_prefix(&format!("{key}=")))
                .unwrap_or_else(|| panic!("no {key} in {report}"))
                .to_string()
        };
        assert_eq!(field("pid"), pid.to_string());
        assert_eq!(field("sid"), pid.to_string(), "child leads a new session");
        assert_eq!(field("pgrp"), pid.to_string(), "child leads a new group");
        assert_eq!(field("marker"), "1");
        assert_eq!(
            field("args"),
            ["--exact", HELPER_TEST, "--ignored", "--quiet"].join("\u{1f}")
        );
        assert_eq!(
            field("cwd"),
            std::env::current_dir().unwrap().display().to_string()
        );
        for fd in 0..3 {
            assert_eq!(field(&format!("fd{fd}")), fd_identity(fd), "fd {fd}");
        }
        assert_eq!(field("home"), home.display().to_string());
        assert_eq!(field("rival_home"), rival_home.display().to_string());
        assert_eq!(field("secrets"), "0");
    }
}
