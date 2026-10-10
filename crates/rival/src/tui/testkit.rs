//! Test helpers: key presses, pinned-clock models, TestBackend frames and a
//! synchronous job runner with fake processes.

use std::cell::{Cell as StdCell, RefCell};
use std::collections::VecDeque;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, FixedOffset, NaiveDateTime, TimeDelta};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::{Buffer, Cell};
use ratatui::style::{Color, Modifier, Style};

use super::styles::STYLES;
use unicode_width::UnicodeWidthStr;

use rival_core::logfmt;
use rival_core::paths::Paths;
use rival_core::session::Session;
use rival_core::sessionview::SessionEvent;

use super::jobs::{JobEnv, JobOutput, LogViews};
use super::kill::ProcessOps;
use super::model::{Cmd, DisplayItem, Model, Msg};

// --- jobs and fake processes ------------------------------------------------

thread_local! {
    static ALIVE: StdCell<bool> = const { StdCell::new(false) };
    static SIGNAL_FAILS: StdCell<bool> = const { StdCell::new(false) };
    static SIGNALS: RefCell<Vec<i64>> = const { RefCell::new(Vec::new()) };
    static TERMINATED: RefCell<Vec<i64>> = const { RefCell::new(Vec::new()) };
    static READS: StdCell<usize> = const { StdCell::new(0) };
    static LAUNCHED: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
    static LAUNCH_FAILS: StdCell<bool> = const { StdCell::new(false) };
}

/// The files the fake viewer launcher was asked to open on this thread.
pub fn launched() -> Vec<PathBuf> {
    LAUNCHED.with_borrow(Clone::clone)
}

/// Makes the fake viewer launcher fail, as for a missing opener.
pub fn set_launch_fails(fails: bool) {
    LAUNCH_FAILS.set(fails);
}

/// The fake viewer launcher: records the path and starts nothing.
fn fake_launch(path: &Path) -> io::Result<Option<std::process::Child>> {
    LAUNCHED.with_borrow_mut(|l| l.push(path.to_path_buf()));
    if LAUNCH_FAILS.get() {
        return Err(io::Error::from(io::ErrorKind::NotFound));
    }
    Ok(None)
}

/// What the fake `alive` answers for every PID on this test's thread.
pub fn set_alive(alive: bool) {
    ALIVE.set(alive);
}

/// Makes the fake signal fail with ESRCH, as for a process that just died.
pub fn set_signal_fails(fails: bool) {
    SIGNAL_FAILS.set(fails);
}

/// The PIDs the fake signal recorded on this test's thread.
pub fn signals() -> Vec<i64> {
    SIGNALS.with_borrow(Clone::clone)
}

/// How many log reads (not stats) ran on this test's thread.
pub fn reads() -> usize {
    READS.get()
}

/// The fake `alive`: whatever [`set_alive`] said. It never looks at a real
/// process.
pub fn fake_alive(_pid: i64, _start: i64) -> bool {
    ALIVE.get()
}

/// The fake running state (the Windows stop's death wait). Only the
/// Windows stop reads it. A successful fake terminate ends the stopped owner
/// and, as its kill-on-close Job does, every fake process: the fixtures'
/// providers all run under the owner that was stopped. Identity
/// ([`fake_alive`]) stays readable afterwards, as a Windows process's does
/// while any handle holds it.
pub fn fake_running(pid: i64, start: i64) -> bool {
    fake_alive(pid, start) && TERMINATED.with_borrow(Vec::is_empty)
}

/// The fake signal: records the PID and never sends anything. A successful
/// one ends the fake processes for [`fake_running`] only; `alive`
/// (identity) keeps answering what [`set_alive`] said, so every other
/// target of the same stop still verifies.
fn fake_terminate(pid: i64, _start: i64) -> io::Result<()> {
    SIGNALS.with_borrow_mut(|s| s.push(pid));
    if SIGNAL_FAILS.get() {
        return Err(io::Error::from(io::ErrorKind::NotFound));
    }
    TERMINATED.with_borrow_mut(|t| t.push(pid));
    Ok(())
}

