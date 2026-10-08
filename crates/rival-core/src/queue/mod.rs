//! Bounded review executions across independent rival processes.
//!
//! Go: `internal/queue`. Ticket files in `~/.rival/queue/` are guarded by an
//! exclusive lock on `<dir>/.lock`. No daemon: each process scans, reaps dead
//! tickets, and promotes itself when a slot is free, all inside one locked
//! critical section. Queue ordering depends only on ticket files; the
//! liveness check may additionally read the PIDs of the sessions a ticket
//! references (a SIGKILL'd rival can leave a provider CLI child running — the
//! slot stays held until that child dies).
//!
//! Liveness is PID + process-start-time based (see [`crate::procinfo`]): a
//! ticket records the owner's start time, and a recycled PID — belonging to a
//! different process with a different start time — is correctly treated as
//! dead. On a platform where start time is unreadable, the check degrades to
//! a bare existence test.
//!
//! As in Go, [`Manager::enqueue`] and [`Manager::release`] do not take the
//! lock: a ticket is created and removed with single atomic file operations
//! (temp file + rename, unlink), and only scans and promotion are serialized.

mod ticket;

#[cfg(test)]
mod crossproc_tests;
#[cfg(test)]
mod tests;

pub use ticket::{STATE_RUNNING, STATE_WAITING, Ticket};

use std::ffi::OsString;
use std::fmt;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use anyhow::anyhow;
use chrono::{DateTime, FixedOffset, Local, TimeDelta};

use crate::cancel::{Context, ContextError};
use crate::config::{self, Config};
use crate::gostd;
use crate::logging;
use crate::paths::{self, Paths};
use crate::procinfo;
use crate::session::{path_error, proc_pid};
use ticket::{ticket_filename, write_ticket};

/// Go: the text of `ErrQueueTimeout`.
pub const ERR_QUEUE_TIMEOUT: &str = "queue timeout";

/// How old (strictly more than) an unparseable `.json` or leftover `.tmp`
/// must be before scanners delete it; younger ones may be writes in flight.
const STALE_FILE_AGE: Duration = Duration::from_secs(60);

/// A wall clock, injectable for tests. Go: `Manager.Now`.
pub type Clock = Arc<dyn Fn() -> DateTime<FixedOffset> + Send + Sync>;

/// Reports whether a session is "running" with a live PID. Go:
/// `Manager.SessionLive`.
pub type SessionLiveFn = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// Why [`Manager::wait_for_slot`] gave up.
#[derive(Debug)]
pub enum WaitError {
    /// Go: `errors.New("WaitForSlot called before Enqueue")`.
    NotEnqueued,
    /// Go: `fmt.Errorf("%w after %s", ErrQueueTimeout, m.Timeout)`.
    Timeout(Duration),
    /// Go: `ctx.Err()`.
    Context(ContextError),
    /// A lock or ticket write failure, passed through unwrapped as in Go.
    Io(anyhow::Error),
}

impl WaitError {
    /// Go: `errors.Is(err, ErrQueueTimeout)`.
    pub fn is_queue_timeout(&self) -> bool {
        matches!(self, WaitError::Timeout(_))
    }
}

impl fmt::Display for WaitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WaitError::NotEnqueued => f.write_str("WaitForSlot called before Enqueue"),
            WaitError::Timeout(d) => write!(
                f,
                "{ERR_QUEUE_TIMEOUT} after {}",
                gostd::format_duration(duration_nanos(*d))
            ),
            WaitError::Context(e) => fmt::Display::fmt(e, f),
            WaitError::Io(e) => write!(f, "{e:#}"),
        }
    }
}

impl std::error::Error for WaitError {}

/// A ticket with its computed queue position (0 for running tickets).
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub ticket: Ticket,
    pub position: usize,
}

