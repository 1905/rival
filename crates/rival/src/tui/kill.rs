//! Stopping runs from the detail screen. Go: `internal/dashboard/kill.go`
//! and the stop half of `detail_view.go` / `model.go`.
//!
//! A stop never trusts a bare PID. A session's `pid` is the provider child
//! while it runs and the waiting rival process (its owner) while it is
//! queued; SIGTERM to either ends the run, and the owner then finalizes the
//! record itself. Either way the PID is signalled only when its recorded
//! start time still matches the live process, checked again right before the
//! signal, because the OS hands out a dead run's PID to unrelated processes.
//!
//! Windows has no signal a detached run can catch, so a stop targets the
//! owner instead (`owner_pid`, `owner_pid_start`; see [`stop_identity`]):
//! `TerminateProcess` on it, after the same identity check, through a handle
//! that pins the process. The owner's Job closes with it and ends the
//! provider tree. The owner cannot finalize the record, so the stop waits up
//! to the drain grace for the owner and the provider to die, then runs the
//! crashed-owner path: the session reaper fails the record and the queue's
//! `reap_dead` frees the slot.

use std::io;
use std::sync::Arc;

use chrono::{DateTime, FixedOffset};

use rival_core::logging;
use rival_core::paths::Paths;
use rival_core::procinfo;
use rival_core::session::Session;

use super::model::DisplayItem;
use super::session_list::is_live;

/// Reports whether `pid` is still the process that started at `start` (Unix
/// ns). It authorizes signals, so it must never pass an unrecorded start.
pub type AliveFn = fn(pid: i64, start: i64) -> bool;

/// Go: `procinfo.SameProcess` behind the TUI's `alive` seam.
pub fn same_process(pid: i64, start: i64) -> bool {
    i32::try_from(pid).is_ok_and(|pid| procinfo::same_process(pid, start))
}

/// The process calls a stop makes. Tests replace them with recorders; the
/// default is [`ProcessOps::SYSTEM`].
#[derive(Debug, Clone, Copy)]
pub struct ProcessOps {
    /// Identity: authorizes a signal.
    pub alive: AliveFn,
    /// Running state: whether `pid`, started at `start`, has not exited.
    /// The Windows stop waits on it. Identity alone cannot tell death there:
    /// an exited process that any handle still holds keeps its creation
    /// time. Unix stops do not wait (the owner finalizes the record).
    #[cfg_attr(not(windows), allow(dead_code))]
    pub running: fn(pid: i64, start: i64) -> bool,
    /// Ends the run that owns `pid`, whose recorded start is `start`.
    pub terminate: fn(pid: i64, start: i64) -> io::Result<()>,
}

impl ProcessOps {
    pub const SYSTEM: ProcessOps = ProcessOps {
        alive: same_process,
        running,
        terminate,
    };
}

/// Go `procinfo.Alive` behind the stop's death wait: the process exists and
/// has not exited (Windows: its handle is not signaled), and a recorded
/// start still matches, so a reused PID reads as dead.
pub fn running(pid: i64, start: i64) -> bool {
    i32::try_from(pid).is_ok_and(|pid| procinfo::alive(pid, start))
}

/// The process a stop targets, with its recorded start time. Unix: the
/// session's `pid` (the provider while it runs, the owner while queued).
/// Windows: always the owner rival process; its Job takes the provider down.
pub fn stop_identity(s: &Session) -> (i64, i64) {
    if cfg!(windows) {
        (s.owner_pid, s.owner_pid_start)
    } else {
        (s.pid, s.pid_start)
    }
}

