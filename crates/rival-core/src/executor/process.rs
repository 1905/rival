//! Platform process operations for provider subprocesses: executable lookup,
//! the provider process group, pipe IO that a cancellation can interrupt,
//! and reaping.
//!
//! Unix is the real backend. The `not(unix)` items are placeholders that
//! keep the crate compiling; P5 replaces them with a Windows Job Object
//! backend (group kill) and cancellable pipe IO.

use std::ffi::OsStr;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::gostd;
use crate::paths;

/// Go `exec.ErrNotFound`.
const ERR_NOT_FOUND: &str = "executable file not found in $PATH";
/// Go `exec.ErrDot`.
const ERR_DOT: &str = "cannot run executable found relative to current directory";

/// Go `*exec.Error` from `exec.LookPath`: `exec: "<name>": <err>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LookPathError {
    pub name: String,
    pub err: String,
    /// Go returns the relative path together with `ErrDot`.
    pub dot_path: Option<PathBuf>,
}

impl fmt::Display for LookPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "exec: {}: {}", gostd::quote(&self.name), self.err)
    }
}

impl std::error::Error for LookPathError {}

/// Go `exec.LookPath` (Unix) with an explicit `$PATH` value (`None` = unset).
/// A name containing `/` is checked as-is. Otherwise each `$PATH` element is
/// tried in order; an empty element means `.`, and a hit that is not
/// absolute is an `ErrDot` error.
pub fn look_path(file: &str, path_env: Option<&OsStr>) -> Result<PathBuf, LookPathError> {
    let error = |err: String, dot_path: Option<PathBuf>| LookPathError {
        name: file.to_string(),
        err,
        dot_path,
    };
    if file.contains('/') {
        return match find_executable(Path::new(file)) {
            Ok(()) => Ok(PathBuf::from(file)),
            Err(err) => Err(error(err, None)),
        };
    }
    let path_env = path_env.unwrap_or_default();
    if !path_env.is_empty() {
        for dir in path_env.as_encoded_bytes().split(|&b| b == b':') {
            // SAFETY: split at an ASCII byte of a valid encoded OsStr.
            let dir = unsafe { OsStr::from_encoded_bytes_unchecked(dir) };
            let dir = if dir.is_empty() { OsStr::new(".") } else { dir };
            let path = paths::clean(&Path::new(dir).join(file));
            if find_executable(&path).is_ok() {
                if !path.is_absolute() {
                    return Err(error(ERR_DOT.to_string(), Some(path)));
                }
                return Ok(path);
            }
        }
    }
    Err(error(ERR_NOT_FOUND.to_string(), None))
}

/// Go `exec.findExecutable`: stat (following links), not a directory, then
/// `faccessat(X_OK, AT_EACCESS)`; ENOSYS/EPERM fall back to the mode bits.
#[cfg(unix)]
fn find_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::PermissionsExt;

    let meta = std::fs::metadata(path)
        .map_err(|e| format!("stat {}: {}", path.display(), gostd::os_error_text(&e)))?;
    if meta.is_dir() {
        return Err(gostd::os_error_text(&io::Error::from_raw_os_error(
            libc::EISDIR,
        )));
    }
    let cpath = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| gostd::os_error_text(&io::Error::from_raw_os_error(libc::EINVAL)))?;
    // SAFETY: cpath is a valid NUL-terminated string for the call.
    let rc =
        unsafe { libc::faccessat(libc::AT_FDCWD, cpath.as_ptr(), libc::X_OK, libc::AT_EACCESS) };
    if rc == 0 {
        return Ok(());
    }
    let err = io::Error::last_os_error();
    match err.raw_os_error() {
        Some(libc::ENOSYS) | Some(libc::EPERM) => {
            if meta.permissions().mode() & 0o111 != 0 {
                Ok(())
            } else {
                Err("permission denied".to_string())
            }
        }
        _ => Err(gostd::os_error_text(&err)),
    }
}

/// Placeholder until P5 (Go's Windows LookPath also tries `PATHEXT`).
#[cfg(not(unix))]
fn find_executable(path: &Path) -> Result<(), String> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_dir() => Err("is a directory".to_string()),
        Ok(_) => Ok(()),
        Err(e) => Err(format!(
            "stat {}: {}",
            path.display(),
            gostd::os_error_text(&e)
        )),
    }
}

