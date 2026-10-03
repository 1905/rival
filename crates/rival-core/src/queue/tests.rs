//! Go: `internal/queue/queue_test.go`, plus Rust-only checks of the record
//! bytes, the decoder and the injected clock.

use super::*;
use crate::cancel::Context;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::mpsc;
use std::thread;

/// deadPID is far above any real PID on macOS/Linux defaults.
const DEAD_PID: i64 = 1 << 24;

fn own_pid() -> i64 {
    i64::from(std::process::id())
}

/// Whether process start time is readable on this platform.
fn proc_start_ok() -> bool {
    procinfo::start_nanos(proc_pid(own_pid())).is_some()
}

/// A concurrency-safe stub for `Manager::session_live`.
#[derive(Clone, Default)]
struct LiveSessions(Arc<Mutex<HashMap<String, bool>>>);

impl LiveSessions {
    fn set(&self, id: &str, live: bool) {
        self.0.lock().unwrap().insert(id.to_string(), live);
    }

    fn func(&self) -> SessionLiveFn {
        let ids = Arc::clone(&self.0);
        Arc::new(move |id: &str| ids.lock().unwrap().get(id).copied().unwrap_or(false))
    }
}

fn new_test_manager(dir: &Path, max_concurrent: usize, live: &LiveSessions) -> Manager {
    let mut m = Manager::with_settings(
        dir.to_path_buf(),
        max_concurrent,
        Duration::from_millis(5),
        Duration::from_secs(2),
    );
    m.session_live = Some(live.func());
    m
}

fn unix(nano: i64) -> DateTime<FixedOffset> {
    DateTime::from_timestamp_nanos(nano).fixed_offset()
}

/// Plants a ticket file directly, simulating another process.
fn write_raw_ticket(dir: &Path, nano: i64, pid: i64, state: &str, session_ids: &[&str]) -> String {
    fs::create_dir_all(dir).unwrap();
    let id = format!("raw-{nano}");
    // Record the real start time for live PIDs so the reuse guard sees a
    // match; dead PIDs get 0 (irrelevant — they're not alive anyway).
    let pid_start = procinfo::start_nanos(proc_pid(pid)).unwrap_or(0);
    let t = Ticket {
        id: id.clone(),
        session_ids: session_ids.iter().map(|s| s.to_string()).collect(),
        mode: "test".to_string(),
        pid,
        pid_start,
        state: state.to_string(),
        created_at: unix(nano),
        file: ticket_filename(&unix(nano), pid, &id),
        ..Ticket::default()
    };
    write_ticket(dir, &t).unwrap();
    t.file
}

fn bg() -> Context {
    Context::background()
}

/// Joins a waiter thread with a bound, so a hang fails instead of stalling.
fn recv<T>(rx: &mpsc::Receiver<T>, what: &str) -> T {
    rx.recv_timeout(Duration::from_secs(10))
        .unwrap_or_else(|_| panic!("{what}: no result within 10s"))
}

fn spawn_wait(mut m: Manager, ctx: Context) -> mpsc::Receiver<(Result<(), WaitError>, Manager)> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let r = m.wait_for_slot(&ctx, None);
        let _ = tx.send((r, m));
    });
    rx
}

// Go: TestImmediatePromoteOnEmptyQueue.
#[test]
fn immediate_promote_on_empty_queue() {
    let dir = tempfile::tempdir().unwrap();
    let mut m = new_test_manager(dir.path(), 1, &LiveSessions::default());
    m.enqueue("", &["s1".to_string()], "review", "/tmp")
        .unwrap();
    let start = Instant::now();
    m.wait_for_slot(&bg(), None).unwrap();
    let d = start.elapsed();
    assert!(
        d <= Duration::from_millis(500),
        "promotion on empty queue took {d:?}, want immediate"
    );
    assert_eq!(m.ticket().unwrap().state, STATE_RUNNING);
}

