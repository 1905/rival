//! `rival queue`, `rival queue clear` and `rival sessions`.

use std::io::Write;

use chrono::{DateTime, FixedOffset, Local};
use rival_core::config::{self, Config};
use rival_core::duration;
use rival_core::queue::{self, ClearReport, Entry};
use rival_core::session::Session;

use crate::root::{CmdEnv, CmdError};
use crate::tree::Invocation;

#[cfg(test)]
mod tests;

const NANOS_PER_SECOND: i64 = 1_000_000_000;
/// The 30-minute "stale?" threshold for a waiting ticket.
const STALE_NANOS: i64 = 30 * 60 * NANOS_PER_SECOND;

/// `rival queue`: lists the queue tickets.
pub fn queue_list_action(env: &mut CmdEnv<'_>) -> Result<(), CmdError> {
    let entries = manager(env.cfg)
        .list()
        .map_err(|e| CmdError::plain(format!("read queue: {e:#}")))?;
    write_queue(env.stdout, &entries, Local::now().fixed_offset());
    Ok(())
}

/// `rival queue clear [--force]`: removes queue tickets.
pub fn queue_clear_action(env: &mut CmdEnv<'_>, inv: &Invocation) -> Result<(), CmdError> {
    let force = inv.bool("force");
    let report = manager(env.cfg)
        .clear(force)
        .map_err(|e| CmdError::plain(format!("clear queue: {e:#}")))?;
    let _ = env
        .stdout
        .write_all(clear_message(report, force).as_bytes());
    Ok(())
}

fn manager(cfg: &Config) -> queue::Manager {
    queue::Manager::new(cfg.paths(), cfg)
}

/// The queue table, or `Queue is empty.`.
pub fn write_queue(out: &mut dyn Write, entries: &[Entry], now: DateTime<FixedOffset>) {
    if entries.is_empty() {
        let _ = writeln!(out, "Queue is empty.");
        return;
    }
    let _ = writeln!(
        out,
        "{:<4}  {:<8}  {:<12}  {:<7}  {:<9}  WORKDIR",
        "POS", "STATE", "MODE", "PID", "WAIT"
    );
    for e in entries {
        let t = &e.ticket;
        let pos = if e.position > 0 {
            format!("#{}", e.position)
        } else {
            "-".to_string()
        };
        // Running tickets show time since promotion, waiting since creation.
        let since = match (&t.started_at, t.state.as_str()) {
            (Some(started), queue::STATE_RUNNING) => Some(*started),
            _ => t.created_at,
        };
        // An unset time saturates, as the distant unset time of older
        // releases did.
        let age = |t: Option<DateTime<FixedOffset>>| t.map_or(i64::MAX, |t| sub(now, t));
        let wait = duration::format(round_seconds(age(since)));
        // A waiting ticket far past the default timeout is suspect.
        let state = if t.state == queue::STATE_WAITING && age(t.created_at) > STALE_NANOS {
            "stale?"
        } else {
            t.state.as_str()
        };
        let _ = writeln!(
            out,
            "{pos:<4}  {state:<8}  {:<12}  {:<7}  {wait:<9}  {}",
            t.mode, t.pid, t.work_dir
        );
    }
}

/// `Removed N dead tickets.` and its variants.
pub fn clear_message(report: ClearReport, force: bool) -> String {
    let noun = if force { "ticket" } else { "dead ticket" };
    let removed = report.removed;
    let mut out = if removed == 1 {
        format!("Removed 1 {noun}.\n")
    } else {
        format!("Removed {removed} {noun}s.\n")
    };
    // A plain clear keeps every live ticket, so only `--force` reports it.
    if force && report.kept_live > 0 {
        let kept = report.kept_live;
        let s = if kept == 1 { "" } else { "s" };
        out.push_str(&format!("Kept {kept} live running ticket{s}.\n"));
    }
    out
}

/// `a - b` in nanoseconds, saturating at the int64 range.
fn sub(a: DateTime<FixedOffset>, b: DateTime<FixedOffset>) -> i64 {
    let d = a.signed_duration_since(b);
    d.num_nanoseconds().unwrap_or(if d.num_seconds() < 0 {
        i64::MIN
    } else {
        i64::MAX
    })
}

/// Rounds to whole seconds: halves round away from zero, overflow saturates.
fn round_seconds(d: i64) -> i64 {
    let m = NANOS_PER_SECOND;
    let mut r = d % m;
    if d < 0 {
        r = -r;
        if r + r < m {
            return d + r;
        }
        return d.checked_sub(m - r).unwrap_or(i64::MIN);
    }
    if r + r < m {
        return d - r;
    }
    d.checked_add(m - r).unwrap_or(i64::MAX)
}

/// `rival sessions [--active] [--recent N]`: lists sessions.
pub fn sessions_action(env: &mut CmdEnv<'_>, inv: &Invocation) -> Result<(), CmdError> {
    let all = Session::load_all(env.cfg.paths());
    write_sessions(env.stdout, all, inv.bool("active"), inv.int("recent"));
    Ok(())
}

/// The session list, or `No sessions found.`. `all` is newest first.
pub fn write_sessions(out: &mut dyn Write, all: Vec<Session>, active: bool, recent: i64) {
    let mut sessions: Vec<Session> = all
        .into_iter()
        .filter(|s| !active || s.status == "running")
        .collect();
    if recent > 0 && (recent as u64) < sessions.len() as u64 {
        sessions.truncate(recent as usize);
    }
    if sessions.is_empty() {
        let _ = writeln!(out, "No sessions found.");
        return;
    }
    for s in &sessions {
        let mut dur = s.duration.clone();
        if dur.is_empty() && s.status == "running" {
            dur = "running...".to_string();
        }
        // The first 8 bytes of the id.
        let id = if s.id.len() > 8 {
            String::from_utf8_lossy(&s.id.as_bytes()[..8]).into_owned()
        } else {
            s.id.clone()
        };
        let _ = writeln!(
            out,
            "{id:<8}  {:<20}  {:<10}  {:<6}  {dur}",
            config::engine_label(&s.cli, &s.model),
            s.status,
            s.effort
        );
    }
}
