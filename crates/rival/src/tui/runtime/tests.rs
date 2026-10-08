//! The runtime with injected parts: a ratatui `TestBackend` for the
//! terminal, temp homes for the watcher and jobs, fake input sources, a
//! fake screen and task-owned `/bin/sh` stand-ins for the viewer launcher.
//! No real terminal, viewer, signal or `~/.rival` is involved.

use super::*;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Child;

use ratatui::backend::TestBackend;

use rival_core::session::Session;
use rival_core::sessionview::SessionEvent;

use crate::tui::jobs::PromptsRequest;
#[cfg(unix)]
use crate::tui::jobs::{LOG_VIEW_TTL, OpenedLog};
use crate::tui::kill::StopRequest;
use crate::tui::logview::{LogKey, LogPane, LogRequest};
use crate::tui::result_view::{ResultRequest, ResultTarget};
use crate::tui::testkit::{
    self, Harness, draw, fixed_now, harness, loading_model, press, preview_fixture_logs, rows,
};

const WAIT: Duration = Duration::from_secs(10);

fn opts() -> LoopOpts {
    LoopOpts {
        resize: true,
        sweep_every: SWEEP_INTERVAL,
        clock: Instant::now,
    }
}

fn run_loop(
    model: &mut Model,
    events: &Receiver<Event>,
    jobs: &JobPool,
    views: &mut LogViews,
    opts: &LoopOpts,
) -> Result<(), String> {
    let mut terminal = Terminal::new(TestBackend::new(130, 30)).unwrap();
    event_loop(model, &mut terminal, events, jobs, views, opts)
}

fn pool(h: &Harness, out: &Sender<Event>) -> JobPool {
    JobPool::start(h.env.clone(), JOB_WORKERS, out.clone()).unwrap()
}

fn term_key(name: &str) -> Event {
    Event::Term(TermEvent::Key(press(name)))
}

/// Ends a loop that never got its expected event, so a broken test fails
/// on its assertion instead of hanging. Dropping it cancels the timer and
/// joins its thread, so nothing outlives the test.
struct Backstop {
    cancel: Option<Sender<()>>,
    timer: Option<JoinHandle<()>>,
}

impl Backstop {
    fn arm(out: &Sender<Event>) -> Backstop {
        let out = out.clone();
        let (cancel, cancelled) = mpsc::channel::<()>();
        let timer = thread::spawn(move || {
            if cancelled.recv_timeout(WAIT) == Err(RecvTimeoutError::Timeout) {
                let _ = out.send(Event::Signal(Signal::Terminate));
            }
        });
        Backstop {
            cancel: Some(cancel),
            timer: Some(timer),
        }
    }
}

impl Drop for Backstop {
    fn drop(&mut self) {
        drop(self.cancel.take());
        if let Some(timer) = self.timer.take() {
            let _ = timer.join();
        }
    }
}

fn finished_sessions() -> Vec<Arc<Session>> {
    vec![Arc::new(Session {
        id: "f0000000-done".into(),
        cli: "codex".into(),
        model: "gpt-6-astra".into(),
        status: "completed".into(),
        start_time: Some(fixed_now()),
        ..Session::default()
    })]
}

// --- exits ------------------------------------------------------------------

/// Bubbletea's exits (controller preflight on 4.1.1): q and a raw ctrl+c
/// quit with exit 0; SIGTERM is `QuitMsg` (exit 0); SIGINT is
/// `InterruptMsg`, and `Program.Run` wraps `ErrInterrupted` in
/// `ErrProgramKilled`, so the root prints
/// "tui: program was killed: program was interrupted" and exits 1.
#[test]
fn keys_and_signals_end_the_loop_like_bubbletea() {
    let h = harness();
    let cases: [(&str, Event, Result<(), String>); 5] = [
        ("q", term_key("q"), Ok(())),
        ("raw ctrl+c", term_key("ctrl+c"), Ok(())),
        ("SIGTERM", Event::Signal(Signal::Terminate), Ok(())),
        (
            "SIGINT",
            Event::Signal(Signal::Interrupt),
            Err("program was killed: program was interrupted".into()),
        ),
        (
            "input failure",
            Event::InputFailed("read /dev/tty: input/output error".into()),
            Err("program was killed: read /dev/tty: input/output error".into()),
        ),
    ];
    for (what, ev, want) in cases {
        let (tx, rx) = mpsc::channel();
        let jobs = pool(&h, &tx);
        let mut m = loading_model(100, 30);
        tx.send(ev).unwrap();
        let got = run_loop(&mut m, &rx, &jobs, &mut LogViews::default(), &opts());
        assert_eq!(got, want, "{what}");
    }
    assert_eq!(INTERRUPTED, "program was killed: program was interrupted");
}

