//! Go: `internal/executor/subprocess.go`, with `subprocess_unix.go` and
//! `subprocess_other.go` behind [`super::process`].
//!
//! Go's goroutines map to scoped threads: one writes the prompt, one tees
//! stdout, one copies stderr. The calling thread plays `exec.Cmd`'s context
//! watcher: it kills the provider group on cancellation, bounds the drain,
//! then reaps. Only this thread signals or reaps, and it never signals after
//! the reap, so a recycled PID is never hit. All threads end before return.

#[cfg(all(test, unix))]
mod tests;
#[cfg(all(test, windows))]
mod windows_tests;

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, PipeReader, PipeWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail};

use super::process::{self, Abort, ExitState, Io, KillOutcome, ProcessHandle};
use crate::cancel::{CancelFunc, Context};
use crate::gitscope;
use crate::gostd;
use crate::logging::{self, Event};
use crate::paths::Paths;
use crate::procinfo;
use crate::session::{self, Session};

/// Env var prefixes that should not leak from .env to child CLIs.
/// `OPENCODE_` is blocked because a reviewed repo's .env could otherwise set
/// `OPENCODE_PERMISSION` (or `OPENCODE_CONFIG*`) to defeat the read-only
/// reviewer sandbox — the reviewer must never take permission/config from the
/// code it reviews. Moonshot credentials are blocked because the .env loader
/// may load them from a reviewed repo. The key reaches OpenCode through
/// `OPENCODE_CONFIG_CONTENT` instead, so no child needs the raw source
/// variable. `GROK_` and `XAI_` are blocked for the same reason: a reviewed
/// repo's .env must not be able to reconfigure the authenticated grok runtime
/// — proxy/base URLs, `GROK_HOME`, auth helpers — and `XAI_API_KEY` fallback
/// auth is deliberately unsupported, since grok runs on the user's own
/// `grok login` credentials.
const BLOCKED_ENV_PREFIXES: [&str; 16] = [
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "no_proxy",
    "NODE_OPTIONS",
    "LD_PRELOAD",
    "DYLD_",
    "OPENCODE_",
    "KIMI_",
    "MOONSHOT_",
    "GROK_",
    "XAI_",
];

/// Bounds how long [`run_subprocess`] keeps reading the provider's output
/// after its context is cancelled. The process-group kill normally closes
/// every pipe writer at once; this only matters when something outside the
/// group (a provider child that called setsid) still holds them.
pub const PIPE_DRAIN_GRACE: Duration = Duration::from_secs(5);

/// Go's `os.Pipe` names: the read end is `|0`, the write end `|1`.
const PIPE_READ_NAME: &str = "|0";
const PIPE_WRITE_NAME: &str = "|1";
/// Go `os.ErrClosed`: IO on a pipe file closed by the drain grace.
const ERR_CLOSED: &str = "file already closed";

const STDOUT_BUF: usize = 64 * 1024;
/// Go `io.Copy`'s buffer.
const STDERR_BUF: usize = 32 * 1024;
/// Upper bound of one sleep while polling for the leader's exit.
const REAP_POLL_MAX: Duration = Duration::from_millis(50);

/// Go `executor.Result`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunResult {
    pub exit_code: i64,
    pub output_bytes: i64,
    pub output_lines: i64,
}

/// The arguments of Go's `RunSubprocess`, minus the context, session and
/// mirror. No `Debug`: it carries the prompt and credential env.
#[derive(Clone, Copy)]
pub struct Request<'a> {
    pub binary: &'a str,
    pub args: &'a [String],
    /// `KEY=VALUE` entries appended after the filtered inherited env.
    pub env: &'a [String],
    pub prompt: &'a str,
    /// Vars removed from the inherited env before `env` is appended. An
    /// entry ending in `_` drops every variable with that prefix.
    pub drop_env: &'a [&'a str],
    /// Go `os.Environ()`: the inherited env in order (`KEY=VALUE`). It is
    /// also where `$PATH` for the binary lookup comes from. Production code
    /// passes [`crate::config::Config::environ`].
    pub environ: &'a [OsString],
}