/// A test-owned stand-in for a viewer launcher that has not exited: `sh`
/// blocked on its stdin pipe. The `Child` goes to the exit policy, which
/// drops it without waiting; the guard keeps the pipe, so the helper runs
/// until the guard drops. Bind the guard before the `LogViews` so it drops
/// last.
#[cfg(unix)]
pub fn running_launcher() -> (std::process::Child, HelperGuard) {
    let mut cmd = super::jobs::quiet_command("/bin/sh");
    cmd.args(["-c", "read x"])
        .stdin(std::process::Stdio::piped());
    let mut child = rival_core::executor::process::spawn(&mut cmd).unwrap();
    let guard = HelperGuard {
        pid: libc::pid_t::try_from(child.id()).unwrap(),
        stdin: child.stdin.take(),
    };
    (child, guard)
}

/// Reaps exactly one helper on drop, also when the test panics: it closes
/// the pipe and waits up to `HELPER_DEADLINE`, then kills and reaps that
/// PID. The PID cannot be reused before this reap, so the kill is scoped.
#[cfg(unix)]
#[derive(Debug)]
pub struct HelperGuard {
    pid: libc::pid_t,
    stdin: Option<std::process::ChildStdin>,
}

#[cfg(unix)]
const HELPER_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

#[cfg(unix)]
impl HelperGuard {
    /// One `waitpid` call; `true` once the PID is reaped or no longer ours.
    fn try_reap(&self, flags: libc::c_int) -> bool {
        loop {
            let mut status = 0;
            // SAFETY: waitpid on this test's own child PID with a valid
            // status pointer.
            let r = unsafe { libc::waitpid(self.pid, &mut status, flags) };
            if r == 0 {
                return false;
            }
            if r == self.pid || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                return true;
            }
        }
    }
}