// Go: TestFIFOOrder.
#[test]
fn fifo_order() {
    let dir = tempfile::tempdir().unwrap();
    let live = LiveSessions::default();

    let mut m1 = new_test_manager(dir.path(), 1, &live);
    m1.enqueue("", &[], "review", "").unwrap();
    m1.wait_for_slot(&bg(), None).unwrap();

    let mut m2 = new_test_manager(dir.path(), 1, &live);
    let mut m3 = new_test_manager(dir.path(), 1, &live);
    m2.enqueue("", &[], "review", "").unwrap();
    thread::sleep(Duration::from_millis(1)); // distinct unixnano prefixes
    m3.enqueue("", &[], "review", "").unwrap();

    let (order_tx, order_rx) = mpsc::channel();
    let mut handles = Vec::new();
    for (n, mut m) in [(2, m2), (3, m3)] {
        let tx = order_tx.clone();
        handles.push(thread::spawn(move || {
            m.wait_for_slot(&bg(), None)
                .unwrap_or_else(|e| panic!("waiter {n}: {e}"));
            tx.send(n).unwrap();
            m.release();
        }));
    }
    drop(order_tx);

    if let Ok(n) = order_rx.recv_timeout(Duration::from_millis(50)) {
        panic!("waiter {n} promoted while slot held");
    }

    m1.release();
    for h in handles {
        h.join().unwrap();
    }
    let got: Vec<i32> = order_rx.iter().collect();
    assert_eq!(got, vec![2, 3], "promotion order");
}

// Go: TestCapacityTwoPromotesBothInOneCycle.
#[test]
fn capacity_two_promotes_both_in_one_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let live = LiveSessions::default();

    let mut m1 = new_test_manager(dir.path(), 2, &live);
    let mut m2 = new_test_manager(dir.path(), 2, &live);
    let mut m3 = new_test_manager(dir.path(), 2, &live);
    for m in [&mut m1, &mut m2, &mut m3] {
        m.enqueue("", &[], "review", "").unwrap();
        thread::sleep(Duration::from_millis(1));
    }

    for (i, m) in [&mut m1, &mut m2].into_iter().enumerate() {
        m.wait_for_slot(&bg(), None)
            .unwrap_or_else(|e| panic!("waiter {}: {e}", i + 1));
    }

    let done = spawn_wait(m3, bg());
    if done.recv_timeout(Duration::from_millis(50)).is_ok() {
        panic!("third waiter promoted with both slots held");
    }

    m1.release();
    let (r, _m3) = recv(&done, "third waiter");
    r.unwrap();
}

// Go: TestDeadTicketReapedThenPromote.
#[test]
fn dead_ticket_reaped_then_promote() {
    let dir = tempfile::tempdir().unwrap();
    write_raw_ticket(dir.path(), 1, DEAD_PID, STATE_RUNNING, &[]);
    write_raw_ticket(dir.path(), 2, DEAD_PID, STATE_WAITING, &[]);

    let mut m = new_test_manager(dir.path(), 1, &LiveSessions::default());
    m.enqueue("", &[], "review", "").unwrap();
    m.wait_for_slot(&bg(), None).unwrap();
    let entries = m.list().unwrap();
    assert_eq!(entries.len(), 1, "entries (dead tickets reaped)");
}

// Go: TestSIGKILLSurvivorHoldsSlot.
#[test]
fn sigkill_survivor_holds_slot() {
    let dir = tempfile::tempdir().unwrap();
    let live = LiveSessions::default();
    live.set("survivor", true);
    // Running ticket: rival PID dead, but its session's provider child lives.
    write_raw_ticket(dir.path(), 1, DEAD_PID, STATE_RUNNING, &["survivor"]);

    let mut m = new_test_manager(dir.path(), 1, &live);
    m.enqueue("", &[], "review", "").unwrap();
    let done = spawn_wait(m, bg());
    if done.recv_timeout(Duration::from_millis(100)).is_ok() {
        panic!("promoted while surviving child holds the slot");
    }

    live.set("survivor", false); // child exited
    let (r, _m) = recv(&done, "waiter");
    r.unwrap();
}

