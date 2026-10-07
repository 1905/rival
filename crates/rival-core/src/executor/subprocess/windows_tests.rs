//! Native Windows process-tree tests for [`run_subprocess`] (Task 5.1).
//!
//! The providers, launchers, grandchildren and review owners are this test
//! binary, re-run on the ignored [`windows_helper`] in a mode named by
//! `RIVAL_WIN_HELPER`. Helpers report PIDs with their creation times in a
//! file. The controller opens a handle on each reported process right away,
//! checks its creation time, and keeps the handle: the PID cannot be reused
//! while it is open, so later "dead" checks and failure cleanup always reach
//! the reported process. Every wait is bounded. Every test uses a temp home
//! and an injected environment; the real `~/.rival` is never read.

use std::ffi::OsString;
use std::fs;
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_BREAKAWAY_OK,
    JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject,
};

use super::*;
use crate::cancel::Context;
use crate::executor::process::windows as winproc;
use crate::procinfo;
use crate::session::NewSession;

const HELPER: &str = "executor::subprocess::windows_tests::windows_helper";
const MODE: &str = "RIVAL_WIN_HELPER";
const REPORT: &str = "RIVAL_WIN_REPORT";
const HOME: &str = "RIVAL_WIN_HOME";
const ARGS_MARK: &str = "RIVAL-ARGS";
/// `;`-separated names the `env` helper looks up through the OS.
const LOOKUP: &str = "RIVAL_WIN_LOOKUP";
/// Report key prefix of those lookups.
const LOOKUP_KEY: &str = "lookup:";
/// Every wait for a helper is bounded by this.
const BOUND: Duration = Duration::from_secs(30);
/// How long a helper that waits to be killed lives at most.
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
        .expect("test binary path is UTF-8")
        .to_string()
}

/// The environment providers get: the system essentials plus a private
/// home, never the developer's own profile.
fn environ(home: &Path, extra: &[(&str, &str)]) -> Vec<OsString> {
    let mut env = Vec::new();
    for key in ["SYSTEMROOT", "PATH", "TEMP", "TMP", "PATHEXT", "COMSPEC"] {
        if let Some(v) = std::env::var_os(key) {
            let mut kv = OsString::from(format!("{key}="));
            kv.push(v);
            env.push(kv);
        }
    }
    for (key, value) in [
        ("HOME", home.as_os_str()),
        ("USERPROFILE", home.as_os_str()),
        ("RIVAL_HOME", home.join(".rival").as_os_str()),
    ] {
        let mut kv = OsString::from(format!("{key}="));
        kv.push(value);
        env.push(kv);
    }
    for (key, value) in extra {
        env.push(format!("{key}={value}").into());
    }
    env
}

struct Fixture {
    dir: tempfile::TempDir,
    paths: Paths,
    sess: Session,
}

impl Fixture {
    fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        Fixture::in_home(dir)
    }

    fn in_home(dir: tempfile::TempDir) -> Fixture {
        let work = dir.path().join("work");
        fs::create_dir_all(&work).unwrap();
        let paths = Paths::from_home(dir.path());
        let mut sess = Session::new_queued(
            &paths,
            NewSession {
                cli: "test",
                mode: "raw",
                model: "none",
                effort: "low",
                workdir: work.to_str().unwrap(),
                ..NewSession::default()
            },
        )
        .unwrap();
        sess.mark_running(&paths).unwrap();
        Fixture { dir, paths, sess }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn log(&self) -> String {
        String::from_utf8_lossy(&fs::read(&self.sess.log_file).unwrap()).into_owned()
    }
}

/// Writes `lines` to `path` atomically (temp file + rename).
fn write_report(path: &Path, lines: &[String]) {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, lines.join("\n")).unwrap();
    fs::rename(&tmp, path).unwrap();
}

