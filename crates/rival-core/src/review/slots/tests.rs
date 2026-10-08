//! Slot tests, plus queue-path pins with an injected manager and stderr
//! writer.

use std::time::Duration;

use super::*;
use crate::config::{GPT56_SOL_MODEL, GROK_LABEL, GROK_MODEL, KIMI_MODEL};
use crate::review::testutil::{config_in, temp_config};
use crate::session::NewSession;

fn queued(cfg: &Config) -> Session {
    Session::new_queued(
        cfg.paths(),
        NewSession {
            cli: "codex",
            mode: "review",
            model: GPT56_SOL_MODEL,
            effort: "high",
            workdir: "/w",
            prompt: "p",
            review_scope: "",
            group_id: "grp",
        },
    )
    .unwrap()
}

const SLOT: GroupSlot<'static> = GroupSlot {
    no_queue: false,
    workdir: "/w",
    group_id: "grp",
    mode: "review",
};

/// A partial mark-running failure must not strand earlier sessions
/// "running" with no process.
#[test]
fn rolls_back_partial_start() {
    let (_home, cfg) = temp_config();
    let first = queued(&cfg);
    // An ID with a path separator makes save fail for this member after the
    // first one already flipped to running.
    let broken = Session {
        id: "missing-dir/broken".into(),
        group_id: "grp".into(),
        status: "queued".into(),
        ..Session::default()
    };
    let mut sessions = vec![first, broken];
    let slot = GroupSlot {
        no_queue: true,
        ..SLOT
    };
    let mut stderr = Vec::new();
    let err = wait_for_group_slot(
        &Context::background(),
        &cfg,
        &slot,
        &mut sessions,
        &mut stderr,
    )
    .expect_err("a session cannot be marked running");
    assert!(
        err.to_string().starts_with("mark session running: "),
        "{err}"
    );

    let reloaded = Session::load(cfg.paths(), &sessions[0].id).unwrap();
    assert_eq!(reloaded.status, "failed");
    assert_eq!(reloaded.error_msg, "aborted: failed to start review batch");
    assert!(stderr.is_empty());
}

#[test]
fn format_skipped_distinguishes_models() {
    assert_eq!(format_skipped(&[]), "none");
    let got = format_skipped(&[
        SkippedCLI {
            cli: "opencode".into(),
            model: KIMI_MODEL.into(),
            reason: "failed".into(),
        },
        SkippedCLI {
            cli: GROK_LABEL.into(),
            model: GROK_MODEL.into(),
            reason: "unavailable".into(),
        },
    ]);
    assert_eq!(got, "kimi-k3: failed; grok: unavailable");
}

/// `RIVAL_NO_QUEUE` skips the queue; nothing is written to the
/// queue dir.
#[test]
fn queue_disabled_by_env_marks_running_without_a_ticket() {
    let home = tempfile::tempdir().unwrap();
    let cfg = config_in(home.path(), &[("RIVAL_NO_QUEUE", "1")]);
    let mut sessions = vec![queued(&cfg)];
    let mut stderr = Vec::new();
    let guard = wait_for_group_slot(
        &Context::background(),
        &cfg,
        &SLOT,
        &mut sessions,
        &mut stderr,
    )
    .unwrap();
    assert_eq!(sessions[0].status, "running");
    assert!(!cfg.paths().queue_dir().exists());
    guard.release();
}

fn manager(cfg: &Config, max_concurrent: usize, timeout: Duration) -> Manager {
    Manager::with_settings(
        cfg.paths().queue_dir(),
        max_concurrent,
        Duration::from_millis(5),
        timeout,
    )
}

/// Ticket files in the queue dir (the lock file is not one).
fn ticket_count(cfg: &Config) -> usize {
    std::fs::read_dir(cfg.paths().queue_dir()).map_or(0, |d| {
        d.filter(|e| {
            e.as_ref()
                .is_ok_and(|e| e.file_name().to_string_lossy().ends_with(".json"))
        })
        .count()
    })
}

/// A free slot marks the sessions running and holds a ticket
/// until the guard drops.
#[test]
fn free_slot_holds_ticket_until_guard_drops() {
    let (_home, cfg) = temp_config();
    let mut sessions = vec![queued(&cfg), queued(&cfg)];
    let mut stderr = Vec::new();
    let m = manager(&cfg, 2, Duration::ZERO);
    let guard = wait_with_manager(
        &Context::background(),
        cfg.paths(),
        Some(m),
        &SLOT,
        &mut sessions,
        &mut stderr,
    )
    .unwrap();
    assert!(sessions.iter().all(|s| s.status == "running"));
    assert_eq!(ticket_count(&cfg), 1);
    drop(guard);
    assert_eq!(ticket_count(&cfg), 0);
    // No wait, so no queue lines.
    assert!(stderr.is_empty(), "{}", String::from_utf8_lossy(&stderr));
}