// Go: TestUnparseableFileTolerance.
#[test]
fn unparseable_file_tolerance() {
    let dir = tempfile::tempdir().unwrap();
    let fresh = dir.path().join("0000000001-1-garbage.json");
    fs::write(&fresh, "{not json").unwrap();
    let old = dir.path().join("0000000002-1-old.json");
    fs::write(&old, "{not json").unwrap();
    let past = SystemTime::now() - 2 * STALE_FILE_AGE;
    File::options()
        .write(true)
        .open(&old)
        .unwrap()
        .set_modified(past)
        .unwrap();

    let mut m = new_test_manager(dir.path(), 1, &LiveSessions::default());
    m.enqueue("", &[], "review", "").unwrap();
    m.wait_for_slot(&bg(), None)
        .unwrap_or_else(|e| panic!("garbage files must not block promotion: {e}"));
    assert!(
        fresh.exists(),
        "fresh unparseable file was deleted; could be a write in flight"
    );
    assert!(!old.exists(), "stale unparseable file was not cleaned up");
}

// Go: TestCtxCancelWhileWaiting.
#[test]
fn ctx_cancel_while_waiting() {
    let dir = tempfile::tempdir().unwrap();
    write_raw_ticket(dir.path(), 1, own_pid(), STATE_RUNNING, &[]);

    let mut m = new_test_manager(dir.path(), 1, &LiveSessions::default());
    m.enqueue("", &[], "review", "").unwrap();
    let (ctx, cancel) = bg().with_cancel();
    let done = spawn_wait(m, ctx);
    thread::sleep(Duration::from_millis(20));
    cancel.cancel();
    let (r, _m) = recv(&done, "waiter");
    match r {
        Err(WaitError::Context(ContextError::Canceled)) => {}
        other => panic!("err = {other:?}, want context.Canceled"),
    }
}

// Go: TestQueueTimeout.
#[test]
fn queue_timeout() {
    let dir = tempfile::tempdir().unwrap();
    write_raw_ticket(dir.path(), 1, own_pid(), STATE_RUNNING, &[]);

    let mut m = new_test_manager(dir.path(), 1, &LiveSessions::default());
    m.timeout = Duration::from_millis(50);
    m.enqueue("", &[], "review", "").unwrap();
    let err = m.wait_for_slot(&bg(), None).unwrap_err();
    assert!(
        err.is_queue_timeout(),
        "err = {err:?}, want ErrQueueTimeout"
    );
    assert_eq!(err.to_string(), "queue timeout after 50ms");
}

// Go: TestPIDReuseGuardReapsRecycledHolder. A running ticket whose PID is
// live but whose recorded start time does NOT match the live process (i.e.
// the PID was recycled by an unrelated process) must be reaped, freeing the
// slot.
#[test]
fn pid_reuse_guard_reaps_recycled_holder() {
    // Go skips where the start time is unsupported. macOS, Linux and Windows
    // (GetProcessTimes) all support it, so here the case always runs.
    assert!(
        proc_start_ok(),
        "process start time must be readable on this platform"
    );
    let dir = tempfile::tempdir().unwrap();
    // Plant a running ticket with OUR live pid but a bogus start time.
    let id = "recycled";
    let t = Ticket {
        id: id.to_string(),
        mode: "test".to_string(),
        pid: own_pid(),
        pid_start: 1, // deliberately wrong — simulates a recycled PID
        state: STATE_RUNNING.to_string(),
        created_at: unix(1),
        file: ticket_filename(&unix(1), own_pid(), id),
        ..Ticket::default()
    };
    write_ticket(dir.path(), &t).unwrap();

    let mut m = new_test_manager(dir.path(), 1, &LiveSessions::default());
    m.enqueue("", &[], "review", "").unwrap();
    // The recycled-PID holder must be reaped → we promote immediately.
    m.wait_for_slot(&bg(), None)
        .unwrap_or_else(|e| panic!("recycled-PID ticket should have been reaped: {e}"));
}