/// Coordinates one process's place in the queue. Every field is injectable
/// for tests; use [`Manager::new`] for production values.
pub struct Manager {
    pub dir: PathBuf,
    pub max_concurrent: usize,
    pub poll_interval: Duration,
    /// Zero waits forever.
    pub timeout: Duration,
    /// `None` is the real clock (Go's `time.Now`, with its monotonic reading
    /// for the timeout deadline).
    pub now: Option<Clock>,
    /// `None` treats every referenced session as dead.
    pub session_live: Option<SessionLiveFn>,
    /// Own ticket after [`Manager::enqueue`].
    ticket: Option<Ticket>,
}

impl fmt::Debug for Manager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Manager")
            .field("dir", &self.dir)
            .field("max_concurrent", &self.max_concurrent)
            .field("poll_interval", &self.poll_interval)
            .field("timeout", &self.timeout)
            .field("now", &self.now.as_ref().map(|_| "<clock>"))
            .field("session_live", &self.session_live.as_ref().map(|_| "<fn>"))
            .field("ticket", &self.ticket)
            .finish()
    }
}

/// The `WaitForSlot` deadline: monotonic on the real clock, wall time on an
/// injected one (an injected `time.Time` carries no monotonic reading).
enum Deadline {
    Mono(Instant),
    Wall(DateTime<FixedOffset>),
}

fn duration_nanos(d: Duration) -> i64 {
    i64::try_from(d.as_nanos()).unwrap_or(i64::MAX)
}

impl Manager {
    /// Go: `queue.New()`. Production settings from `config` and its env.
    pub fn new(paths: &Paths, config: &Config) -> Manager {
        let sessions = paths.sessions_dir();
        Manager {
            dir: paths.queue_dir(),
            max_concurrent: config.max_concurrent(),
            poll_interval: config::QUEUE_POLL_INTERVAL,
            timeout: config.queue_timeout(),
            now: None,
            session_live: Some(Arc::new(move |id: &str| session_live(&sessions, id))),
            ticket: None,
        }
    }

    /// A manager with explicit settings, the real clock and no session
    /// liveness (Go: a `&Manager{...}` literal leaving `Now` and
    /// `SessionLive` nil).
    pub fn with_settings(
        dir: PathBuf,
        max_concurrent: usize,
        poll_interval: Duration,
        timeout: Duration,
    ) -> Manager {
        Manager {
            dir,
            max_concurrent,
            poll_interval,
            timeout,
            now: None,
            session_live: None,
            ticket: None,
        }
    }

    /// This process's own ticket, once enqueued.
    pub fn ticket(&self) -> Option<&Ticket> {
        self.ticket.as_ref()
    }

    /// Creates this process's ticket at the tail of the queue. Returns a
    /// copy: [`Manager::wait_for_slot`] mutates the internal ticket
    /// (promotion, self-heal).
    pub fn enqueue(
        &mut self,
        group_id: &str,
        session_ids: &[String],
        mode: &str,
        workdir: &str,
    ) -> anyhow::Result<Ticket> {
        mkdir_all(&self.dir)?;
        let now = self.now_wall();
        let pid = i64::from(std::process::id());
        let pid_start = procinfo::start_nanos(proc_pid(pid)).unwrap_or(0);
        let id = uuid::Uuid::new_v4().to_string();
        let mut t = Ticket {
            id,
            group_id: group_id.to_string(),
            session_ids: session_ids.to_vec(),
            mode: mode.to_string(),
            pid,
            pid_start,
            state: STATE_WAITING.to_string(),
            created_at: Some(now),
            started_at: None,
            work_dir: workdir.to_string(),
            file: String::new(),
        };
        t.file = ticket_filename(&now, t.pid, &t.id);
        write_ticket(&self.dir, &t)?;
        self.ticket = Some(t.clone());
        Ok(t)
    }

