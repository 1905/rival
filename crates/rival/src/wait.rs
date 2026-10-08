//! `rival wait`: block until review sessions finish; the exit code reflects
//! the outcome.
//!
//! Two modes:
//!
//! - `--log <stderr-file>` (used by skills): parse the detached rival PID and
//!   session IDs from a run's stderr file, poll the rival process for
//!   liveness, then summarize the sessions when it exits. Detects a crashed
//!   rival (process dead, sessions not finalized).
//! - `<session-id>...` (terminal status only): poll the named sessions' JSON
//!   until all reach a terminal state.
//!
//! Durations are signed nanoseconds.

use std::fmt;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use regex::Regex;
use regex::bytes::Regex as BytesRegex;
use rival_core::cancel::Context;
use rival_core::paths::Paths;
use rival_core::{duration, procinfo, session};

/// All watched sessions completed.
pub const WAIT_EXIT_COMPLETED: i32 = 0;
/// At least one session failed (including a run timeout).
pub const WAIT_EXIT_FAILED: i32 = 2;
/// rival died but left a session non-terminal, or the wait was interrupted.
pub const WAIT_EXIT_CRASHED: i32 = 3;
/// `--timeout` elapsed while still running.
pub const WAIT_EXIT_TIMEOUT: i32 = 4;
pub const WAIT_EXIT_USAGE: i32 = 64;

/// The `--poll` default: the queue poll interval (2s).
pub const DEFAULT_POLL: i64 = rival_core::config::QUEUE_POLL_INTERVAL.as_nanos() as i64;

/// "rival: detached pid=12345" (from `detach.rs`). Digits are ASCII only.
static DETACHED_PID_RE: LazyLock<BytesRegex> =
    LazyLock::new(|| BytesRegex::new(r"rival: detached pid=([0-9]+)").unwrap());
/// zerolog: ..."session":"<uuid>"...
static SESSION_ID_RE: LazyLock<BytesRegex> =
    LazyLock::new(|| BytesRegex::new(r#""session":"([0-9a-fA-F-]{36})""#).unwrap());
/// Only THIS run's start lines (message:"starting codex|…"); excludes
/// reaper/maintenance lines that carry old session IDs.
static STARTING_MARKER_RE: LazyLock<BytesRegex> =
    LazyLock::new(|| BytesRegex::new(r#""message":"starting "#).unwrap());
/// A bare UUID. Validates user-supplied positional IDs so they can never
/// escape the session dir via path separators (`rival wait ../x`).
static SESSION_ID_ONLY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$")
        .unwrap()
});

pub fn is_session_id(s: &str) -> bool {
    SESSION_ID_ONLY_RE.is_match(s)
}

/// An error with an exit code from [`wait_action`]. Root prints `message`
/// on stderr and exits with `code`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitError {
    pub code: i32,
    pub message: String,
}

impl WaitError {
    fn usage(message: impl Into<String>) -> WaitError {
        WaitError {
            code: WAIT_EXIT_USAGE,
            message: message.into(),
        }
    }
}

impl fmt::Display for WaitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for WaitError {}

/// The `wait` flags. `timeout` defaults to a value computed from the process
/// env before `.env` loads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitOptions {
    /// `--log`; empty means unset.
    pub log: PathBuf,
    /// `--timeout`, nanoseconds.
    pub timeout: i64,
    /// `--poll`, nanoseconds.
    pub poll: i64,
    /// What `--log` mode prints after the summary: [`Config::auto_fix_policy`].
    ///
    /// [`Config::auto_fix_policy`]: rival_core::config::Config::auto_fix_policy
    pub auto_fix_policy: &'static str,
}