/// Waits for a helper's report file and returns its `key=value` lines.
fn read_report(path: &Path, what: &str) -> Vec<(String, String)> {
    let deadline = Instant::now() + BOUND;
    loop {
        if let Ok(text) = fs::read_to_string(path) {
            return text
                .lines()
                .filter_map(|l| l.split_once('='))
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
        }
        assert!(
            Instant::now() < deadline,
            "{what}: no report after {BOUND:?}"
        );
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

/// `pid:start` of this process.
fn me() -> String {
    ident(std::process::id())
}

fn ident(pid: u32) -> String {
    let start = procinfo::start_nanos(pid as i32).expect("start time");
    format!("{pid}:{start}")
}

/// A reported process, held open so its PID stays ours to check. Dropping
/// it while the process still runs terminates that process.
struct Watched {
    name: String,
    pid: u32,
    handle: OwnedHandle,
}

impl Watched {
    /// Opens `pid:start` from a report; panics if the process is gone or the
    /// PID now belongs to another process.
    fn open(name: &str, ident: &str) -> Watched {
        let (pid, start) = ident.split_once(':').expect("pid:start");
        let pid: u32 = pid.parse().unwrap();
        let start: i64 = start.parse().unwrap();
        let handle = procinfo::windows::open(pid as i32, PROCESS_SYNCHRONIZE | PROCESS_TERMINATE)
            .unwrap_or_else(|| panic!("{name} {pid}: cannot open"));
        assert_eq!(
            procinfo::windows::creation_nanos(&handle),
            Some(start),
            "{name} {pid}: not the reported process"
        );
        Watched {
            name: name.to_string(),
            pid,
            handle,
        }
    }

    fn wait_exit(&self, timeout: Duration) -> bool {
        let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
        // SAFETY: a valid handle with SYNCHRONIZE access.
        unsafe { WaitForSingleObject(self.handle.as_raw_handle(), ms) == WAIT_OBJECT_0 }
    }

    fn running(&self) -> bool {
        !self.wait_exit(Duration::ZERO)
    }

    fn assert_dies(&self) {
        assert!(
            self.wait_exit(Duration::from_secs(10)),
            "{} {} still running",
            self.name,
            self.pid
        );
    }
}

/// How long a cleanup guard waits for the process it terminated.
const CLEANUP: Duration = Duration::from_secs(10);

impl Drop for Watched {
    fn drop(&mut self) {
        if self.running() {
            // SAFETY: our open handle pins this exact process.
            unsafe { TerminateProcess(self.handle.as_raw_handle(), 1) };
            if !self.wait_exit(CLEANUP) {
                eprintln!(
                    "cleanup: {} {} still running {CLEANUP:?} after TerminateProcess",
                    self.name, self.pid
                );
            }
        }
    }
}

/// A helper process this test started. Dropping it unexited kills it and
/// waits a bounded time for the exit.
struct Helper(std::process::Child);

impl Helper {
    fn spawn(cmd: &mut Command) -> Helper {
        Helper(crate::executor::process::spawn(cmd).unwrap())
    }

    fn wait(&mut self, timeout: Duration) -> Option<std::process::ExitStatus> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                return Some(status);
            }
            if Instant::now() > deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            if self.wait(CLEANUP).is_none() {
                eprintln!(
                    "cleanup: helper {} still running {CLEANUP:?} after kill",
                    self.0.id()
                );
            }
        }
    }
}

