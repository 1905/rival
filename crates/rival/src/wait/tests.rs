use std::cell::Cell;
use std::rc::Rc;

use super::*;

const ID1: &str = "11111111-1111-1111-1111-111111111111";
const ID2: &str = "22222222-2222-2222-2222-222222222222";
const MS: i64 = 1_000_000;
const SECOND: i64 = 1_000 * MS;

fn write_log(content: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("err.log");
    std::fs::write(&p, content).unwrap();
    (dir, p)
}

// ---- Go: TestParseLogFile ----

#[test]
fn parse_log_file_cases() {
    struct Case {
        name: &'static str,
        content: String,
        want_pid: i64,
        want_ids: Vec<&'static str>,
        want_err: bool,
    }
    let cases = [
        Case {
            name: "single CLI",
            content: format!(
                "rival: detached pid=4242\n{{\"level\":\"info\",\"session\":\"{ID1}\",\"effort\":\"low\",\"mode\":\"review\",\"message\":\"starting codex (command mode)\"}}\n"
            ),
            want_pid: 4242,
            want_ids: vec![ID1],
            want_err: false,
        },
        Case {
            name: "multi-id deduped",
            content: format!(
                "rival: detached pid=99\n\
                 {{\"session\":\"{ID1}\",\"cli\":\"codex\",\"message\":\"starting codex\"}}\n\
                 {{\"session\":\"{ID2}\",\"cli\":\"claude\",\"message\":\"starting claude\"}}\n\
                 {{\"session\":\"{ID1}\",\"cli\":\"codex\",\"message\":\"starting codex\"}}\n"
            ),
            want_pid: 99,
            want_ids: vec![ID1, ID2],
            want_err: false,
        },
        Case {
            // THE regression: a ReapOrphans line carries an old session ID
            // but message:"reaping …" — only the run's "starting" session
            // counts.
            name: "ignores reaper session IDs",
            content: format!(
                "rival: detached pid=7\n\
                 {{\"session\":\"{ID2}\",\"pid\":111,\"status\":\"running\",\"message\":\"reaping orphaned session\"}}\n\
                 {{\"session\":\"{ID1}\",\"effort\":\"low\",\"message\":\"starting codex (command mode)\"}}\n"
            ),
            want_pid: 7,
            want_ids: vec![ID1],
            want_err: false,
        },
        Case {
            name: "no pid no run session is error",
            content: format!(
                "some unrelated output\n{{\"session\":\"{ID2}\",\"message\":\"reaping orphaned session\"}}\n"
            ),
            want_pid: 0,
            want_ids: vec![],
            want_err: true,
        },
        Case {
            name: "ids without pid (non-detached) still parse",
            content: format!(
                "{{\"session\":\"{ID1}\",\"message\":\"starting codex (command mode)\"}}\n"
            ),
            want_pid: 0,
            want_ids: vec![ID1],
            want_err: false,
        },
    ];
    for tt in cases {
        let (_dir, p) = write_log(&tt.content);
        let got = parse_log_file(&p);
        if tt.want_err {
            assert!(got.is_err(), "{}: expected error, got {got:?}", tt.name);
            continue;
        }
        let got = got.unwrap_or_else(|e| panic!("{}: unexpected error: {e}", tt.name));
        assert_eq!(got.pid, tt.want_pid, "{}", tt.name);
        assert_eq!(got.ids, tt.want_ids, "{}", tt.name);
    }
}

#[test]
fn parse_log_file_missing_file_errors() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("nope");
    let err = parse_log_file(&p).unwrap_err();
    let path = p.to_string_lossy();
    assert_eq!(
        err,
        format!("read log file \"{path}\": open {path}: no such file or directory")
    );
}

#[test]
fn parse_log_file_directory_is_a_read_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_string_lossy().into_owned();
    assert_eq!(
        parse_log_file(dir.path()).unwrap_err(),
        format!("read log file \"{path}\": read {path}: is a directory")
    );
}

#[test]
fn parse_log_file_no_pid_no_session_message() {
    let (_dir, p) = write_log("nothing here\n");
    assert_eq!(
        parse_log_file(&p).unwrap_err(),
        format!(
            "no detached pid or run session found in \"{}\" (run may have failed before launch)",
            p.to_string_lossy()
        )
    );
}