/// Go `safeEnv`: `environ` without Git repository overrides and without the
/// blocked prefixes that a repo-local .env could inject.
pub fn safe_env(environ: &[OsString]) -> Vec<OsString> {
    gitscope::repository_env(environ)
        .into_iter()
        .filter(|kv| {
            let bytes = kv.as_encoded_bytes();
            !BLOCKED_ENV_PREFIXES
                .iter()
                .any(|prefix| bytes.starts_with(prefix.as_bytes()))
        })
        .collect()
}

/// Go `dropMatches`: whether `kv` (a `KEY=VALUE` entry) is named by
/// `drop_env`. An entry ending in `_` is a prefix match ("AWS_" drops every
/// AWS_* var — exact name lists rot as providers add credential vars); any
/// other entry matches that exact variable name.
pub(crate) fn drop_matches(kv: &OsStr, drop_env: &[&str]) -> bool {
    let bytes = kv.as_encoded_bytes();
    drop_env.iter().any(|name| {
        if name.ends_with('_') {
            bytes.starts_with(name.as_bytes())
        } else {
            bytes.starts_with(name.as_bytes()) && bytes.get(name.len()) == Some(&b'=')
        }
    })
}

/// The env the child gets: Go's `append(base, env...)` after `safeEnv` and
/// `dropEnv`.
pub fn child_env(req: &Request<'_>) -> Vec<OsString> {
    let mut base = safe_env(req.environ);
    if !req.drop_env.is_empty() {
        base.retain(|kv| !drop_matches(kv, req.drop_env));
    }
    base.extend(req.env.iter().map(OsString::from));
    base
}

/// Where Go splits an env entry into key and value (`dedupEnvCase`): the
/// first `=`, but a leading `=` belongs to the key. `None` = no `=`.
fn env_key_end(kv: &[u8]) -> Option<usize> {
    match kv.iter().position(|&b| b == b'=') {
        Some(0) => Some(kv[1..].iter().position(|&b| b == b'=').map_or(0, |i| i + 1)),
        other => other,
    }
}

/// Go `exec.dedupEnv`: the last value of each key wins, in the order of
/// each key's last occurrence. An entry without `=` is kept as-is. An entry
/// with a NUL byte is skipped and reported. Windows compares keys case
/// insensitively (Go `strings.ToLower`).
pub(crate) fn dedup_env(env: &[OsString]) -> Result<Vec<OsString>, String> {
    dedup_env_case(cfg!(windows), env)
}

/// Go `exec.dedupEnvCase` (`nulOK` false).
pub(crate) fn dedup_env_case(
    case_insensitive: bool,
    env: &[OsString],
) -> Result<Vec<OsString>, String> {
    let mut err = None;
    let mut out = Vec::with_capacity(env.len());
    let mut saw = std::collections::HashSet::new();
    for kv in env.iter().rev() {
        let bytes = kv.as_encoded_bytes();
        if bytes.contains(&0) {
            err = Some("exec: environment variable contains NUL".to_string());
            continue;
        }
        let Some(i) = env_key_end(bytes) else {
            if !bytes.is_empty() {
                out.push(kv.clone());
            }
            continue;
        };
        let key = &bytes[..i];
        let key = if case_insensitive {
            match std::str::from_utf8(key) {
                Ok(k) => gostd::to_lower(k).into_bytes(),
                Err(_) => key.to_ascii_lowercase(),
            }
        } else {
            key.to_vec()
        };
        if saw.insert(key) {
            out.push(kv.clone());
        }
    }
    out.reverse();
    err.map_or(Ok(out), Err)
}