/// Joins `worker` once it finished, or panics after [`BOUND`].
fn join_bounded<T>(worker: std::thread::JoinHandle<T>, what: &str) -> T {
    let deadline = Instant::now() + BOUND;
    while !worker.is_finished() {
        assert!(
            Instant::now() < deadline,
            "{what} did not return in {BOUND:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    worker.join().unwrap()
}

fn helper_command(mode: &str, home: &Path, report: &Path) -> Command {
    let mut cmd = Command::new(exe());
    cmd.args(helper_args())
        .env(MODE, mode)
        .env(HOME, home)
        .env(REPORT, report)
        .env("USERPROFILE", home)
        .env("HOME", home)
        .env("RIVAL_HOME", home.join(".rival"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd
}

fn sleep_bounded() -> ! {
    std::thread::sleep(LINGER);
    std::process::exit(3);
}

/// Runs only inside helper processes (see the module docs).
#[test]
#[ignore = "helper process for the Windows process-tree tests"]
fn windows_helper() {
    let Ok(mode) = std::env::var(MODE) else {
        return;
    };
    let report = std::env::var_os(REPORT).map(PathBuf::from);
    match mode.as_str() {
        "grandchild" => {
            println!("grandchild up");
            sleep_bounded();
        }
        "launcher" => {
            let mut cmd = Command::new(exe());
            cmd.args(helper_args()).env(MODE, "grandchild");
            // stdout and stderr are inherited: the grandchild holds the
            // provider pipes too.
            // Not waited on: the test kills this launcher and checks that
            // the provider Job kills the grandchild too.
            #[allow(clippy::zombie_processes)]
            let grandchild = crate::executor::process::spawn(&mut cmd).unwrap();
            write_report(
                &report.unwrap(),
                &[
                    format!("launcher={}", me()),
                    format!("grandchild={}", ident(grandchild.id())),
                ],
            );
            println!("launcher up");
            sleep_bounded();
        }
        "echo" => {
            let mut input = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut input).unwrap();
            // One write per line. Unbuffered stderr would send `eprintln!`'s
            // pieces as separate writes, and the merged log could put a
            // stdout chunk between "err:" and the prompt.
            let mut out = std::io::stdout().lock();
            std::io::Write::write_all(&mut out, format!("out:{input}\n").as_bytes()).unwrap();
            std::io::Write::flush(&mut out).unwrap();
            drop(out);
            std::io::Write::write_all(&mut std::io::stderr(), format!("err:{input}\n").as_bytes())
                .unwrap();
            let args: Vec<String> = std::env::args()
                .skip_while(|a| a != ARGS_MARK)
                .skip(1)
                .collect();
            if let Some(report) = report {
                write_report(&report, &[format!("args={}", args.join("\u{1f}"))]);
            }
        }
        "env" => {
            let mut lines: Vec<String> = std::env::vars_os()
                .map(|(k, v)| format!("{}={}", k.to_string_lossy(), v.to_string_lossy()))
                .collect();
            // The OS's own lookup (GetEnvironmentVariableW) of each name.
            if let Some(names) = std::env::var(LOOKUP).ok().filter(|n| !n.is_empty()) {
                for name in names.split(';') {
                    lines.push(match std::env::var_os(name) {
                        Some(v) => format!("{LOOKUP_KEY}{name}=set:{}", v.to_string_lossy()),
                        None => format!("{LOOKUP_KEY}{name}=unset"),
                    });
                }
            }
            write_report(&report.unwrap(), &lines);
        }
        "owner-race" => owner_race(&report.unwrap()),
        "owner-exit" => owner_exit(&report.unwrap()),
        other => panic!("unknown helper mode {other}"),
    }
}

/// Where the startup-race owner reports its suspended provider.
static RACE_REPORT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// The startup-race barrier: runs after the suspended provider exists and
/// before its own Job assignment. It reports the child and never returns;
/// the controller kills this owner here.
fn race_barrier(pid: u32) {
    write_report(
        RACE_REPORT.get().unwrap(),
        &[
            format!("child={}", ident(pid)),
            format!("in_owner_job={}", winproc::pid_in_owner_job(pid)),
            format!("owner={}", me()),
        ],
    );
    sleep_bounded();
}

fn owner_fixture() -> Fixture {
    let home = PathBuf::from(std::env::var_os(HOME).unwrap());
    let dir = tempfile::tempdir_in(&home).unwrap();
    Fixture::in_home(dir)
}

fn owner_race(report: &Path) {
    RACE_REPORT.set(report.to_path_buf()).unwrap();
    *winproc::hooks::AFTER_SUSPENDED_SPAWN.lock().unwrap() = Some(race_barrier);
    let mut fx = owner_fixture();
    let env = environ(fx.dir.path(), &[(MODE, "grandchild")]);
    let args = helper_args();
    let binary = exe();
    let req = Request {
        binary: &binary,
        args: &args,
        env: &[],
        prompt: "",
        drop_env: &[],
        environ: &env,
    };
    let res = run_subprocess(&Context::background(), &fx.paths, &mut fx.sess, &req, None);
    // Unreachable: the barrier never returns.
    write_report(
        report,
        &[format!("returned={:?}", res.map(|r| r.exit_code))],
    );
}

fn owner_exit(report: &Path) {
    let mut fx = owner_fixture();
    let env = environ(fx.dir.path(), &[]);
    let echo_env = [format!("{MODE}=echo")];
    let args = helper_args();
    let binary = exe();
    let req = Request {
        binary: &binary,
        args: &args,
        env: &echo_env,
        prompt: "ping",
        drop_env: &[],
        environ: &env,
    };
    let res = run_subprocess(&Context::background(), &fx.paths, &mut fx.sess, &req, None);
    // A process left in the owner Job when the owner exits normally.
    let mut cmd = Command::new(exe());
    cmd.args(helper_args())
        .env(MODE, "grandchild")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Not waited on: the test checks that closing the owner Job on a normal
    // owner exit kills this stray.
    #[allow(clippy::zombie_processes)]
    let stray = crate::executor::process::spawn(&mut cmd).unwrap();
    write_report(
        report,
        &[
            format!(
                "result={:?}",
                res.map(|r| r.exit_code).map_err(|e| e.to_string())
            ),
            format!("stray={}", ident(stray.id())),
            format!(
                "stray_in_owner_job={}",
                winproc::pid_in_owner_job(stray.id())
            ),
        ],
    );
    // Return normally: libtest exits 0 and the system closes the owner Job.
}

/// Waits in another thread for a launcher report, then opens both
/// processes.
fn watch_tree(report: PathBuf) -> mpsc::Receiver<(Watched, Watched)> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let r = read_report(&report, "launcher");
        let launcher = Watched::open("launcher", field(&r, "launcher"));
        let grandchild = Watched::open("grandchild", field(&r, "grandchild"));
        let _ = tx.send((launcher, grandchild));
    });
    rx
}

