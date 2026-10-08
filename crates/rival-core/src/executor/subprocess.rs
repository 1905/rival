//! Runs a provider subprocess; the platform parts live in [`super::process`].
//!
//! Scoped threads do the IO: one writes the prompt, one tees
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
use crate::envname;
use crate::gitscope;
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

/// Pipe file names in error texts: the read end is `|0`, the write end `|1`.
pub(crate) const PIPE_READ_NAME: &str = "|0";
const PIPE_WRITE_NAME: &str = "|1";
/// Error text for IO on a pipe file closed by the drain grace.
pub(crate) const ERR_CLOSED: &str = "file already closed";

const STDOUT_BUF: usize = 64 * 1024;
/// The stderr copy buffer.
const STDERR_BUF: usize = 32 * 1024;
/// Upper bound of one sleep while polling for the leader's exit.
const REAP_POLL_MAX: Duration = Duration::from_millis(50);

/// The outcome of a provider run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunResult {
    pub exit_code: i64,
    pub output_bytes: i64,
    pub output_lines: i64,
}

/// The arguments of [`run_subprocess`], minus the context, session and
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
    /// The inherited env in order (`KEY=VALUE`). It is
    /// also where `$PATH` for the binary lookup comes from. Production code
    /// passes [`crate::config::Config::environ`].
    pub environ: &'a [OsString],
    /// The file that gets the child output. `None` is the session log.
    /// `Some(path)` is created (mode 0600 on Unix) or appended to; the
    /// session log and the session record are then not touched.
    pub log: Option<&'a str>,
}

/// Opens `path` to append, creating it with mode 0600 on Unix (as
/// [`Session::open_log`] does).
fn open_append(path: &str) -> io::Result<File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    opts.open(path)
}

/// `environ` without Git repository overrides and without the
/// blocked prefixes that a repo-local .env could inject. Windows compares
/// the prefixes case-insensitively (see [`safe_env_case`]).
pub fn safe_env(environ: &[OsString]) -> Vec<OsString> {
    safe_env_case(cfg!(windows), environ)
}

/// [`safe_env`] with an explicit name rule (see [`crate::envname`]). A
/// plain prefix match is case-sensitive; Windows env names are not, so
/// there `Node_Options` must go as `NODE_OPTIONS` does.
pub(crate) fn safe_env_case(case_insensitive: bool, environ: &[OsString]) -> Vec<OsString> {
    gitscope::repository_env(environ)
        .into_iter()
        .filter(|kv| {
            !BLOCKED_ENV_PREFIXES
                .iter()
                .any(|prefix| envname::has_prefix(case_insensitive, kv, prefix))
        })
        .collect()
}

/// Whether `kv` (a `KEY=VALUE` entry) is named by
/// `drop_env`. An entry ending in `_` is a prefix match ("AWS_" drops every
/// AWS_* var — exact name lists rot as providers add credential vars); any
/// other entry matches that exact variable name. `case_insensitive` (the
/// Windows rule) compares names case-insensitively (see [`crate::envname`]).
#[cfg(test)]
pub(crate) fn drop_matches_case(case_insensitive: bool, kv: &OsStr, drop_env: &[&str]) -> bool {
    drop_hit(case_insensitive, kv, &drop_prefixes(drop_env))
}

/// The prefix each `drop_env` entry matches: the entry itself when it ends
/// in `_`, else `NAME=` for the exact variable.
fn drop_prefixes(drop_env: &[&str]) -> Vec<String> {
    drop_env
        .iter()
        .map(|name| {
            if name.ends_with('_') {
                (*name).to_string()
            } else {
                format!("{name}=")
            }
        })
        .collect()
}

fn drop_hit(case_insensitive: bool, kv: &OsStr, prefixes: &[String]) -> bool {
    prefixes
        .iter()
        .any(|p| envname::has_prefix(case_insensitive, kv, p))
}