/// The loop leaves on the quit key even when more events wait behind it.
#[test]
fn nothing_after_the_quit_key_reaches_the_model() {
    let h = harness();
    let (tx, rx) = mpsc::channel();
    let jobs = pool(&h, &tx);
    let mut m = loading_model(100, 30);
    tx.send(term_key("q")).unwrap();
    tx.send(Event::Term(TermEvent::Resize(50, 12))).unwrap();
    run_loop(&mut m, &rx, &jobs, &mut LogViews::default(), &opts()).unwrap();
    assert!(m.quitting());
    assert_eq!(m.lay.width, 100);
}

/// A resize reaches the model while stdout is a terminal. Without one the
/// TUI never learns a size, so it is ignored.
#[test]
fn resizes_reach_the_model_only_from_a_terminal() {
    let h = harness();
    for (resize, want) in [(true, (50, 12)), (false, (100, 30))] {
        let (tx, rx) = mpsc::channel();
        let jobs = pool(&h, &tx);
        let mut m = loading_model(100, 30);
        tx.send(Event::Term(TermEvent::Resize(50, 12))).unwrap();
        tx.send(Event::Signal(Signal::Terminate)).unwrap();
        let opts = LoopOpts { resize, ..opts() };
        run_loop(&mut m, &rx, &jobs, &mut LogViews::default(), &opts).unwrap();
        assert_eq!((m.lay.width, m.lay.height), want, "resize={resize}");
    }
}

/// A bracketed paste lands in the filter, as a typed string would.
#[test]
fn a_paste_reaches_the_focused_input() {
    let h = harness();
    let (tx, rx) = mpsc::channel();
    let jobs = pool(&h, &tx);
    let mut m = loading_model(100, 30);
    m.update(Msg::Sessions(SessionEvent {
        sessions: finished_sessions(),
    }));
    tx.send(term_key("/")).unwrap();
    tx.send(Event::Term(TermEvent::Paste("astra".into())))
        .unwrap();
    tx.send(Event::Signal(Signal::Terminate)).unwrap();
    run_loop(&mut m, &rx, &jobs, &mut LogViews::default(), &opts()).unwrap();
    assert_eq!(m.list.filter.value(), "astra");
}

// --- watcher, jobs and timers -------------------------------------------------

/// Forwards the pool's outputs to the loop and ends the loop right after
/// the first log result, so that result is handled before the exit.
fn relay_until_log(from: Receiver<Event>, to: Sender<Event>) -> JoinHandle<()> {
    thread::spawn(move || {
        for ev in from {
            let log = matches!(ev, Event::Job(JobOutput::Msg(Msg::Log(_))));
            if to.send(ev).is_err() {
                return;
            }
            if log {
                let _ = to.send(Event::Signal(Signal::Terminate));
                return;
            }
        }
    })
}

/// A snapshot asks for the preview's log; a worker reads it and the result
/// reaches the model, which then shows the tail.
#[test]
fn a_snapshot_job_runs_on_a_worker_and_its_result_reaches_the_model() {
    let h = harness();
    let (tx, rx) = mpsc::channel();
    let (pool_tx, pool_rx) = mpsc::channel();
    let mut jobs = JobPool::start(h.env.clone(), JOB_WORKERS, pool_tx).unwrap();
    let relay = relay_until_log(pool_rx, tx.clone());
    let _backstop = Backstop::arm(&tx);
    let mut m = loading_model(130, 30);
    tx.send(Event::Msg(Msg::Sessions(SessionEvent {
        sessions: preview_fixture_logs(&h),
    })))
    .unwrap();
    run_loop(&mut m, &rx, &jobs, &mut LogViews::default(), &opts()).unwrap();
    jobs.shutdown();
    relay.join().unwrap();
    let text = rows(&draw(&m, 130, 30)).join("\n");
    assert!(text.contains("LIVE-RUN-OUTPUT"), "{text}");
}

fn log_job(pane: LogPane, seq: u64, s: &Session, width: usize) -> Job {
    Job::Log(LogRequest {
        pane,
        seq,
        key: LogKey::new(s, width, 10),
        known: None,
    })
}

fn stop_job(item: &str) -> Job {
    Job::Stop(StopRequest {
        item_key: item.into(),
        targets: Vec::new(),
    })
}

