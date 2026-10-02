use super::*;
use rival_core::session::NewSession;
use rival_core::session::summary::load_summary_file;

use crate::tui::testkit::{FAKE_PROCS, harness, set_alive, set_signal_fails, signals};

const STORED_PROMPT: &str = "the full prompt that a summary drops on the floor";

/// A running session stored under `paths`, with fake PID `pid`.
fn stored_running(paths: &Paths, pid: i64) -> Session {
    let mut s = Session::new_queued(
        paths,
        NewSession {
            cli: "codex",
            mode: "review",
            model: "gpt-5.6-sol",
            effort: "high",
            workdir: "/src/proj",
            prompt: STORED_PROMPT,
            review_scope: "src/",
            group_id: "",
        },
    )
    .unwrap();
    s.mark_running(paths).unwrap();
    s.pid = pid;
    s.save(paths).unwrap();
    s
}

/// The summary the list holds for `s`: no prompt.
fn summary_of(paths: &Paths, s: &Session) -> Session {
    let path = paths.sessions_dir().join(format!("{}.json", s.id));
    let size = std::fs::metadata(&path).unwrap().len() as i64;
    let summary = load_summary_file(&path, size).unwrap();
    assert_eq!(
        summary.prompt, "",
        "test premise broken: the summary carries a prompt"
    );
    summary
}

// Go: TestKillReloadsBeforeFail. The TUI list is built from summaries,
// which never carry the full prompt. Killing a session must reload the
// stored record before it writes, or the save destroys the prompt on disk.
#[test]
fn kill_reloads_before_fail() {
    let h = harness();
    let full = stored_running(h.paths(), 4242);
    let summary = summary_of(h.paths(), &full);

    let update = fail_session_for_kill(h.paths(), &summary, 137, "killed by user").unwrap();

    let reloaded = Session::load(h.paths(), &full.id).unwrap();
    assert_eq!(reloaded.prompt, STORED_PROMPT, "prompt after kill");
    assert_eq!(reloaded.status, "failed");
    assert_eq!(reloaded.exit_code, Some(137));
    assert_eq!(reloaded.error_msg, "killed by user");
    // The row on screen copies exactly what was stored.
    assert_eq!(
        update,
        KillUpdate {
            status: "failed".into(),
            exit_code: Some(137),
            error_msg: "killed by user".into(),
            end_time: reloaded.end_time,
            duration: reloaded.duration.clone(),
        }
    );
    let mut row = summary.clone();
    update.apply(&mut row);
    assert_eq!((row.status.as_str(), row.exit_code), ("failed", Some(137)));
    assert_eq!(row.prompt, "", "apply copies the status fields only");
}

// Go fallback: a record that cannot be reloaded is failed from the
// in-memory copy. Its prompt is lost, but the session does not stay stuck in
// "running".
#[test]
fn kill_fails_the_in_memory_copy_when_the_reload_fails() {
    let h = harness();
    let full = stored_running(h.paths(), 4242);
    let summary = summary_of(h.paths(), &full);
    let file = h.paths().sessions_dir().join(format!("{}.json", full.id));
    std::fs::write(&file, b"{ not json").unwrap();

    let update = fail_session_for_kill(h.paths(), &summary, 1, "killed (process already dead)");

    assert_eq!(update.unwrap().status, "failed");
    let stored = Session::load(h.paths(), &full.id).unwrap();
    assert_eq!(stored.status, "failed");
    assert_eq!(stored.exit_code, Some(1));
    assert_eq!(
        stored.prompt, "",
        "the fallback writes the summary, as in Go"
    );
}

// A save that fails is reported as no update, so the row keeps its status.
#[test]
fn kill_returns_no_update_when_the_save_fails() {
    let h = harness();
    // "sessions" is a file, so both the reload and the save fail.
    std::fs::create_dir_all(&h.paths().root).unwrap();
    std::fs::write(h.paths().sessions_dir(), b"").unwrap();
    let s = Session {
        id: "nosave00-0000".into(),
        status: "running".into(),
        ..Session::default()
    };
    assert_eq!(
        fail_session_for_kill(h.paths(), &s, 137, "killed by user"),
        None
    );
}