// Go: TestSelfHealAfterTicketRemoved.
#[test]
fn self_heal_after_ticket_removed() {
    let dir = tempfile::tempdir().unwrap();
    let holder = write_raw_ticket(dir.path(), 1, own_pid(), STATE_RUNNING, &[]);

    let mut m = new_test_manager(dir.path(), 1, &LiveSessions::default());
    let tk = m.enqueue("", &[], "review", "").unwrap();
    let done = spawn_wait(m, bg());
    thread::sleep(Duration::from_millis(20));

    fs::remove_file(dir.path().join(&tk.file)).unwrap();
    thread::sleep(Duration::from_millis(50)); // let the waiter self-heal
    fs::remove_file(dir.path().join(&holder)).unwrap();
    let (r, m) = recv(&done, "waiter");
    r.unwrap_or_else(|e| panic!("waiter did not recover from removed ticket: {e}"));
    // Rust-only: the healed ticket kept its id under a new tail filename.
    let healed = m.ticket().unwrap();
    assert_eq!(healed.id, tk.id);
    assert_ne!(healed.file, tk.file);
    assert!(healed.created_at > tk.created_at);
}

// Go: TestPositionCallback.
#[test]
fn position_callback() {
    let dir = tempfile::tempdir().unwrap();
    let holder = write_raw_ticket(dir.path(), 1, own_pid(), STATE_RUNNING, &[]);
    let ahead = write_raw_ticket(dir.path(), 2, own_pid(), STATE_WAITING, &[]);

    let mut m = new_test_manager(dir.path(), 1, &LiveSessions::default());
    m.enqueue("", &[], "review", "").unwrap();

    let positions = Arc::new(Mutex::new(Vec::new()));
    let (tx, rx) = mpsc::channel();
    {
        let positions = Arc::clone(&positions);
        thread::spawn(move || {
            let mut cb = |pos: usize, _total: usize, _running: usize| {
                positions.lock().unwrap().push(pos);
            };
            let _ = tx.send(m.wait_for_slot(&bg(), Some(&mut cb)));
        });
    }
    thread::sleep(Duration::from_millis(30));
    fs::remove_file(dir.path().join(&ahead)).unwrap();
    thread::sleep(Duration::from_millis(30));
    if let Err(e) = fs::remove_file(dir.path().join(&holder)) {
        assert_eq!(e.kind(), io::ErrorKind::NotFound, "{e}");
    }
    recv(&rx, "waiter").unwrap();
    let positions = positions.lock().unwrap();
    assert!(
        positions.len() >= 2 && positions[0] == 2 && *positions.last().unwrap() == 1,
        "positions = {positions:?}, want [2 ... 1]"
    );
}

// Go: TestClearForceAndDeadOnly.
#[test]
fn clear_force_and_dead_only() {
    let dir = tempfile::tempdir().unwrap();
    write_raw_ticket(dir.path(), 1, DEAD_PID, STATE_RUNNING, &[]);
    let live_file = write_raw_ticket(dir.path(), 2, own_pid(), STATE_WAITING, &[]);

    let m = new_test_manager(dir.path(), 1, &LiveSessions::default());
    assert_eq!(m.clear(false).unwrap(), 1, "dead-only clear removed");
    assert!(
        dir.path().join(&live_file).exists(),
        "dead-only clear removed a live ticket"
    );

    assert_eq!(m.clear(true).unwrap(), 1, "force clear removed");
}

// ---- Rust-only checks ----