/// A log read 20 ms slow: a worker that cannot keep up.
fn slow_tail(path: &Path, max: i64) -> io::Result<(Vec<u8>, bool)> {
    thread::sleep(Duration::from_millis(20));
    rival_core::logfmt::read_tail(path, max)
}

fn result_job(seq: u64, s: &Session) -> Job {
    Job::Result(ResultRequest {
        seq,
        target: ResultTarget::of(s),
        known: None,
    })
}

/// Feedback findings 2 and 3: a slow worker and fast navigation (a new key
/// for every request) keep the waiting work bounded, and the newest read of
/// each pane, the newest result parse and the newest prompt load still
/// arrive.
#[test]
fn a_slow_worker_keeps_waiting_work_bounded_and_delivers_the_newest() {
    let h = harness();
    let sessions = preview_fixture_logs(&h);
    let env = JobEnv {
        read_tail: slow_tail,
        ..h.env.clone()
    };
    let (tx, rx) = mpsc::channel();
    let mut jobs = JobPool::start(env, 1, tx).unwrap();
    let n: u64 = 300;
    for seq in 1..=n {
        let s = &sessions[usize::try_from(seq % 3).unwrap()];
        let width = 20 + usize::try_from(seq).unwrap();
        jobs.submit(log_job(LogPane::Preview, seq, s, width))
            .unwrap();
        jobs.submit(log_job(LogPane::Detail, seq, s, width))
            .unwrap();
        jobs.submit(result_job(seq, s)).unwrap();
        jobs.submit(Job::Prompts(PromptsRequest {
            item_key: format!("solo:{seq}"),
            ids: vec![s.id.clone()],
        }))
        .unwrap();
        assert!(jobs.pending() <= MAX_PENDING, "{}", jobs.pending());
    }
    assert!(jobs.pending() <= 4, "{} waiting", jobs.pending());
    let (mut preview, mut detail, mut result, mut prompts, mut outputs) =
        (0, 0, 0, String::new(), 0);
    let last_prompts = format!("solo:{n}");
    while preview != n || detail != n || result != n || prompts != last_prompts {
        match rx
            .recv_timeout(WAIT)
            .expect("the newest work never arrived")
        {
            Event::Job(JobOutput::Msg(Msg::Log(res))) if res.pane == LogPane::Preview => {
                preview = res.seq;
            }
            Event::Job(JobOutput::Msg(Msg::Log(res))) => detail = res.seq,
            Event::Job(JobOutput::Msg(Msg::Result(res))) => result = res.seq,
            Event::Job(JobOutput::Msg(Msg::Prompts(res))) => prompts = res.item_key,
            other => panic!("unexpected {other:?}"),
        }
        outputs += 1;
    }
    assert!(outputs < 40, "{outputs} jobs ran for {} requests", 4 * n);
    jobs.shutdown();
}

static GATE_OPEN: AtomicBool = AtomicBool::new(false);
static GATE_ENTERED: AtomicBool = AtomicBool::new(false);

/// A log read that holds its worker until the gate opens.
fn gated_tail(path: &Path, max: i64) -> io::Result<(Vec<u8>, bool)> {
    GATE_ENTERED.store(true, Ordering::Release);
    while !GATE_OPEN.load(Ordering::Acquire) {
        thread::sleep(Duration::from_millis(1));
    }
    rival_core::logfmt::read_tail(path, max)
}

/// Opens the gate on drop, so a failed assertion never leaves the worker
/// (and the pool's join) stuck.
struct OpenGate;

impl Drop for OpenGate {
    fn drop(&mut self) {
        GATE_OPEN.store(true, Ordering::Release);
    }
}

