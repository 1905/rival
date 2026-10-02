//! Orphaned-session cleanup.
//!
//! Go: `internal/session/reaper.go`.

use super::summary::load_all_summaries;
use super::{Session, proc_pid};
use crate::logging;
use crate::paths::Paths;
use crate::procinfo;

/// Finds sessions stuck in "running" or "queued" whose process is dead, and
/// marks them failed. A session is only an orphan when BOTH its tracked
/// process (the provider child, once the subprocess starts) AND its owning
/// rival process are dead: between the provider exiting and the owner writing
/// the final status, the session file still says "running" with a dead
/// provider PID, and a concurrent rival invocation's reap in that window would
/// stomp a successful run to failed. A live owner always finalizes its own
/// sessions. Sessions from older releases have no owner recorded (owner_pid 0)
/// and keep the provider-only check.
pub fn reap_orphans(paths: &Paths) {
    reap_orphans_with(paths, |s| {
        logging::info()
            .str("session", &s.id)
            .int("pid", s.pid)
            .str("status", &s.status)
            .msg("reaping orphaned session");
    });
}

/// [`reap_orphans`] with the log line injected, so tests stay off the global
/// log sink.
fn reap_orphans_with(paths: &Paths, mut log_reap: impl FnMut(&Session)) {
    for s in load_all_summaries(paths) {
        if s.status != "running" && s.status != "queued" {
            continue;
        }
        if owner_alive(&s) {
            continue;
        }
        if !procinfo::alive(proc_pid(s.pid), s.pid_start) {
            let msg = if s.status == "queued" {
                "orphaned while queued (process dead)"
            } else {
                "orphaned (process dead)"
            };
            log_reap(&s);
            // Reload the complete record only for an orphan we will mutate, so
            // saving the failure preserves its full prompt and all legacy data.
            let Ok(mut full) = Session::load(paths, &s.id) else {
                continue;
            };
            if full.status != "running" && full.status != "queued" {
                continue;
            }
            if owner_alive(&full) {
                continue;
            }
            if procinfo::alive(proc_pid(full.pid), full.pid_start) {
                continue;
            }
            let _ = full.fail(paths, 1, msg);
        }
    }
}

fn owner_alive(s: &Session) -> bool {
    s.owner_pid != 0 && procinfo::alive(proc_pid(s.owner_pid), s.owner_pid_start)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;
    use crate::session::NewSession;

    // deadPID is above the darwin PID ceiling (~99998) and far beyond typical
    // linux pid_max defaults, so procinfo::alive always reports it dead; the
    // bogus start time guards the unlikely platform where such a PID could exist.
    const DEAD_PID: i64 = 999999;
    const DEAD_PID_START: i64 = 12345;

    fn temp_paths() -> (tempfile::TempDir, Paths) {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(home.path());
        (home, paths)
    }

    fn running(paths: &Paths) -> Session {
        let work = tempfile::tempdir().unwrap();
        let mut s = Session::new_queued(
            paths,
            NewSession {
                cli: "opencode",
                mode: "raw",
                model: config::KIMI_MODEL,
                effort: "max",
                workdir: &work.path().to_string_lossy(),
                prompt: "prompt",
                ..NewSession::default()
            },
        )
        .unwrap();
        // New() used to create a running session outright; mark_running
        // reproduces that state now that queued is the only entry point.
        s.mark_running(paths).unwrap();
        s
    }

    fn reload_by_id(paths: &Paths, id: &str) -> Session {
        Session::load_all(paths)
            .into_iter()
            .find(|s| s.id == id)
            .unwrap_or_else(|| panic!("session {id} not found after reap"))
    }

    fn reap(paths: &Paths) -> Vec<String> {
        let mut logged = Vec::new();
        reap_orphans_with(paths, |s| logged.push(s.id.clone()));
        logged
    }

    // Go: TestReapOrphansSparesDeadProviderWithLiveOwner. A running session
    // whose provider child already exited but whose owning rival is still
    // alive is mid-finalization, not orphaned.
    #[test]
    fn reap_orphans_spares_dead_provider_with_live_owner() {
        let (_home, paths) = temp_paths();
        let mut s = running(&paths);
        // create() records this test process as the (alive) owner; simulate
        // the provider child having already exited.
        s.pid = DEAD_PID;
        s.pid_start = DEAD_PID_START;
        s.save(&paths).unwrap();

        assert!(reap(&paths).is_empty());

        let got = reload_by_id(&paths, &s.id).status;
        assert_eq!(got, "running", "live owner must block the reap");
    }

    // Go: TestReapOrphansReapsWhenOwnerAndProviderDead.
    #[test]
    fn reap_orphans_reaps_when_owner_and_provider_dead() {
        let (_home, paths) = temp_paths();
        let mut s = running(&paths);
        s.pid = DEAD_PID;
        s.pid_start = DEAD_PID_START;
        s.owner_pid = DEAD_PID;
        s.owner_pid_start = DEAD_PID_START;
        s.save(&paths).unwrap();

        assert_eq!(reap(&paths), vec![s.id.clone()]);

        let got = reload_by_id(&paths, &s.id);
        assert_eq!(got.status, "failed");
        assert_eq!(got.error_msg, "orphaned (process dead)");
        assert_eq!(got.prompt, "prompt", "original prompt preserved");
        assert_eq!(got.exit_code, Some(1));
        assert!(got.end_time.is_some());
    }

    // Go: TestReapOrphansReapsLegacySessionWithoutOwner. Sessions written by
    // releases without owner tracking (owner_pid 0) keep the provider-only
    // liveness check.
    #[test]
    fn reap_orphans_reaps_legacy_session_without_owner() {
        let (_home, paths) = temp_paths();
        let mut s = running(&paths);
        s.pid = DEAD_PID;
        s.pid_start = DEAD_PID_START;
        s.owner_pid = 0;
        s.owner_pid_start = 0;
        s.save(&paths).unwrap();

        reap(&paths);

        let got = reload_by_id(&paths, &s.id).status;
        assert_eq!(got, "failed", "legacy sessions keep old behavior");
    }

    #[test]
    fn reap_orphans_marks_dead_queued_session_and_skips_finished_ones() {
        let (_home, paths) = temp_paths();
        let mut queued = running(&paths);
        queued.status = "queued".into();
        queued.pid = DEAD_PID;
        queued.pid_start = DEAD_PID_START;
        queued.owner_pid = 0;
        queued.save(&paths).unwrap();
        let mut done = running(&paths);
        done.status = "completed".into();
        done.pid = DEAD_PID;
        done.owner_pid = 0;
        done.save(&paths).unwrap();

        assert_eq!(reap(&paths), vec![queued.id.clone()]);

        let got = reload_by_id(&paths, &queued.id);
        assert_eq!(got.status, "failed");
        assert_eq!(got.error_msg, "orphaned while queued (process dead)");
        assert_eq!(reload_by_id(&paths, &done.id).status, "completed");
    }
}