/// A cancelled wait reports its position on stderr, frees the
/// ticket and fails every session.
#[test]
fn cancelled_while_queued() {
    let (_home, cfg) = temp_config();
    let mut sessions = vec![queued(&cfg)];
    let mut stderr = Vec::new();
    let (ctx, cancel) = Context::background().with_cancel();
    cancel.cancel();
    let err = wait_with_manager(
        &ctx,
        cfg.paths(),
        Some(manager(&cfg, 0, Duration::ZERO)),
        &SLOT,
        &mut sessions,
        &mut stderr,
    )
    .unwrap_err();
    assert_eq!(err.to_string(), "rival queue: cancelled while queued");
    assert_eq!(
        String::from_utf8(stderr).unwrap(),
        "rival queue: position 1/1 (0 running), waiting 0s\n"
    );
    assert_eq!(ticket_count(&cfg), 0);
    let reloaded = Session::load(cfg.paths(), &sessions[0].id).unwrap();
    assert_eq!(reloaded.status, "failed");
    assert_eq!(reloaded.error_msg, "cancelled while queued");
    assert_eq!(reloaded.queue_position, 1);
}

/// A queue timeout names the timeout and the recovery commands.
#[test]
fn queue_timeout_message() {
    let (_home, cfg) = temp_config();
    let mut sessions = vec![queued(&cfg)];
    let mut stderr = Vec::new();
    let err = wait_with_manager(
        &Context::background(),
        cfg.paths(),
        Some(manager(&cfg, 0, Duration::from_millis(20))),
        &SLOT,
        &mut sessions,
        &mut stderr,
    )
    .unwrap_err();
    let msg = "queue timeout after 20ms — queue may be wedged; inspect with 'rival queue', purge with 'rival queue clear'";
    assert_eq!(err.to_string(), format!("rival queue: {msg}"));
    assert_eq!(sessions[0].status, "failed");
    assert_eq!(sessions[0].error_msg, msg);
    assert_eq!(ticket_count(&cfg), 0);
}

/// A queue I/O error during the wait prints its own text, not the cancel
/// text. A directory at `.lock` lets `enqueue` (which takes no lock) work
/// and makes the wait's lock open fail.
#[test]
fn queue_io_error_while_waiting_prints_the_error() {
    let (_home, cfg) = temp_config();
    let lock = cfg.paths().queue_dir().join(".lock");
    std::fs::create_dir_all(&lock).unwrap();
    let mut sessions = vec![queued(&cfg)];
    let mut stderr = Vec::new();
    let err = wait_with_manager(
        &Context::background(),
        cfg.paths(),
        Some(manager(&cfg, 1, Duration::from_secs(5))),
        &SLOT,
        &mut sessions,
        &mut stderr,
    )
    .unwrap_err()
    .to_string();
    let prefix = format!(
        "rival queue: queue wait failed: open queue lock: open {}",
        lock.display()
    );
    assert!(err.starts_with(&prefix), "{err}");
    assert!(!err.contains("cancelled"), "{err}");
    assert_eq!(sessions[0].status, "failed");
    assert_eq!(
        format!("rival queue: {}", sessions[0].error_msg),
        err,
        "session error"
    );
    assert_eq!(ticket_count(&cfg), 0);
}

/// An unusable queue dir logs a warning and runs unqueued.
#[test]
fn enqueue_failure_runs_without_queueing() {
    let (_home, cfg) = temp_config();
    std::fs::create_dir_all(cfg.paths().queue_dir().parent().unwrap()).unwrap();
    std::fs::write(cfg.paths().queue_dir(), "not a dir").unwrap();
    let mut sessions = vec![queued(&cfg)];
    let mut stderr = Vec::new();
    // The warning goes to the global log sink; switching that off here would
    // race the logging tests, so it is left on.
    let guard = wait_with_manager(
        &Context::background(),
        cfg.paths(),
        Some(manager(&cfg, 2, Duration::ZERO)),
        &SLOT,
        &mut sessions,
        &mut stderr,
    )
    .unwrap();
    assert_eq!(sessions[0].status, "running");
    assert!(stderr.is_empty());
    drop(guard);
}

/// When marking running fails after the slot was won, the guard
/// still frees the ticket.
#[test]
fn mark_running_failure_after_slot_frees_the_ticket() {
    let (_home, cfg) = temp_config();
    let broken = Session {
        id: "missing-dir/broken".into(),
        status: "queued".into(),
        ..Session::default()
    };
    let mut sessions = vec![queued(&cfg), broken];
    let mut stderr = Vec::new();
    let err = wait_with_manager(
        &Context::background(),
        cfg.paths(),
        Some(manager(&cfg, 2, Duration::ZERO)),
        &SLOT,
        &mut sessions,
        &mut stderr,
    )
    .unwrap_err();
    assert!(
        err.to_string().starts_with("mark session running: "),
        "{err}"
    );
    assert_eq!(ticket_count(&cfg), 0);
    assert_eq!(sessions[0].status, "failed");
}