/// Stops and log opens queue up to a bound; an equal one merges with the
/// waiting twin, and one past the bound comes back to the caller. Each
/// queued one runs once the worker is free.
#[test]
fn a_full_action_queue_refuses_and_merges_twins() {
    let h = harness();
    let session = preview_fixture_logs(&h).remove(0);
    let env = JobEnv {
        read_tail: gated_tail,
        ..h.env.clone()
    };
    let (tx, rx) = mpsc::channel();
    let mut jobs = JobPool::start(env, 1, tx).unwrap();
    // Declared after the pool, so on a failed assertion it drops (and
    // opens) first, before the pool joins its gated worker.
    let gate = OpenGate;
    jobs.submit(log_job(LogPane::Detail, 1, &session, 40))
        .unwrap();
    let start = Instant::now();
    while !GATE_ENTERED.load(Ordering::Acquire) {
        assert!(start.elapsed() < WAIT, "the worker never took the read");
        thread::sleep(Duration::from_millis(1));
    }
    for i in 0..MAX_PENDING_ACTIONS {
        jobs.submit(stop_job(&format!("solo:{i}"))).unwrap();
    }
    assert_eq!(jobs.pending(), MAX_PENDING_ACTIONS);
    jobs.submit(stop_job("solo:0")).expect("a twin merges");
    assert_eq!(jobs.pending(), MAX_PENDING_ACTIONS);
    assert_eq!(jobs.submit(stop_job("solo:extra")), Err(Refused));
    drop(gate);
    let mut stopped = Vec::new();
    let mut logs = 0;
    while stopped.len() < MAX_PENDING_ACTIONS || logs < 1 {
        match rx.recv_timeout(WAIT).expect("queued work never ran") {
            Event::Job(JobOutput::Msg(Msg::Stopped(res))) => stopped.push(res.item_key),
            Event::Job(JobOutput::Msg(Msg::Log(_))) => logs += 1,
            other => panic!("unexpected {other:?}"),
        }
    }
    jobs.shutdown();
    let want: Vec<String> = (0..MAX_PENDING_ACTIONS)
        .map(|i| format!("solo:{i}"))
        .collect();
    assert_eq!(stopped, want, "in order, the twin merged");
    assert!(rx.try_recv().is_err(), "the refused stop ran");
}

/// A refused "o" reaches the user as the detail screen's notice, never as
/// silence.
#[test]
fn a_refused_job_reaches_the_user_as_a_notice() {
    let h = harness();
    let (tx, _rx) = mpsc::channel();
    // No worker: nothing drains the queue.
    let jobs = JobPool::start(h.env.clone(), 0, tx).unwrap();
    for i in 0..MAX_PENDING_ACTIONS {
        jobs.submit(stop_job(&format!("solo:{i}"))).unwrap();
    }
    let mut m = testkit::open_detail(&h.env, preview_fixture_logs(&h), 100, 30);
    let mut timers = Timers::default();
    let end = step(&mut m, testkit::key("o"), &mut timers, &jobs, Instant::now);
    assert!(end.is_none());
    assert_eq!(m.detail.notice, BUSY_NOTICE);
    let text = rows(&draw(&m, 100, 30)).join("\n");
    assert!(text.contains(BUSY_NOTICE), "{text}");
}