/// The env the child gets: the safe base env with `drop_env` applied, then
/// the request env appended.
pub fn child_env(req: &Request<'_>) -> Vec<OsString> {
    child_env_case(cfg!(windows), req)
}

/// [`child_env`] with an explicit name rule. Only the inherited env is
/// filtered; the trusted `req.env` entries are appended unchanged.
pub(crate) fn child_env_case(case_insensitive: bool, req: &Request<'_>) -> Vec<OsString> {
    let mut base = safe_env_case(case_insensitive, req.environ);
    if !req.drop_env.is_empty() {
        let prefixes = drop_prefixes(req.drop_env);
        base.retain(|kv| !drop_hit(case_insensitive, kv, &prefixes));
    }
    base.extend(req.env.iter().map(OsString::from));
    base
}

/// Where an env entry splits into key and value: the
/// first `=`, but a leading `=` belongs to the key. `None` = no `=`.
fn env_key_end(kv: &[u8]) -> Option<usize> {
    match kv.iter().position(|&b| b == b'=') {
        Some(0) => Some(kv[1..].iter().position(|&b| b == b'=').map_or(0, |i| i + 1)),
        other => other,
    }
}

/// The last value of each key wins, in the order of
/// each key's last occurrence. An entry without `=` is kept as-is. An entry
/// with a NUL byte is skipped and reported. Windows compares keys case
/// insensitively (lowercased).
pub(crate) fn dedup_env(env: &[OsString]) -> Result<Vec<OsString>, String> {
    dedup_env_case(cfg!(windows), env)
}

/// [`dedup_env`] with an explicit name rule.
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
                Ok(k) => k.to_lowercase().into_bytes(),
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
        // Command needs a key and a value, so an entry without "=" is
        // skipped.
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

/// Looks up `key` in an env list: the first entry wins. Windows
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

/// A name without a separator is looked up in `$PATH`
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

/// Command start on Windows: a bare name (no directory, no drive) is looked
/// up in `%PATH%` with `PATHEXT`; any other name gets its `PATHEXT`
/// extension from `look_extensions`, a relative one against `dir` (the
/// child's directory).
#[cfg(windows)]
fn resolve(binary: &str, environ: &[OsString], dir: &str) -> Result<PathBuf, String> {
    use super::process::windows::{LookEnv, is_bare_name, look_extensions};

    if binary.is_empty() {
        return Err("exec: no command".to_string());
    }
    if is_bare_name(binary) {
        return process::look_path(binary, getenv(environ, "PATH")).map_err(|e| e.to_string());
    }
    let dir = if Path::new(binary).is_absolute() {
        ""
    } else {
        dir
    };
    look_extensions(binary, dir, &LookEnv::process()).map_err(|e| e.to_string())
}

/// The text of an `io::Error` without a file name. A write that stops
/// short reads `short write`.
pub(crate) fn io_text(err: &io::Error) -> String {
    if err.kind() == io::ErrorKind::WriteZero {
        return "short write".to_string();
    }
    err.to_string()
}

/// `<op> <name>: <errno text>` for IO on a named file.
fn file_error(op: &str, name: &str, err: &io::Error) -> String {
    format!("{op} {name}: {}", io_text(err))
}

/// A full-write contract over a writer: loops over short writes and
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

/// A locked writer around the session log file.
struct SyncLog {
    file: Mutex<File>,
    name: String,
}

impl SyncLog {
    /// One locked write; the error text is `write <path>: <errno>`.
    fn write(&self, p: &[u8]) -> (usize, Option<String>) {
        let mut file = self.file.lock().unwrap_or_else(|e| e.into_inner());
        let (n, err) = write_full(&mut *file, p);
        (n, err.map(|e| file_error("write", &self.name, &e)))
    }
}

/// Counts the workers still running. The last one to finish (or panic)
/// wakes the calling thread through `wake`.
pub(crate) struct Drain {
    remaining: Mutex<usize>,
    cond: Condvar,
    wake: CancelFunc,
}

