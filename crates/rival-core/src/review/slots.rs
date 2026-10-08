//! Queue slots for a review batch, and skipped-reviewer records. Go:
//! `internal/review/slots.go`.

use std::io::Write;
use std::time::{Duration, Instant};

use anyhow::anyhow;

use crate::cancel::Context;
use crate::config::{self, Config};
use crate::duration;
use crate::logging;
use crate::paths::Paths;
use crate::queue::Manager;
use crate::session::{Session, duration_text};

#[cfg(test)]
mod tests;

/// A reviewer that was unavailable or failed during a review.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkippedCLI {
    pub cli: String,
    pub model: String,
    pub reason: String,
}

impl SkippedCLI {
    /// The display label for a skipped reviewer.
    pub fn label(&self) -> String {
        config::engine_label(&self.cli, &self.model)
    }
}

/// Renders skipped reviewers as "label: reason" pairs for error messages.
pub fn format_skipped(skipped: &[SkippedCLI]) -> String {
    if skipped.is_empty() {
        return "none".to_string();
    }
    skipped
        .iter()
        .map(|s| {
            let reason = config::public_runtime_error(&s.cli, &s.model, &s.reason);
            format!("{}: {reason}", s.label())
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// What a review batch queues as.
#[derive(Debug, Clone, Copy, Default)]
pub struct GroupSlot<'a> {
    /// Skip the queue (`--no-queue`).
    pub no_queue: bool,
    pub workdir: &'a str,
    pub group_id: &'a str,
    pub mode: &'a str,
}

/// Holds a queue slot. Dropping it, or calling [`SlotRelease::release`],
/// frees the slot, so every return path of the caller releases it. Go: the
/// `release` func, called via `defer`.
#[must_use = "dropping the guard frees the queue slot at once"]
#[derive(Debug)]
pub struct SlotRelease {
    manager: Option<Manager>,
}

impl SlotRelease {
    /// A guard with no slot behind it (queue skipped or unavailable).
    fn none() -> SlotRelease {
        SlotRelease { manager: None }
    }

    /// Frees the slot now.
    pub fn release(self) {
        drop(self);
    }
}

impl Drop for SlotRelease {
    fn drop(&mut self) {
        if let Some(m) = self.manager.as_mut() {
            m.release();
        }
    }
}

/// Enqueues one ticket covering `sessions` and blocks until a slot is free,
/// then marks them running.
///
/// Go takes the ticket sessions and the run sessions as two slices; every
/// caller passes the same one, so this takes one.
///
/// Progress goes to `stderr` (the process stderr in production) with the
/// "rival queue:" prefix, because stdout carries the final output that
/// skills present verbatim. On cancel or timeout the sessions are failed
/// with a clear reason. Keep the returned guard alive while the batch runs.
pub fn wait_for_group_slot(
    ctx: &Context,
    cfg: &Config,
    slot: &GroupSlot<'_>,
    sessions: &mut [Session],
    stderr: &mut dyn Write,
) -> anyhow::Result<SlotRelease> {
    let manager = (!slot.no_queue && !cfg.queue_disabled()).then(|| Manager::new(cfg.paths(), cfg));
    wait_with_manager(ctx, cfg.paths(), manager, slot, sessions, stderr)
}

/// [`wait_for_group_slot`] with the queue manager injected; `None` skips
/// the queue.
fn wait_with_manager(
    ctx: &Context,
    paths: &Paths,
    manager: Option<Manager>,
    slot: &GroupSlot<'_>,
    sessions: &mut [Session],
    stderr: &mut dyn Write,
) -> anyhow::Result<SlotRelease> {
    let Some(mut m) = manager else {
        mark_running(paths, sessions)?;
        return Ok(SlotRelease::none());
    };

    let ids: Vec<String> = sessions.iter().map(|s| s.id.clone()).collect();
    if let Err(e) = m.enqueue(slot.group_id, &ids, slot.mode, slot.workdir) {
        logging::warn()
            .err(format!("{e:#}"))
            .str("mode", slot.mode)
            .msg("queue unavailable — running without queueing");
        mark_running(paths, sessions)?;
        return Ok(SlotRelease::none());
    }

    let start = Instant::now();
    let mut on_position = |pos: usize, total: usize, running: usize| {
        let _ = writeln!(
            stderr,
            "rival queue: position {pos}/{total} ({running} running), waiting {}",
            duration_text(nanos(start.elapsed()))
        );
        for s in sessions.iter_mut() {
            let _ = s.set_queue_position(paths, i64::try_from(pos).unwrap_or(i64::MAX));
        }
    };
    if let Err(wait_err) = m.wait_for_slot(ctx, Some(&mut on_position)) {
        m.release();
        let msg = if wait_err.is_queue_timeout() {
            format!(
                "queue timeout after {} — queue may be wedged; inspect with 'rival queue', purge with 'rival queue clear'",
                duration::format(nanos(m.timeout))
            )
        } else {
            "cancelled while queued".to_string()
        };
        for s in sessions.iter_mut() {
            let _ = s.fail(paths, 1, &msg);
        }
        return Err(anyhow!("rival queue: {msg}"));
    }

    // From here every return path frees the slot through the guard.
    let guard = SlotRelease { manager: Some(m) };
    mark_running(paths, sessions)?;
    let waited = start.elapsed();
    if waited >= Duration::from_secs(1) {
        let _ = writeln!(
            stderr,
            "rival queue: slot acquired after {}",
            duration_text(nanos(waited))
        );
    }
    Ok(guard)
}

/// Marks every session running. On a failure it rolls back the sessions
/// already flipped, so a partial failure never strands a session "running"
/// with no process.
fn mark_running(paths: &Paths, sessions: &mut [Session]) -> anyhow::Result<()> {
    for i in 0..sessions.len() {
        let (done, rest) = sessions.split_at_mut(i);
        if let Err(e) = rest[0].mark_running(paths) {
            for prev in done {
                let _ = prev.fail(paths, 1, "aborted: failed to start review batch");
            }
            return Err(anyhow!("mark session running: {e}"));
        }
    }
    Ok(())
}

fn nanos(d: Duration) -> i64 {
    i64::try_from(d.as_nanos()).unwrap_or(i64::MAX)
}