/// After quit no screen needs a read or a prompt load, so waiting ones are
/// skipped; a confirmed stop and an "o" still run.
#[test]
fn closing_workers_skip_reads_but_run_stops_and_opens() {
    let h = harness();
    let session = preview_fixture_logs(&h).remove(0);
    let shared = Shared {
        pending: Mutex::new(Pending::default()),
        wake: Condvar::new(),
    };
    {
        let mut pending = shared.lock();
        pending
            .push(log_job(LogPane::Detail, 1, &session, 40))
            .unwrap();
        pending.push(result_job(1, &session)).unwrap();
        pending
            .push(Job::Prompts(PromptsRequest {
                item_key: "solo:x".into(),
                ids: vec![session.id.clone()],
            }))
            .unwrap();
        pending.push(stop_job("solo:x")).unwrap();
        pending
            .push(Job::OpenLog(crate::tui::jobs::OpenLogRequest {
                sessions: vec![Arc::clone(&session)],
                group: false,
            }))
            .unwrap();
        pending.closing = true;
    }
    let (out, events) = mpsc::channel();
    // On this thread, so the testkit fakes apply.
    work(&h.env, &shared, &out);
    let got: Vec<&str> = events
        .try_iter()
        .map(|ev| match ev {
            Event::Job(JobOutput::Msg(Msg::Stopped(_))) => "stopped",
            Event::Job(JobOutput::Opened(_)) => "opened",
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert_eq!(got, ["stopped", "opened"]);
    assert_eq!(testkit::launched().len(), 1);
    assert_eq!(
        shared.lock().len(),
        3,
        "the read, the parse and the prompt load stay unrun"
    );
}

/// The Result tab's parses share one coalesced slot: a newer parse
/// replaces the waiting one, and shutdown drops it unrun.
#[test]
fn result_parses_coalesce_and_shutdown_drops_them() {
    let h = harness();
    let sessions = preview_fixture_logs(&h);
    let (tx, rx) = mpsc::channel();
    // No worker: nothing drains the queue.
    let mut jobs = JobPool::start(h.env.clone(), 0, tx).unwrap();
    for (seq, s) in (1..=5).zip(sessions.iter().cycle()) {
        jobs.submit(result_job(seq, s)).unwrap();
        assert_eq!(jobs.pending(), 1);
    }
    jobs.submit(log_job(LogPane::Detail, 1, &sessions[0], 40))
        .unwrap();
    assert_eq!(jobs.pending(), 2, "the parse has its own slot");
    jobs.shutdown();
    assert_eq!(jobs.pending(), 0);
    assert!(rx.try_recv().is_err(), "a dropped parse ran");
}

/// The model's tick and spinner chains stop once nothing is live, but the
/// runtime still sweeps: an exited launcher is reaped and an expired copy
/// removed.
#[cfg(unix)]
#[test]
fn opened_copies_are_swept_while_nothing_is_live() {
    fn late() -> Instant {
        Instant::now() + LOG_VIEW_TTL + Duration::from_secs(1)
    }
    let h = harness();
    let (tx, rx) = mpsc::channel();
    let jobs = pool(&h, &tx);
    let mut m = loading_model(100, 30);
    m.update(Msg::Sessions(SessionEvent {
        sessions: finished_sessions(),
    }));
    assert!(!m.list.any_live);
    let copy = h.env.temp_dir.join("rival-log-old.txt");
    fs::write(&copy, "old\n").unwrap();
    // Exits at once, but nobody has waited for it yet.
    let launcher = testkit::exiting_launcher();
    tx.send(Event::Job(JobOutput::Opened(OpenedLog::new(
        copy.clone(),
        Some(launcher),
        Instant::now(),
    ))))
    .unwrap();
    let ender = {
        let tx = tx.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(300));
            let _ = tx.send(Event::Signal(Signal::Terminate));
        })
    };
    let mut views = LogViews::default();
    let opts = LoopOpts {
        sweep_every: Duration::from_millis(10),
        clock: late,
        ..opts()
    };
    run_loop(&mut m, &rx, &jobs, &mut views, &opts).unwrap();
    ender.join().unwrap();
    assert!(!copy.exists(), "the sweep never expired the copy");
    assert!(views.is_empty(), "the exited launcher was never reaped");
}

/// The injected opener: records the copy and starts a task-owned `sh` that
/// waits on its stdin, a launcher that has not read the path yet. No viewer
/// is started.
#[cfg(unix)]
static BUSY: Mutex<Vec<(PathBuf, testkit::HelperGuard)>> = Mutex::new(Vec::new());

#[cfg(unix)]
fn busy_launch(path: &Path) -> io::Result<Option<Child>> {
    let (child, guard) = testkit::running_launcher();
    BUSY.lock().unwrap().push((path.to_path_buf(), guard));
    Ok(Some(child))
}

#[cfg(unix)]
static DONE: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// The injected opener whose launcher has already exited.
#[cfg(unix)]
fn done_launch(path: &Path) -> io::Result<Option<Child>> {
    DONE.lock().unwrap().push(path.to_path_buf());
    Ok(Some(testkit::finished_launcher()))
}

/// "o" then an immediate "q": the open job may still be queued when the
/// loop ends. Shutdown runs it and adopts its copy. The fresh copy stays
/// whether the launcher still runs (it is never killed) or is already done
/// (finding 10: plain `open` returns before the app reads the path).
#[cfg(unix)]
#[test]
fn open_then_quit_follows_the_launcher_policy() {
    for busy in [true, false] {
        let h = harness();
        let launch = if busy { busy_launch } else { done_launch };
        let env = JobEnv {
            launch,
            ..h.env.clone()
        };
        let (tx, rx) = mpsc::channel();
        let mut jobs = JobPool::start(env, JOB_WORKERS, tx.clone()).unwrap();
        let mut m = testkit::open_detail(&h.env, preview_fixture_logs(&h), 100, 30);
        tx.send(term_key("o")).unwrap();
        tx.send(term_key("q")).unwrap();
        let mut views = LogViews::default();
        run_loop(&mut m, &rx, &jobs, &mut views, &opts()).unwrap();
        finish_jobs(&mut jobs, &rx, &mut views);
        assert!(views.is_empty());
        if busy {
            let held = std::mem::take(&mut *BUSY.lock().unwrap());
            assert_eq!(held.len(), 1, "one launch");
            let copy = &held[0].0;
            assert!(
                copy.exists(),
                "quit removed a copy its launcher may still need"
            );
            assert_eq!(fs::read_to_string(copy).unwrap(), "LIVE-RUN-OUTPUT\n");
            // Dropping the guard closes the helper's stdin; the guard fails
            // the test if the helper had to be killed, so this also proves
            // nothing above killed the launcher.
            drop(held);
        } else {
            let done = std::mem::take(&mut *DONE.lock().unwrap());
            assert_eq!(done.len(), 1, "one launch");
            assert_eq!(
                fs::read_to_string(&done[0]).unwrap(),
                "LIVE-RUN-OUTPUT\n",
                "quit removed a fresh copy the viewer may not have read"
            );
        }
    }
}

