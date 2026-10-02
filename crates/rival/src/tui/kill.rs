//! Stopping runs from the detail screen. Go: `internal/dashboard/kill.go`
//! and the stop half of `detail_view.go` / `model.go`.
//!
//! A stop never trusts a bare PID. A session's `pid` is the provider child
//! while it runs and the waiting rival process (its owner) while it is
//! queued; SIGTERM to either ends the run, and the owner then finalizes the
//! record itself. Either way the PID is signalled only when its recorded
//! start time still matches the live process, checked again right before the
//! signal, because the OS hands out a dead run's PID to unrelated processes.

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

/// The process calls a stop makes. Tests replace both with recorders; the
/// default is [`ProcessOps::SYSTEM`].
#[derive(Debug, Clone, Copy)]
pub struct ProcessOps {
    pub alive: AliveFn,
    /// Ends the run that owns `pid`.
    pub terminate: fn(pid: i64) -> io::Result<()>,
}

impl ProcessOps {
    pub const SYSTEM: ProcessOps = ProcessOps {
        alive: same_process,
        terminate,
    };
}

/// Unix: SIGTERM to `pid`. Non-positive PIDs are refused: `kill(0)` and
/// `kill(-1)` would signal whole process groups.
#[cfg(unix)]
pub fn terminate(pid: i64) -> io::Result<()> {
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

/// Windows stop lands with Task 5.1: TerminateProcess on the detached owner,
/// whose Job Object then closes the provider. Until then nothing is
/// signalled. `procinfo::same_process` is false there too, so the confirm bar
/// never opens.
#[cfg(not(unix))]
pub fn terminate(_pid: i64) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "stopping a run is not supported on this platform yet",
    ))
}

/// Go: `unverifiedNotice`. Why a live-looking run got no signal.
pub const UNVERIFIED_NOTICE: &str = "cannot verify process identity";

/// Go: `mayStop`. A recorded non-zero PID start that `alive` confirms still
/// matches the process. Without a start time the PID could belong to any
/// process that reused it, so it is never signalled.
pub fn may_stop(s: &Session, alive: AliveFn) -> bool {
    s.pid > 0 && s.pid_start != 0 && alive(s.pid, s.pid_start)
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
        item.sessions
            .iter()
            .any(|s| is_live(&s.status) && s.pid > 0 && s.pid_start == 0)
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
        .filter(|s| confirmed.iter().any(|c| c.id == s.id) && is_live(&s.status) && s.pid > 0)
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
    for s in &req.targets {
        if s.pid_start == 0 {
            continue;
        }
        if Session::load(paths, &s.id).is_ok_and(|now| !is_live(&now.status)) {
            continue;
        }
        // The process can die between the confirm and y, and its PID can
        // then go to an unrelated process: check identity right before the
        // signal.
        let update = if !may_stop(s, procs.alive) || (procs.terminate)(s.pid).is_err() {
            fail_session_for_kill(paths, s, 1, "killed (process already dead)")
        } else {
            // Signal sent. The subprocess executor overwrites this with its
            // own status.
            fail_session_for_kill(paths, s, 137, "killed by user")
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

#[cfg(test)]
mod tests;