/// (a) A deadline ends the run: the launcher and the grandchild that holds
/// stdout both die, the pipes close and the run returns. Go's Windows kill
/// leaves exit code 1.
#[test]
fn owner_timeout_kills_launcher_and_grandchild() {
    let mut fx = Fixture::new();
    let report = fx.path("tree");
    let env = environ(fx.dir.path(), &[]);
    let provider_env = [
        format!("{MODE}=launcher"),
        format!("{REPORT}={}", report.display()),
    ];
    let args = helper_args();
    let binary = exe();
    let req = Request {
        binary: &binary,
        args: &args,
        env: &provider_env,
        prompt: "prompt",
        drop_env: &[],
        environ: &env,
    };
    let tree = watch_tree(report);
    let (ctx, _cancel) = Context::background().with_timeout(Duration::from_secs(10));
    let start = Instant::now();
    let res = run_subprocess(&ctx, &fx.paths, &mut fx.sess, &req, None).unwrap();
    let elapsed = start.elapsed();
    let (launcher, grandchild) = tree
        .recv_timeout(Duration::from_secs(1))
        .expect("the launcher reported before the deadline");
    assert_eq!(res.exit_code, 1, "TerminateJobObject exit code");
    assert!(
        elapsed < Duration::from_secs(25),
        "returned after {elapsed:?}"
    );
    launcher.assert_dies();
    grandchild.assert_dies();
    let log = fx.log();
    assert!(log.contains("launcher up"), "{log}");
    assert!(log.contains("grandchild up"), "{log}");
    assert_eq!(
        fx.sess.pid,
        i64::from(launcher.pid),
        "the session names the provider"
    );
}

/// Two providers of one owner run in separate Jobs: cancelling one leaves
/// the other's whole tree running.
#[test]
fn concurrent_providers_cancel_independently() {
    let run = |tag: &'static str| {
        let (ctx, cancel) = Context::background().with_cancel();
        let (tx, rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut fx = Fixture::new();
            let report = fx.path(tag);
            let env = environ(fx.dir.path(), &[]);
            let provider_env = [
                format!("{MODE}=launcher"),
                format!("{REPORT}={}", report.display()),
            ];
            let _ = tx.send(watch_tree(report));
            let args = helper_args();
            let binary = exe();
            let req = Request {
                binary: &binary,
                args: &args,
                env: &provider_env,
                prompt: "",
                drop_env: &[],
                environ: &env,
            };
            run_subprocess(&ctx, &fx.paths, &mut fx.sess, &req, None).map(|r| r.exit_code)
        });
        let tree = rx.recv_timeout(BOUND).unwrap();
        (cancel, worker, tree)
    };
    let (cancel1, worker1, tree1) = run("one");
    let (cancel2, worker2, tree2) = run("two");
    let (l1, g1) = tree1.recv_timeout(BOUND).expect("first tree");
    let (l2, g2) = tree2.recv_timeout(BOUND).expect("second tree");

    cancel1.cancel();
    l1.assert_dies();
    g1.assert_dies();
    assert!(l2.running() && g2.running(), "the second tree must survive");
    assert_eq!(join_bounded(worker1, "first run").unwrap(), 1);
    assert!(!worker2.is_finished(), "the second run must still wait");

    cancel2.cancel();
    l2.assert_dies();
    g2.assert_dies();
    assert_eq!(join_bounded(worker2, "second run").unwrap(), 1);
}

