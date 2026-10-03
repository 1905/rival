//! Native Windows stop from a second process (Task 5.1 test b).
//!
//! The owner is this test binary on the ignored [`kill_win_helper`]: a real
//! review owner with a session, a running queue ticket and a provider run
//! (`run_subprocess`) whose launcher starts a grandchild that holds stdout.
//! This test process is the second process: it stops the run through the
//! real [`stop_sessions`] with [`ProcessOps::SYSTEM`], then checks the owner,
//! the launcher and the grandchild are dead, the session was failed by the
//! reaper (not by the stop itself), and the queue slot is free again.
//! Reported processes are opened at once and identity-checked; open handles
//! keep their PIDs from reuse. Every wait is bounded.

use std::ffi::OsString;
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
use windows_sys::Win32::System::Console::{GetConsoleCP, GetConsoleWindow};
use windows_sys::Win32::System::Threading::{
    PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject,
};

use super::*;
use rival_core::cancel::Context;
use rival_core::executor::{Request, run_subprocess};
use rival_core::queue::{self, STATE_RUNNING};
use rival_core::session::NewSession;

const HELPER: &str = "tui::kill::windows_tests::kill_win_helper";
const MODE: &str = "RIVAL_KILL_WIN_MODE";
const HOME: &str = "RIVAL_KILL_WIN_HOME";
const REPORT: &str = "RIVAL_KILL_WIN_REPORT";
const BOUND: Duration = Duration::from_secs(30);
const LINGER: Duration = Duration::from_secs(120);

fn helper_args() -> Vec<String> {
    [
        "--exact",
        HELPER,
        "--ignored",
        "--nocapture",
        "--test-threads=1",
    ]
    .map(String::from)
    .to_vec()
}

fn exe() -> String {
    std::env::current_exe()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string()
}

fn ident(pid: u32) -> String {
    let start = procinfo::start_nanos(pid as i32).expect("start time");
    format!("{pid}:{start}")
}

fn write_report(path: &Path, lines: &[String]) {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, lines.join("\n")).unwrap();
    std::fs::rename(&tmp, path).unwrap();
}

fn read_report(path: &Path, what: &str) -> Vec<(String, String)> {
    let deadline = Instant::now() + BOUND;
    loop {
        if let Ok(text) = std::fs::read_to_string(path) {
            return text
                .lines()
                .filter_map(|l| l.split_once('='))
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
        }
        assert!(Instant::now() < deadline, "{what}: no report");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn field<'a>(report: &'a [(String, String)], key: &str) -> &'a str {
    report
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
        .unwrap_or_else(|| panic!("no {key} in {report:?}"))
}

fn queue_manager(paths: &Paths) -> queue::Manager {
    queue::Manager::with_settings(
        paths.queue_dir(),
        1,
        Duration::from_millis(50),
        Duration::from_secs(5),
    )
}

/// Runs only inside helper processes.
#[test]
#[ignore = "helper process for the Windows stop test"]
fn kill_win_helper() {
    let Ok(mode) = std::env::var(MODE) else {
        return;
    };
    let report = PathBuf::from(std::env::var_os(REPORT).unwrap());
    match mode.as_str() {
        "grandchild" => {
            println!("grandchild up");
            std::thread::sleep(LINGER);
        }
        "launcher" => {
            let mut cmd = Command::new(exe());
            cmd.args(helper_args()).env(MODE, "grandchild");
            // Not waited on: the stop under test kills this launcher, and
            // the provider Job kills the grandchild with it.
            #[allow(clippy::zombie_processes)]
            let grandchild = rival_core::executor::process::spawn(&mut cmd).unwrap();
            // SAFETY: a plain query; NULL means no console window.
            let window = !unsafe { GetConsoleWindow() }.is_null();
            write_report(
                &report,
                &[
                    format!("launcher={}", ident(std::process::id())),
                    format!("grandchild={}", ident(grandchild.id())),
                    format!("console_window={window}"),
                ],
            );
            println!("launcher up");
            std::thread::sleep(LINGER);
        }
        "owner" => owner(&report),
        other => panic!("unknown mode {other}"),
    }
}