fn live(id: &str, status: &str, pid: i64, pid_start: i64) -> Arc<Session> {
    Arc::new(Session {
        id: id.into(),
        status: status.into(),
        pid,
        pid_start,
        ..Session::default()
    })
}

fn item(sessions: Vec<Arc<Session>>) -> DisplayItem {
    DisplayItem { sessions }
}

fn ids(list: &[Arc<Session>]) -> Vec<&str> {
    list.iter().map(|s| s.id.as_str()).collect()
}

#[test]
fn live_targets_need_a_verified_identity() {
    let _h = harness();
    let it = item(vec![
        live("running", "running", 11, 5),
        live("queued", "queued", 12, 5),
        live("no-start", "running", 13, 0),
        live("no-pid", "running", 0, 5),
        live("done", "completed", 14, 5),
        live("failed", "failed", 15, 5),
    ]);
    set_alive(true);
    assert_eq!(
        ids(&live_targets(Some(&it), fake_alive_ref())),
        ["running", "queued"]
    );
    // alive says the process is gone (or the PID was reused): no targets.
    set_alive(false);
    assert!(live_targets(Some(&it), fake_alive_ref()).is_empty());
    assert!(live_targets(None, fake_alive_ref()).is_empty());

    assert!(has_unverified(Some(&it)), "no-start is live with a PID");
    let verified = item(vec![
        live("a", "running", 11, 5),
        live("b", "running", 0, 0),
    ]);
    assert!(!has_unverified(Some(&verified)));
    let finished = item(vec![live("a", "completed", 11, 0)]);
    assert!(
        !has_unverified(Some(&finished)),
        "a finished run is not unverified"
    );
    assert!(!has_unverified(None));
}

fn fake_alive_ref() -> AliveFn {
    FAKE_PROCS.alive
}

#[test]
fn may_stop_never_passes_an_unrecorded_start() {
    fn always(_: i64, _: i64) -> bool {
        true
    }
    assert!(may_stop(&live("a", "running", 1, 9), always));
    assert!(!may_stop(&live("a", "running", 1, 0), always), "PIDStart 0");
    assert!(!may_stop(&live("a", "running", 0, 9), always), "PID 0");
    assert!(
        !may_stop(&live("a", "running", -1, 9), always),
        "negative PID"
    );
}

#[test]
fn recheck_keeps_confirmed_targets_still_live_now() {
    let confirmed = vec![live("a", "running", 11, 5), live("b", "running", 12, 5)];
    let now = item(vec![
        live("a", "completed", 11, 5), // finished meanwhile
        live("b", "running", 12, 5),
        live("c", "running", 13, 5), // never confirmed
    ]);
    assert_eq!(ids(&recheck_targets(Some(&now), &confirmed)), ["b"]);
    // A dead process still "running" goes through, so the stop can fail it.
    let dead = item(vec![live("a", "running", 11, 5)]);
    assert_eq!(ids(&recheck_targets(Some(&dead), &confirmed)), ["a"]);
    assert!(recheck_targets(None, &confirmed).is_empty());
}

#[test]
fn stop_sessions_signals_only_verified_processes() {
    let h = harness();
    let paths = h.paths();
    let ok = stored_running(paths, 990001);
    let gone = stored_running(paths, 990002);
    let mut unverified = stored_running(paths, 990003);
    unverified.pid_start = 0;
    unverified.save(paths).unwrap();

    // ok is alive and signals; then alive flips for the rest.
    set_alive(true);
    let res = stop_sessions(
        paths,
        &FAKE_PROCS,
        StopRequest {
            item_key: "solo:x".into(),
            targets: vec![Arc::new(summary_of(paths, &ok))],
        },
    );
    set_alive(false);
    let res2 = stop_sessions(
        paths,
        &FAKE_PROCS,
        StopRequest {
            item_key: "solo:y".into(),
            targets: vec![
                Arc::new(summary_of(paths, &gone)),
                Arc::new(summary_of(paths, &unverified)),
            ],
        },
    );
    assert_eq!(signals(), [990001], "only the verified live process");
    assert_eq!(res.item_key, "solo:x");
    assert_eq!(ids_of(&res), [ok.id.as_str()]);
    assert_eq!(
        ids_of(&res2),
        [gone.id.as_str()],
        "unverified is not rewritten"
    );

    let load = |s: &Session| Session::load(paths, &s.id).unwrap();
    let stored = load(&ok);
    assert_eq!(
        (
            stored.exit_code,
            stored.error_msg.as_str(),
            stored.prompt.as_str()
        ),
        (Some(137), "killed by user", STORED_PROMPT)
    );
    let stored = load(&gone);
    assert_eq!(
        (stored.exit_code, stored.error_msg.as_str()),
        (Some(1), "killed (process already dead)")
    );
    assert_eq!(load(&unverified).status, "running");
}