/// `rival wait`. `paths` locates the session files; `ctx` is the signal
/// context (done on SIGINT or SIGTERM); `out` is stdout.
pub fn wait_action(
    opts: &WaitOptions,
    args: &[String],
    paths: &Paths,
    ctx: &Context,
    out: &mut dyn Write,
) -> Result<(), WaitError> {
    if opts.poll <= 0 {
        return Err(WaitError::usage("--poll must be > 0"));
    }

    let sessions_dir = paths.sessions_dir();
    let base = Instant::now();
    let mut w = Waiter {
        pid: 0,
        pid_start: 0,
        ids: Vec::new(),
        log_file: None,
        poll: opts.poll,
        timeout: opts.timeout,
        load_session: Box::new(move |id| load_session_status(&sessions_dir, id)),
        ralive: Box::new(rival_alive),
        now: Box::new(move || base.elapsed().as_nanos() as i128),
        out,
    };

    if !opts.log.as_os_str().is_empty() {
        if !args.is_empty() {
            return Err(WaitError::usage(
                "pass either --log or session IDs, not both",
            ));
        }
        let parsed = parse_log_file(&opts.log).map_err(WaitError::usage)?;
        // IDs from --log are regex-matched UUIDs already; positional IDs
        // below are user input and must be validated before becoming a path.
        w.pid = parsed.pid;
        w.pid_start = parsed.pid_start;
        w.ids = parsed.ids;
        w.log_file = Some(opts.log.clone());
    } else {
        if args.is_empty() {
            return Err(WaitError::usage(
                "provide --log <file> or one or more session IDs",
            ));
        }
        for id in args {
            if !is_session_id(id) {
                return Err(WaitError::usage(format!(
                    "invalid session ID {:?} (expected a UUID)",
                    id
                )));
            }
        }
        w.ids = args.to_vec();
    }

    let log_mode = w.log_file.is_some();
    let code = w.run(ctx);
    // Skills read this line to decide whether to fix CONFIRMED critical/high
    // findings without asking. Only a summarized run has output to act on.
    if log_mode && (code == WAIT_EXIT_COMPLETED || code == WAIT_EXIT_FAILED) {
        w.print(format!("auto-fix: {}\n", opts.auto_fix_policy).as_bytes());
    }
    if code == WAIT_EXIT_COMPLETED {
        return Ok(());
    }
    Err(WaitError {
        code,
        message: format!("rival wait: exit {code}"),
    })
}

/// The minimal slice of a session JSON wait needs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionStatus {
    pub id: String,
    pub status: String,
    pub exit_code: Option<i64>,
    pub duration: String,
    pub error_msg: String,
    pub found: bool,
}

impl SessionStatus {
    pub fn terminal(&self) -> bool {
        self.status == "completed" || self.status == "failed"
    }
}

/// The poll loop's state. Every dependency is injectable for tests.
pub struct Waiter<'a> {
    pub pid: i64,
    pub pid_start: i64,
    pub ids: Vec<String>,
    /// `Some` in `--log` mode.
    pub log_file: Option<PathBuf>,

    /// Nanoseconds.
    pub poll: i64,
    /// Nanoseconds.
    pub timeout: i64,

    pub load_session: Box<dyn FnMut(&str) -> SessionStatus + 'a>,
    /// Whether `pid` with this start time is still alive.
    pub ralive: Box<dyn FnMut(i64, i64) -> bool + 'a>,
    /// A monotonic clock in nanoseconds (any origin).
    pub now: Box<dyn FnMut() -> i128 + 'a>,
    pub out: &'a mut dyn Write,
}