/// The startup race: the owner dies after the provider was created
/// suspended and before its own Job assignment. The inherited owner Job
/// still ends it.
fn assert_race_child_dies(mut owner: Helper, report: &Path) {
    let r = read_report(report, "owner-race");
    let child = Watched::open("suspended provider", field(&r, "child"));
    assert_eq!(field(&r, "in_owner_job"), "true", "{r:?}");
    assert!(child.running(), "the provider waits at the barrier");
    // Terminate the owner at the barrier, from this (second) process.
    let _ = owner.0.kill();
    owner.wait(BOUND).expect("owner did not exit");
    child.assert_dies();
}

#[test]
fn owner_killed_before_provider_job_assignment_still_kills_provider() {
    let home = tempfile::tempdir().unwrap();
    let report = home.path().join("race");
    let owner = Helper::spawn(&mut helper_command("owner-race", home.path(), &report));
    assert_race_child_dies(owner, &report);
}

/// An outer Job that lets its processes' children break away (as some CI
/// runners do) must not let the provider escape the owner Job: the owner
/// Job, nested inside it, allows no breakaway.
#[test]
fn breakaway_outer_job_does_not_bypass_owner_containment() {
    // SAFETY: NULL attributes and name.
    let outer = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    assert!(!outer.is_null(), "{}", std::io::Error::last_os_error());
    // SAFETY: a fresh Job handle we own.
    let outer =
        unsafe { <OwnedHandle as std::os::windows::io::FromRawHandle>::from_raw_handle(outer) };
    // SAFETY: all-zero is a valid value of this plain-data struct.
    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    info.BasicLimitInformation.LimitFlags =
        JOB_OBJECT_LIMIT_BREAKAWAY_OK | JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK;
    // SAFETY: the struct this information class expects, with its size.
    let ok = unsafe {
        SetInformationJobObject(
            outer.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            (&raw const info).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    };
    assert_ne!(ok, 0, "{}", std::io::Error::last_os_error());

    let home = tempfile::tempdir().unwrap();
    let report = home.path().join("race");
    let mut cmd = helper_command("owner-race", home.path(), &report);
    cmd.creation_flags(CREATE_SUSPENDED);
    let owner = Helper::spawn(&mut cmd);
    // SAFETY: two valid handles we own.
    let ok = unsafe { AssignProcessToJobObject(outer.as_raw_handle(), owner.0.as_raw_handle()) };
    assert_ne!(
        ok,
        0,
        "assign outer job: {}",
        std::io::Error::last_os_error()
    );
    winproc::resume_threads(owner.0.id()).unwrap();
    assert_race_child_dies(owner, &report);
}

/// A normal owner exit returns 0, and the system's close of the owner Job
/// then ends a process the owner left behind.
#[test]
fn normal_owner_exit_returns_zero_and_cleans_up() {
    let home = tempfile::tempdir().unwrap();
    let report = home.path().join("exit");
    let mut owner = Helper::spawn(&mut helper_command("owner-exit", home.path(), &report));
    let status = owner
        .wait(Duration::from_secs(60))
        .expect("owner did not exit");
    let r = read_report(&report, "owner-exit");
    assert_eq!(field(&r, "result"), "Ok(0)", "{r:?}");
    assert_eq!(field(&r, "stray_in_owner_job"), "true", "{r:?}");
    assert_eq!(status.code(), Some(0), "owner exit status {status:?}");
    // The stray may already be gone (and its PID reused): identity decides.
    let (pid, start) = field(&r, "stray").split_once(':').unwrap();
    let (pid, start): (i32, i64) = (pid.parse().unwrap(), start.parse().unwrap());
    let deadline = Instant::now() + Duration::from_secs(10);
    while procinfo::alive(pid, start) {
        assert!(Instant::now() < deadline, "stray {pid} outlived its owner");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A `.cmd` provider found through `PATHEXT` in a directory with a space,
/// started by std through `cmd.exe`: every argument (spaces and cmd.exe
/// metacharacters) arrives unchanged, and stdin, stdout and stderr pass
/// through to the program the script runs.
#[test]
fn cmd_shim_keeps_argv_and_redirected_stdio() {
    let mut fx = Fixture::new();
    let bin = fx.path("npm dir (x86) & co");
    fs::create_dir_all(&bin).unwrap();
    let script = format!(
        "@echo off\r\n\"%RIVAL_TEST_EXE%\" {} {ARGS_MARK} %*\r\n",
        helper_args().join(" ")
    );
    fs::write(bin.join("codex.cmd"), script).unwrap();
    let report = fx.path("argv");
    let mut env = environ(fx.dir.path(), &[]);
    env.retain(|kv| {
        !kv.as_encoded_bytes()
            .to_ascii_uppercase()
            .starts_with(b"PATH=")
    });
    let mut path = OsString::from("PATH=");
    path.push(bin.as_os_str());
    path.push(";");
    path.push(std::env::var_os("PATH").unwrap_or_default());
    env.push(path);
    let provider_env = [
        format!("{MODE}=echo"),
        format!("{REPORT}={}", report.display()),
        format!("RIVAL_TEST_EXE={}", exe()),
    ];
    let args: Vec<String> = [
        "exec",
        "with space",
        "a&b",
        "x|y",
        "<in>",
        "caret^",
        "(paren)",
        "pct%PATH%",
        "semi;colon",
        "comma,eq=",
    ]
    .map(String::from)
    .to_vec();
    let req = Request {
        binary: "codex",
        args: &args,
        env: &provider_env,
        prompt: "the prompt",
        drop_env: &[],
        environ: &env,
    };
    let (ctx, _cancel) = Context::background().with_timeout(BOUND);
    let res = run_subprocess(&ctx, &fx.paths, &mut fx.sess, &req, None).unwrap();
    assert_eq!(res.exit_code, 0);
    let r = read_report(&report, "echo");
    assert_eq!(field(&r, "args"), args.join("\u{1f}"));
    let log = fx.log();
    assert!(log.contains("out:the prompt"), "{log}");
    assert!(log.contains("err:the prompt"), "{log}");
}

/// Windows env names are case-insensitive: mixed-case inherited variants of
/// the blocked prefixes and dropped credentials never reach the provider,
/// near names survive, and the trusted adapter entries win.
#[test]
fn child_env_drops_mixed_case_inherited_names() {
    let mut fx = Fixture::new();
    let report = fx.path("env");
    let env = environ(
        fx.dir.path(),
        &[
            ("Node_Options", "--require=./payload.cjs"),
            ("Https_Proxy", "http://evil:8080"),
            ("Opencode_Permission", r#"{"bash":"allow"}"#),
            ("Opencode_Config_Content", "evil"),
            ("Xai_Api_Key", "evil"),
            ("Anthropic_Api_Key", "evil"),
            ("aws_session_token", "evil"),
            ("Anthropic_Api_Key_2", "keep"),
            ("Node_Option", "near"),
            ("Rival_Keep", "yes"),
            ("Rival_Mode", "inherited"),
        ],
    );
    let provider_env = [
        format!("{MODE}=env"),
        format!("{REPORT}={}", report.display()),
        "OPENCODE_CONFIG_CONTENT=trusted".to_string(),
        "RIVAL_MODE=trusted".to_string(),
    ];
    let args = helper_args();
    let binary = exe();
    let req = Request {
        binary: &binary,
        args: &args,
        env: &provider_env,
        prompt: "",
        drop_env: &["ANTHROPIC_API_KEY", "AWS_"],
        environ: &env,
    };
    let (ctx, _cancel) = Context::background().with_timeout(BOUND);
    let res = run_subprocess(&ctx, &fx.paths, &mut fx.sess, &req, None).unwrap();
    assert_eq!(res.exit_code, 0, "{}", fx.log());

    let r = read_report(&report, "env");
    let values = |name: &str| -> Vec<&str> {
        r.iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
            .collect()
    };
    for name in [
        "NODE_OPTIONS",
        "HTTPS_PROXY",
        "OPENCODE_PERMISSION",
        "XAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "AWS_SESSION_TOKEN",
    ] {
        assert_eq!(values(name), Vec::<&str>::new(), "{name} leaked: {r:?}");
    }
    assert!(
        !r.iter().any(|(_, v)| v == "evil" || v.contains("payload")),
        "an unsafe inherited value reached the provider: {r:?}"
    );
    assert_eq!(values("OPENCODE_CONFIG_CONTENT"), ["trusted"]);
    assert_eq!(values("RIVAL_MODE"), ["trusted"]);
    assert_eq!(values("ANTHROPIC_API_KEY_2"), ["keep"]);
    assert_eq!(values("NODE_OPTION"), ["near"]);
    assert_eq!(values("RIVAL_KEEP"), ["yes"]);
}

/// Runs the `env` helper as a provider. `inherited` entries go through the
/// filters; `trusted` ones are appended as adapter values. The child also
/// reports the OS lookup of each of `lookups`, which the base env does not
/// set.
fn env_child(
    inherited: &[(&str, &str)],
    trusted: &[(&str, &str)],
    lookups: &[&str],
    drop_env: &[&str],
) -> Vec<(String, String)> {
    let mut fx = Fixture::new();
    let report = fx.path("env");
    let mut env = environ(fx.dir.path(), &[]);
    env.retain(|kv| {
        !lookups.iter().any(|name| {
            kv.as_encoded_bytes()
                .starts_with(format!("{name}=").as_bytes())
        })
    });
    env.extend(
        inherited
            .iter()
            .map(|(k, v)| OsString::from(format!("{k}={v}"))),
    );
    let mut provider_env = vec![
        format!("{MODE}=env"),
        format!("{REPORT}={}", report.display()),
        format!("{LOOKUP}={}", lookups.join(";")),
    ];
    provider_env.extend(trusted.iter().map(|(k, v)| format!("{k}={v}")));
    let args = helper_args();
    let binary = exe();
    let req = Request {
        binary: &binary,
        args: &args,
        env: &provider_env,
        prompt: "",
        drop_env,
        environ: &env,
    };
    let (ctx, _cancel) = Context::background().with_timeout(BOUND);
    let res = run_subprocess(&ctx, &fx.paths, &mut fx.sess, &req, None).unwrap();
    assert_eq!(res.exit_code, 0, "{}", fx.log());
    read_report(&report, "env")
}

/// The values of the variable spelled exactly `name` in an `env` report.
fn exact<'a>(report: &'a [(String, String)], name: &str) -> Vec<&'a str> {
    report
        .iter()
        .filter(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
        .collect()
}

/// The child's OS lookup of `name`: its value, or `None` when unset.
fn lookup<'a>(report: &'a [(String, String)], name: &str) -> Option<&'a str> {
    let got = field(report, &format!("{LOOKUP_KEY}{name}"));
    if got == "unset" {
        return None;
    }
    Some(got.strip_prefix("set:").expect("set:<value>"))
}

/// Measures whether the OS resolves `protected` to a variable spelled
/// `candidate`. The child gets `candidate` as a trusted adapter value, which
/// no filter touches, and looks `protected` up.
fn os_resolves(candidate: &str, protected: &str) -> bool {
    let r = env_child(&[], &[(candidate, "probe")], &[protected], &[]);
    assert_eq!(
        exact(&r, candidate),
        ["probe"],
        "trusted {candidate:?}: {r:?}"
    );
    let alias = lookup(&r, protected) == Some("probe");
    eprintln!("measured: {candidate:?} resolves as {protected}: {alias}");
    alias
}

/// Candidate spellings of protected names, with the name the child looks
/// up. Mixed ASCII case is always the same name. U+0131 (dotless i) and
/// U+017F (long s) may fold to I and S in the OS uppercase table; the test
/// measures that instead of assuming it.
const ALIASES: [(&str, &str); 9] = [
    ("Node_Options", "NODE_OPTIONS"),
    ("NODE_OPT\u{131}ONS", "NODE_OPTIONS"),
    ("NODE_OPTION\u{17f}", "NODE_OPTIONS"),
    ("HTTP\u{17f}_PROXY", "HTTPS_PROXY"),
    ("K\u{131}MI_API_KEY", "KIMI_API_KEY"),
    ("MOON\u{17f}HOT_API_KEY", "MOONSHOT_API_KEY"),
    ("XA\u{131}_API_KEY", "XAI_API_KEY"),
    ("ANTHROP\u{131}C_API_KEY", "ANTHROPIC_API_KEY"),
    ("AW\u{17f}_SESSION_TOKEN", "AWS_SESSION_TOKEN"),
];

/// The filters' name comparison is the OS's: every inherited spelling the
/// child's OS lookup would resolve as a protected name is removed, every
/// other spelling passes, and trusted adapter values still arrive.
#[test]
fn child_env_blocks_every_os_alias_of_a_protected_name() {
    let measured: Vec<bool> = ALIASES
        .iter()
        .map(|(candidate, protected)| os_resolves(candidate, protected))
        .collect();
    assert!(
        measured[0],
        "mixed ASCII case must resolve as the same name"
    );
    for ((candidate, protected), alias) in ALIASES.iter().zip(&measured) {
        assert_eq!(
            envname::eq(true, OsStr::new(candidate), protected),
            *alias,
            "comparison of {candidate:?} with {protected} disagrees with the OS lookup"
        );
    }

    let values: Vec<String> = (0..ALIASES.len()).map(|i| format!("evil-{i}")).collect();
    let mut inherited: Vec<(&str, &str)> = ALIASES
        .iter()
        .zip(&values)
        .map(|((candidate, _), value)| (*candidate, value.as_str()))
        .collect();
    inherited.extend([("Node_Option", "near"), ("NODE_OPT\u{cd}ONS", "near-u")]);
    let mut names: Vec<&str> = ALIASES.iter().map(|(_, protected)| *protected).collect();
    names.dedup();
    names.push("OPENCODE_CONFIG_CONTENT");
    let r = env_child(
        &inherited,
        &[("OPENCODE_CONFIG_CONTENT", "trusted")],
        &names,
        &["ANTHROPIC_API_KEY", "AWS_"],
    );
    for (candidate, protected) in ALIASES {
        assert_eq!(
            lookup(&r, protected),
            None,
            "{protected} via {candidate:?}: {r:?}"
        );
    }
    for (((candidate, _), alias), value) in ALIASES.iter().zip(&measured).zip(&values) {
        let want = if *alias { vec![] } else { vec![value.as_str()] };
        assert_eq!(exact(&r, candidate), want, "{candidate:?}: {r:?}");
    }
    assert_eq!(exact(&r, "Node_Option"), ["near"]);
    assert_eq!(exact(&r, "NODE_OPT\u{cd}ONS"), ["near-u"]);
    assert_eq!(lookup(&r, "OPENCODE_CONFIG_CONTENT"), Some("trusted"));
}

/// The state-root check refuses exactly the spellings the OS resolves as
/// `RIVAL_HOME`. A repository `.env` with a parseable spelling loads only
/// when it is no alias; near names and ordinary keys still load. godotenv
/// rejects the U+0131 key, so that file sets nothing: it never loads from
/// `.env`, whatever the OS says about it.
#[test]
fn state_root_check_refuses_every_os_alias() {
    use crate::paths::{STATE_ROOT_VAR, is_state_root_var, load_dotenv_with, parse_dotenv};

    let dir = tempfile::tempdir().unwrap();
    let dotenv = dir.path().join(".env");
    // (spelling, known OS answer, godotenv parses it).
    for (candidate, known, parses) in [
        ("Rival_Home", Some(true), true),
        ("rival_home", Some(true), true),
        ("R\u{131}VAL_HOME", None, false),
        ("RIVAL_HOM\u{f3}", Some(false), true),
        ("RIVAL_HOMEX", Some(false), true),
    ] {
        let alias = os_resolves(candidate, STATE_ROOT_VAR);
        if let Some(known) = known {
            assert_eq!(alias, known, "{candidate:?}");
        }
        assert_eq!(
            is_state_root_var(candidate, true),
            alias,
            "state-root check of {candidate:?} disagrees with the OS lookup"
        );

        let text = format!("{candidate}=repo\nOK=1\n");
        assert_eq!(parse_dotenv(&text).is_ok(), parses, "{candidate:?}");
        fs::write(&dotenv, &text).unwrap();
        let mut set = Vec::new();
        load_dotenv_with(&dotenv, |_| false, |k, v| set.push(format!("{k}={v}")));
        set.sort();
        let mut want = Vec::new();
        if parses {
            want.push("OK=1".to_string());
            if !alias {
                want.push(format!("{candidate}=repo"));
            }
        }
        want.sort();
        assert_eq!(set, want, "{candidate:?}");
    }
}

/// The prompt-write and output paths on a plain `.exe` provider.
#[test]
fn exe_provider_round_trip() {
    let mut fx = Fixture::new();
    let env = environ(fx.dir.path(), &[]);
    let provider_env = [format!("{MODE}=echo")];
    let args = helper_args();
    let binary = exe();
    let req = Request {
        binary: &binary,
        args: &args,
        env: &provider_env,
        prompt: "hello",
        drop_env: &[],
        environ: &env,
    };
    let res = run_subprocess(&Context::background(), &fx.paths, &mut fx.sess, &req, None).unwrap();
    assert_eq!(res.exit_code, 0);
    assert!(fx.log().contains("out:hello"));
    assert!(
        winproc::owner_job_active(),
        "a provider run creates the owner Job"
    );
}