    /// Blocks until this process's ticket is promoted to running.
    /// `on_position` fires (outside the lock) whenever the 1-based position
    /// among waiting tickets changes; `running` is the count of held slots at
    /// that moment. Each pass scans and may promote before the context is
    /// checked, so a cancelled context still gets one promotion attempt.
    pub fn wait_for_slot(
        &mut self,
        ctx: &Context,
        mut on_position: Option<&mut dyn FnMut(usize, usize, usize)>,
    ) -> Result<(), WaitError> {
        if self.ticket.is_none() {
            return Err(WaitError::NotEnqueued);
        }
        let deadline = if self.timeout > Duration::ZERO {
            self.deadline_after(self.timeout)
        } else {
            None
        };
        let dir = self.dir.clone();
        let mut last_pos = None;
        loop {
            let mut promoted = false;
            let mut healed = false;
            let (mut pos, mut total, mut running) = (0, 0, 0);
            with_lock(&dir, || {
                let (waiting, running_count) = self.scan_locked();
                running = running_count;
                let own_id = &self.ticket.as_ref().expect("enqueued").id;
                let Some(idx) = waiting.iter().position(|t| &t.id == own_id) else {
                    // Own ticket vanished (manual rm, queue clear) — self-heal
                    // by re-creating at the tail. Position is recomputed next
                    // loop.
                    let now = self.now_wall();
                    let t = self.ticket.as_mut().expect("enqueued");
                    t.created_at = Some(now);
                    t.file = ticket_filename(&now, t.pid, &t.id);
                    healed = true;
                    logging::warn()
                        .str("ticket", &t.id)
                        .msg("queue ticket vanished, re-creating at tail");
                    return write_ticket(&self.dir, t);
                };
                (pos, total) = (idx + 1, waiting.len());
                // Go: idx < m.MaxConcurrent-runningCount, in signed ints.
                if (idx as i128) < self.max_concurrent as i128 - running_count as i128 {
                    let now = self.now_wall();
                    let t = self.ticket.as_mut().expect("enqueued");
                    t.state = STATE_RUNNING.to_string();
                    t.started_at = Some(now);
                    promoted = true;
                    return write_ticket(&self.dir, t);
                }
                Ok(())
            })
            .map_err(WaitError::Io)?;
            if promoted {
                return Ok(());
            }
            if !healed
                && last_pos != Some(pos)
                && let Some(cb) = on_position.as_mut()
            {
                cb(pos, total, running);
                last_pos = Some(pos);
            }
            if deadline.as_ref().is_some_and(|d| self.past(d)) {
                return Err(WaitError::Timeout(self.timeout));
            }
            if let Some(err) = ctx.wait_timeout(self.poll_interval) {
                return Err(WaitError::Context(err));
            }
        }
    }

    /// Removes this process's ticket, freeing its slot. Idempotent.
    pub fn release(&mut self) {
        let Some(t) = self.ticket.take() else {
            return;
        };
        let path = self.dir.join(&t.file);
        if let Err(e) = fs::remove_file(&path)
            && e.kind() != io::ErrorKind::NotFound
        {
            logging::warn()
                .err(path_error("remove", &path, &e))
                .str("ticket", &t.id)
                .msg("failed to remove queue ticket");
        }
    }

    /// Returns all live tickets: running ones first, then waiting in FIFO
    /// order with 1-based positions. Dead tickets are reaped as a side
    /// effect.
    pub fn list(&self) -> anyhow::Result<Vec<Entry>> {
        with_lock(&self.dir, || {
            let (waiting, _) = self.scan_locked();
            let mut entries: Vec<Entry> = self
                .read_running_locked()
                .into_iter()
                .map(|ticket| Entry {
                    ticket,
                    position: 0,
                })
                .collect();
            entries.extend(waiting.into_iter().enumerate().map(|(i, ticket)| Entry {
                ticket,
                position: i + 1,
            }));
            Ok(entries)
        })
    }

    /// Removes tickets whose rival process and referenced sessions are all
    /// dead. Called before every command so stale tickets clear even when no
    /// waiter is polling.
    pub fn reap_dead(&self) {
        if fs::metadata(&self.dir).is_err() {
            return; // no queue dir yet — nothing to reap
        }
        let _ = with_lock(&self.dir, || {
            self.scan_locked();
            Ok(())
        });
    }

    /// Removes dead tickets, or all tickets when `force` is set (live
    /// waiters self-heal back to the tail). Returns the number removed.
    pub fn clear(&self, force: bool) -> anyhow::Result<usize> {
        with_lock(&self.dir, || {
            let mut removed = 0;
            for f in self.ticket_files()? {
                let Some(t) = self.read_ticket(&f) else {
                    continue;
                };
                if (force || !self.ticket_alive(&t)) && fs::remove_file(self.dir.join(&f)).is_ok() {
                    removed += 1;
                }
            }
            Ok(removed)
        })
    }

