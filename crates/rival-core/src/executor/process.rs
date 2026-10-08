//! Platform process operations for provider subprocesses: executable lookup,
//! the provider process group, pipe IO that a cancellation can interrupt,
//! and reaping.
//!
//! Unix uses a process group and nonblocking pipes with bounded polls.
//! Windows ([`windows`]) uses an owner Job plus one nested Job per provider,
//! and overlapped pipes whose IO an abort event cancels.

#[cfg(all(test, target_os = "macos"))]
mod spawn_tests;
#[cfg(windows)]
pub mod windows;
#[cfg(all(test, windows))]
mod windows_tests;

use std::ffi::OsStr;
use std::fmt;
use std::io::{self, PipeReader, PipeWriter};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};

#[cfg(not(windows))]
use crate::paths;

/// Go `syscall.ForkLock` on macOS. std's `io::pipe` there is `pipe(2)`
/// followed by two separate close-on-exec calls; a fork between them gives
/// the child both pipe ends. Pipe creation holds this lock shared, every
/// Rival spawn holds it exclusively, as Go's `os.Pipe` and `forkExec` do.
///
/// It covers only Rival's own [`pipe`] and [`spawn`] calls. A library that
/// creates descriptors or forks on its own does not take it.
#[cfg(target_os = "macos")]
static FORK_LOCK: std::sync::RwLock<()> = std::sync::RwLock::new(());

/// Runs `create` while no Rival spawn can fork (Go `ForkLock.RLock`).
/// Other platforms keep std's behavior unchanged and take no lock: Linux
/// uses `pipe2(O_CLOEXEC)`, Windows creates non-inheritable handles.
fn with_pipe_lock<T>(create: impl FnOnce() -> T) -> T {
    #[cfg(target_os = "macos")]
    let _guard = FORK_LOCK
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    create()
}

/// Go `os.Pipe`: std's `io::pipe` under the fork lock. Every pipe Rival
/// creates outside a spawn goes through here.
pub fn pipe() -> io::Result<(PipeReader, PipeWriter)> {
    with_pipe_lock(io::pipe)
}

/// `cmd.spawn()` under the fork lock (Go `forkExec`'s `acquireForkLock`).
/// The lock covers std's own stdio and error pipes, which it creates inside
/// `spawn`, and is released when the child has exec'd or failed to; never
/// held while the caller waits for or reads from the child. Nothing locks
/// in the forked child. Other platforms spawn exactly as std does.
///
/// Callers keep std's stream defaults: an unset stream is inherited, as
/// with `Command::spawn`. A converted `output()` sets null stdin and piped
/// stdout/stderr itself, then calls `Child::wait_with_output`.
pub fn spawn(cmd: &mut Command) -> io::Result<Child> {
    #[cfg(target_os = "macos")]
    let _guard = FORK_LOCK
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cmd.spawn()
}

/// The parent's ends of a provider's stdio pipes. Unix: [`pipe`] (the caller
/// then makes its ends nonblocking). Windows: the parent end is overlapped
/// ([`windows::overlapped_pipe`]); only [`read_some`]/[`write_some`] may do
/// IO on it.
pub(crate) fn provider_pipe(parent_reads: bool) -> io::Result<(PipeReader, PipeWriter)> {
    #[cfg(windows)]
    return windows::overlapped_pipe(parent_reads);
    #[cfg(not(windows))]
    {
        let _ = parent_reads;
        pipe()
    }
}

/// Starts a provider: its own process group on Unix; on Windows inside the
/// owner Job and then its own nested Job (see [`windows`]). The caller set
/// the group attributes with [`configure_group`] first.
pub(crate) fn start_provider(cmd: &mut Command) -> io::Result<ProcessHandle> {
    #[cfg(windows)]
    {
        let (child, job) = windows::spawn_contained(cmd)?;
        Ok(windows::ProcessHandle::new(child, job))
    }
    #[cfg(not(windows))]
    spawn(cmd).map(ProcessHandle::new)
}

/// The program path to start when the child runs in `dir` (Go `cmd.Dir`).
/// Unix `execve` resolves a relative program after the `chdir`, as Go does,
/// so the path is kept. Windows makes it absolute against `dir` with Go's
/// `StartProcess` rule ([`windows::program_in_dir`]); its error fails the
/// start like Go's.
pub fn program_in_dir(program: &Path, dir: &Path) -> io::Result<PathBuf> {
    #[cfg(windows)]
    return windows::program_in_dir(program, dir);
    #[cfg(not(windows))]
    {
        let _ = dir;
        Ok(program.to_path_buf())
    }
}