/// Go `json.MarshalIndent` bytes of a waiting and a promoted ticket, from Go
/// 1.25.14 against the current `ticket.go` (`Ticket{...}` literal values
/// below, UTC and +03:00 zones).
#[test]
fn ticket_json_matches_go_bytes() {
    let created = DateTime::parse_from_rfc3339("2026-10-02T14:05:09.1234Z").unwrap();
    let started = DateTime::parse_from_rfc3339("2026-10-02T17:06:00+03:00").unwrap();
    let minimal = Ticket {
        id: "abc".into(),
        mode: "review".into(),
        pid: 42,
        state: STATE_WAITING.into(),
        created_at: created,
        ..Ticket::default()
    };
    assert_eq!(
        String::from_utf8(minimal.to_json().unwrap()).unwrap(),
        "{\n  \"id\": \"abc\",\n  \"mode\": \"review\",\n  \"pid\": 42,\n  \"state\": \"waiting\",\n  \"created_at\": \"2026-10-02T14:05:09.1234Z\"\n}"
    );
    let full = Ticket {
        id: "6f1c2e7a-0000-4000-8000-000000000001".into(),
        group_id: "g<1>".into(),
        session_ids: vec!["s1".into(), "s&2".into()],
        mode: "review".into(),
        pid: 4242,
        pid_start: 1_790_000_000_123_456_000,
        state: STATE_RUNNING.into(),
        created_at: created,
        started_at: Some(started),
        work_dir: "/tmp/w".into(),
        ..Ticket::default()
    };
    assert_eq!(
        String::from_utf8(full.to_json().unwrap()).unwrap(),
        "{\n  \"id\": \"6f1c2e7a-0000-4000-8000-000000000001\",\n  \"group_id\": \"g\\u003c1\\u003e\",\n  \"session_ids\": [\n    \"s1\",\n    \"s\\u00262\"\n  ],\n  \"mode\": \"review\",\n  \"pid\": 4242,\n  \"pid_start\": 1790000000123456000,\n  \"state\": \"running\",\n  \"created_at\": \"2026-10-02T14:05:09.1234Z\",\n  \"started_at\": \"2026-10-02T17:06:00+03:00\",\n  \"work_dir\": \"/tmp/w\"\n}"
    );
    assert_eq!(Ticket::from_json(&full.to_json().unwrap()).unwrap(), full);
}

/// Go `fmt.Sprintf("%019d-%d-%s.json", ...)`, including a negative nano.
#[test]
fn ticket_filename_matches_go() {
    let id = "6f1c2e7a-0000-4000-8000-000000000001";
    assert_eq!(
        ticket_filename(&unix(1_790_000_000_123_456_789), 4242, id),
        "1790000000123456789-4242-6f1c2e7a.json"
    );
    assert_eq!(
        ticket_filename(&unix(1), 7, "raw-1"),
        "0000000000000000001-7-raw-1.json"
    );
    assert_eq!(
        ticket_filename(&unix(-1), 7, "abcdefgh"),
        "-000000000000000001-7-abcdefgh.json"
    );
}

/// Go's decoder rules: case-insensitive keys, duplicates in order, `null`,
/// unknown keys; any error skips the file.
#[test]
fn ticket_from_json_follows_go_decoder() {
    let t = Ticket::from_json(
        br#"{"ID":"a","id":"b","Session_IDs":["x",null],"pid":7,"extra":1,
             "started_at":null,"created_at":"2026-01-02T03:04:05Z","mode":null}"#,
    )
    .unwrap();
    assert_eq!(t.id, "b");
    assert_eq!(t.session_ids, vec!["x".to_string(), String::new()]);
    assert_eq!(t.pid, 7);
    assert_eq!(t.started_at, None);
    assert_eq!(t.mode, "");
    assert_eq!(
        gojson::format_time(&t.created_at).unwrap(),
        "2026-01-02T03:04:05Z"
    );

    for bad in [
        &br#"{"id":"a","pid":"7"}"#[..],
        br#"{"id":"a","session_ids":"x"}"#,
        br#"{"id":"a","session_ids":[1]}"#,
        br#"{"id":"a","created_at":"yesterday"}"#,
        br#"[]"#,
        b"{not json",
    ] {
        assert!(
            Ticket::from_json(bad).is_err(),
            "{}",
            String::from_utf8_lossy(bad)
        );
    }
    // A null document decodes to a zero ticket, which the scanner rejects
    // for its empty id.
    assert_eq!(Ticket::from_json(b"null").unwrap().id, "");
}