#[cfg(unix)]
impl Drop for HelperGuard {
    fn drop(&mut self) {
        use std::time::{Duration, Instant};
        drop(self.stdin.take());
        let panicking = std::thread::panicking();
        let deadline = Instant::now()
            + if panicking {
                Duration::ZERO
            } else {
                HELPER_DEADLINE
            };
        loop {
            if self.try_reap(libc::WNOHANG) {
                return;
            }
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        // SAFETY: the PID is still an unreaped child of this test, so it
        // names the helper and nothing else.
        unsafe { libc::kill(self.pid, libc::SIGKILL) };
        self.try_reap(0);
        assert!(
            panicking,
            "helper {} ignored its closed stdin and was killed",
            self.pid
        );
    }
}

/// A launcher that already exited (and was waited for).
#[cfg(unix)]
pub fn finished_launcher() -> std::process::Child {
    let mut child = exiting_launcher();
    child.wait().unwrap();
    child
}

/// A launcher that exits at once; nobody has waited for it yet.
#[cfg(unix)]
pub fn exiting_launcher() -> std::process::Child {
    let mut cmd = super::jobs::quiet_command("/bin/sh");
    cmd.args(["-c", "exit 0"]);
    rival_core::executor::process::spawn(&mut cmd).unwrap()
}

/// `logfmt::read_tail`, counted.
fn counting_read_tail(path: &Path, max_bytes: i64) -> io::Result<(Vec<u8>, bool)> {
    READS.set(READS.get() + 1);
    logfmt::read_tail(path, max_bytes)
}

pub const FAKE_PROCS: ProcessOps = ProcessOps {
    alive: fake_alive,
    running: fake_running,
    terminate: fake_terminate,
};

/// What the fake proxy lists: two Claude accounts and Codex without a
/// prefix.
pub const FAKE_PROXY_IDS: [&str; 6] = [
    "emcd_/claude-opus-5-5",
    "emcd_/claude-fable-5-1",
    "emcd2_/claude-opus-5-5",
    "emcd2_/claude-fable-5-1",
    "gpt-6-astra",
    "gpt-6.1-sol",
];

/// The fake `/v1/models`: [`FAKE_PROXY_IDS`] for any URL whose key is
/// `test-key-a91f`, a 401 for any other key. It never opens a socket.
pub fn fake_models(
    route: &rival_core::config::Route,
) -> Result<Vec<String>, rival_core::proxy::ModelsError> {
    if route.key() != "test-key-a91f" {
        return Err(rival_core::proxy::ModelsError::Rejected(401));
    }
    Ok(FAKE_PROXY_IDS.iter().map(|s| s.to_string()).collect())
}

/// The fake check: every model passes in 1.5 s with `ok`, through the
/// route the config picks, and each row is reported. No adapter runs.
pub fn fake_check(
    _ctx: &rival_core::cancel::Context,
    cfg: &rival_core::config::Config,
    targets: &[crate::check::Target],
    on_row: &(dyn Fn(&crate::check::CheckRow) + Sync),
) -> crate::check::Report {
    let rows: Vec<crate::check::CheckRow> = targets
        .iter()
        .map(|t| {
            let route = t.provider().and_then(|p| cfg.proxy_route(p).ok().flatten());
            let row = crate::check::CheckRow {
                name: t.name.to_string(),
                runtime: t.runtime.to_string(),
                route: if route.is_some() { "proxy" } else { "direct" }.to_string(),
                wire_model: route.map_or_else(
                    || t.model.to_string(),
                    |r| rival_core::proxy::wire_id(&r.prefix, t.model),
                ),
                ok: true,
                ms: 1500,
                reply: "ok".to_string(),
                called: true,
                ..Default::default()
            };
            on_row(&row);
            row
        })
        .collect();
    crate::check::Report {
        proxy: crate::check::ProxyLine::Off,
        rows,
    }
}

/// The proxy key the config fixtures store; [`fake_models`] accepts it.
pub const TEST_KEY: &str = "test-key-a91f";

/// A config with both routes on, the URL, a Claude prefix and a role.
pub const PROXY_YAML: &str = "proxy:
  url: http://127.0.0.1:8317
  claude:
    enabled: true
    model_prefix: emcd2_
  codex:
    enabled: true
roles:
  reviewer: Review like a staff engineer.
";

/// Writes `yaml` (when given) and `key` (mode 0600, when given) into the
/// harness home and loads the config window's seed from it, as the runtime
/// does. Every runtime counts as installed but grok, so frames do not
/// depend on the host `PATH`.
pub fn config_seed(
    h: &Harness,
    yaml: Option<&str>,
    key: Option<&str>,
) -> super::config_check::ConfigSeed {
    let paths = h.paths();
    std::fs::create_dir_all(&paths.root).unwrap();
    if let Some(yaml) = yaml {
        std::fs::write(paths.config_file(), yaml).unwrap();
    }
    if let Some(key) = key {
        let path = paths.proxy_key_file();
        std::fs::write(&path, format!("{key}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
    let env = [(
        rival_core::paths::HOME_VAR.to_string(),
        h.home.path().to_string_lossy().into_owned(),
    )]
    .into_iter()
    .collect();
    let cfg = rival_core::config::Config::new(paths.clone(), env, None);
    let mut seed = super::config_check::ConfigSeed::load(&cfg);
    seed.installed = super::config_form::MODELS
        .iter()
        .map(|m| m.name != "grok")
        .collect();
    seed
}

/// `rival config` on [`config_seed`] at `width`×`height`, with its first
/// probe answered by [`fake_models`].
pub fn config_model(
    h: &Harness,
    yaml: Option<&str>,
    key: Option<&str>,
    width: u16,
    height: u16,
) -> Model {
    let mut m = Model::new("3.34.0");
    m.clock = fixed_now;
    m.list.zone = fixed_zone;
    m.update(Msg::Resize { width, height });
    let seed = config_seed(h, yaml, key);
    let cmds = m.open_config_only(seed);
    for cmd in cmds {
        if let Cmd::Job(job) = cmd
            && let JobOutput::Msg(msg) = h.env.run(job)
        {
            drive(&mut m, &h.env, [msg]);
        }
    }
    m
}

/// A temp home and the job environment pointing at it. The fakes on this
/// thread start from "not alive, signals succeed, nothing recorded".
pub struct Harness {
    pub home: tempfile::TempDir,
    pub env: JobEnv,
}

impl Harness {
    pub fn paths(&self) -> &Paths {
        &self.env.paths
    }

    /// Writes `content` to a log file in the temp home and returns its path.
    pub fn log(&self, name: &str, content: &str) -> String {
        let dir = self.home.path().join("logs");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        path.to_string_lossy().into_owned()
    }
}

pub fn harness() -> Harness {
    ALIVE.set(false);
    SIGNAL_FAILS.set(false);
    SIGNALS.with_borrow_mut(Vec::clear);
    TERMINATED.with_borrow_mut(Vec::clear);
    READS.set(0);
    LAUNCHED.with_borrow_mut(Vec::clear);
    LAUNCH_FAILS.set(false);
    let home = tempfile::tempdir().unwrap();
    let tmp = home.path().join("tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    let env = JobEnv {
        paths: Paths::from_home(home.path()),
        procs: FAKE_PROCS,
        read_tail: counting_read_tail,
        launch: fake_launch,
        temp_dir: tmp,
        models: fake_models,
        check: fake_check,
    };
    Harness { home, env }
}

/// Feeds `msgs` to `m` and runs every job it asks for to completion on this
/// thread, feeding the results back, as the runtime's workers would. Opened
/// log copies go to `views` when given.
pub fn drive_with(
    m: &mut Model,
    env: &JobEnv,
    mut views: Option<&mut LogViews>,
    msgs: impl IntoIterator<Item = Msg>,
) {
    for msg in msgs {
        let mut queue = VecDeque::from([msg]);
        while let Some(msg) = queue.pop_front() {
            for cmd in m.update(msg) {
                let Cmd::Job(job) = cmd else {
                    continue;
                };
                let streamed = std::sync::Mutex::new(Vec::new());
                let out = env.run_with(job, &|msg| streamed.lock().unwrap().push(msg));
                queue.extend(streamed.into_inner().unwrap());
                match out {
                    JobOutput::Msg(res) => queue.push_back(res),
                    JobOutput::Opened(opened) => {
                        if let Some(views) = views.as_deref_mut() {
                            views.adopt(opened);
                        }
                    }
                    JobOutput::Nothing => {}
                }
            }
        }
    }
}

/// [`drive_with`] without a log-view owner.
pub fn drive(m: &mut Model, env: &JobEnv, msgs: impl IntoIterator<Item = Msg>) {
    drive_with(m, env, None, msgs);
}

/// A loaded model at `width`×`height` with its jobs run, fake processes
/// and the pinned clock.
pub fn job_model(env: &JobEnv, sessions: Vec<Arc<Session>>, width: u16, height: u16) -> Model {
    let mut m = loading_model(width, height);
    m.alive = fake_alive;
    drive(&mut m, env, [Msg::Sessions(SessionEvent { sessions })]);
    m
}

/// [`job_model`], then the real enter key.
pub fn open_detail(env: &JobEnv, sessions: Vec<Arc<Session>>, width: u16, height: u16) -> Model {
    let mut m = job_model(env, sessions, width, height);
    drive(&mut m, env, [key("enter")]);
    assert!(m.in_detail(), "enter did not open the detail screen");
    m
}

/// The pinned "now" every test model uses.
pub fn fixed_now() -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339("2026-10-03T12:00:00+02:00").unwrap()
}

/// The pinned zone every test model uses: [`fixed_now`]'s offset, with no
/// DST switch, so frames do not depend on the host zone.
pub fn fixed_zone(_utc: NaiveDateTime) -> FixedOffset {
    *fixed_now().offset()
}

/// A key press by bubbletea name.
pub fn press(name: &str) -> KeyEvent {
    let none = KeyModifiers::NONE;
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        let mods = if c.is_uppercase() {
            KeyModifiers::SHIFT
        } else {
            none
        };
        return KeyEvent::new(KeyCode::Char(c), mods);
    }
    let (code, mods) = match name {
        "enter" => (KeyCode::Enter, none),
        "esc" => (KeyCode::Esc, none),
        "tab" => (KeyCode::Tab, none),
        "shift+tab" => (KeyCode::BackTab, KeyModifiers::SHIFT),
        "ctrl+c" => (KeyCode::Char('c'), KeyModifiers::CONTROL),
        "backspace" => (KeyCode::Backspace, none),
        "space" => (KeyCode::Char(' '), none),
        "up" => (KeyCode::Up, none),
        "down" => (KeyCode::Down, none),
        "left" => (KeyCode::Left, none),
        "right" => (KeyCode::Right, none),
        "home" => (KeyCode::Home, none),
        "end" => (KeyCode::End, none),
        "pgup" => (KeyCode::PageUp, none),
        "pgdown" => (KeyCode::PageDown, none),
        _ => panic!("unhandled key {name}"),
    };
    KeyEvent::new(code, mods)
}

pub fn key(name: &str) -> Msg {
    Msg::Key(press(name))
}

/// Feeds `msgs` to `m` in order.
pub fn send(m: &mut Model, msgs: impl IntoIterator<Item = Msg>) {
    for msg in msgs {
        m.update(msg);
    }
}

/// One key message per char of `s`.
pub fn type_text(s: &str) -> Vec<Msg> {
    s.chars().map(|c| key(&c.to_string())).collect()
}

/// A model with a pinned clock and zone that has seen one resize but no
/// snapshot.
pub fn loading_model(width: u16, height: u16) -> Model {
    let mut m = Model::new("3.34.0");
    m.clock = fixed_now;
    m.list.zone = fixed_zone;
    m.update(Msg::Resize { width, height });
    m
}

/// A loaded model showing `sessions`.
pub fn list_model(sessions: Vec<Arc<Session>>, width: u16, height: u16) -> Model {
    let mut m = loading_model(width, height);
    m.update(Msg::Sessions(SessionEvent { sessions }));
    m
}

/// One running, one completed today, two old runs.
pub fn list_fixture() -> Vec<Arc<Session>> {
    let now = fixed_now();
    let s = |id: &str,
             cli: &str,
             model: &str,
             mode: &str,
             effort: &str,
             status: &str,
             start,
             workdir: &str| {
        Arc::new(Session {
            id: id.into(),
            cli: cli.into(),
            model: model.into(),
            mode: mode.into(),
            effort: effort.into(),
            status: status.into(),
            start_time: Some(start),
            work_dir: workdir.into(),
            ..Session::default()
        })
    };
    vec![
        s(
            "a0000000-run",
            "codex",
            "gpt-6-astra",
            "review",
            "xhigh",
            "running",
            now - TimeDelta::minutes(1),
            "/src/orbit-web",
        ),
        s(
            "b0000000-done",
            "claude",
            "claude-opus-5-5",
            "plan",
            "medium",
            "completed",
            now - TimeDelta::minutes(2),
            "/src/ledger",
        ),
        s(
            "c0000000-fail",
            "codex",
            "gpt-5.5",
            "review",
            "high",
            "failed",
            now - TimeDelta::days(40),
            "/src/orbit-web",
        ),
        s(
            "d0000000-old",
            "codex",
            "gpt-5.5",
            "review",
            "high",
            "completed",
            now - TimeDelta::days(41),
            "/src/mathquest",
        ),
    ]
}

/// A solo session with the fields the list shows.
#[allow(clippy::too_many_arguments)]
pub fn run(
    id: &str,
    cli: &str,
    model: &str,
    mode: &str,
    effort: &str,
    status: &str,
    start: DateTime<FixedOffset>,
    workdir: &str,
) -> Session {
    Session {
        id: id.into(),
        cli: cli.into(),
        model: model.into(),
        mode: mode.into(),
        effort: effort.into(),
        status: status.into(),
        start_time: Some(start),
        work_dir: workdir.into(),
        ..Session::default()
    }
}

/// One run per item. Two TODAY, one YESTERDAY, one
/// OLDER; ids start a, b, c, d.
pub fn filter_fixture(now: DateTime<FixedOffset>) -> Vec<DisplayItem> {
    let solo = |s: Session| DisplayItem {
        sessions: vec![Arc::new(s)],
    };
    vec![
        solo(Session {
            prompt_preview: "review the fingerprint re-key".into(),
            ..run(
                "aaaaaaaa-1",
                "codex",
                "gpt-6-astra",
                "review",
                "xhigh",
                "running",
                now - TimeDelta::minutes(1),
                "/src/orbit-web",
            )
        }),
        solo(Session {
            review_scope: "plans/2026-09-26-service-identity".into(),
            ..run(
                "bbbbbbbb-2",
                "claude",
                "claude-opus-5-5",
                "plan",
                "medium",
                "completed",
                now - TimeDelta::hours(2),
                "/src/ledger",
            )
        }),
        solo(run(
            "cccccccc-3",
            "codex",
            "gpt-5.5",
            "review",
            "high",
            "failed",
            now - TimeDelta::days(1),
            "/src/orbit-web",
        )),
        solo(run(
            "dddddddd-4",
            "codex",
            "gpt-5.5",
            "review",
            "high",
            "completed",
            now - TimeDelta::days(40),
            "/src/mathquest",
        )),
    ]
}

/// `n` runs, newest first, ids r000, r001, ...: the first 30
/// today, the next 40 yesterday, the rest older. Every third run failed.
pub fn many_runs(n: usize, now: DateTime<FixedOffset>) -> Vec<Arc<Session>> {
    (0..n)
        .map(|i| {
            let start = match i {
                0..30 => now - TimeDelta::milliseconds(i as i64),
                30..70 => now - TimeDelta::days(1),
                _ => now - TimeDelta::days(40),
            };
            let status = if i % 3 == 0 { "failed" } else { "completed" };
            Arc::new(Session {
                duration: "1m".into(),
                ..run(
                    &format!("r{i:03}"),
                    "codex",
                    "gpt-5.5",
                    "review",
                    "high",
                    status,
                    start,
                    "/src/proj",
                )
            })
        })
        .collect()
}

/// The preview runs without the log files the list never reads: one
/// running, one completed, one failed run, all today.
pub fn preview_fixture() -> Vec<Arc<Session>> {
    let now = fixed_now();
    vec![
        Arc::new(Session {
            pid: 101,
            ..run(
                "a0000000-live",
                "codex",
                "gpt-6-astra",
                "review",
                "xhigh",
                "running",
                now - TimeDelta::minutes(1),
                "/src/orbit-web",
            )
        }),
        Arc::new(Session {
            duration: "2m36s".into(),
            ..run(
                "b0000000-done",
                "claude",
                "claude-opus-5-5",
                "plan",
                "medium",
                "completed",
                now - TimeDelta::minutes(2),
                "/src/ledger",
            )
        }),
        Arc::new(Session {
            duration: "23s".into(),
            ..run(
                "c0000000-fail",
                "codex",
                "gpt-5.5",
                "review",
                "high",
                "failed",
                now - TimeDelta::minutes(3),
                "/src/mathquest",
            )
        }),
    ]
}

/// [`preview_fixture`] with a log per run that names
/// it, so a test can tell which run a pane shows.
pub fn preview_fixture_logs(h: &Harness) -> Vec<Arc<Session>> {
    let logs = [
        ("live.log", "LIVE-RUN-OUTPUT\n"),
        ("done.log", "DONE-RUN-OUTPUT\n"),
        ("fail.log", "FAIL-RUN-OUTPUT\n"),
    ];
    preview_fixture()
        .into_iter()
        .zip(logs)
        .map(|(s, (name, body))| {
            Arc::new(Session {
                log_file: h.log(name, body),
                ..(*s).clone()
            })
        })
        .collect()
}

/// Two reviewers plus the judge, handed over in the
/// wrong order so grouping has to sort them.
pub fn group_fixture(h: &Harness) -> Vec<Arc<Session>> {
    let base = fixed_now() - TimeDelta::minutes(5);
    let (q1, q2, q3) = (
        base,
        base + TimeDelta::seconds(1),
        base + TimeDelta::seconds(2),
    );
    let member = |id: &str,
                  cli: &str,
                  model: &str,
                  mode: &str,
                  status: &str,
                  q,
                  pid,
                  log: &str,
                  body: &str| {
        Arc::new(Session {
            group_id: "grp00000-0000".into(),
            queued_at: Some(q),
            pid,
            log_file: h.log(log, body),
            ..run(id, cli, model, mode, "", status, q, "/src/orbit-web")
        })
    };
    vec![
        member(
            "judge000-0000",
            "codex",
            "gpt-6-astra",
            "consilium",
            "running",
            q3,
            901,
            "judge.log",
            "JUDGE-OUTPUT\nVERDICT: 6/10\n",
        ),
        member(
            "gemini00-0000",
            "opencode",
            "gemini-3.1",
            "megareview",
            "completed",
            q2,
            0,
            "gemini.log",
            "GEMINI-OUTPUT\n",
        ),
        member(
            "gpt55000-0000",
            "codex",
            "gpt-5.5",
            "megareview",
            "running",
            q1,
            902,
            "gpt55.log",
            "GPT55-OUTPUT\n",
        ),
    ]
}

/// A reviewer answer with every finding shape: details on the first, none
/// on the second, a suggestion only and no location on the third.
pub const FINDINGS_JSON: &str = r#"{"summary": "Two real problems in the fingerprint re-key and one nit. The migration path is otherwise sound.", "rating": 7, "findings": [
{"file": "src/auth/fingerprint.rs", "line": 42, "severity": "critical", "category": "bug", "title": "Re-key drops sessions created during the migration window", "body": "The loop reads the old key once and writes the new one after every batch, so sessions created mid-run keep the old fingerprint.", "failure_scenario": "A user logs in while the re-key runs; their next request fails the fingerprint check and they are signed out.", "suggestion": "Take the write lock for the whole re-key, or re-read the key per batch.", "confidence": 90},
{"file": "src/auth/store.rs", "line": 118, "severity": "high", "category": "concurrency", "title": "Unbounded retry on lock contention", "body": "The retry has no cap and no backoff.", "confidence": 75},
{"file": "", "line": 0, "severity": "medium", "category": "tests", "title": "No test covers the empty store", "body": "", "suggestion": "Add a table case with zero sessions.", "confidence": 60}
]}"#;