/// Go `exec.ErrNotFound`.
#[cfg(not(windows))]
const ERR_NOT_FOUND: &str = "executable file not found in $PATH";
#[cfg(windows)]
use windows::ERR_NOT_FOUND;
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
        write!(f, "exec: {:?}: {}", self.name, self.err)
    }
}

impl std::error::Error for LookPathError {}

/// Go `exec.LookPath` (Unix) with an explicit `$PATH` value (`None` = unset).
/// A name containing `/` is checked as-is. Otherwise each `$PATH` element is
/// tried in order; an empty element means `.`, and a hit that is not
/// absolute is an `ErrDot` error.
///
/// Windows follows Go's Windows `LookPath` instead (`PATHEXT`, the implicit
/// current-directory hit, `%PATH%` quoting); see [`windows::look_path_exts`].
/// `PATHEXT` and `NoDefaultCurrentDirectoryInExePath` come from the process
/// environment, as Go reads them.
#[cfg(windows)]
pub fn look_path(file: &str, path_env: Option<&OsStr>) -> Result<PathBuf, LookPathError> {
    if matches!(file, "" | "." | "..") {
        // Go `validateLookPath`.
        return Err(LookPathError {
            name: file.to_string(),
            err: ERR_NOT_FOUND.to_string(),
            dot_path: None,
        });
    }
    let env = windows::LookEnv::process();
    windows::look_path_exts(
        file,
        &windows::path_ext(env.path_ext.as_deref()),
        path_env,
        env.no_dot,
        windows::same_file,
    )
}

#[cfg(not(windows))]
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

    let meta = std::fs::metadata(path).map_err(|e| format!("stat {}: {}", path.display(), e))?;
    if meta.is_dir() {
        return Err(io::Error::from_raw_os_error(libc::EISDIR).to_string());
    }
    let cpath = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::from_raw_os_error(libc::EINVAL).to_string())?;
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
                Err(io::Error::from_raw_os_error(libc::EACCES).to_string())
            }
        }
        _ => Err(err.to_string()),
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

/// Go's Windows `setProcessGroup` is a no-op. Rust contains the provider in
/// Jobs instead, set up by [`start_provider`].
#[cfg(windows)]
pub(crate) fn configure_group(_cmd: &mut Command) {}

/// Makes `cmd` start `program` the way Go's `syscall.forkExec` does: a raw
/// `execve(program, [arg0, args...], env)`. `arg0` is the name the caller
/// gave (Go's `cmd.Args[0]`), not the resolved path. `env` is the final,
/// deduped `KEY=VALUE` list; it is passed as-is, in order, and an entry
/// without `=` is kept, as Go does.
///
/// Why not std's own exec: std ends in libc `posix_spawnp` or `execvp`, and
/// Apple's libc retries an ENOEXEC image (an executable text file without a
/// shebang) through `/bin/sh`. Go reports `exec format error` instead.
///
/// The `pre_exec` hook runs in the forked child after std has set up the
/// stdio, the cwd, the process group and the SIGPIPE reset, and before std's
/// own env swap and `execvp`. A hook also turns off std's `posix_spawn`
/// path. If `execve` returns, the hook returns its errno, and std reports it
/// to the parent through its launch-error pipe as a spawn error.
///
/// A NUL in `program`, `arg0` or an argument is EINVAL here, before the
/// spawn, as in Go's `forkExec`. `env` was already checked by `dedup_env`.
#[cfg(unix)]
pub(crate) fn set_exec<S: AsRef<OsStr>>(
    cmd: &mut Command,
    program: &Path,
    arg0: &str,
    args: &[S],
    env: &[std::ffi::OsString],
) -> io::Result<()> {
    use std::os::unix::process::CommandExt;

    let image = ExecImage::new(program, arg0, args, env)?;
    // SAFETY: the hook only calls execve on pointers built in the parent and
    // reads errno. It allocates, locks, formats and drops nothing in the child.
    unsafe {
        cmd.pre_exec(move || Err(image.exec()));
    }
    Ok(())
}