/// Starts the provider in its own process group (Go `Setpgid: true`), so a
/// cancellation can SIGKILL the launcher and every descendant that stayed in
/// the group. `process_group(0)` is std's `setpgid(0, 0)` in the child.
#[cfg(unix)]
pub(crate) fn configure_group(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
}

/// Go is a no-op on Windows; P5 assigns a Job Object instead.
#[cfg(not(unix))]
pub(crate) fn configure_group(_cmd: &mut Command) {}

/// Go: `cmd.Args[0]` stays the name the caller gave, not the resolved path.
#[cfg(unix)]
pub(crate) fn set_arg0(cmd: &mut Command, arg0: &str) {
    use std::os::unix::process::CommandExt;
    cmd.arg0(arg0);
}

#[cfg(not(unix))]
pub(crate) fn set_arg0(_cmd: &mut Command, _arg0: &str) {}

/// Result of the group kill, as Go's `cmd.Cancel` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KillOutcome {
    /// The signal was sent.
    Sent,
    /// ESRCH: Go maps it to `os.ErrProcessDone`.
    Done,
    /// Any other errno text.
    Failed(String),
}

/// An exit as Go's `ProcessState` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ExitState {
    /// Go `ExitCode()`: the code, or -1 when a signal ended the process.
    pub(crate) code: i64,
    pub(crate) success: bool,
}

/// The started provider. It stays unreaped until [`ProcessHandle::try_reap`]
/// returns its status, so its PID and process group ID cannot be reused while
/// [`ProcessHandle::kill_group`] may still run. After a reap no signal is sent.
pub(crate) struct ProcessHandle {
    #[cfg(unix)]
    pid: i32,
    #[cfg(unix)]
    reaped: bool,
    #[cfg(not(unix))]
    child: std::process::Child,
}

#[cfg(unix)]
impl ProcessHandle {
    /// Takes ownership of the child's reaping. std's `Child` neither waits
    /// nor kills on drop, so dropping it here leaves the PID with us.
    pub(crate) fn new(child: std::process::Child) -> ProcessHandle {
        ProcessHandle {
            pid: child.id() as i32,
            reaped: false,
        }
    }

    pub(crate) fn pid(&self) -> i32 {
        self.pid
    }

    /// Go's `cmd.Cancel`: `kill(-pid, SIGKILL)`; with setpgid the group ID
    /// equals the leader's PID.
    pub(crate) fn kill_group(&mut self) -> KillOutcome {
        if self.reaped {
            return KillOutcome::Done;
        }
        // SAFETY: plain syscall. The leader is unreaped, so the group ID
        // still names our provider group.
        if unsafe { libc::kill(-self.pid, libc::SIGKILL) } == 0 {
            return KillOutcome::Sent;
        }
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ESRCH) {
            KillOutcome::Done
        } else {
            KillOutcome::Failed(gostd::os_error_text(&err))
        }
    }

    /// `waitpid(pid, WNOHANG)`, EINTR retried. `Ok(None)` = still running.
    pub(crate) fn try_reap(&mut self) -> io::Result<Option<ExitState>> {
        let mut status = 0;
        loop {
            // SAFETY: status is a valid out pointer.
            let rc = unsafe { libc::waitpid(self.pid, &mut status, libc::WNOHANG) };
            if rc == 0 {
                return Ok(None);
            }
            if rc == self.pid {
                self.reaped = true;
                let code = if libc::WIFEXITED(status) {
                    i64::from(libc::WEXITSTATUS(status))
                } else {
                    -1
                };
                return Ok(Some(ExitState {
                    code,
                    success: libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
                }));
            }
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            if err.raw_os_error() == Some(libc::ECHILD) {
                // Someone else reaped it; never signal this PID again.
                self.reaped = true;
            }
            return Err(err);
        }
    }
}

/// Go `Process.Wait` error prefix (`NewSyscallError`).
#[cfg(target_os = "linux")]
pub(crate) const WAIT_SYSCALL: &str = "waitid";
#[cfg(not(target_os = "linux"))]
pub(crate) const WAIT_SYSCALL: &str = "wait";

/// Go `os.Pipe` error prefix (`NewSyscallError`).
#[cfg(target_os = "linux")]
pub(crate) const PIPE_SYSCALL: &str = "pipe2";
#[cfg(not(target_os = "linux"))]
pub(crate) const PIPE_SYSCALL: &str = "pipe";