    // --- internals ---

    fn now_wall(&self) -> DateTime<FixedOffset> {
        match &self.now {
            Some(clock) => clock(),
            None => Local::now().fixed_offset(),
        }
    }

    /// `None` when the deadline is beyond the clock's range.
    fn deadline_after(&self, d: Duration) -> Option<Deadline> {
        Some(match &self.now {
            None => Deadline::Mono(Instant::now().checked_add(d)?),
            Some(clock) => Deadline::Wall(
                TimeDelta::from_std(d)
                    .ok()
                    .and_then(|d| clock().checked_add_signed(d))
                    .unwrap_or(DateTime::<FixedOffset>::MAX_UTC.fixed_offset()),
            ),
        })
    }

    /// Go: `m.now().After(deadline)`.
    fn past(&self, deadline: &Deadline) -> bool {
        match deadline {
            Deadline::Mono(d) => Instant::now() > *d,
            Deadline::Wall(d) => self.now_wall() > *d,
        }
    }

    /// Reads all tickets, reaps dead ones and stale garbage, and returns the
    /// waiting tickets in FIFO order plus the live running count. Call with
    /// the lock held.
    fn scan_locked(&self) -> (Vec<Ticket>, usize) {
        let Ok(files) = self.ticket_files() else {
            return (Vec::new(), 0);
        };
        let mut waiting = Vec::new();
        let mut running = 0;
        for f in files {
            let Some(t) = self.read_ticket(&f) else {
                continue;
            };
            if !self.ticket_alive(&t) {
                logging::info()
                    .str("ticket", &t.id)
                    .int("pid", t.pid)
                    .str("state", &t.state)
                    .msg("reaping dead queue ticket");
                let _ = fs::remove_file(self.dir.join(&f));
                continue;
            }
            if t.state == STATE_RUNNING {
                running += 1;
            } else {
                waiting.push(t);
            }
        }
        (waiting, running)
    }

    /// Returns live running tickets (no reaping; assumes `scan_locked`
    /// already ran in this critical section).
    fn read_running_locked(&self) -> Vec<Ticket> {
        let Ok(files) = self.ticket_files() else {
            return Vec::new();
        };
        files
            .iter()
            .filter_map(|f| self.read_ticket(f))
            .filter(|t| t.state == STATE_RUNNING)
            .collect()
    }

    /// Returns ticket filenames sorted FIFO (by name = by unixnano prefix)
    /// and cleans up stale `.tmp` leftovers.
    fn ticket_files(&self) -> anyhow::Result<Vec<OsString>> {
        let entries =
            fs::read_dir(&self.dir).map_err(|e| anyhow!(path_error("open", &self.dir, &e)))?;
        let mut files = Vec::new();
        for entry in entries {
            // Go's ReadDir error discards the partial listing too.
            let entry = entry.map_err(|e| anyhow!(path_error("readdirent", &self.dir, &e)))?;
            let name = entry.file_name();
            let bytes = name.as_encoded_bytes();
            if bytes.ends_with(b".json") {
                files.push(name);
            } else if bytes.ends_with(b".tmp") {
                self.remove_if_stale(&name, &entry);
            }
        }
        // Byte order, as Go's sort.Strings.
        files.sort_by(|a, b| a.as_encoded_bytes().cmp(b.as_encoded_bytes()));
        Ok(files)
    }

    /// Parses a ticket file. Unparseable files are skipped, and deleted only
    /// once old enough that they cannot be a write in flight.
    fn read_ticket(&self, name: &OsString) -> Option<Ticket> {
        let path = self.dir.join(name);
        let data = fs::read(&path).ok()?;
        match Ticket::from_json(&data) {
            Ok(mut t) if !t.id.is_empty() => {
                t.file = name.to_string_lossy().into_owned();
                Some(t)
            }
            _ => {
                if let Ok(meta) = fs::metadata(&path)
                    && self.is_stale(&meta)
                {
                    logging::warn()
                        .str("file", name.to_string_lossy())
                        .msg("removing stale unparseable queue file");
                    let _ = fs::remove_file(&path);
                }
                None
            }
        }
    }