/// Replaces `cmd`'s whole environment with `env` (`KEY=VALUE` entries).
/// Unix passes the env to `execve` itself (see [`process::set_exec`]).
#[cfg(windows)]
pub(crate) fn set_env(cmd: &mut Command, env: &[OsString]) {
    cmd.env_clear();
    for kv in env {
        let bytes = kv.as_encoded_bytes();
        // Rust's Command needs a key and a value; Go would pass an
        // entry without "=" through unchanged.
        if let Some(i) = env_key_end(bytes) {
            // SAFETY: both halves split at an ASCII '='.
            let (k, v) = unsafe {
                (
                    OsStr::from_encoded_bytes_unchecked(&bytes[..i]),
                    OsStr::from_encoded_bytes_unchecked(&bytes[i + 1..]),
                )
            };
            cmd.env(k, v);
        }
    }
}

/// Go `os.Getenv` on an `os.Environ()` list: the first entry wins. Windows
/// names are case-insensitive (ASCII), as `GetEnvironmentVariableW` treats
/// them.
pub(crate) fn getenv<'a>(environ: &'a [OsString], key: &str) -> Option<&'a OsStr> {
    getenv_case(cfg!(windows), environ, key)
}

pub(crate) fn getenv_case<'a>(
    case_insensitive: bool,
    environ: &'a [OsString],
    key: &str,
) -> Option<&'a OsStr> {
    environ.iter().find_map(|kv| {
        let bytes = kv.as_encoded_bytes();
        let head = bytes.get(..key.len())?;
        let same = if case_insensitive {
            head.eq_ignore_ascii_case(key.as_bytes())
        } else {
            head == key.as_bytes()
        };
        if !same {
            return None;
        }
        let rest = bytes[key.len()..].strip_prefix(b"=")?;
        // SAFETY: a suffix after an ASCII '=' of a valid encoded OsStr.
        Some(unsafe { OsStr::from_encoded_bytes_unchecked(rest) })
    })
}

/// Go `exec.Command`: a name without a separator is looked up in `$PATH`
/// now; the error is reported by `Start`.
#[cfg(not(windows))]
fn resolve(binary: &str, environ: &[OsString], _dir: &str) -> Result<PathBuf, String> {
    if binary.is_empty() {
        return Err("exec: no command".to_string());
    }
    if binary.contains('/') {
        return Ok(PathBuf::from(binary));
    }
    process::look_path(binary, getenv(environ, "PATH")).map_err(|e| e.to_string())
}

/// Go `exec.Command` plus `Start` on Windows: a bare name (`filepath.Base`
/// of itself) is looked up in `%PATH%` with `PATHEXT`; any other name gets
/// its `PATHEXT` extension from `lookExtensions`, a relative one against
/// `dir` (the `cmd.Dir`).
#[cfg(windows)]
fn resolve(binary: &str, environ: &[OsString], dir: &str) -> Result<PathBuf, String> {
    use super::process::windows::{LookEnv, look_extensions};
    use crate::winpath;

    if binary.is_empty() {
        return Err("exec: no command".to_string());
    }
    if winpath::base(binary.as_bytes()) == binary.as_bytes() {
        return process::look_path(binary, getenv(environ, "PATH")).map_err(|e| e.to_string());
    }
    let dir = if winpath::is_abs(binary.as_bytes()) {
        ""
    } else {
        dir
    };
    look_extensions(binary, dir, &LookEnv::process()).map_err(|e| e.to_string())
}

/// Go's text for an `io::Error` without a file name.
pub(crate) fn io_text(err: &io::Error) -> String {
    if err.kind() == io::ErrorKind::WriteZero {
        return "short write".to_string();
    }
    if err.raw_os_error().is_some() {
        return gostd::os_error_text(err);
    }
    err.to_string()
}

/// Go `*os.PathError` text for IO on a named file.
fn file_error(op: &str, name: &str, err: &io::Error) -> String {
    format!("{op} {name}: {}", io_text(err))
}

/// Go's `fork/exec` error text. A NUL in an argument is EINVAL in Go.
pub(crate) fn spawn_error_text(err: &io::Error) -> String {
    if err.raw_os_error().is_none() && err.kind() == io::ErrorKind::InvalidInput {
        return "invalid argument".to_string();
    }
    io_text(err)
}