/// Draws `m` on a `width`×`height` TestBackend and returns the buffer.
pub fn draw(m: &Model, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| m.draw(f)).unwrap();
    terminal.backend().buffer().clone()
}

/// The rows of `buf` as plain text, wide glyphs counted once.
pub fn rows(buf: &Buffer) -> Vec<String> {
    let area = buf.area;
    (area.top()..area.bottom())
        .map(|y| {
            let mut row = String::new();
            let mut x = area.left();
            while x < area.right() {
                let sym = buf[(x, y)].symbol();
                row.push_str(sym);
                x += u16::try_from(sym.width().max(1)).unwrap();
            }
            row
        })
        .collect()
}

/// One letter for the style of a cell, so a golden frame pins colour and
/// weight as well as text. Styles the theme does not name map to "g" (the
/// logo gradient) when they only set a foreground, else "?".
fn style_code(cell: &Cell) -> char {
    let s = &STYLES;
    let cursor = s.accent.add_modifier(Modifier::REVERSED);
    let named = [
        ('S', s.selected),
        ('M', s.matched),
        ('A', s.active_tab),
        ('C', cursor),
        ('D', s.section),
        ('r', s.running),
        ('q', s.queued),
        ('c', s.completed),
        ('f', s.failed),
        ('T', s.value),
        ('t', s.text),
        ('d', s.dim),
        ('a', s.accent),
        ('.', Style::new()),
    ];
    let key = (cell.fg, cell.bg, cell.modifier);
    for (code, style) in named {
        let want = (
            style.fg.unwrap_or(Color::Reset),
            style.bg.unwrap_or(Color::Reset),
            style.add_modifier,
        );
        if key == want {
            return code;
        }
    }
    if cell.bg == Color::Reset && matches!(cell.fg, Color::Rgb(..)) {
        'g'
    } else {
        '?'
    }
}