impl Waiter<'_> {
    /// Polls until an outcome is decided and returns the exit code. Prints a
    /// one-line summary per session (or a crash/timeout line) first.
    pub fn run(&mut self, ctx: &Context) -> i32 {
        let deadline = (self.now)() + i128::from(self.timeout);
        let have_pid = self.pid > 0;
        let mut first_poll = true;

        loop {
            // In --log mode, re-scan the file each tick: a run logs its
            // "starting" session lines only after it leaves the queue, so the
            // IDs can appear after wait has started.
            if let Some(log) = &self.log_file
                && let Ok(text) = read_file(log)
            {
                let ids = scan_session_ids(&text);
                if ids.len() > self.ids.len() {
                    self.ids = ids;
                }
            }

            let (mut statuses, mut all_terminal, any_missing) = self.load_statuses();

            // session-ID mode has no rival PID to watch, so a never-appearing
            // file would otherwise hang until --timeout. A session JSON is
            // created before the run is even visible to a caller, so a
            // missing file on the first poll means a bad/nonexistent ID.
            if !have_pid && first_poll && any_missing {
                self.print("no such session (file not found) — check the ID\n".as_bytes());
                return WAIT_EXIT_USAGE;
            }
            first_poll = false;

            // Primary signal in --log mode: rival process death. By the time
            // the rival process exits, stdout has been flushed.
            let ralive = have_pid && (self.ralive)(self.pid, self.pid_start);

            if have_pid && !ralive {
                // rival is gone. Re-read once before deciding: the process can
                // finalize the session JSON and then exit between our status
                // read and this liveness check, which would look like a crash.
                (statuses, all_terminal, _) = self.load_statuses();
                if all_terminal && !self.ids.is_empty() {
                    return self.summarize(&statuses);
                }
                let line = format!(
                    "crashed: rival (pid {}) exited before finalizing sessions\n",
                    self.pid
                );
                self.print(line.as_bytes());
                return WAIT_EXIT_CRASHED;
            } else if !have_pid && all_terminal && !self.ids.is_empty() {
                // session-ID mode: terminal status is the only signal.
                return self.summarize(&statuses);
            }

            if (self.now)() >= deadline {
                let mut line = format!(
                    "still running after {} (rival pid {})",
                    duration::format(self.timeout),
                    self.pid
                )
                .into_bytes();
                line.extend_from_slice(&self.last_queue_line());
                line.push(b'\n');
                self.print(&line);
                return WAIT_EXIT_TIMEOUT;
            }

            // Sleep one poll interval, or less if the context ends first.
            let poll = Duration::from_nanos(self.poll.max(0) as u64);
            if ctx.wait_timeout(poll).is_some() {
                self.print(b"interrupted while waiting\n");
                return WAIT_EXIT_CRASHED;
            }
        }
    }

    /// Reads every watched session. Returns the statuses, whether all are
    /// terminal, and whether any session file is missing.
    fn load_statuses(&mut self) -> (Vec<SessionStatus>, bool, bool) {
        let mut statuses = Vec::with_capacity(self.ids.len());
        let mut all_terminal = true;
        let mut any_missing = false;
        for id in &self.ids {
            let mut st = (self.load_session)(id);
            st.id = id.clone();
            if !st.found {
                any_missing = true;
            }
            if !st.terminal() {
                all_terminal = false;
            }
            statuses.push(st);
        }
        (statuses, all_terminal, any_missing)
    }

    /// Prints one line per session; returns 0 if all completed, else 2.
    fn summarize(&mut self, statuses: &[SessionStatus]) -> i32 {
        let mut code = WAIT_EXIT_COMPLETED;
        for s in statuses {
            let exit = match s.exit_code {
                Some(c) => c.to_string(),
                None => "-".to_string(),
            };
            // The first 8 bytes of the id.
            let id = s.id.as_bytes();
            let id = &id[..id.len().min(8)];
            let mut line = id.to_vec();
            line.extend_from_slice(
                format!(" {} exit={} {}", s.status, exit, s.duration).as_bytes(),
            );
            if !s.error_msg.is_empty() {
                line.extend_from_slice(" \u{2014} ".as_bytes());
                line.extend_from_slice(s.error_msg.as_bytes());
            }
            line.push(b'\n');
            self.print(&line);
            if s.status != "completed" {
                code = WAIT_EXIT_FAILED;
            }
        }
        code
    }

    /// Stdout write errors are ignored here.
    fn print(&mut self, bytes: &[u8]) {
        let _ = self.out.write_all(bytes);
    }

    /// The last "rival queue:" progress line from the log file (for the
    /// timeout message), prefixed with " — ", or empty if none.
    fn last_queue_line(&self) -> Vec<u8> {
        let Some(log) = &self.log_file else {
            return Vec::new();
        };
        let Ok(data) = read_file(log) else {
            return Vec::new();
        };
        let last = data
            .split(|&b| b == b'\n')
            .rfind(|line| line.starts_with(b"rival queue:"));
        match last {
            Some(line) => {
                let mut out = " \u{2014} ".as_bytes().to_vec();
                out.extend_from_slice(line);
                out
            }
            None => Vec::new(),
        }
    }
}