/// Go's `w.Write(p)` contract over a Rust writer: loops over short writes and
/// returns the bytes written plus the first error.
fn write_full<W: Write + ?Sized>(w: &mut W, mut buf: &[u8]) -> (usize, Option<io::Error>) {
    let mut n = 0;
    while !buf.is_empty() {
        match w.write(buf) {
            Ok(0) => return (n, Some(io::ErrorKind::WriteZero.into())),
            Ok(k) => {
                n += k;
                buf = &buf[k..];
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return (n, Some(e)),
        }
    }
    (n, None)
}

/// Go `syncWriter` around the session log file.
struct SyncLog {
    file: Mutex<File>,
    name: String,
}

impl SyncLog {
    /// One locked write; the error text is Go's `write <path>: <errno>`.
    fn write(&self, p: &[u8]) -> (usize, Option<String>) {
        let mut file = self.file.lock().unwrap_or_else(|e| e.into_inner());
        let (n, err) = write_full(&mut *file, p);
        (n, err.map(|e| file_error("write", &self.name, &e)))
    }
}

/// Counts the workers still running. The last one to finish (or panic)
/// wakes the calling thread through `wake`.
struct Drain {
    remaining: Mutex<usize>,
    cond: Condvar,
    wake: CancelFunc,
}

impl Drain {
    fn lock(&self) -> std::sync::MutexGuard<'_, usize> {
        self.remaining.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn is_done(&self) -> bool {
        *self.lock() == 0
    }

    /// Waits until every worker finished or `timeout` passed.
    fn wait_timeout(&self, timeout: Duration) -> bool {
        let guard = self.lock();
        let (guard, _) = self
            .cond
            .wait_timeout_while(guard, timeout, |n| *n > 0)
            .unwrap_or_else(|e| e.into_inner());
        *guard == 0
    }

    fn wait(&self) {
        let guard = self.lock();
        let _guard = self
            .cond
            .wait_while(guard, |n| *n > 0)
            .unwrap_or_else(|e| e.into_inner());
    }
}

/// Go's `defer wg.Done()`, also on panic. A panicking worker fires the
/// abort so its siblings stop too; the calling thread then kills and reaps.
struct WorkerGuard<'a> {
    drain: &'a Drain,
    abort: &'a Abort,
}

impl Drop for WorkerGuard<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.abort.fire();
        }
        let mut n = self.drain.lock();
        *n -= 1;
        if *n == 0 {
            self.drain.cond.notify_all();
            drop(n);
            self.drain.wake.cancel();
        }
    }
}

/// What the stdout worker returns.
struct StdoutTotals {
    bytes: i64,
    lines: i64,
    err: Option<String>,
}