#[cfg(not(unix))]
impl ProcessHandle {
    pub(crate) fn new(child: std::process::Child) -> ProcessHandle {
        ProcessHandle { child }
    }

    pub(crate) fn pid(&self) -> i32 {
        self.child.id() as i32
    }

    /// Placeholder until P5: kills the direct child only (Go's default
    /// `Cancel` on Windows); a Job Object will kill the whole tree.
    pub(crate) fn kill_group(&mut self) -> KillOutcome {
        match self.child.kill() {
            Ok(()) => KillOutcome::Sent,
            Err(e) if e.kind() == io::ErrorKind::InvalidInput => KillOutcome::Done,
            Err(e) => KillOutcome::Failed(gostd::os_error_text(&e)),
        }
    }

    pub(crate) fn try_reap(&mut self) -> io::Result<Option<ExitState>> {
        Ok(self.child.try_wait()?.map(|status| ExitState {
            code: status.code().map_or(-1, i64::from),
            success: status.success(),
        }))
    }
}

/// Ends worker pipe IO once the drain grace has passed. Go closes the pipe
/// files; closing a descriptor from another thread does not reliably
/// interrupt a blocked read on Linux (close(2)), so the workers use
/// nonblocking IO and bounded poll waits that recheck this flag.
#[derive(Default)]
pub(crate) struct Abort(std::sync::atomic::AtomicBool);