/// What [`parse_log_file`] extracts from a run's stderr file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedLog {
    /// The detached rival PID, or 0 when the run was not detached.
    pub pid: i64,
    /// Start time of `pid`, pinned at parse time (0 if unknown).
    pub pid_start: i64,
    /// This run's session IDs, de-duplicated, in first-seen order.
    pub ids: Vec<String>,
}

/// Extracts the detached rival PID and the run's session
/// IDs. The PID's start time is pinned now so a later PID reuse cannot make
/// a recycled PID look like our still-running rival.
pub fn parse_log_file(path: &Path) -> Result<ParsedLog, String> {
    parse_log_file_with(path, |pid| {
        procinfo::start_nanos(proc_pid(pid)).unwrap_or(0)
    })
}

/// [`parse_log_file`] with an injectable start-time lookup.
pub fn parse_log_file_with(
    path: &Path,
    start_nanos: impl FnOnce(i64) -> i64,
) -> Result<ParsedLog, String> {
    let quoted = format!("{:?}", path.to_string_lossy());
    let data = read_file(path).map_err(|e| format!("read log file {quoted}: {e}"))?;

    let mut pid = 0;
    if let Some(m) = DETACHED_PID_RE.captures(&data) {
        // An out-of-range value leaves pid at 0.
        pid = std::str::from_utf8(&m[1])
            .ok()
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0);
    }
    let pid_start = if pid > 0 { start_nanos(pid) } else { 0 };

    let ids = scan_session_ids(&data);
    if pid == 0 && ids.is_empty() {
        return Err(format!(
            "no detached pid or run session found in {quoted} (run may have failed before launch)"
        ));
    }
    Ok(ParsedLog {
        pid,
        pid_start,
        ids,
    })
}

/// Collects session IDs only from THIS run's "starting …" lines. Startup
/// maintenance (ReapOrphans, queue ReapDead) logs old session IDs into the
/// same detached stderr with message:"reaping …"; tracking those would
/// mis-report a healthy run. Both fields must be on the same line.
fn scan_session_ids(data: &[u8]) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for line in data.split(|&b| b == b'\n') {
        if !STARTING_MARKER_RE.is_match(line) {
            continue;
        }
        let Some(m) = SESSION_ID_RE.captures(line) else {
            continue;
        };
        // The class is ASCII, so the capture is valid UTF-8.
        let id = String::from_utf8_lossy(&m[1]).into_owned();
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
}

/// Decodes only the four fields wait needs, so an
/// unrelated malformed field (prompt, times) does not matter. Any decode
/// error, or a missing file, is "not found".
pub fn load_session_status(sessions_dir: &Path, id: &str) -> SessionStatus {
    let Ok(data) = std::fs::read(sessions_dir.join(format!("{id}.json"))) else {
        return SessionStatus::default();
    };
    decode_session_status(&data).unwrap_or_default()
}

/// Decodes only the four fields wait needs. A missing key or `null` gives
/// the default; a wrong type in one of them is an error.
fn decode_session_status(data: &[u8]) -> Option<SessionStatus> {
    let o = session::Outcome::from_json(data).ok()?;
    Some(SessionStatus {
        status: o.status,
        exit_code: o.exit_code,
        duration: o.duration,
        error_msg: o.error_msg,
        found: true,
        ..SessionStatus::default()
    })
}

/// Reads a file; the error text is `open <path>: <errno>` or
/// `read <path>: <errno>`.
fn read_file(path: &Path) -> Result<Vec<u8>, String> {
    let op_err = |op: &str, err: io::Error| format!("{op} {}: {}", path.to_string_lossy(), err);
    let mut file = std::fs::File::open(path).map_err(|e| op_err("open", e))?;
    let mut data = Vec::new();
    file.read_to_end(&mut data).map_err(|e| op_err("read", e))?;
    Ok(data)
}

/// A PID narrowed to the platform `pid_t`; out of range is 0.
fn proc_pid(pid: i64) -> i32 {
    i32::try_from(pid).unwrap_or(0)
}

/// Whether the rival process `pid` with start time `start` is alive.
fn rival_alive(pid: i64, start: i64) -> bool {
    procinfo::alive(proc_pid(pid), start)
}

#[cfg(test)]
mod tests;