/// Windows: std's own `CreateProcessW`, which keeps std's argument quoting
/// and its safe `.bat`/`.cmd` handling (through `cmd.exe`). `env` replaces
/// the whole environment, plus Go's `addCriticalEnv`: `SYSTEMROOT` from this
/// process when `env` has none.
///
/// Differences from Go, which builds the command line itself: the child's
/// first command-line word is the program path std was given, not `arg0`;
/// and std drops an entry without `=` (Go would pass it through).
#[cfg(windows)]
pub(crate) fn set_exec<S: AsRef<OsStr>>(
    cmd: &mut Command,
    _program: &Path,
    _arg0: &str,
    args: &[S],
    env: &[std::ffi::OsString],
) -> io::Result<()> {
    cmd.args(args);
    let env = add_critical_env(env, std::env::var_os("SYSTEMROOT"));
    super::subprocess::set_env(cmd, &env);
    Ok(())
}

/// Go `exec.addCriticalEnv` (Windows): appends `SYSTEMROOT=<systemroot>` when
/// no entry has that key (ASCII case ignored). Pure, for tests on every
/// platform.
pub fn add_critical_env(
    env: &[std::ffi::OsString],
    systemroot: Option<std::ffi::OsString>,
) -> Vec<std::ffi::OsString> {
    let mut env = env.to_vec();
    let has = env.iter().any(|kv| {
        let bytes = kv.as_encoded_bytes();
        bytes
            .iter()
            .position(|&b| b == b'=')
            .is_some_and(|i| bytes[..i].eq_ignore_ascii_case(b"SYSTEMROOT"))
    });
    if !has {
        let mut kv = std::ffi::OsString::from("SYSTEMROOT=");
        kv.push(systemroot.unwrap_or_default());
        env.push(kv);
    }
    env
}

/// The `execve` arguments, built in the parent so the child only reads them.
#[cfg(unix)]
struct ExecImage {
    path: std::ffi::CString,
    /// Owns the argv and env strings. The pointer arrays point into their
    /// heap buffers, which stay put when this Vec moves.
    _strings: Vec<std::ffi::CString>,
    /// NULL-terminated.
    argv: Vec<*const libc::c_char>,
    /// NULL-terminated.
    envp: Vec<*const libc::c_char>,
}

// SAFETY: std needs the hook to be Send + Sync. The raw pointers point only
// into `_strings`, which the image owns and nobody mutates; only the forked
// child reads them, in `exec`.
#[cfg(unix)]
unsafe impl Send for ExecImage {}
#[cfg(unix)]
unsafe impl Sync for ExecImage {}

#[cfg(unix)]
impl ExecImage {
    fn new<S: AsRef<OsStr>>(
        program: &Path,
        arg0: &str,
        args: &[S],
        env: &[std::ffi::OsString],
    ) -> io::Result<ExecImage> {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let cstring = |s: &OsStr| {
            CString::new(s.as_bytes()).map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))
        };
        let path = cstring(program.as_os_str())?;
        let mut strings = Vec::with_capacity(1 + args.len() + env.len());
        strings.push(cstring(OsStr::new(arg0))?);
        for arg in args {
            strings.push(cstring(arg.as_ref())?);
        }
        for kv in env {
            strings.push(cstring(kv)?);
        }
        let ptrs = |s: &[CString]| {
            s.iter()
                .map(|c| c.as_ptr())
                .chain(std::iter::once(std::ptr::null()))
                .collect::<Vec<_>>()
        };
        let argv = ptrs(&strings[..1 + args.len()]);
        let envp = ptrs(&strings[1 + args.len()..]);
        Ok(ExecImage {
            path,
            _strings: strings,
            argv,
            envp,
        })
    }

    /// Runs in the forked child: replaces the process image, or returns the
    /// errno. `last_os_error` only reads errno; it allocates nothing.
    fn exec(&self) -> io::Error {
        // SAFETY: every pointer is a NUL-terminated string owned by `self`,
        // and both arrays end in NULL.
        unsafe { libc::execve(self.path.as_ptr(), self.argv.as_ptr(), self.envp.as_ptr()) };
        io::Error::last_os_error()
    }
}

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
#[cfg(unix)]
pub(crate) struct ProcessHandle {
    pid: i32,
    reaped: bool,
}