impl Drain {
    /// A drain for `workers` workers.
    pub(crate) fn new(workers: usize, wake: CancelFunc) -> Drain {
        Drain {
            remaining: Mutex::new(workers),
            cond: Condvar::new(),
            wake,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, usize> {
        self.remaining.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn is_done(&self) -> bool {
        *self.lock() == 0
    }

    /// Waits until every worker finished or `timeout` passed.
    pub(crate) fn wait_timeout(&self, timeout: Duration) -> bool {
        let guard = self.lock();
        let (guard, _) = self
            .cond
            .wait_timeout_while(guard, timeout, |n| *n > 0)
            .unwrap_or_else(|e| e.into_inner());
        *guard == 0
    }

    pub(crate) fn wait(&self) {
        let guard = self.lock();
        let _guard = self
            .cond
            .wait_while(guard, |n| *n > 0)
            .unwrap_or_else(|e| e.into_inner());
    }
}

/// Marks a worker done, also on panic. A panicking worker fires the
/// abort so its siblings stop too; the calling thread then kills and reaps.
pub(crate) struct WorkerGuard<'a> {
    pub(crate) drain: &'a Drain,
    pub(crate) abort: &'a Abort,
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

/// Runs `req.binary` in its own process group in
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

    let log_name = req.log.unwrap_or(&sess.log_file).to_string();
    let opened = match req.log {
        None => sess.open_log(),
        Some(path) => open_append(path),
    };
    let log_file = opened.map_err(|e| {
        anyhow!(
            "open log: {}",
            session::path_error("open", Path::new(&log_name), &e)
        )
    })?;
    let log = SyncLog {
        file: Mutex::new(log_file),
        name: log_name,
    };

    let result = (|| {
        let lp = resolved.map_err(|e| anyhow!("start {binary}: {e}"))?;
        if let Some(err) = ctx.err() {
            bail!("start {binary}: {err}");
        }
        let env = dedup_env(&env).map_err(|e| anyhow!("start {binary}: {e}"))?;

        let fork_error =
            |e: &io::Error| anyhow!("start {binary}: {}", super::oscmd::fork_error(&lp, e));
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
        // Closes our copies of the child's pipe ends after the start, so EOF
        // arrives once the provider side is done.
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

/// Kills the group on cancellation and returns the error to report when the
/// child itself exits 0.
pub(crate) fn cancel_group(proc: &mut ProcessHandle, ctx: &Context) -> Option<String> {
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
    let drain = Drain::new(3, wake.clone());
    // `Some(err)` once the group kill ran; the kill runs at most once.
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
            // A worker panicked. Kill and reap the provider first.
            drop((stdout, stderr));
            proc.kill_group();
            let _ = reap(&mut proc, None, &mut watch);
            let payload = [a.err(), b.err(), c.err()].into_iter().flatten().next();
            std::panic::resume_unwind(payload.expect("a worker panicked"));
        }
    };
    if aborted {
        // Close the read ends at the grace, before the reap.
        drop((stdout, stderr));
    }

    // Reap the leader; the context watcher still kills the
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
            ..
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
/// cancellation seen before the reap kills the group once: the watcher
/// kills as soon as the context is done, until the leader is reaped. The
/// check runs before each reap attempt, and no signal is ever sent after
/// the reap.
pub(crate) fn reap(
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

/// The stdin worker: write the prompt, then close the pipe.
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

/// The stdout worker: each complete
/// line, or the nonempty tail before EOF or an error — goes to the log and
/// then the mirror. Lines have no length limit.
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

/// Writes one line to the log and the mirror: the log first (a full
/// `os.File` write), then one mirror `Write`. It stops at the first failing
/// writer and returns that writer's count; a short write without an error
/// is a short-write error. A writer reports no count with an error, so
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

/// The stderr worker: copies stderr to the log. A log write error
/// stops the copy; the pipe stays open until the reap.
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

/// Both name rules, on every host: `true` is the case-insensitive rule (the
/// OS's on Windows, the ASCII model elsewhere), `false` the Unix
/// case-sensitive one. Every name here is ASCII, so both agree.
#[cfg(test)]
mod env_case_tests {
    use super::*;

    fn environ(items: &[&str]) -> Vec<OsString> {
        items.iter().map(OsString::from).collect()
    }

    /// `head`, one unpaired surrogate (Windows) or invalid byte (Unix), then
    /// `tail`.
    fn non_utf8(head: &str, tail: &str) -> OsString {
        #[cfg(unix)]
        let s = {
            use std::os::unix::ffi::OsStringExt;
            let mut b = head.as_bytes().to_vec();
            b.push(0xff);
            b.extend_from_slice(tail.as_bytes());
            OsString::from_vec(b)
        };
        #[cfg(windows)]
        let s = {
            use std::os::windows::ffi::OsStringExt;
            let mut w: Vec<u16> = head.encode_utf16().collect();
            w.push(0xD800);
            w.extend(tail.encode_utf16());
            OsString::from_wide(&w)
        };
        assert!(s.to_str().is_none(), "{s:?} is valid UTF-8");
        s
    }

    /// (entry, kept on Windows, kept on Unix).
    fn safe_env_cases() -> Vec<(OsString, bool, bool)> {
        let mut cases: Vec<(OsString, bool, bool)> = [
            ("PATH=/usr/bin", true, true),
            ("Node_Options=--require=./payload.cjs", false, true),
            ("node_options=--inspect", false, true),
            ("NODE_OPTIONS_X=1", false, false),
            ("Https_Proxy=http://evil:8080", false, true),
            ("hTTP_pROXY=http://evil", false, true),
            ("http_proxy=http://evil", false, false),
            ("No_Proxy=*", false, true),
            ("All_Proxy=socks5://evil", false, true),
            ("Ld_Preload=evil.so", false, true),
            ("Dyld_Insert_Libraries=evil", false, true),
            (r#"Opencode_Permission={"bash":"allow"}"#, false, true),
            ("Kimi_Api=sk", false, true),
            ("Moonshot_Api_Key=sk", false, true),
            ("Grok_Home=evil", false, true),
            ("Xai_Api_Key=sk", false, true),
            // Near names: no blocked prefix in any case.
            ("Node_Option=near", true, true),
            ("Https_Proxi=near", true, true),
            ("Ld_Preloa=near", true, true),
            ("Opencode=near", true, true),
            ("Grokx_Home=near", true, true),
            ("Xai=near", true, true),
            ("NODE=short", true, true),
            ("Rival_Keep=yes", true, true),
        ]
        .into_iter()
        .map(|(kv, win, unix)| (OsString::from(kv), win, unix))
        .collect();
        cases.push((non_utf8("Node_Options=", "x"), false, true));
        cases.push((non_utf8("RIVAL_", "=1"), true, true));
        cases
    }

    fn kept(cases: &[(OsString, bool, bool)], windows: bool) -> Vec<OsString> {
        cases
            .iter()
            .filter(|(_, win, unix)| if windows { *win } else { *unix })
            .map(|(kv, _, _)| kv.clone())
            .collect()
    }

    #[test]
    fn safe_env_windows_rule_blocks_mixed_case_prefixes() {
        let cases = safe_env_cases();
        let input: Vec<OsString> = cases.iter().map(|(kv, _, _)| kv.clone()).collect();
        assert_eq!(safe_env_case(true, &input), kept(&cases, true));
    }

    #[test]
    fn safe_env_unix_rule_keeps_mixed_case_names() {
        let cases = safe_env_cases();
        let input: Vec<OsString> = cases.iter().map(|(kv, _, _)| kv.clone()).collect();
        assert_eq!(safe_env_case(false, &input), kept(&cases, false));
    }

    #[test]
    fn drop_matches_follows_the_name_rule() {
        let drop = ["ANTHROPIC_API_KEY", "AWS_", "CLAUDECODE", ""];
        // (entry, dropped on Windows, dropped on Unix).
        let cases = [
            ("ANTHROPIC_API_KEY=k", true, true),
            ("Anthropic_Api_Key=k", true, false),
            ("anthropic_api_key=k", true, false),
            ("ANTHROPIC_API_KEY_2=k", false, false),
            ("Anthropic_Api_Key_2=k", false, false),
            ("Anthropic_Api_Ke=k", false, false),
            ("Anthropic_Api_Key", false, false),
            ("AWS_REGION=x", true, true),
            ("aws_session_token=x", true, false),
            ("Aws_Region=x", true, false),
            ("AwsX=x", false, false),
            ("ClaudeCode=1", true, false),
            ("ClaudeCodeX=1", false, false),
            ("=C:=C:\\", true, true),
            ("PATH=/usr/bin", false, false),
        ];
        for (kv, win, unix) in cases {
            let kv = OsStr::new(kv);
            assert_eq!(drop_matches_case(true, kv, &drop), win, "windows {kv:?}");
            assert_eq!(drop_matches_case(false, kv, &drop), unix, "unix {kv:?}");
        }
        let bad = non_utf8("Aws_", "=x");
        assert!(drop_matches_case(true, &bad, &drop));
        assert!(!drop_matches_case(false, &bad, &drop));
    }

    #[test]
    fn child_env_filters_inherited_names_and_keeps_trusted_entries() {
        let inherited = environ(&[
            "PATH=/usr/bin",
            "Node_Options=--require=./payload.cjs",
            "Opencode_Config_Content=evil",
            "Anthropic_Api_Key=secret",
            "aws_session_token=secret",
            "Anthropic_Api_Key_2=keep",
            "Rival_Mode=inherited",
            "Rival_Keep=yes",
        ]);
        let trusted = [
            "OPENCODE_CONFIG_CONTENT=trusted".to_string(),
            "RIVAL_MODE=trusted".to_string(),
        ];
        let req = Request {
            binary: "provider",
            args: &[],
            env: &trusted,
            prompt: "",
            drop_env: &["ANTHROPIC_API_KEY", "AWS_"],
            environ: &inherited,
            log: None,
        };

        let windows = child_env_case(true, &req);
        assert_eq!(
            windows,
            environ(&[
                "PATH=/usr/bin",
                "Anthropic_Api_Key_2=keep",
                "Rival_Mode=inherited",
                "Rival_Keep=yes",
                "OPENCODE_CONFIG_CONTENT=trusted",
                "RIVAL_MODE=trusted",
            ])
        );
        // The trusted entry wins over the inherited one with the same name.
        assert_eq!(
            dedup_env_case(true, &windows).unwrap(),
            environ(&[
                "PATH=/usr/bin",
                "Anthropic_Api_Key_2=keep",
                "Rival_Keep=yes",
                "OPENCODE_CONFIG_CONTENT=trusted",
                "RIVAL_MODE=trusted",
            ])
        );

        let mut unix = inherited.clone();
        unix.extend(trusted.iter().map(OsString::from));
        assert_eq!(child_env_case(false, &req), unix);
    }

    #[test]
    fn platform_wrappers_use_the_host_rule() {
        let cases = safe_env_cases();
        let input: Vec<OsString> = cases.iter().map(|(kv, _, _)| kv.clone()).collect();
        assert_eq!(safe_env(&input), kept(&cases, cfg!(windows)));
        let kv = OsStr::new("Anthropic_Api_Key=k");
        assert_eq!(
            drop_matches_case(cfg!(windows), kv, &["ANTHROPIC_API_KEY"]),
            cfg!(windows)
        );
    }
}