/// Go `RunSubprocess`: runs `req.binary` in its own process group in
/// `sess.work_dir`, writes `req.prompt` to its stdin, tees stdout to the
/// session log and `mirror`, and copies stderr to the log only.
///
/// The child's PID and start time are saved to the session right after the
/// start; the owner fields are not touched. Cancelling `ctx` SIGKILLs the
/// whole group and drains the pipes for at most [`PIPE_DRAIN_GRACE`]. A
/// signal-ended child reports exit code -1. A prompt write error is returned
/// only when the child exited 0 (a broken pipe is expected when the child
/// dies early).
pub fn run_subprocess(
    ctx: &Context,
    paths: &Paths,
    sess: &mut Session,
    req: &Request<'_>,
    mirror: Option<&mut (dyn Write + Send)>,
) -> anyhow::Result<RunResult> {
    let binary = req.binary;
    let resolved = resolve(binary, req.environ, &sess.work_dir);
    let env = child_env(req);

    let pipe = |what: &str, parent_reads: bool| {
        let (r, w) = process::provider_pipe(parent_reads)
            .map_err(|e| anyhow!("{what} pipe: {}: {}", process::PIPE_SYSCALL, io_text(&e)))?;
        anyhow::Ok((r, w))
    };
    // Our ends must be nonblocking (Unix) or overlapped (Windows): the drain
    // bound relies on it (see process::Abort). A setup failure stops before
    // the child starts.
    let nonblocking =
        |what: &str, e: io::Error| anyhow!("{what} pipe: set nonblocking: {}", io_text(&e));
    let (stdin_r, stdin_w) = pipe("stdin", false)?;
    process::set_nonblocking(&stdin_w).map_err(|e| nonblocking("stdin", e))?;
    let (stdout_r, stdout_w) = pipe("stdout", true)?;
    process::set_nonblocking(&stdout_r).map_err(|e| nonblocking("stdout", e))?;
    let (stderr_r, stderr_w) = pipe("stderr", true)?;
    process::set_nonblocking(&stderr_r).map_err(|e| nonblocking("stderr", e))?;
    let abort = Abort::new().map_err(|e| anyhow!("abort event: {}", io_text(&e)))?;

    let log_file = sess.open_log().map_err(|e| {
        anyhow!(
            "open log: {}",
            session::path_error("open", Path::new(&sess.log_file), &e)
        )
    })?;
    let log = SyncLog {
        file: Mutex::new(log_file),
        name: sess.log_file.clone(),
    };

    let result = (|| {
        let lp = resolved.map_err(|e| anyhow!("start {binary}: {e}"))?;
        if let Some(err) = ctx.err() {
            bail!("start {binary}: {err}");
        }
        let env = dedup_env(&env).map_err(|e| anyhow!("start {binary}: {e}"))?;

        let fork_error = |e: &io::Error| {
            anyhow!(
                "start {binary}: fork/exec {}: {}",
                lp.display(),
                spawn_error_text(e)
            )
        };
        let program =
            process::program_in_dir(&lp, Path::new(&sess.work_dir)).map_err(|e| fork_error(&e))?;
        let mut cmd = Command::new(program);
        let image = process::set_exec(&mut cmd, &lp, binary, req.args, &env);
        if !sess.work_dir.is_empty() {
            cmd.current_dir(&sess.work_dir);
        }
        process::configure_group(&mut cmd);
        cmd.stdin(Stdio::from(stdin_r))
            .stdout(Stdio::from(stdout_w))
            .stderr(Stdio::from(stderr_w));
        let spawned = image.and_then(|()| process::start_provider(&mut cmd));
        // Closes our copies of the child's pipe ends (Go closes childIOFiles
        // after Start), so EOF arrives once the provider side is done.
        drop(cmd);
        let proc: ProcessHandle = spawned.map_err(|e| fork_error(&e))?;

        // Record the child PID + its start time (the latter guards the PID
        // against reuse: the queue's liveness check holds a slot while a
        // SIGKILL-orphaned provider child still runs, and must not be fooled
        // by a recycled PID).
        sess.pid = i64::from(proc.pid());
        sess.pid_start = procinfo::start_nanos(proc.pid()).unwrap_or(0);
        if let Err(e) = sess.save(paths) {
            logging::warn()
                .err(&e)
                .str("session", sess.id.clone())
                .msg("failed to save session with PID");
        }

        let io = Pipes {
            stdin: stdin_w,
            stdout: stdout_r,
            stderr: stderr_r,
            abort,
        };
        supervise(ctx, &sess.id, binary, proc, io, req.prompt, &log, mirror)
    })();

    let SyncLog { file, name } = log;
    let file = file.into_inner().unwrap_or_else(|e| e.into_inner());
    if let Err(e) = process::close_file(file) {
        logging::warn()
            .err(file_error("close", &name, &e))
            .str("session", sess.id.clone())
            .msg("failed to close log file");
    }
    result
}

/// The parent ends of the three pipes, and the abort that ends their IO.
struct Pipes {
    stdin: PipeWriter,
    stdout: PipeReader,
    stderr: PipeReader,
    abort: Abort,
}