/// Go reuses the slice a duplicate `session_ids` key decodes into. Expected
/// values come from Go 1.25.14 `json.Unmarshal` into `queue.Ticket`.
#[test]
fn ticket_session_ids_reuse_go_slice() {
    let ids = |doc: &str| {
        Ticket::from_json(format!(r#"{{"id":"t",{doc}}}"#).as_bytes())
            .unwrap()
            .session_ids
    };
    let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(
        ids(r#""session_ids":["a","b"],"session_ids":[null]"#),
        v(&["a"])
    );
    assert_eq!(
        ids(
            r#""session_ids":["a","b","c"],"session_ids":["x"],"session_ids":[null,null,null,null]"#
        ),
        v(&["x", "b", "c", ""])
    );
    assert_eq!(
        ids(
            r#""session_ids":["a","b","c"],"session_ids":["x"],"session_ids":[null,null,null,null,null]"#
        ),
        v(&["x", "b", "c", "", ""])
    );
    assert_eq!(
        ids(r#""session_ids":["a","b"],"session_ids":[],"session_ids":[null]"#),
        v(&[""])
    );
    assert_eq!(
        ids(r#""session_ids":["a","b"],"session_ids":null,"session_ids":[null]"#),
        v(&[""])
    );
    assert_eq!(ids(r#""session_ids":["a"],"session_ids":[]"#), v(&[]));
    assert_eq!(ids(r#""session_ids":["a"],"session_ids":null"#), v(&[]));

    let err =
        Ticket::from_json(br#"{"session_ids":["a","b"],"session_ids":[1,null]}"#).unwrap_err();
    assert_eq!(
        err,
        "json: cannot unmarshal number into Go struct field Ticket.session_ids of type string"
    );
    let err = Ticket::from_json(br#"{"session_ids":["a"],"session_ids":"x"}"#).unwrap_err();
    assert_eq!(
        err,
        "json: cannot unmarshal string into Go struct field Ticket.session_ids of type []string"
    );
}

#[test]
fn enqueue_writes_go_record_and_release_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let qdir = dir.path().join("nested/queue");
    let mut m = Manager::with_settings(qdir.clone(), 1, Duration::from_millis(5), Duration::ZERO);
    let t = m
        .enqueue("grp", &["s1".to_string()], "review", "/w")
        .unwrap();
    assert_eq!(t.state, STATE_WAITING);
    assert_eq!(t.pid, own_pid());
    assert_eq!(t.file, ticket_filename(&t.created_at, t.pid, &t.id));
    let on_disk = Ticket::from_json(&fs::read(qdir.join(&t.file)).unwrap()).unwrap();
    assert_eq!(
        Ticket {
            file: t.file.clone(),
            ..on_disk
        },
        t
    );
    assert!(!qdir.join(format!("{}.tmp", t.file)).exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&qdir), 0o700);
        assert_eq!(mode(&qdir.join(&t.file)), 0o600);
    }

    m.wait_for_slot(&bg(), None).unwrap();
    let promoted = Ticket::from_json(&fs::read(qdir.join(&t.file)).unwrap()).unwrap();
    assert_eq!(promoted.state, STATE_RUNNING);
    assert!(promoted.started_at.is_some());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let lock = fs::metadata(qdir.join(".lock")).unwrap();
        assert_eq!(lock.permissions().mode() & 0o777, 0o600);
    }

    m.release();
    assert!(!qdir.join(&t.file).exists());
    m.release(); // no ticket: no-op
    assert!(m.ticket().is_none());
}

#[test]
fn wait_before_enqueue_errors() {
    let dir = tempfile::tempdir().unwrap();
    let mut m = Manager::with_settings(dir.path().into(), 1, Duration::ZERO, Duration::ZERO);
    let err = m.wait_for_slot(&bg(), None).unwrap_err();
    assert!(matches!(err, WaitError::NotEnqueued));
    assert_eq!(err.to_string(), "WaitForSlot called before Enqueue");
}

/// Go checks the context only after a scan, so a cancelled context still
/// promotes into a free slot, and is reported only when the slot is held.
#[test]
fn promotion_runs_before_cancellation_check() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, cancel) = bg().with_cancel();
    cancel.cancel();

    let mut free = new_test_manager(dir.path(), 1, &LiveSessions::default());
    free.enqueue("", &[], "review", "").unwrap();
    free.wait_for_slot(&ctx, None).unwrap();

    let mut held = new_test_manager(dir.path(), 1, &LiveSessions::default());
    held.enqueue("", &[], "review", "").unwrap();
    let mut calls = Vec::new();
    let mut cb = |pos, total, running| calls.push((pos, total, running));
    let err = held.wait_for_slot(&ctx, Some(&mut cb)).unwrap_err();
    assert!(matches!(err, WaitError::Context(ContextError::Canceled)));
    assert_eq!(err.to_string(), "context canceled");
    assert_eq!(calls, vec![(1, 1, 1)]);
}

/// The callback fires once per distinct position, never on a self-heal pass.
#[test]
fn callback_only_on_position_change_and_not_on_heal() {
    let dir = tempfile::tempdir().unwrap();
    write_raw_ticket(dir.path(), 1, own_pid(), STATE_RUNNING, &[]);
    let mut m = new_test_manager(dir.path(), 1, &LiveSessions::default());
    m.timeout = Duration::from_millis(60);
    let t = m.enqueue("", &[], "review", "").unwrap();
    fs::remove_file(dir.path().join(&t.file)).unwrap(); // heal on pass one
    let mut calls = Vec::new();
    let mut cb = |pos, total, running| calls.push((pos, total, running));
    let err = m.wait_for_slot(&bg(), Some(&mut cb)).unwrap_err();
    assert!(err.is_queue_timeout());
    assert_eq!(calls, vec![(1, 1, 1)], "one call despite many polls");
}

/// The timeout runs on the injected clock; staleness is strictly > 60s on
/// that clock.
#[test]
fn injected_clock_drives_timeout_and_staleness() {
    let dir = tempfile::tempdir().unwrap();
    write_raw_ticket(dir.path(), 1, own_pid(), STATE_RUNNING, &[]);
    let base = Local::now().fixed_offset();
    let offset = Arc::new(Mutex::new(TimeDelta::zero()));
    let clock: Clock = {
        let offset = Arc::clone(&offset);
        Arc::new(move || base + *offset.lock().unwrap())
    };
    let mut m = new_test_manager(dir.path(), 1, &LiveSessions::default());
    m.now = Some(clock);
    m.timeout = Duration::from_secs(3600);
    let t = m.enqueue("", &[], "review", "").unwrap();
    assert_eq!(t.created_at, base);

    let tmp = dir.path().join("x.json.tmp");
    fs::write(&tmp, "").unwrap();
    let mtime =
        DateTime::<Local>::from(fs::metadata(&tmp).unwrap().modified().unwrap()).fixed_offset();
    // Exactly 60s old on the injected clock: kept (Go uses a strict >).
    *offset.lock().unwrap() = mtime - base + TimeDelta::seconds(60);
    m.list().unwrap();
    assert!(tmp.exists(), "a .tmp exactly 60s old must stay");
    *offset.lock().unwrap() = mtime - base + TimeDelta::seconds(60) + TimeDelta::nanoseconds(1);
    m.list().unwrap();
    assert!(!tmp.exists(), "a .tmp older than 60s must go");

    // Jump the clock past the hour-long timeout: the next pass times out.
    // The first position callback runs after the deadline is captured, so
    // the jump waits for it.
    let (ctx, cancel) = bg().with_cancel();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (tx, rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        let mut cb = |_, _, _| {
            let _ = ready_tx.send(());
        };
        let _ = tx.send(m.wait_for_slot(&ctx, Some(&mut cb)));
    });
    if ready_rx.recv_timeout(Duration::from_secs(10)).is_err() {
        cancel.cancel();
        let _ = worker.join();
        panic!("waiter never reported its position");
    }
    *offset.lock().unwrap() = TimeDelta::hours(2);
    let result = rx.recv_timeout(Duration::from_secs(10));
    cancel.cancel(); // frees the worker if it is still waiting
    worker.join().unwrap();
    let err = result.expect("waiter: no result within 10s").unwrap_err();
    assert_eq!(err.to_string(), "queue timeout after 1h0m0s");
}

#[test]
fn list_orders_running_first_then_fifo_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let w2 = write_raw_ticket(dir.path(), 20, own_pid(), STATE_WAITING, &[]);
    let r1 = write_raw_ticket(dir.path(), 30, own_pid(), STATE_RUNNING, &[]);
    let w1 = write_raw_ticket(dir.path(), 10, own_pid(), STATE_WAITING, &[]);
    write_raw_ticket(dir.path(), 5, DEAD_PID, STATE_WAITING, &[]);
    let m = new_test_manager(dir.path(), 1, &LiveSessions::default());
    let got: Vec<(String, usize)> = m
        .list()
        .unwrap()
        .into_iter()
        .map(|e| (e.ticket.file, e.position))
        .collect();
    assert_eq!(got, vec![(r1, 0), (w1, 1), (w2, 2)]);
    assert_eq!(m.list().unwrap().len(), 3, "dead ticket reaped");
}

#[test]
fn reap_dead_without_dir_creates_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let qdir = dir.path().join("queue");
    let m = new_test_manager(&qdir, 1, &LiveSessions::default());
    m.reap_dead();
    assert!(!qdir.exists());

    write_raw_ticket(&qdir, 1, DEAD_PID, STATE_RUNNING, &[]);
    m.reap_dead();
    let names: Vec<_> = fs::read_dir(&qdir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, vec![OsString::from(".lock")]);
}

/// The production liveness reads only status/pid/pid_start.
#[test]
fn session_live_reads_minimal_fields() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path();
    let start = procinfo::start_nanos(proc_pid(own_pid())).unwrap_or(0);
    let write =
        |id: &str, body: String| fs::write(sessions.join(format!("{id}.json")), body).unwrap();
    // A bad start_time or prompt type is irrelevant: those fields are not read.
    write(
        "live",
        format!(
            r#"{{"Status":"running","pid":{},"pid_start":{start},"start_time":"bad","prompt":7}}"#,
            own_pid()
        ),
    );
    write(
        "done",
        format!(r#"{{"status":"completed","pid":{}}}"#, own_pid()),
    );
    write(
        "dead",
        format!(r#"{{"status":"running","pid":{DEAD_PID}}}"#),
    );
    write("badpid", r#"{"status":"running","pid":"1"}"#.to_string());
    write("garbage", "{".to_string());
    assert!(session_live(sessions, "live"));
    for id in ["done", "dead", "badpid", "garbage", "missing"] {
        assert!(!session_live(sessions, id), "{id}");
    }

    // Manager::new wires it to <root>/sessions.
    let paths = Paths::from_home(dir.path());
    fs::create_dir_all(paths.sessions_dir()).unwrap();
    fs::copy(
        sessions.join("live.json"),
        paths.sessions_dir().join("live.json"),
    )
    .unwrap();
    let cfg = Config::new(paths.clone(), HashMap::new(), None);
    let m = Manager::new(&paths, &cfg);
    assert_eq!(m.dir, paths.queue_dir());
    assert_eq!(m.max_concurrent, config::DEFAULT_MAX_CONCURRENT);
    assert_eq!(m.poll_interval, config::QUEUE_POLL_INTERVAL);
    assert_eq!(m.timeout, config::DEFAULT_QUEUE_TIMEOUT);
    let live = m.session_live.as_ref().unwrap();
    assert!(live("live"));
    assert!(!live("missing"));
}

/// The lock is exclusive across handles and released by the guard on drop,
/// also when the critical section panics.
#[test]
fn lock_is_exclusive_and_released_on_panic() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().to_path_buf();
    let r = std::panic::catch_unwind(|| {
        let _ = with_lock(&d, || -> anyhow::Result<()> { panic!("boom") });
    });
    assert!(r.is_err());

    let (held_tx, held_rx) = mpsc::channel();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let d2 = d.clone();
    let holder = thread::spawn(move || {
        with_lock(&d2, || {
            held_tx.send(()).unwrap();
            go_rx.recv().unwrap();
            Ok(())
        })
        .unwrap();
    });
    held_rx.recv().unwrap();
    let (got_tx, got_rx) = mpsc::channel();
    let d3 = d.clone();
    thread::spawn(move || {
        with_lock(&d3, || Ok(())).unwrap();
        got_tx.send(()).unwrap();
    });
    assert!(
        got_rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "second locker entered while the first held the lock"
    );
    go_tx.send(()).unwrap();
    holder.join().unwrap();
    recv(&got_rx, "second locker");
}