#[test]
fn stop_sessions_fails_a_process_the_signal_cannot_reach() {
    let h = harness();
    let s = stored_running(h.paths(), 990004);
    set_alive(true);
    set_signal_fails(true);
    let res = stop_sessions(
        h.paths(),
        &FAKE_PROCS,
        StopRequest {
            item_key: String::new(),
            targets: vec![Arc::new(s.clone())],
        },
    );
    assert_eq!(signals(), [990004]);
    assert_eq!(res.updates[0].1.exit_code, Some(1));
    assert_eq!(
        Session::load(h.paths(), &s.id).unwrap().error_msg,
        "killed (process already dead)"
    );
}

/// Controller finding 3: the stop job runs after a queue wait. A run that
/// completed meanwhile is neither signalled nor rewritten as failed, even
/// though its PID now looks dead.
#[test]
fn stop_job_leaves_a_run_that_completed_while_it_waited() {
    let h = harness();
    let paths = h.paths();
    let mut full = stored_running(paths, 990005);
    let req = StopRequest {
        item_key: "solo:x".into(),
        targets: vec![Arc::new(summary_of(paths, &full))],
    };
    full.complete(paths, 0, 10, 1).unwrap();
    set_alive(false);
    let res = stop_sessions(paths, &FAKE_PROCS, req.clone());
    assert!(signals().is_empty());
    assert!(res.updates.is_empty());
    let stored = Session::load(paths, &full.id).unwrap();
    assert_eq!(
        (stored.status.as_str(), stored.exit_code),
        ("completed", Some(0))
    );
    // Even a live-looking PID gets no signal once the record says done.
    set_alive(true);
    assert!(stop_sessions(paths, &FAKE_PROCS, req).updates.is_empty());
    assert!(signals().is_empty());
}

/// When the record cannot be read, the confirmed snapshot decides, as in
/// Go: the live target is signalled and failed from the in-memory copy.
#[test]
fn stop_job_falls_back_to_the_snapshot_when_the_record_is_unreadable() {
    let h = harness();
    let paths = h.paths();
    let full = stored_running(paths, 990006);
    let summary = summary_of(paths, &full);
    std::fs::write(paths.sessions_dir().join(format!("{}.json", full.id)), b"{").unwrap();
    set_alive(true);
    let res = stop_sessions(
        paths,
        &FAKE_PROCS,
        StopRequest {
            item_key: String::new(),
            targets: vec![Arc::new(summary)],
        },
    );
    assert_eq!(signals(), [990006]);
    assert_eq!(res.updates[0].1.exit_code, Some(137));
}

fn ids_of(res: &StopResult) -> Vec<&str> {
    res.updates.iter().map(|(id, _)| id.as_str()).collect()
}

/// The real signal refuses PIDs that would reach process groups or wrap. No
/// signal is sent for any of these.
#[cfg(unix)]
#[test]
fn terminate_refuses_group_and_out_of_range_pids() {
    for pid in [0, -1, -42, i64::from(i32::MAX) + 1, i64::MIN] {
        let err = terminate(pid).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "pid {pid}");
    }
}

#[test]
fn same_process_never_passes_an_unrecorded_start() {
    let me = i64::from(std::process::id());
    assert!(!same_process(me, 0));
    assert!(!same_process(i64::from(i32::MAX) + 1, 1), "out of range");
    assert!(!same_process(-1, 1));
}