/// Go's `cmd.Cancel` plus `watchCtx`: kills the group and returns the error
/// `cmd.Wait` would report when the child itself exits 0.
fn cancel_group(proc: &mut ProcessHandle, ctx: &Context) -> Option<String> {
    match proc.kill_group() {
        KillOutcome::Sent => ctx.err().map(|e| e.to_string()),
        KillOutcome::Done => None,
        KillOutcome::Failed(text) => Some(format!("error sending signal to Cmd: {text}")),
    }
}

fn grace_event(session_id: &str) -> Event {
    logging::warn()
        .str("session", session_id)
        .dur("grace", PIPE_DRAIN_GRACE)
}

#[allow(clippy::too_many_arguments)]
fn supervise(
    ctx: &Context,
    session_id: &str,
    binary: &str,
    mut proc: ProcessHandle,
    io: Pipes,
    prompt: &str,
    log: &SyncLog,
    mirror: Option<&mut (dyn Write + Send)>,
) -> anyhow::Result<RunResult> {
    let Pipes {
        stdin,
        stdout,
        stderr,
        abort,
    } = io;
    let (wake_ctx, wake) = ctx.with_cancel();
    let drain = Drain {
        remaining: Mutex::new(3),
        cond: Condvar::new(),
        wake: wake.clone(),
    };
    // `Some(err)` once the group kill ran; Go calls Cancel at most once.
    let mut watch: Option<Option<String>> = None;
    let mut aborted = false;

    let joined = std::thread::scope(|s| {
        let spawn = |name: &str| std::thread::Builder::new().name(name.to_string());
        let guard = || WorkerGuard {
            drain: &drain,
            abort: &abort,
        };
        let (drain_ref, abort_ref) = (&drain, &abort);
        let (stdout_ref, stderr_ref) = (&stdout, &stderr);

        let workers = (|| {
            let stdin_h = spawn("rival-stdin").spawn_scoped(s, {
                let g = guard();
                move || {
                    let _g = g;
                    write_prompt(stdin, prompt, abort_ref, session_id)
                }
            })?;
            let stdout_h = spawn("rival-stdout").spawn_scoped(s, {
                let g = guard();
                move || {
                    let _g = g;
                    tee_stdout(stdout_ref, log, mirror, abort_ref, session_id)
                }
            })?;
            let stderr_h = spawn("rival-stderr").spawn_scoped(s, {
                let g = guard();
                move || {
                    let _g = g;
                    copy_stderr(stderr_ref, log, abort_ref, session_id)
                }
            })?;
            io::Result::Ok((stdin_h, stdout_h, stderr_h))
        })();
        let (stdin_h, stdout_h, stderr_h) = match workers {
            Ok(handles) => handles,
            Err(e) => {
                // Unreachable in practice. Stop whatever started; the scope
                // joins the spawned workers, the caller kills and reaps.
                abort_ref.fire();
                return Err(e);
            }
        };

        // Drain the pipes before reaping: reaping first would close the
        // pipes while output is still buffered. On cancel the whole process
        // group is SIGKILLed, so every writer dies and the readers hit EOF.
        // If a writer escaped the group, stop reading after the grace; that
        // ends the workers (stdin too, if its write is stuck on a full
        // pipe), so the joins cannot hang.
        wake_ctx.wait();
        if !drain_ref.is_done() && !abort_ref.is_fired() {
            watch = Some(cancel_group(&mut proc, ctx));
            if !drain_ref.wait_timeout(PIPE_DRAIN_GRACE) {
                grace_event(session_id)
                    .msg("provider pipes still open after cancel — closing them");
                abort_ref.fire();
                aborted = true;
            }
        }
        drain_ref.wait();
        Ok((stdin_h.join(), stdout_h.join(), stderr_h.join()))
    });
    wake.cancel();

    let (stdin_res, stdout_res, stderr_res) = match joined {
        Ok(results) => results,
        Err(e) => {
            proc.kill_group();
            drop((stdout, stderr));
            let _ = reap(&mut proc, None, &mut watch);
            bail!("subprocess {binary}: start worker thread: {}", io_text(&e));
        }
    };
    let (stdin_err, totals, stderr_err) = match (stdin_res, stdout_res, stderr_res) {
        (Ok(a), Ok(b), Ok(c)) => (a, b, c),
        (a, b, c) => {
            // Go would crash the process. Kill and reap the provider first.
            drop((stdout, stderr));
            proc.kill_group();
            let _ = reap(&mut proc, None, &mut watch);
            let payload = [a.err(), b.err(), c.err()].into_iter().flatten().next();
            std::panic::resume_unwind(payload.expect("a worker panicked"));
        }
    };
    if aborted {
        // Go closes the read ends at the grace, before cmd.Wait.
        drop((stdout, stderr));
    }

    // Go's cmd.Wait: reap the leader; the context watcher still kills the
    // group if the context ends first.
    let state = reap(&mut proc, Some(ctx), &mut watch);

    let exit_code = match state {
        Err(e) => bail!(
            "subprocess {binary}: {}: {}",
            process::WAIT_SYSCALL,
            io_text(&e)
        ),
        Ok(ExitState {
            success: false,
            code,
        }) => code,
        Ok(_) => {
            if let Some(Some(err)) = watch {
                bail!("subprocess {binary}: {err}");
            }
            0
        }
    };

    // Only report the stdin error if the child exited 0 — broken pipe is
    // expected when the child dies early.
    if let Some(err) = stdin_err
        && exit_code == 0
    {
        bail!("write prompt to stdin: {err}");
    }
    if let Some(err) = &totals.err {
        logging::warn()
            .err(err)
            .str("session", session_id)
            .msg("stdout was truncated");
    }
    if let Some(err) = &stderr_err {
        logging::warn()
            .err(err)
            .str("session", session_id)
            .msg("stderr capture failed");
    }
    Ok(RunResult {
        exit_code,
        output_bytes: totals.bytes,
        output_lines: totals.lines,
    })
}