/// The review owner: queued, promoted, then running its provider.
fn owner(report: &Path) {
    let home = PathBuf::from(std::env::var_os(HOME).unwrap());
    let paths = Paths::from_home(&home);
    let work = home.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let mut sess = Session::new_queued(
        &paths,
        NewSession {
            cli: "test",
            mode: "raw",
            model: "none",
            effort: "low",
            workdir: work.to_str().unwrap(),
            prompt: "the prompt",
            ..NewSession::default()
        },
    )
    .unwrap();
    let mut q = queue_manager(&paths);
    q.enqueue(
        "",
        std::slice::from_ref(&sess.id),
        "review",
        work.to_str().unwrap(),
    )
    .unwrap();
    q.wait_for_slot(&Context::background(), None).unwrap();
    sess.mark_running(&paths).unwrap();
    write_report(
        &home.join("owner"),
        &[
            format!("owner={}", ident(std::process::id())),
            format!("session={}", sess.id),
            // SAFETY: a plain query; 0 means no console is attached.
            format!("has_console={}", unsafe { GetConsoleCP() } != 0),
        ],
    );
    let mut environ: Vec<OsString> = Vec::new();
    for key in ["SYSTEMROOT", "PATH", "TEMP", "TMP"] {
        if let Some(v) = std::env::var_os(key) {
            let mut kv = OsString::from(format!("{key}="));
            kv.push(v);
            environ.push(kv);
        }
    }
    let provider_env = [
        format!("{MODE}=launcher"),
        format!("{REPORT}={}", report.display()),
        format!("USERPROFILE={}", home.display()),
        format!("HOME={}", home.display()),
    ];
    let args = helper_args();
    let binary = exe();
    let req = Request {
        binary: &binary,
        args: &args,
        env: &provider_env,
        prompt: "the prompt",
        drop_env: &[],
        environ: &environ,
    };
    // Blocks until this owner is terminated by the test.
    let _ = run_subprocess(&Context::background(), &paths, &mut sess, &req, None);
    std::process::exit(4);
}

/// A reported process held open; dropping it while it runs ends it.
struct Watched(String, OwnedHandle);

impl Watched {
    fn open(name: &str, ident: &str) -> Watched {
        let (pid, start) = ident.split_once(':').unwrap();
        let (pid, start): (i32, i64) = (pid.parse().unwrap(), start.parse().unwrap());
        let handle = procinfo::windows::open(pid, PROCESS_SYNCHRONIZE | PROCESS_TERMINATE)
            .unwrap_or_else(|| panic!("{name} {pid} is gone"));
        assert_eq!(
            procinfo::windows::creation_nanos(&handle),
            Some(start),
            "{name} {pid}: PID reused"
        );
        Watched(name.to_string(), handle)
    }

    fn wait_exit(&self, timeout: Duration) -> bool {
        let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
        // SAFETY: a valid handle with SYNCHRONIZE access; a bounded wait.
        unsafe { WaitForSingleObject(self.1.as_raw_handle(), ms) == WAIT_OBJECT_0 }
    }

    fn running(&self) -> bool {
        !self.wait_exit(Duration::ZERO)
    }

    fn assert_dead(&self) {
        assert!(
            self.wait_exit(Duration::from_secs(10)),
            "{} still running after the stop",
            self.0
        );
    }
}

/// How long a cleanup guard waits for the process it ended.
const CLEANUP: Duration = Duration::from_secs(10);

impl Drop for Watched {
    fn drop(&mut self) {
        if self.running() {
            // SAFETY: our handle pins this exact process.
            unsafe { TerminateProcess(self.1.as_raw_handle(), 1) };
            if !self.wait_exit(CLEANUP) {
                eprintln!("cleanup: {} still running after TerminateProcess", self.0);
            }
        }
    }
}

/// The owner process, guarded from the moment it exists: dropping it
/// unexited kills it and waits a bounded time.
struct OwnerChild(std::process::Child);

impl OwnerChild {
    fn wait_for(&mut self, timeout: Duration) -> Option<std::process::ExitStatus> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                return Some(status);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for OwnerChild {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            if self.wait_for(CLEANUP).is_none() {
                eprintln!("cleanup: owner {} still running after kill", self.0.id());
            }
        }
    }
}