    fn remove_if_stale(&self, name: &OsString, entry: &fs::DirEntry) {
        if let Ok(meta) = entry.metadata()
            && self.is_stale(&meta)
        {
            let _ = fs::remove_file(self.dir.join(name));
        }
    }

    /// Go: `m.now().Sub(info.ModTime()) > staleFileAge`.
    fn is_stale(&self, meta: &fs::Metadata) -> bool {
        let Ok(mtime) = meta.modified() else {
            return false;
        };
        let mtime_nanos = match mtime.duration_since(SystemTime::UNIX_EPOCH) {
            Ok(d) => i128::try_from(d.as_nanos()).unwrap_or(i128::MAX),
            Err(e) => -i128::try_from(e.duration().as_nanos()).unwrap_or(i128::MAX),
        };
        let now = self.now_wall();
        let now_nanos =
            i128::from(now.timestamp()) * 1_000_000_000 + i128::from(now.timestamp_subsec_nanos());
        // Go's Sub saturates at the Duration range.
        let age = (now_nanos - mtime_nanos).clamp(i128::from(i64::MIN), i128::from(i64::MAX));
        age > STALE_FILE_AGE.as_nanos() as i128
    }

    /// The rival process is alive, or one of the ticket's sessions is still
    /// running with a live PID (e.g. a surviving provider CLI child after
    /// the rival process was SIGKILL'd — that child still consumes rate
    /// limit, so the slot must stay held).
    fn ticket_alive(&self, t: &Ticket) -> bool {
        if t.pid > 0 && procinfo::alive(proc_pid(t.pid), t.pid_start) {
            return true;
        }
        let Some(live) = &self.session_live else {
            return false;
        };
        t.session_ids.iter().any(|sid| live(sid))
    }
}

/// Go: `os.MkdirAll(dir, 0700)` with the queue's error wrapping.
fn mkdir_all(dir: &Path) -> anyhow::Result<()> {
    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder
        .create(dir)
        .map_err(|e| anyhow!("create queue dir: {}", path_error("mkdir", dir, &e)))
}

/// Runs `f` while holding an exclusive lock on `<dir>/.lock`. The lock file
/// is never deleted (delete + recreate would split the lock across inodes).
/// The lock is released when the guard drops, also on panic.
fn with_lock<T>(dir: &Path, f: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<T> {
    mkdir_all(dir)?;
    let path = dir.join(".lock");
    let mut opts = OpenOptions::new();
    opts.create(true).read(true).write(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let file: File = opts
        .open(&path)
        .map_err(|e| anyhow!("open queue lock: {}", path_error("open", &path, &e)))?;
    let mut lock = fd_lock::RwLock::new(file);
    let _guard = lock
        .write()
        .map_err(|e| anyhow!("flock queue: {}", gostd::os_error_text(&e)))?;
    f()
}

/// The production [`Manager::session_live`]: reads only `status`, `pid` and
/// `pid_start` from `<sessions_dir>/<id>.json` (never the prompt) and checks
/// that the session is running with a live PID (PID-reuse-guarded via the
/// recorded start time).
pub fn session_live(sessions_dir: &Path, id: &str) -> bool {
    let path = paths::clean(&sessions_dir.join(format!("{id}.json")));
    let Ok(data) = fs::read(path) else {
        return false;
    };
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct Live {
        #[serde(deserialize_with = "crate::json::nullable")]
        status: String,
        #[serde(deserialize_with = "crate::json::nullable")]
        pid: i64,
        #[serde(deserialize_with = "crate::json::nullable")]
        pid_start: i64,
    }
    let Ok(Live {
        status,
        pid,
        pid_start,
    }) = crate::json::decode::<Live>(&data)
    else {
        return false;
    };
    status == "running" && procinfo::alive(proc_pid(pid), pid_start)
}