/// Polls `waitpid(WNOHANG)` until the leader is reaped. With a context, a
/// cancellation seen before the reap kills the group once: Go's watcher
/// calls Cancel as soon as the context is done, until `cmd.Wait` has reaped
/// the leader. The check runs before each reap attempt, and no signal is
/// ever sent after the reap.
fn reap(
    proc: &mut ProcessHandle,
    ctx: Option<&Context>,
    watch: &mut Option<Option<String>>,
) -> io::Result<ExitState> {
    let mut pause = Duration::from_millis(1);
    loop {
        let watching = ctx.filter(|_| watch.is_none());
        if let Some(ctx) = watching
            && ctx.is_done()
        {
            *watch = Some(cancel_group(proc, ctx));
        }
        if let Some(state) = proc.try_reap()? {
            return Ok(state);
        }
        match ctx.filter(|_| watch.is_none()) {
            Some(ctx) => {
                ctx.wait_timeout(pause);
            }
            None => std::thread::sleep(pause),
        }
        pause = (pause * 2).min(REAP_POLL_MAX);
    }
}

/// Go's stdin goroutine: write the prompt, then close the pipe.
fn write_prompt(
    stdin: PipeWriter,
    prompt: &str,
    abort: &Abort,
    session_id: &str,
) -> Option<String> {
    let mut rest = prompt.as_bytes();
    let err = loop {
        match process::write_some(&stdin, rest, abort) {
            Io::Done(0) if !rest.is_empty() => {
                break Some(format!("write {PIPE_WRITE_NAME}: unexpected EOF"));
            }
            Io::Done(n) => {
                rest = &rest[n..];
                if rest.is_empty() {
                    break None;
                }
            }
            Io::Aborted => break Some(format!("write {PIPE_WRITE_NAME}: {ERR_CLOSED}")),
            Io::Err(e) => break Some(file_error("write", PIPE_WRITE_NAME, &e)),
        }
    };
    drop(stdin);
    if let Some(e) = &err {
        logging::error()
            .err(e)
            .str("session", session_id)
            .msg("failed to write prompt to stdin");
    }
    err
}