impl Abort {
    pub(crate) fn fire(&self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub(crate) fn is_fired(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// How long a worker's poll may sleep before it rechecks [`Abort`].
#[cfg(unix)]
const POLL_SLICE_MS: libc::c_int = 50;

/// One step of pipe IO.
#[derive(Debug)]
pub(crate) enum Io {
    Done(usize),
    /// Go's read/write on a file closed by the drain grace.
    Aborted,
    Err(io::Error),
}

/// Puts a parent-side pipe end into nonblocking mode. The child's end is a
/// separate open file description and stays blocking.
/// The cancellation bound depends on it, so a failure is an error, not a
/// silent fallback to blocking IO. EINTR is retried.
#[cfg(unix)]
pub(crate) fn set_nonblocking(fd: &impl std::os::fd::AsRawFd) -> io::Result<()> {
    let fd = fd.as_raw_fd();
    let fcntl = |cmd: libc::c_int, arg: libc::c_int| loop {
        // SAFETY: fcntl on a descriptor we own.
        let rc = unsafe { libc::fcntl(fd, cmd, arg) };
        if rc >= 0 {
            return Ok(rc);
        }
        let err = io::Error::last_os_error();
        if err.kind() != io::ErrorKind::Interrupted {
            return Err(err);
        }
    };
    let flags = fcntl(libc::F_GETFL, 0)?;
    fcntl(libc::F_SETFL, flags | libc::O_NONBLOCK)?;
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn set_nonblocking<T>(_fd: &T) -> io::Result<()> {
    Ok(())
}

/// Waits until `fd` is ready for `events` or the abort fires. Returns false
/// on abort.
#[cfg(unix)]
fn wait_ready(fd: std::os::fd::RawFd, events: libc::c_short, abort: &Abort) -> bool {
    loop {
        if abort.is_fired() {
            return false;
        }
        let mut pfd = libc::pollfd {
            fd,
            events,
            revents: 0,
        };
        // SAFETY: pfd is one valid pollfd.
        let rc = unsafe { libc::poll(&mut pfd, 1, POLL_SLICE_MS) };
        if rc > 0 {
            // Readable/writable, or POLLHUP/POLLERR: the IO call reports it.
            return true;
        }
        if rc < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            // Let the IO call itself report the descriptor state.
            return true;
        }
    }
}

/// Reads once. Blocks (interruptibly) until data, EOF, an error, or abort.
pub(crate) fn read_some(r: &std::io::PipeReader, buf: &mut [u8], abort: &Abort) -> Io {
    use std::io::Read;
    loop {
        if abort.is_fired() {
            return Io::Aborted;
        }
        match (&*r).read(buf) {
            Ok(n) => return Io::Done(n),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            #[cfg(unix)]
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                use std::os::fd::AsRawFd;
                if !wait_ready(r.as_raw_fd(), libc::POLLIN, abort) {
                    return Io::Aborted;
                }
            }
            Err(e) => return Io::Err(e),
        }
    }
}

/// Writes once (possibly short). Blocks (interruptibly) like [`read_some`].
/// An empty `buf` still makes one write call, as Go's `poll.FD.Write` does.
pub(crate) fn write_some(w: &std::io::PipeWriter, buf: &[u8], abort: &Abort) -> Io {
    use std::io::Write;
    loop {
        if abort.is_fired() {
            return Io::Aborted;
        }
        match (&*w).write(buf) {
            Ok(n) => return Io::Done(n),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            #[cfg(unix)]
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                use std::os::fd::AsRawFd;
                if !wait_ready(w.as_raw_fd(), libc::POLLOUT, abort) {
                    return Io::Aborted;
                }
            }
            Err(e) => return Io::Err(e),
        }
    }
}

/// Go `(*os.File).Close` with its error: std's `File` drop ignores it.
#[cfg(unix)]
pub(crate) fn close_file(file: std::fs::File) -> io::Result<()> {
    use std::os::fd::IntoRawFd;
    let fd = file.into_raw_fd();
    // SAFETY: we own fd and close it exactly once.
    if unsafe { libc::close(fd) } == 0 {
        return Ok(());
    }
    let err = io::Error::last_os_error();
    // Go's poll.FD ignores EINTR from close: the descriptor is gone either way.
    if err.kind() == io::ErrorKind::Interrupted {
        return Ok(());
    }
    Err(err)
}

#[cfg(not(unix))]
pub(crate) fn close_file(file: std::fs::File) -> io::Result<()> {
    drop(file);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn look_path_error_text_matches_go() {
        let err = look_path("no-such-rival-binary", Some(OsStr::new("/nonexistent"))).unwrap_err();
        assert_eq!(
            err.to_string(),
            r#"exec: "no-such-rival-binary": executable file not found in $PATH"#
        );
        // Unset and empty PATH search nothing.
        assert!(look_path("sh", None).is_err());
        assert!(look_path("sh", Some(OsStr::new(""))).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn look_path_follows_go_unix_rules() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let exe = bin.join("tool");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let plain = bin.join("plain");
        std::fs::write(&plain, "x").unwrap();
        std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::create_dir(bin.join("dir")).unwrap();

        let path: OsString = format!("/nonexistent::{}", bin.display()).into();
        assert_eq!(look_path("tool", Some(&path)).unwrap(), exe);
        // Not executable and directories are skipped.
        assert_eq!(
            look_path("plain", Some(&path)).unwrap_err().err,
            "executable file not found in $PATH"
        );
        assert!(look_path("dir", Some(&path)).is_err());

        // A slash bypasses $PATH and reports the precise reason.
        let exe_s = exe.to_str().unwrap();
        assert_eq!(look_path(exe_s, None).unwrap(), exe);
        let plain_s = plain.to_str().unwrap();
        assert_eq!(
            look_path(plain_s, None).unwrap_err().to_string(),
            format!("exec: \"{plain_s}\": permission denied")
        );
        let dir_s = bin.join("dir");
        let dir_s = dir_s.to_str().unwrap();
        assert_eq!(
            look_path(dir_s, None).unwrap_err().to_string(),
            format!("exec: \"{dir_s}\": is a directory")
        );
        let missing = bin.join("missing");
        let missing = missing.to_str().unwrap();
        assert_eq!(
            look_path(missing, None).unwrap_err().to_string(),
            format!("exec: \"{missing}\": stat {missing}: no such file or directory")
        );

        // A relative $PATH hit is Go's ErrDot. The process cwd is never
        // changed: the relative dir climbs from the cwd to "/" first.
        let cwd = std::env::current_dir().unwrap();
        let mut rel = PathBuf::new();
        for _ in cwd.components().skip(1) {
            rel.push("..");
        }
        rel.push(bin.strip_prefix("/").unwrap());
        let err = look_path("tool", Some(rel.as_os_str())).unwrap_err();
        assert_eq!(err.err, ERR_DOT);
        assert_eq!(err.dot_path, Some(paths::clean(&rel.join("tool"))));
        assert!(!err.dot_path.unwrap().is_absolute());
    }
}