#[test]
fn parse_log_file_pins_start_once_and_only_for_a_pid() {
    let (_dir, p) = write_log(&format!(
        "rival: detached pid=31\n{{\"session\":\"{ID1}\",\"message\":\"starting codex\"}}\n"
    ));
    let mut calls = Vec::new();
    let got = parse_log_file_with(&p, |pid| {
        calls.push(pid);
        555
    })
    .unwrap();
    assert_eq!(calls, [31]);
    assert_eq!(got.pid_start, 555);

    let (_dir2, p2) = write_log(&format!(
        "{{\"session\":\"{ID1}\",\"message\":\"starting codex\"}}\n"
    ));
    let got = parse_log_file_with(&p2, |_| panic!("no pid, no lookup")).unwrap();
    assert_eq!((got.pid, got.pid_start), (0, 0));
}

#[test]
fn parse_log_file_first_pid_wins_dedupes_by_first_occurrence() {
    let (_dir, p) = write_log(&format!(
        "rival: detached pid=12\nrival: detached pid=13\n\
         {{\"session\":\"{ID2}\",\"message\":\"starting claude\"}}\n\
         {{\"message\":\"starting codex\",\"session\":\"{ID1}\"}}\n\
         {{\"session\":\"{ID2}\",\"message\":\"starting claude\"}}\n"
    ));
    let got = parse_log_file_with(&p, |_| 0).unwrap();
    assert_eq!(got.pid, 12);
    assert_eq!(got.ids, [ID2, ID1]);
}

#[test]
fn parse_log_file_pid_edge_cases() {
    // Sscanf overflow leaves pid 0; Go's \d is ASCII only.
    for content in [
        "rival: detached pid=99999999999999999999\n",
        "rival: detached pid=\u{0661}\u{0662}\n",
    ] {
        let (_dir, p) = write_log(&format!(
            "{content}{{\"session\":\"{ID1}\",\"message\":\"starting codex\"}}\n"
        ));
        let got = parse_log_file_with(&p, |_| panic!("pid must be 0")).unwrap();
        assert_eq!(got.pid, 0, "{content:?}");
        assert_eq!(got.ids, [ID1]);
    }
    let (_dir, p) = write_log("rival: detached pid=0042\n");
    assert_eq!(parse_log_file_with(&p, |_| 0).unwrap().pid, 42);
    // A marker and a session on different lines do not pair up.
    let (_dir, p) = write_log(&format!(
        "rival: detached pid=5\n\"message\":\"starting x\"\n{{\"session\":\"{ID1}\"}}\n"
    ));
    assert!(parse_log_file_with(&p, |_| 0).unwrap().ids.is_empty());
}

// ---- Go: TestWaiterRun and friends ----

fn status(st: &str, exit: Option<i64>, duration: &str, err: &str) -> SessionStatus {
    SessionStatus {
        status: st.to_string(),
        exit_code: exit,
        duration: duration.to_string(),
        error_msg: err.to_string(),
        found: true,
        ..SessionStatus::default()
    }
}

fn completed() -> SessionStatus {
    status("completed", Some(0), "5s", "")
}

fn failed() -> SessionStatus {
    status(
        "failed",
        Some(1),
        "2s",
        "run timeout after 5s (RIVAL_RUN_TIMEOUT)",
    )
}

fn running() -> SessionStatus {
    status("running", None, "", "")
}

fn real_clock() -> Box<dyn FnMut() -> i128> {
    let base = Instant::now();
    Box::new(move || base.elapsed().as_nanos() as i128)
}

fn waiter<'a>(
    pid: i64,
    ids: &[&str],
    load: impl FnMut(&str) -> SessionStatus + 'a,
    alive: impl FnMut(i64, i64) -> bool + 'a,
    out: &'a mut Vec<u8>,
) -> Waiter<'a> {
    Waiter {
        pid,
        pid_start: 123,
        ids: ids.iter().map(|s| s.to_string()).collect(),
        log_file: None,
        poll: MS,
        timeout: 2 * SECOND,
        load_session: Box::new(load),
        ralive: Box::new(alive),
        now: real_clock(),
        out,
    }
}