#[test]
fn second_process_stop_ends_owner_tree_and_recovers_session_and_queue() {
    let home = tempfile::tempdir().unwrap();
    let paths = Paths::from_home(home.path());
    let report = home.path().join("tree");
    let mut cmd = Command::new(exe());
    // Started like a fully redirected `rival --detach` owner: the exact
    // production creation flags, so it has no console and its own group.
    let flags = crate::detach::detach_creation_flags(false);
    std::os::windows::process::CommandExt::creation_flags(&mut cmd, flags);
    cmd.args(helper_args())
        .env(MODE, "owner")
        .env(HOME, home.path())
        .env(REPORT, &report)
        .env("USERPROFILE", home.path())
        .env("HOME", home.path())
        .env("RIVAL_HOME", home.path().join(".rival"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut owner_child = OwnerChild(rival_core::executor::process::spawn(&mut cmd).unwrap());

    let owner_report = read_report(&home.path().join("owner"), "owner");
    let owner = Watched::open("owner", field(&owner_report, "owner"));
    assert_eq!(
        field(&owner_report, "has_console"),
        "false",
        "a detached owner has no console"
    );
    let tree = read_report(&report, "launcher");
    let launcher = Watched::open("launcher", field(&tree, "launcher"));
    let grandchild = Watched::open("grandchild", field(&tree, "grandchild"));
    assert_eq!(
        field(&tree, "console_window"),
        "false",
        "a provider of a console-less owner gets no console window"
    );
    let id = field(&owner_report, "session").to_string();

    // The session names the provider; the owner fields name the owner.
    let deadline = Instant::now() + BOUND;
    let before = loop {
        let s = Session::load(&paths, &id).unwrap();
        if s.pid == i64::from(launcher_pid(&tree)) {
            break s;
        }
        assert!(Instant::now() < deadline, "provider PID never saved");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(before.status, "running");
    assert_eq!(before.owner_pid, i64::from(owner_child.0.id()));
    let tickets = queue_manager(&paths).list().unwrap();
    assert_eq!(tickets.len(), 1, "{tickets:?}");
    assert_eq!(tickets[0].ticket.state, STATE_RUNNING);
    assert!(
        may_stop(&before, ProcessOps::SYSTEM.alive),
        "owner identity verified"
    );

    let res = stop_sessions(
        &paths,
        &ProcessOps::SYSTEM,
        StopRequest {
            item_key: "solo:x".into(),
            targets: vec![Arc::new(before)],
        },
    );

    owner.assert_dead();
    launcher.assert_dead();
    grandchild.assert_dead();
    let status = owner_child.wait_for(BOUND).expect("owner not reaped");
    assert_eq!(status.code(), Some(1), "TerminateProcess exit code");
    // Every handle above (owner_child, owner, launcher, grandchild) was
    // still open through the stop: identity stayed readable, so only the
    // running-state wait let the stop reach the reaper.
    let launcher_start: i64 = field(&tree, "launcher")
        .split_once(':')
        .unwrap()
        .1
        .parse()
        .unwrap();
    let launcher_pid = i64::from(launcher_pid(&tree));
    assert!(
        (ProcessOps::SYSTEM.alive)(launcher_pid, launcher_start),
        "the retained launcher handle keeps its identity"
    );
    assert!(
        !(ProcessOps::SYSTEM.running)(launcher_pid, launcher_start),
        "but it is not running"
    );

    // The reaper's crashed-owner failure, with the full record kept.
    let after = Session::load(&paths, &id).unwrap();
    assert_eq!(after.status, "failed");
    assert_eq!(after.error_msg, "orphaned (process dead)");
    assert_eq!(after.exit_code, Some(1));
    assert_eq!(after.prompt, "the prompt");
    assert_eq!(res.updates.len(), 1, "{res:?}");
    assert_eq!(res.updates[0].1.error_msg, "orphaned (process dead)");

    // ReapDead freed the slot: no ticket is left, and a new run gets the
    // only slot at once.
    assert!(queue_manager(&paths).list().unwrap().is_empty());
    let mut next = queue_manager(&paths);
    next.enqueue("", &[], "review", "").unwrap();
    next.wait_for_slot(&Context::background(), None)
        .expect("the slot is free");
    next.release();
}

fn launcher_pid(tree: &[(String, String)]) -> u32 {
    field(tree, "launcher")
        .split_once(':')
        .unwrap()
        .0
        .parse()
        .unwrap()
}