static SHELL_OPENED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// The Windows opener's shape (`ShellExecuteW`): the shell took the path and
/// there is no launcher process to reap.
fn shell_launch(path: &Path) -> io::Result<Option<Child>> {
    SHELL_OPENED.lock().unwrap().push(path.to_path_buf());
    Ok(None)
}

/// "o" then "q" with a launcher-less opener: the copy is adopted and stays
/// for the viewer the shell started, which nothing waits for or ends.
#[test]
fn open_then_quit_keeps_the_copy_without_a_launcher() {
    let h = harness();
    let env = JobEnv {
        launch: shell_launch,
        ..h.env.clone()
    };
    let (tx, rx) = mpsc::channel();
    let mut jobs = JobPool::start(env, JOB_WORKERS, tx.clone()).unwrap();
    let mut m = testkit::open_detail(&h.env, preview_fixture_logs(&h), 100, 30);
    tx.send(term_key("o")).unwrap();
    tx.send(term_key("q")).unwrap();
    let mut views = LogViews::default();
    run_loop(&mut m, &rx, &jobs, &mut views, &opts()).unwrap();
    finish_jobs(&mut jobs, &rx, &mut views);
    assert!(views.is_empty());
    let opened = std::mem::take(&mut *SHELL_OPENED.lock().unwrap());
    assert_eq!(opened.len(), 1, "one launch");
    assert_eq!(
        fs::read_to_string(&opened[0]).unwrap(),
        "LIVE-RUN-OUTPUT\n",
        "quit removed a fresh copy the viewer may not have read"
    );
    let _ = fs::remove_file(&opened[0]);
}

/// Watch: the first snapshot reaches the loop's channel; stop joins every
/// thread, so no sender is left behind.
#[test]
fn watch_delivers_snapshots_and_stop_joins_every_thread() {
    let tmp = tempfile::tempdir().unwrap();
    let (tx, rx) = mpsc::channel();
    let mut watch = Watch::start(Paths::from_home(tmp.path()), tx).unwrap();
    let first = loop {
        match rx.recv_timeout(WAIT).expect("no snapshot") {
            Event::Msg(Msg::Sessions(ev)) => break ev,
            Event::Msg(Msg::Progress(_)) => continue,
            other => panic!("unexpected {other:?}"),
        }
    };
    assert!(first.sessions.is_empty());
    let start = Instant::now();
    watch.stop();
    assert!(start.elapsed() < WAIT);
    // The test gave its only sender away: once every thread is gone, the
    // channel reports the disconnect.
    loop {
        match rx.recv_timeout(WAIT) {
            Ok(_) => continue,
            Err(e) => {
                assert_eq!(e, RecvTimeoutError::Disconnected, "a thread kept a sender");
                break;
            }
        }
    }
}

/// A watcher that cannot start reports its error, and the
/// watch still stops cleanly.
#[test]
fn a_watch_that_cannot_start_reports_its_error() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = Paths::from_home(tmp.path());
    // A file where the .rival directory must go.
    fs::write(&paths.root, "x").unwrap();
    let (tx, rx) = mpsc::channel();
    let mut watch = Watch::start(paths.clone(), tx).unwrap();
    match rx.recv_timeout(WAIT).expect("no error") {
        Event::Msg(Msg::Error(text)) => assert!(
            text.starts_with(&format!("mkdir {}: ", paths.sessions_dir().display())),
            "{text}"
        ),
        other => panic!("unexpected {other:?}"),
    }
    watch.stop();
}

/// Dropping the watch (an unwind, say) stops it as `stop` does.
#[test]
fn dropping_the_watch_stops_it() {
    let tmp = tempfile::tempdir().unwrap();
    let (tx, rx) = mpsc::channel();
    let watch = Watch::start(Paths::from_home(tmp.path()), tx).unwrap();
    drop(watch);
    let mut left = 0;
    while let Ok(_ev) = rx.recv_timeout(WAIT) {
        left += 1;
        assert!(left < 10, "the watch kept sending after drop");
    }
}