fn store(entries: Vec<(&'static str, SessionStatus)>) -> impl FnMut(&str) -> SessionStatus {
    move |id| {
        entries
            .iter()
            .find(|(k, _)| *k == id)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    }
}

#[test]
fn waiter_run_cases() {
    struct Case {
        name: &'static str,
        pid: i64,
        ids: Vec<&'static str>,
        store: Vec<(&'static str, SessionStatus)>,
        alive: bool,
        want_code: i32,
        want_out: &'static str,
    }
    let cases = [
        Case {
            name: "log-mode all completed after rival dies",
            pid: 7,
            ids: vec!["a"],
            store: vec![("a", completed())],
            alive: false,
            want_code: WAIT_EXIT_COMPLETED,
            want_out: "completed exit=0",
        },
        Case {
            name: "log-mode failed session → exit 2",
            pid: 7,
            ids: vec!["a"],
            store: vec![("a", failed())],
            alive: false,
            want_code: WAIT_EXIT_FAILED,
            want_out: "RIVAL_RUN_TIMEOUT",
        },
        Case {
            name: "log-mode rival dead but session stuck running → crash",
            pid: 7,
            ids: vec!["a"],
            store: vec![("a", running())],
            alive: false,
            want_code: WAIT_EXIT_CRASHED,
            want_out: "crashed",
        },
        Case {
            name: "session-id mode all terminal",
            pid: 0,
            ids: vec!["a", "b"],
            store: vec![("a", completed()), ("b", failed())],
            alive: false,
            want_code: WAIT_EXIT_FAILED,
            want_out: "failed exit=1",
        },
    ];
    for tt in cases {
        let mut buf = Vec::new();
        let alive = tt.alive;
        let mut w = waiter(
            tt.pid,
            &tt.ids,
            store(tt.store),
            move |_, start| {
                assert_eq!(start, 123, "pinned start time is passed through");
                alive
            },
            &mut buf,
        );
        let code = w.run(&Context::background());
        drop(w);
        let out = String::from_utf8(buf).unwrap();
        assert_eq!(code, tt.want_code, "{} (out: {out:?})", tt.name);
        assert!(out.contains(tt.want_out), "{}: output {out:?}", tt.name);
    }
}

#[test]
fn is_session_id_cases() {
    assert!(is_session_id(ID1));
    assert!(is_session_id("abcdefAB-1234-5678-9abc-DEF012345678"));
    for bad in [
        "../../etc/passwd",
        "not-a-uuid",
        "11111111-1111-1111-1111-111111111111/x",
        "",
        "11111111111111111111111111111111",
        "11111111-1111-1111-1111-111111111111\n",
    ] {
        assert!(!is_session_id(bad), "{bad:?} must be rejected");
    }
}

#[test]
fn waiter_session_id_mode_missing_fails_fast() {
    // No PID, session file never found → fail fast (usage), not hang.
    let mut buf = Vec::new();
    let mut w = waiter(
        0,
        &[ID1],
        |_| SessionStatus::default(),
        |_, _| false,
        &mut buf,
    );
    w.timeout = 3600 * SECOND;
    let code = w.run(&Context::background());
    drop(w);
    assert_eq!(code, WAIT_EXIT_USAGE);
    assert_eq!(
        String::from_utf8(buf).unwrap(),
        "no such session (file not found) — check the ID\n"
    );
}

#[test]
fn waiter_no_false_crash_on_finalize_race() {
    // rival is dead, and the session is non-terminal on the first read but
    // terminal on the re-read. The re-read must give a clean summary.
    let mut buf = Vec::new();
    let mut reads = 0;
    let mut w = waiter(
        7,
        &["a"],
        move |_| {
            reads += 1;
            if reads == 1 { running() } else { completed() }
        },
        |_, _| false,
        &mut buf,
    );
    let code = w.run(&Context::background());
    drop(w);
    assert_eq!(code, WAIT_EXIT_COMPLETED, "out: {buf:?}");
    assert_eq!(String::from_utf8(buf).unwrap(), "a completed exit=0 5s\n");
}

#[test]
fn waiter_timeout() {
    // rival alive forever, sessions never terminal → timeout (exit 4).
    let mut buf = Vec::new();
    let mut calls: i128 = 0;
    let mut w = waiter(7, &["a"], |_| running(), |_, _| true, &mut buf);
    w.timeout = 50 * MS;
    w.now = Box::new(move || {
        calls += 1;
        calls * 20 * i128::from(MS)
    });
    let code = w.run(&Context::background());
    drop(w);
    assert_eq!(code, WAIT_EXIT_TIMEOUT);
    assert_eq!(
        String::from_utf8(buf).unwrap(),
        "still running after 50ms (rival pid 7)\n"
    );
}

// ---- exact output and ordering ----

#[test]
fn summary_lines_are_exact() {
    let mut buf = Vec::new();
    let mut w = waiter(
        0,
        &[ID1, ID2, "abc"],
        store(vec![
            (ID1, status("completed", Some(0), "", "")),
            (ID2, status("failed", None, "1m2s", "boom")),
            ("abc", status("completed", Some(-3), "7s", "")),
        ]),
        |_, _| panic!("no pid, no liveness check"),
        &mut buf,
    );
    let code = w.run(&Context::background());
    drop(w);
    assert_eq!(code, WAIT_EXIT_FAILED);
    assert_eq!(
        String::from_utf8(buf).unwrap(),
        "11111111 completed exit=0 \n22222222 failed exit=- 1m2s — boom\nabc completed exit=-3 7s\n"
    );
}

#[test]
fn crash_line_is_exact() {
    let mut buf = Vec::new();
    let mut w = waiter(4242, &["a"], |_| running(), |_, _| false, &mut buf);
    assert_eq!(w.run(&Context::background()), WAIT_EXIT_CRASHED);
    drop(w);
    assert_eq!(
        String::from_utf8(buf).unwrap(),
        "crashed: rival (pid 4242) exited before finalizing sessions\n"
    );
}

#[test]
fn dead_rival_with_no_ids_is_a_crash() {
    // A detached pid with no "starting" line yet: rival died in the queue.
    let mut buf = Vec::new();
    let mut w = waiter(9, &[], |_| panic!("no ids"), |_, _| false, &mut buf);
    assert_eq!(w.run(&Context::background()), WAIT_EXIT_CRASHED);
    drop(w);
    assert_eq!(
        String::from_utf8(buf).unwrap(),
        "crashed: rival (pid 9) exited before finalizing sessions\n"
    );
}

#[test]
fn zero_or_negative_timeout_checks_status_and_liveness_first() {
    // Terminal sessions in session-ID mode win over an expired timeout.
    for timeout in [0, -SECOND] {
        let mut buf = Vec::new();
        let mut w = waiter(0, &["a"], |_| completed(), |_, _| unreachable!(), &mut buf);
        w.timeout = timeout;
        assert_eq!(w.run(&Context::background()), WAIT_EXIT_COMPLETED);
        drop(w);
        assert_eq!(String::from_utf8(buf).unwrap(), "a completed exit=0 5s\n");
    }
    // A dead rival is reported as such, not as a timeout.
    let mut buf = Vec::new();
    let mut w = waiter(7, &["a"], |_| running(), |_, _| false, &mut buf);
    w.timeout = 0;
    assert_eq!(w.run(&Context::background()), WAIT_EXIT_CRASHED);
    drop(w);
    // Alive and running: immediate timeout, Go duration text.
    for (timeout, text) in [(0, "0s"), (-SECOND, "-1s"), (90 * SECOND, "1m30s")] {
        let mut buf = Vec::new();
        let ticks = Rc::new(Cell::new(0i128));
        let t = Rc::clone(&ticks);
        let mut w = waiter(7, &["a"], |_| running(), |_, _| true, &mut buf);
        w.timeout = timeout;
        // The clock jumps far ahead after the deadline is computed.
        w.now = Box::new(move || {
            let n = t.get();
            t.set(n + 1);
            if n == 0 {
                0
            } else {
                1_000 * i128::from(SECOND)
            }
        });
        assert_eq!(w.run(&Context::background()), WAIT_EXIT_TIMEOUT);
        drop(w);
        assert_eq!(
            ticks.get(),
            2,
            "now() is read once for the deadline, once per check"
        );
        assert_eq!(
            String::from_utf8(buf).unwrap(),
            format!("still running after {text} (rival pid 7)\n")
        );
    }
}

#[test]
fn timeout_line_appends_last_queue_line() {
    let (_dir, p) = write_log(&format!(
        "rival: detached pid=7\n\
         rival queue: position 2/3 (2 running), waiting 0s\n\
         {{\"session\":\"{ID1}\",\"message\":\"starting codex\"}}\n\
         rival queue: position 1/3 (2 running), waiting 2s\n\
         xrival queue: not a prefix\n"
    ));
    let mut buf = Vec::new();
    let mut w = waiter(7, &[ID1], |_| running(), |_, _| true, &mut buf);
    w.log_file = Some(p);
    w.timeout = 0;
    assert_eq!(w.run(&Context::background()), WAIT_EXIT_TIMEOUT);
    drop(w);
    assert_eq!(
        String::from_utf8(buf).unwrap(),
        "still running after 0s (rival pid 7) — rival queue: position 1/3 (2 running), waiting 2s\n"
    );
}

#[test]
fn timeout_line_without_queue_line_or_log() {
    let (_dir, p) = write_log("rival: detached pid=7\n");
    let mut buf = Vec::new();
    let mut w = waiter(7, &[], |_| running(), |_, _| true, &mut buf);
    w.log_file = Some(p);
    w.timeout = 0;
    assert_eq!(w.run(&Context::background()), WAIT_EXIT_TIMEOUT);
    drop(w);
    assert_eq!(
        String::from_utf8(buf).unwrap(),
        "still running after 0s (rival pid 7)\n"
    );
}

#[test]
fn log_gains_ids_while_queued() {
    // The run's "starting" line appears only after it leaves the queue.
    let (_dir, p) = write_log("rival: detached pid=7\n");
    let log = p.clone();
    let mut alive_calls = 0;
    let seen = Rc::new(Cell::new(0usize));
    let seen_in = Rc::clone(&seen);
    let mut buf = Vec::new();
    let mut w = waiter(
        7,
        &[],
        move |id| {
            assert_eq!(id, ID1);
            seen_in.set(seen_in.get() + 1);
            completed()
        },
        move |_, _| {
            alive_calls += 1;
            if alive_calls == 1 {
                let mut f = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
                writeln!(
                    f,
                    "{{\"session\":\"{ID1}\",\"message\":\"starting codex\"}}"
                )
                .unwrap();
                return true;
            }
            false
        },
        &mut buf,
    );
    w.log_file = Some(p);
    assert_eq!(w.run(&Context::background()), WAIT_EXIT_COMPLETED);
    drop(w);
    // Tick 2 rescans (1 read) and re-reads after death (1 read).
    assert_eq!(seen.get(), 2);
    assert_eq!(
        String::from_utf8(buf).unwrap(),
        "11111111 completed exit=0 5s\n"
    );
}

#[test]
fn rescan_never_shrinks_the_id_list() {
    let (_dir, p) = write_log(&format!(
        "{{\"session\":\"{ID1}\",\"message\":\"starting codex\"}}\n"
    ));
    let mut buf = Vec::new();
    let mut w = waiter(
        0,
        &[ID1, ID2],
        |_| completed(),
        |_, _| unreachable!(),
        &mut buf,
    );
    w.log_file = Some(p);
    assert_eq!(w.run(&Context::background()), WAIT_EXIT_COMPLETED);
    drop(w);
    assert_eq!(
        String::from_utf8(buf).unwrap(),
        "11111111 completed exit=0 5s\n22222222 completed exit=0 5s\n"
    );
}

#[test]
fn interrupted_while_waiting() {
    let (ctx, cancel) = Context::background().with_cancel();
    cancel.cancel();
    let mut buf = Vec::new();
    let loads = Rc::new(Cell::new(0));
    let loads_in = Rc::clone(&loads);
    let mut w = waiter(
        7,
        &["a"],
        move |_| {
            loads_in.set(loads_in.get() + 1);
            running()
        },
        |_, _| true,
        &mut buf,
    );
    w.timeout = 3600 * SECOND;
    // A done context is noticed only at the sleep, after a full status pass.
    assert_eq!(w.run(&ctx), WAIT_EXIT_CRASHED);
    drop(w);
    assert_eq!(
        String::from_utf8(buf).unwrap(),
        "interrupted while waiting\n"
    );
    assert_eq!(loads.get(), 1);
}

#[test]
fn polls_until_rival_dies() {
    let mut buf = Vec::new();
    let mut alive_calls = 0;
    let mut w = waiter(
        7,
        &["a"],
        |_| completed(),
        move |_, _| {
            alive_calls += 1;
            alive_calls < 3
        },
        &mut buf,
    );
    assert_eq!(w.run(&Context::background()), WAIT_EXIT_COMPLETED);
    drop(w);
    assert_eq!(String::from_utf8(buf).unwrap(), "a completed exit=0 5s\n");
}

// ---- session status decoding ----

#[test]
fn decode_session_status_matches_go_anonymous_struct() {
    let ok = |json: &str| decode_session_status(json.as_bytes());
    // Unrelated malformed fields do not matter.
    let got = ok(r#"{"id":5,"prompt":{"x":[1]},"created_at":"not a time","status":"completed","exit_code":0,"duration":"3s","error":""}"#).unwrap();
    assert_eq!(
        got,
        SessionStatus {
            status: "completed".into(),
            exit_code: Some(0),
            duration: "3s".into(),
            found: true,
            ..SessionStatus::default()
        }
    );
    // Case-insensitive keys; duplicate keys: last wins; null pointer resets.
    let got = ok(r#"{"STATUS":"running","Status":"failed","exit_code":2,"Exit_Code":null,"ERROR":"x","error":null}"#).unwrap();
    assert_eq!(got.status, "failed");
    assert_eq!(got.exit_code, None);
    assert_eq!(got.error_msg, "x", "null leaves a string unchanged");
    // Top-level null decodes to the zero struct, which counts as found.
    assert_eq!(
        ok("null"),
        Some(SessionStatus {
            found: true,
            ..SessionStatus::default()
        })
    );
    for bad in [
        r#"{"status":5}"#,
        r#"{"exit_code":"0"}"#,
        r#"{"exit_code":1.0}"#,
        r#"{"exit_code":1e2}"#,
        r#"{"exit_code":99999999999999999999}"#,
        r#"{"duration":true}"#,
        r#"{"status":"completed","error":["a"]}"#,
        r#"[]"#,
        r#""s""#,
        r#"{"status":"completed""#,
        "",
    ] {
        assert_eq!(ok(bad), None, "{bad}");
    }
}

#[test]
fn load_session_status_reads_session_dir() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(
        sessions.join(format!("{ID1}.json")),
        r#"{"status":"failed","exit_code":1,"duration":"2s","error":"boom"}"#,
    )
    .unwrap();
    std::fs::write(sessions.join(format!("{ID2}.json")), "{not json").unwrap();
    let got = load_session_status(&sessions, ID1);
    assert_eq!(got, status("failed", Some(1), "2s", "boom"));
    assert_eq!(
        load_session_status(&sessions, ID2),
        SessionStatus::default()
    );
    assert_eq!(
        load_session_status(&sessions, "33333333-3333-3333-3333-333333333333"),
        SessionStatus::default()
    );
}

// ---- wait_action ----

fn opts(log: &str, timeout: i64, poll: i64) -> WaitOptions {
    WaitOptions {
        log: PathBuf::from(log),
        timeout,
        poll,
    }
}

fn temp_paths() -> (tempfile::TempDir, Paths) {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths {
        root: dir.path().join(".rival"),
    };
    std::fs::create_dir_all(paths.sessions_dir()).unwrap();
    (dir, paths)
}

fn action(o: &WaitOptions, args: &[&str], paths: &Paths) -> (Result<(), WaitError>, String) {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let mut out = Vec::new();
    let res = wait_action(o, &args, paths, &Context::background(), &mut out);
    (res, String::from_utf8(out).unwrap())
}

fn usage(msg: &str) -> Result<(), WaitError> {
    Err(WaitError {
        code: WAIT_EXIT_USAGE,
        message: msg.to_string(),
    })
}

#[test]
fn wait_action_usage_errors() {
    let (_dir, paths) = temp_paths();
    for poll in [0, -1] {
        assert_eq!(
            action(&opts("x.log", SECOND, poll), &[ID1], &paths),
            (usage("--poll must be > 0"), String::new())
        );
    }
    assert_eq!(
        action(&opts("x.log", SECOND, MS), &[ID1], &paths),
        (
            usage("pass either --log or session IDs, not both"),
            String::new()
        )
    );
    assert_eq!(
        action(&opts("", SECOND, MS), &[], &paths),
        (
            usage("provide --log <file> or one or more session IDs"),
            String::new()
        )
    );
    assert_eq!(
        action(&opts("", SECOND, MS), &[ID1, "../x\"y"], &paths),
        (
            usage(r#"invalid session ID "../x\"y" (expected a UUID)"#),
            String::new()
        )
    );
    let missing = paths.root.join("missing.log");
    let m = missing.to_string_lossy();
    assert_eq!(
        action(&opts(&m, SECOND, MS), &[], &paths),
        (
            usage(&format!(
                "read log file \"{m}\": open {m}: no such file or directory"
            )),
            String::new()
        )
    );
}

#[test]
fn wait_action_session_id_mode_outcomes() {
    let (_dir, paths) = temp_paths();
    let sessions = paths.sessions_dir();
    std::fs::write(
        sessions.join(format!("{ID1}.json")),
        r#"{"status":"completed","exit_code":0,"duration":"4s"}"#,
    )
    .unwrap();
    assert_eq!(
        action(&opts("", SECOND, MS), &[ID1], &paths),
        (Ok(()), "11111111 completed exit=0 4s\n".to_string())
    );

    std::fs::write(
        sessions.join(format!("{ID2}.json")),
        r#"{"status":"failed","exit_code":1,"duration":"1s","error":"boom"}"#,
    )
    .unwrap();
    assert_eq!(
        action(&opts("", SECOND, MS), &[ID1, ID2], &paths),
        (
            Err(WaitError {
                code: WAIT_EXIT_FAILED,
                message: "rival wait: exit 2".into()
            }),
            "11111111 completed exit=0 4s\n22222222 failed exit=1 1s — boom\n".to_string()
        )
    );

    let ghost = "33333333-3333-3333-3333-333333333333";
    assert_eq!(
        action(&opts("", SECOND, MS), &[ID1, ghost], &paths),
        (
            Err(WaitError {
                code: WAIT_EXIT_USAGE,
                message: "rival wait: exit 64".into()
            }),
            "no such session (file not found) — check the ID\n".to_string()
        )
    );
}

#[test]
fn wait_action_session_id_mode_timeout() {
    let (_dir, paths) = temp_paths();
    std::fs::write(
        paths.sessions_dir().join(format!("{ID1}.json")),
        r#"{"status":"running"}"#,
    )
    .unwrap();
    assert_eq!(
        action(&opts("", 0, MS), &[ID1], &paths),
        (
            Err(WaitError {
                code: WAIT_EXIT_TIMEOUT,
                message: "rival wait: exit 4".into()
            }),
            "still running after 0s (rival pid 0)\n".to_string()
        )
    );
}

#[test]
fn wait_action_log_mode_without_pid() {
    let (dir, paths) = temp_paths();
    std::fs::write(
        paths.sessions_dir().join(format!("{ID1}.json")),
        r#"{"status":"completed","exit_code":0,"duration":"4s"}"#,
    )
    .unwrap();
    let log = dir.path().join("err.log");
    std::fs::write(
        &log,
        format!(
            "{{\"session\":\"{ID2}\",\"message\":\"reaping orphaned session\"}}\n\
             {{\"session\":\"{ID1}\",\"message\":\"starting codex\"}}\n"
        ),
    )
    .unwrap();
    assert_eq!(
        action(&opts(&log.to_string_lossy(), SECOND, MS), &[], &paths),
        (Ok(()), "11111111 completed exit=0 4s\n".to_string())
    );
}

#[test]
fn default_poll_is_queue_poll_interval() {
    assert_eq!(DEFAULT_POLL, 2 * SECOND);
}