/// Unix: SIGTERM to `pid`. Non-positive PIDs are refused: `kill(0)` and
/// `kill(-1)` would signal whole process groups. `start` was checked by
/// [`may_stop`] right before.
#[cfg(unix)]
pub fn terminate(pid: i64, _start: i64) -> io::Result<()> {
    let pid = i32::try_from(pid)
        .ok()
        .filter(|&p| p > 0)
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    // SAFETY: kill only sends a signal; the PID was checked positive.
    if unsafe { libc::kill(pid, libc::SIGTERM) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Windows: `TerminateProcess` on the owner. The handle is opened first and
/// the creation time is checked on that same handle, so the process cannot
/// be swapped between the check and the call. A missing or mismatching
/// identity is refused.
#[cfg(windows)]
pub fn terminate(pid: i64, start: i64) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::Threading::{PROCESS_TERMINATE, TerminateProcess};

    let pid = i32::try_from(pid)
        .ok()
        .filter(|&p| p > 0)
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    if start == 0 {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    let process =
        procinfo::windows::open(pid, PROCESS_TERMINATE).ok_or_else(io::Error::last_os_error)?;
    if procinfo::windows::creation_nanos(&process) != Some(start) {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "process identity changed",
        ));
    }
    // SAFETY: a valid handle with PROCESS_TERMINATE; exit code 1 like Go's Kill.
    if unsafe { TerminateProcess(process.as_raw_handle(), 1) } != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Go: `unverifiedNotice`. Why a live-looking run got no signal.
pub const UNVERIFIED_NOTICE: &str = "cannot verify process identity";

/// Go: `mayStop`. A recorded non-zero PID start that `alive` confirms still
/// matches the process. Without a start time the PID could belong to any
/// process that reused it, so it is never signalled.
pub fn may_stop(s: &Session, alive: AliveFn) -> bool {
    let (pid, start) = stop_identity(s);
    pid > 0 && start != 0 && alive(pid, start)
}

/// Go: `liveTargets`. The members of `item` a stop would signal: running or
/// queued, with a known PID whose process `alive` confirms is still this
/// run's. A stale "running" record whose process is gone is left out.
pub fn live_targets(item: Option<&DisplayItem>, alive: AliveFn) -> Vec<Arc<Session>> {
    let Some(item) = item else {
        return Vec::new();
    };
    item.sessions
        .iter()
        .filter(|s| is_live(&s.status) && may_stop(s, alive))
        .cloned()
        .collect()
}

/// Go: `hasUnverified`. Whether `item` has a live member with a PID but no
/// recorded start time: one a stop refuses to signal.
pub fn has_unverified(item: Option<&DisplayItem>) -> bool {
    item.is_some_and(|item| {
        item.sessions.iter().any(|s| {
            let (pid, start) = stop_identity(s);
            is_live(&s.status) && pid > 0 && start == 0
        })
    })
}

/// Go: the re-check in `updateConfirmKey`. The confirmed targets that the
/// current snapshot of the selected item still shows live with a PID. A run
/// that finished while the bar was open is dropped (its PID may be reused);
/// one still "running" whose process died goes through, so the stop can fail
/// it as dead.
pub fn recheck_targets(
    item: Option<&DisplayItem>,
    confirmed: &[Arc<Session>],
) -> Vec<Arc<Session>> {
    let Some(item) = item else {
        return Vec::new();
    };
    item.sessions
        .iter()
        .filter(|s| {
            confirmed.iter().any(|c| c.id == s.id) && is_live(&s.status) && stop_identity(s).0 > 0
        })
        .cloned()
        .collect()
}

/// What a kill wrote: the fields the row on screen copies from the stored
/// record.
#[derive(Debug, Clone, PartialEq)]
pub struct KillUpdate {
    pub status: String,
    pub exit_code: Option<i64>,
    pub error_msg: String,
    pub end_time: Option<DateTime<FixedOffset>>,
    pub duration: String,
}

impl KillUpdate {
    fn of(s: &Session) -> KillUpdate {
        KillUpdate {
            status: s.status.clone(),
            exit_code: s.exit_code,
            error_msg: s.error_msg.clone(),
            end_time: s.end_time,
            duration: s.duration.clone(),
        }
    }

    /// Copies the update onto the in-memory row.
    pub fn apply(&self, s: &mut Session) {
        s.status.clone_from(&self.status);
        s.exit_code = self.exit_code;
        s.error_msg.clone_from(&self.error_msg);
        s.end_time = self.end_time;
        s.duration.clone_from(&self.duration);
    }
}

/// Go: `failSessionForKill`. Marks `s` failed after the user killed it.
///
/// The list holds summaries, which never carry the full prompt, and `fail`
/// saves the whole record, so failing a summary directly would write an
/// empty prompt over the stored one. This reloads the full record first and
/// fails that. When the reload fails it falls back to the in-memory copy: a
/// lost prompt is better than a session stuck in "running". Returns what was
/// stored, or `None` when the save failed.
pub fn fail_session_for_kill(
    paths: &Paths,
    s: &Session,
    exit_code: i64,
    reason: &str,
) -> Option<KillUpdate> {
    let mut target = match Session::load(paths, &s.id) {
        Ok(full) => full,
        Err(e) => {
            logging::warn()
                .err(&e)
                .str("session", s.id.as_str())
                .msg("could not reload session before kill; failing the in-memory copy");
            s.clone()
        }
    };
    if let Err(e) = target.fail(paths, exit_code, reason) {
        logging::warn()
            .err(&e)
            .str("session", s.id.as_str())
            .msg("failed to save session failure");
        return None;
    }
    Some(KillUpdate::of(&target))
}

/// A confirmed stop for a worker. `item_key` is the run it was confirmed on.
#[derive(Debug, Clone, PartialEq)]
pub struct StopRequest {
    pub item_key: String,
    pub targets: Vec<Arc<Session>>,
}

/// What a stop stored, by session id.
#[derive(Debug, Clone, PartialEq)]
pub struct StopResult {
    pub item_key: String,
    pub updates: Vec<(String, KillUpdate)>,
}

/// Go: `stopSessions`. Sends SIGTERM to each target and marks it failed so
/// the TUI updates at once. Only a target [`may_stop`] confirms, right before
/// the signal, gets it; any other (dead, PID reused) is failed as already
/// dead. A target without a recorded start is neither signalled nor
/// rewritten: a dead run and a live one look the same, so its owner or the
/// reaper finalizes it.
///
/// Rust only: the job runs after a queue wait, so the stored record is read
/// again first. A run that finished while the job waited is left alone: no
/// signal, no "failed" over its real end. When the record cannot be read,
/// the confirmed snapshot decides, as in Go.
///
/// The owning rival process also finalizes a signalled session on SIGTERM.
/// Both writers go through `Session::save`'s unique temp file and rename, so
/// they cannot interleave into partial JSON; the last rename wins.
pub fn stop_sessions(paths: &Paths, procs: &ProcessOps, req: StopRequest) -> StopResult {
    let mut updates = Vec::new();
    // Windows: the owners this stop terminated. A group's members share one
    // owner, so ending it ends them all and the reap fails every member;
    // the later members then only report what was stored. Always empty on
    // Unix, where each member's own process is signalled.
    let mut ended_owners: Vec<(i64, i64)> = Vec::new();
    for s in &req.targets {
        let (pid, start) = stop_identity(s);
        if start == 0 {
            continue;
        }
        if let Ok(now) = Session::load(paths, &s.id)
            && !is_live(&now.status)
        {
            if ended_owners.contains(&(pid, start)) {
                updates.push((s.id.clone(), KillUpdate::of(&now)));
            }
            continue;
        }
        // The process can die between the confirm and y, and its PID can
        // then go to an unrelated process: check identity right before the
        // signal.
        let update = if !may_stop(s, procs.alive) || (procs.terminate)(pid, start).is_err() {
            fail_session_for_kill(paths, s, 1, "killed (process already dead)")
        } else {
            if cfg!(windows) {
                ended_owners.push((pid, start));
            }
            stopped(paths, procs, s)
        };
        if let Some(update) = update {
            updates.push((s.id.clone(), update));
        }
    }
    StopResult {
        item_key: req.item_key,
        updates,
    }
}

/// Unix: the signal was sent. The subprocess executor overwrites this with
/// its own status.
#[cfg(unix)]
fn stopped(paths: &Paths, _procs: &ProcessOps, s: &Session) -> Option<KillUpdate> {
    fail_session_for_kill(paths, s, 137, "killed by user")
}

/// Windows: the owner was terminated. Waits up to the drain grace for the
/// owner and then the recorded provider to die (the owner's Job ends the
/// provider tree when the owner's handles close), then runs the
/// crashed-owner path and reports what the reaper stored. `None` (and a
/// log line) when the run still looks alive after the grace.
///
/// The wait reads running state ([`ProcessOps::running`]), not identity:
/// another observer's open handle keeps an exited process's creation time
/// readable.
#[cfg(windows)]
fn stopped(paths: &Paths, procs: &ProcessOps, s: &Session) -> Option<KillUpdate> {
    use std::time::{Duration, Instant};

    let deadline = Instant::now() + rival_core::executor::PIPE_DRAIN_GRACE;
    let gone = |pid: i64, start: i64| {
        while start != 0 && (procs.running)(pid, start) {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        true
    };
    if !gone(s.owner_pid, s.owner_pid_start) || !gone(s.pid, s.pid_start) {
        logging::warn()
            .str("session", s.id.as_str())
            .msg("run still alive after stop; leaving it to the reaper");
        return None;
    }
    reap_dead(paths);
    match Session::load(paths, &s.id) {
        Ok(now) if !is_live(&now.status) => Some(KillUpdate::of(&now)),
        Ok(_) => {
            logging::warn()
                .str("session", s.id.as_str())
                .msg("stopped run not reaped yet");
            None
        }
        // The reaper cannot read the record either. As on Unix, the
        // confirmed snapshot decides: fail the in-memory copy with the
        // reaper's crashed-owner text, rather than leave it "running".
        Err(e) => {
            logging::warn()
                .err(&e)
                .str("session", s.id.as_str())
                .msg("could not reload stopped session; failing the in-memory copy");
            fail_session_for_kill(paths, s, 1, "orphaned (process dead)")
        }
    }
}

/// The crashed-owner path of `root::reap` with only `paths`: the session
/// reaper first, then the queue's dead-ticket scan (it reads session state).
#[cfg(windows)]
pub fn reap_dead(paths: &Paths) {
    use rival_core::queue;
    use std::time::Duration;

    rival_core::session::reaper::reap_orphans(paths);
    let sessions = paths.sessions_dir();
    let mut manager = queue::Manager::with_settings(
        paths.queue_dir(),
        1,
        rival_core::config::QUEUE_POLL_INTERVAL,
        Duration::ZERO,
    );
    manager.session_live = Some(Arc::new(move |id: &str| queue::session_live(&sessions, id)));
    manager.reap_dead();
}

#[cfg(test)]
mod tests;
#[cfg(all(test, windows))]
mod windows_tests;