/// The complete frame for a golden file: every row as text, then every
/// cell's style code, one character per cell. Each row ends in "|", so an
/// editor that strips trailing spaces cannot change the file unnoticed.
pub fn golden_frame(buf: &Buffer) -> String {
    let area = buf.area;
    let mut out = String::new();
    for row in rows(buf) {
        out.push_str(&row);
        out.push_str("|\n");
    }
    out.push_str("-- styles --\n");
    for y in area.top()..area.bottom() {
        out.extend((area.left()..area.right()).map(|x| style_code(&buf[(x, y)])));
        out.push_str("|\n");
    }
    out
}

/// Where golden frame `name` lives.
pub fn golden_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/tui/testdata")
        .join(format!("{name}.golden"))
}

/// Compares `got` with the committed golden frame `name`. Tests never
/// rewrite the file; the ignored `regenerate_list_golden_frames` test does.
pub fn assert_golden(name: &str, got: &str) {
    let path = golden_path(name);
    let want = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "read {}: {e}; regenerate with \
             cargo test -p rival regenerate_list_golden_frames -- --ignored",
            path.display()
        )
    });
    if got != want {
        panic!(
            "frame {name} differs from {}:\n--- got\n{got}\n--- want\n{want}",
            path.display()
        );
    }
}

/// The frame of `m` at its own layout size, as one string.
pub fn frame_text(m: &Model) -> String {
    let w = u16::try_from(m.lay.width).unwrap();
    let h = u16::try_from(m.lay.height).unwrap();
    rows(&draw(m, w, h)).join("\n")
}