/// Go's stdout goroutine: `bufio.Reader.ReadBytes('\n')` — each complete
/// line, or the nonempty tail before EOF or an error — goes to the log and
/// then the mirror (`io.MultiWriter`). Lines have no length limit.
fn tee_stdout(
    stdout: &PipeReader,
    log: &SyncLog,
    mut mirror: Option<&mut (dyn Write + Send)>,
    abort: &Abort,
    session_id: &str,
) -> StdoutTotals {
    let mut totals = StdoutTotals {
        bytes: 0,
        lines: 0,
        err: None,
    };
    let mut emit = |line: &[u8]| {
        totals.lines += 1;
        let (n, err) = tee_line(line, log, mirror.as_deref_mut());
        if let Some(err) = err {
            logging::error()
                .err(err)
                .str("session", session_id)
                .msg("failed to write output line");
        }
        totals.bytes += n as i64;
    };
    let mut pending: Vec<u8> = Vec::new();
    let mut buf = vec![0u8; STDOUT_BUF];
    let read_err = loop {
        match process::read_some(stdout, &mut buf, abort) {
            Io::Done(0) => break None,
            Io::Done(n) => {
                let scan_from = pending.len();
                pending.extend_from_slice(&buf[..n]);
                let mut start = 0;
                let mut search = scan_from;
                while let Some(off) = pending[search..].iter().position(|&b| b == b'\n') {
                    let end = search + off + 1;
                    emit(&pending[start..end]);
                    start = end;
                    search = end;
                }
                pending.drain(..start);
            }
            Io::Aborted => break Some(format!("read {PIPE_READ_NAME}: {ERR_CLOSED}")),
            Io::Err(e) => break Some(file_error("read", PIPE_READ_NAME, &e)),
        }
    };
    if !pending.is_empty() {
        emit(&pending);
    }
    if let Some(err) = &read_err {
        logging::warn()
            .err(err)
            .str("session", session_id)
            .msg("stdout read error — output may be truncated");
    }
    totals.err = read_err;
    totals
}

/// Go `io.MultiWriter(safeLog, mirror).Write(line)`: the log first (a full
/// `os.File` write), then one mirror `Write`. It stops at the first failing
/// writer and returns that writer's count; a short write without an error
/// is `io.ErrShortWrite`. A Rust writer reports no count with an error, so
/// a failing mirror counts 0.
fn tee_line<W: Write + ?Sized>(
    line: &[u8],
    log: &SyncLog,
    mirror: Option<&mut W>,
) -> (usize, Option<String>) {
    let (n, err) = log.write(line);
    if err.is_some() {
        return (n, err);
    }
    if let Some(m) = mirror {
        let res = loop {
            match m.write(line) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                res => break res,
            }
        };
        match res {
            Ok(n) if n == line.len() => {}
            Ok(n) => return (n, Some("short write".to_string())),
            Err(e) => return (0, Some(io_text(&e))),
        }
    }
    (line.len(), None)
}

/// Go's stderr goroutine: `io.Copy(safeLog, stderr)`. A log write error
/// stops the copy, as in Go; the pipe stays open until the reap.
fn copy_stderr(
    stderr: &PipeReader,
    log: &SyncLog,
    abort: &Abort,
    session_id: &str,
) -> Option<String> {
    let mut buf = vec![0u8; STDERR_BUF];
    let err = loop {
        match process::read_some(stderr, &mut buf, abort) {
            Io::Done(0) => break None,
            Io::Done(n) => {
                if let (_, Some(err)) = log.write(&buf[..n]) {
                    break Some(err);
                }
            }
            Io::Aborted => break Some(format!("read {PIPE_READ_NAME}: {ERR_CLOSED}")),
            Io::Err(e) => break Some(file_error("read", PIPE_READ_NAME, &e)),
        }
    };
    if let Some(e) = &err {
        logging::error()
            .err(e)
            .str("session", session_id)
            .msg("failed to copy stderr to log");
    }
    err
}