#[cfg(windows)]
pub(crate) use windows::ProcessHandle;

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
            KillOutcome::Failed(err.to_string())
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
#[cfg(windows)]
pub(crate) const WAIT_SYSCALL: &str = "WaitForSingleObject";
#[cfg(not(any(target_os = "linux", windows)))]
pub(crate) const WAIT_SYSCALL: &str = "wait";

/// Go `os.Pipe` error prefix (`NewSyscallError`).
#[cfg(target_os = "linux")]
pub(crate) const PIPE_SYSCALL: &str = "pipe2";
#[cfg(not(target_os = "linux"))]
pub(crate) const PIPE_SYSCALL: &str = "pipe";

/// Ends worker pipe IO once the drain grace has passed. Go closes the pipe
/// files; closing a descriptor from another thread does not reliably
/// interrupt a blocked read on Linux (close(2)), so the Unix workers use
/// nonblocking IO and bounded poll waits that recheck this flag. Windows
/// workers wait on their overlapped IO and this manual-reset event together,
/// so a fire at any moment ends the wait (see `windows::overlapped_io`).
pub(crate) struct Abort {
    fired: std::sync::atomic::AtomicBool,
    #[cfg(windows)]
    event: std::os::windows::io::OwnedHandle,
}

impl Abort {
    /// Windows creates the event here; that can fail.
    pub(crate) fn new() -> io::Result<Abort> {
        Ok(Abort {
            fired: std::sync::atomic::AtomicBool::new(false),
            #[cfg(windows)]
            event: windows::manual_event()?,
        })
    }

    pub(crate) fn fire(&self) {
        self.fired.store(true, std::sync::atomic::Ordering::SeqCst);
        #[cfg(windows)]
        windows::set_event(&self.event);
    }

    pub(crate) fn is_fired(&self) -> bool {
        self.fired.load(std::sync::atomic::Ordering::SeqCst)
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
pub fn set_nonblocking(fd: &impl std::os::fd::AsRawFd) -> io::Result<()> {
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

/// Windows needs no mode change: provider pipes are overlapped instead
/// ([`provider_pipe`]).
#[cfg(windows)]
pub fn set_nonblocking<T>(_fd: &T) -> io::Result<()> {
    Ok(())
}

#[cfg(windows)]
pub(crate) use windows::{close_file, read_some, write_some};

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
#[cfg(unix)]
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
#[cfg(unix)]
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

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::ffi::OsString;

    #[test]
    fn look_path_error_text_matches_go() {
        let err = look_path("no-such-rival-binary", Some(OsStr::new("/nonexistent"))).unwrap_err();
        let path_var = if cfg!(windows) { "%PATH%" } else { "$PATH" };
        assert_eq!(
            err.to_string(),
            format!(r#"exec: "no-such-rival-binary": executable file not found in {path_var}"#)
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
            format!("exec: \"{plain_s}\": Permission denied (os error 13)")
        );
        let dir_s = bin.join("dir");
        let dir_s = dir_s.to_str().unwrap();
        assert_eq!(
            look_path(dir_s, None).unwrap_err().to_string(),
            format!("exec: \"{dir_s}\": Is a directory (os error 21)")
        );
        let missing = bin.join("missing");
        let missing = missing.to_str().unwrap();
        assert_eq!(
            look_path(missing, None).unwrap_err().to_string(),
            format!("exec: \"{missing}\": stat {missing}: No such file or directory (os error 2)")
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

    #[test]
    fn add_critical_env_matches_go() {
        let env = |items: &[&str]| -> Vec<std::ffi::OsString> {
            items.iter().map(std::ffi::OsString::from).collect()
        };
        let root = Some(std::ffi::OsString::from(r"C:\Windows"));
        // Present in any case: unchanged.
        let have = env(&["A=1", "SystemRoot=X"]);
        assert_eq!(add_critical_env(&have, root.clone()), have);
        // Missing: appended from the process value, empty when unset.
        assert_eq!(
            add_critical_env(&env(&["A=1", "SYSTEMROOTX=1"]), root),
            env(&["A=1", "SYSTEMROOTX=1", r"SYSTEMROOT=C:\Windows"])
        );
        assert_eq!(add_critical_env(&[], None), env(&["SYSTEMROOT="]));
    }
}