// --- input ------------------------------------------------------------------

fn idle_poll(timeout: Duration) -> io::Result<bool> {
    thread::sleep(timeout);
    Ok(false)
}

fn never_read() -> io::Result<TermEvent> {
    unreachable!("poll never reports input")
}

fn failing_poll(_: Duration) -> io::Result<bool> {
    Err(io::Error::other("tty gone"))
}

fn ready_poll(_: Duration) -> io::Result<bool> {
    Ok(true)
}

fn key_q() -> io::Result<TermEvent> {
    thread::sleep(Duration::from_millis(1));
    Ok(TermEvent::Key(press("q")))
}

#[test]
fn input_stop_joins_an_idle_reader() {
    let (tx, rx) = mpsc::channel();
    let mut input = Input::start(
        InputSource {
            poll: idle_poll,
            read: never_read,
        },
        tx,
    )
    .unwrap();
    thread::sleep(Duration::from_millis(20));
    let start = Instant::now();
    input.stop();
    assert!(start.elapsed() < Duration::from_secs(2));
    assert_eq!(rx.try_recv().err(), Some(mpsc::TryRecvError::Disconnected));
}

#[test]
fn input_forwards_events_and_reports_failure() {
    let (tx, rx) = mpsc::channel();
    let mut input = Input::start(
        InputSource {
            poll: ready_poll,
            read: key_q,
        },
        tx,
    )
    .unwrap();
    match rx.recv_timeout(WAIT).unwrap() {
        Event::Term(TermEvent::Key(k)) => assert_eq!(k, press("q")),
        other => panic!("unexpected {other:?}"),
    }
    input.stop();

    let (tx, rx) = mpsc::channel();
    let _input = Input::start(
        InputSource {
            poll: failing_poll,
            read: never_read,
        },
        tx,
    )
    .unwrap();
    match rx.recv_timeout(WAIT).unwrap() {
        Event::InputFailed(text) => assert_eq!(text, "tty gone"),
        other => panic!("unexpected {other:?}"),
    }
}

// --- the screen -------------------------------------------------------------

#[derive(Clone, Default)]
struct FakeScreen {
    log: Arc<Mutex<Vec<&'static str>>>,
    fail: bool,
}

impl Screen for FakeScreen {
    fn enter(&mut self) -> Result<(), String> {
        self.log.lock().unwrap().push("enter");
        if self.fail {
            return Err("error entering raw mode: operation not supported by device".into());
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.log.lock().unwrap().push("leave");
    }
}

#[test]
fn the_screen_is_restored_on_return_error_and_unwind() {
    // A normal return.
    let screen = FakeScreen::default();
    let guard = ScreenGuard::enter(screen.clone()).unwrap();
    drop(guard);
    assert_eq!(*screen.log.lock().unwrap(), ["enter", "leave"]);

    // An error after entering.
    let screen = FakeScreen::default();
    let failing = || -> Result<(), String> {
        let _guard = ScreenGuard::enter(screen.clone())?;
        Err("boom".into())
    };
    assert_eq!(failing(), Err("boom".to_string()));
    assert_eq!(*screen.log.lock().unwrap(), ["enter", "leave"]);

    // A panic while the screen is held.
    let screen = FakeScreen::default();
    let held = screen.clone();
    let unwound = std::panic::catch_unwind(move || {
        let _guard = ScreenGuard::enter(held).unwrap();
        panic!("model bug");
    });
    assert!(unwound.is_err());
    assert_eq!(*screen.log.lock().unwrap(), ["enter", "leave"]);

    // A failed enter undoes whatever it did and returns the error.
    let screen = FakeScreen {
        fail: true,
        ..FakeScreen::default()
    };
    let err = ScreenGuard::enter(screen.clone()).err().unwrap();
    assert_eq!(
        err,
        "error entering raw mode: operation not supported by device"
    );
    assert_eq!(*screen.log.lock().unwrap(), ["enter", "leave"]);
}

#[test]
fn timers_fire_in_deadline_order_once() {
    let mut timers = Timers::default();
    let t0 = Instant::now();
    timers.add(t0 + TICK_INTERVAL, Msg::Tick);
    timers.add(t0 + SPIN_INTERVAL, Msg::SpinTick);
    assert_eq!(timers.next(), Some(t0 + SPIN_INTERVAL));
    assert!(timers.take_due(t0).is_empty());
    assert_eq!(timers.take_due(t0 + SPIN_INTERVAL), [Msg::SpinTick]);
    assert_eq!(timers.take_due(t0 + TICK_INTERVAL), [Msg::Tick]);
    assert_eq!(timers.next(), None);
}

// --- startup failures and cleanup order --------------------------------------

thread_local! {
    static SPAWNS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static FAIL_AT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The real spawner, except that spawn number `FAIL_AT` fails (and drops
/// its body, as `thread::Builder::spawn` does). No host limit is touched.
fn failing_spawn(name: &str, body: Body) -> io::Result<JoinHandle<()>> {
    let n = SPAWNS.get() + 1;
    SPAWNS.set(n);
    if n == FAIL_AT.get() {
        drop(body);
        return Err(io::Error::other(format!("no thread for {name}")));
    }
    spawn_named(name, body)
}

/// Feedback finding 4: whichever spawn fails, the threads already started
/// see their input close and are joined; the start returns in bounded time
/// and leaves no sender behind.
#[test]
fn a_failed_watch_spawn_closes_and_joins_every_started_thread() {
    for fail_at in 1..=3 {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(tmp.path());
        let (out, events) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        let starter = thread::spawn(move || {
            SPAWNS.set(0);
            FAIL_AT.set(fail_at);
            let result = Watch::start_with(paths, out, failing_spawn).map(drop);
            let _ = done_tx.send(result.map_err(|e| e.to_string()));
        });
        let result = done
            .recv_timeout(WAIT)
            .unwrap_or_else(|_| panic!("spawn {fail_at}: the start never returned"));
        starter.join().unwrap();
        let names = ["sessions", "progress", "watch-start"];
        assert_eq!(
            result,
            Err(format!("no thread for {}", names[fail_at - 1])),
            "spawn {fail_at}"
        );
        assert_eq!(
            events.recv_timeout(WAIT).err(),
            Some(RecvTimeoutError::Disconnected),
            "spawn {fail_at}: a thread kept a sender"
        );
    }
}

static ORDER: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
static SLOW_STARTED: AtomicBool = AtomicBool::new(false);

/// A log read that takes 200 ms and records when it ends.
fn recorded_slow_tail(path: &Path, max: i64) -> io::Result<(Vec<u8>, bool)> {
    SLOW_STARTED.store(true, Ordering::Release);
    thread::sleep(Duration::from_millis(200));
    ORDER.lock().unwrap().push("job finished");
    rival_core::logfmt::read_tail(path, max)
}

struct OrderScreen;

impl Screen for OrderScreen {
    fn enter(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn leave(&mut self) {
        ORDER.lock().unwrap().push("screen left");
    }
}

/// Feedback finding 5: on a normal close, an error return and an unwind,
/// the screen is restored before the owners wait for a running job.
#[test]
fn the_screen_is_restored_before_blocking_joins_on_every_path() {
    let h = harness();
    let session = preview_fixture_logs(&h).remove(0);
    for path in ["close", "error", "unwind"] {
        ORDER.lock().unwrap().clear();
        SLOW_STARTED.store(false, Ordering::Release);
        let (tx, rx) = mpsc::channel();
        let env = JobEnv {
            read_tail: recorded_slow_tail,
            ..h.env.clone()
        };
        let mut owners: Owners<OrderScreen> = Owners::new(rx);
        owners.screen = Some(ScreenGuard::enter(OrderScreen).unwrap());
        let jobs = owners
            .jobs
            .insert(JobPool::start(env, 1, tx.clone()).unwrap());
        jobs.submit(log_job(LogPane::Detail, 1, &session, 40))
            .unwrap();
        let start = Instant::now();
        while !SLOW_STARTED.load(Ordering::Acquire) {
            assert!(start.elapsed() < WAIT, "the job never started");
            thread::sleep(Duration::from_millis(1));
        }
        owners.input = Some(
            Input::start(
                InputSource {
                    poll: idle_poll,
                    read: never_read,
                },
                tx,
            )
            .unwrap(),
        );
        match path {
            "close" => owners.close(),
            "error" => {
                let fail = move || -> Result<(), String> {
                    let _owners = owners;
                    Err("start terminal input: no thread".into())
                };
                assert!(fail().is_err());
            }
            _ => {
                let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                    let _owners = owners;
                    panic!("model bug");
                }));
                assert!(unwound.is_err());
            }
        }
        assert_eq!(
            *ORDER.lock().unwrap(),
            ["screen left", "job finished"],
            "{path}"
        );
    }
}
