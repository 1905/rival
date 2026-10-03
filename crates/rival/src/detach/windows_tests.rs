//! Native Windows detach tests: the console rule, stream inheritance and
//! life after the short-lived parent exits.
//!
//! Roles, all this test binary on the ignored [`detach_win_helper`]:
//! - parent: the short-lived `rival --detach` process. It sets up its
//!   standard handles for the case, starts the child through the real
//!   [`start_and_report`], waits until the child runs, and exits 0.
//! - child: waits until the parent is gone, reads one line from stdin and
//!   writes `out:<line>` to stdout and `err:<line>` to stderr.
//! - observer: attaches to the shared console to type the input line and to
//!   read the screen. The test process itself never attaches to a console.
//!
//! Every wait is bounded; reports are written with temp file + rename.

use std::fs::{self, File};
use std::io::{BufRead, Read, Write};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use windows_sys::Win32::System::Console::{
    AttachConsole, CONSOLE_SCREEN_BUFFER_INFO, COORD, FreeConsole, GetConsoleCP,
    GetConsoleScreenBufferInfo, INPUT_RECORD, KEY_EVENT, ReadConsoleOutputCharacterW,
    STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle, WriteConsoleInputW,
};
use windows_sys::Win32::System::Threading::{
    CREATE_NEW_CONSOLE, CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS,
};

use super::*;
use rival_core::procinfo;

const HELPER: &str = "detach::windows_tests::detach_win_helper";
const MODE: &str = "RIVAL_DETACH_WIN_MODE";
const CASE: &str = "RIVAL_DETACH_WIN_CASE";
const DIR: &str = "RIVAL_DETACH_WIN_DIR";
const PARENT: &str = "RIVAL_DETACH_WIN_PARENT";
const TARGET: &str = "RIVAL_DETACH_WIN_TARGET";
const BOUND: Duration = Duration::from_secs(30);
const LINE: &str = "hello";

fn helper_args() -> Vec<OsString> {
    [
        "--exact",
        HELPER,
        "--ignored",
        "--nocapture",
        "--test-threads=1",
    ]
    .map(OsString::from)
    .to_vec()
}

fn ident(pid: u32) -> String {
    let start = procinfo::start_nanos(pid as i32).expect("start time");
    format!("{pid}:{start}")
}

fn write_report(path: &Path, lines: &[String]) {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, lines.join("\n")).unwrap();
    fs::rename(&tmp, path).unwrap();
}

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
        assert!(Instant::now() < deadline, "{what}: no report at {path:?}");
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

fn wait_for(path: &Path, what: &str) {
    let deadline = Instant::now() + BOUND;
    while !path.exists() {
        assert!(Instant::now() < deadline, "{what}: {path:?} never appeared");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Opens a console buffer (`CONIN$` or `CONOUT$`) of this process's console.
fn open_console(name: &str) -> File {
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(name)
        .unwrap_or_else(|e| panic!("open {name}: {e}"))
}

/// Runs only inside helper processes.
#[test]
#[ignore = "helper process for the Windows detach tests"]
fn detach_win_helper() {
    let Ok(mode) = std::env::var(MODE) else {
        return;
    };
    let dir = PathBuf::from(std::env::var_os(DIR).unwrap());
    match mode.as_str() {
        "parent" => parent(&dir),
        "child" => child(&dir),
        "observer" => observer(&dir),
        other => panic!("unknown mode {other}"),
    }
}

/// The parent: console handles for the streams the case puts on the
/// console, then the real detach start.
fn parent(dir: &Path) {
    let case = std::env::var(CASE).unwrap();
    // The console streams are opened here, in the new console the test
    // created for this parent; file and pipe streams arrived redirected.
    let on_console: &[u32] = match case.as_str() {
        "console" => &[STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE],
        "mixed" => &[STD_INPUT_HANDLE],
        _ => &[],
    };
    let mut keep = Vec::new();
    for &which in on_console {
        let name = if which == STD_INPUT_HANDLE {
            "CONIN$"
        } else {
            "CONOUT$"
        };
        let file = open_console(name);
        // SAFETY: a valid console handle that `keep` holds open.
        unsafe { SetStdHandle(which, file.as_raw_handle()) };
        keep.push(file);
    }
    let any_console = any_std_handle_is_console();
    let mut cmd = detach_command(&std::env::current_exe().unwrap(), &helper_args());
    cmd.env(MODE, "child")
        .env(PARENT, ident(std::process::id()));
    let mut line = Vec::new();
    let outcome = start_and_report(cmd, &mut line);
    let line = String::from_utf8(line).unwrap();
    let pid: u32 = line
        .strip_prefix("rival: detached pid=")
        .and_then(|s| s.strip_suffix('\n'))
        .unwrap_or_else(|| panic!("unexpected line {line:?}"))
        .parse()
        .unwrap();
    write_report(
        &dir.join("parent"),
        &[
            format!("outcome={outcome:?}"),
            format!("child={pid}"),
            format!("any_console={any_console}"),
            format!("flags={:#x}", detach_creation_flags(any_console)),
            format!(
                "owner_job={}",
                rival_core::executor::process::windows::owner_job_active()
            ),
        ],
    );
    wait_for(&dir.join("child-ready"), "child start");
    // Exit now, so the child does its I/O after this parent is gone.
    std::process::exit(0);
}

/// The detached child.
fn child(dir: &Path) {
    let parent = std::env::var(PARENT).unwrap();
    let (pid, start) = parent.split_once(':').unwrap();
    let (pid, start): (i32, i64) = (pid.parse().unwrap(), start.parse().unwrap());
    // Its own identity: the controller checks it on the handle it opens.
    // This child cannot exit before `release`, which the controller writes
    // only after it holds that handle.
    write_report(
        &dir.join("child-ready"),
        &[format!("child={}", ident(std::process::id()))],
    );
    let deadline = Instant::now() + BOUND;
    while procinfo::alive(pid, start) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let parent_gone = !procinfo::alive(pid, start);
    // SAFETY: a plain query; 0 means no console is attached.
    let has_console = unsafe { GetConsoleCP() } != 0;
    let mut input = String::new();
    let read = std::io::stdin().lock().read_line(&mut input);
    let input = input.trim_end().to_string();
    let out = writeln!(std::io::stdout(), "out:{input}").and_then(|()| std::io::stdout().flush());
    let err = writeln!(std::io::stderr(), "err:{input}");
    write_report(
        &dir.join("child"),
        &[
            format!("parent_gone={parent_gone}"),
            format!("has_console={has_console}"),
            format!("marker={}", std::env::var(DETACHED_ENV).unwrap_or_default()),
            format!("read={:?}", read.map(|_| ())),
            format!("line={input}"),
            format!("out={:?}", out),
            format!("err={:?}", err),
        ],
    );
    // Keep the console alive until the observer has read the screen.
    wait_for(&dir.join("release"), "release");
    std::process::exit(0);
}

/// The observer: types the input line into the child's console and reads
/// the screen until the expected lines show (or the bound passes).
fn observer(dir: &Path) {
    let target: u32 = std::env::var(TARGET).unwrap().parse().unwrap();
    let expect: Vec<String> = std::env::var("RIVAL_DETACH_WIN_EXPECT")
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    // SAFETY: this helper has no console (DETACHED_PROCESS); attach to the
    // child's.
    unsafe { FreeConsole() };
    // SAFETY: a plain call by PID.
    let attached = unsafe { AttachConsole(target) } != 0;
    let mut found = Vec::new();
    let mut screen = String::new();
    if attached {
        let conin = open_console("CONIN$");
        let conout = open_console("CONOUT$");
        type_line(&conin, LINE);
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            screen = read_screen(&conout);
            found = expect.iter().map(|e| screen.contains(e.as_str())).collect();
            if found.iter().all(|&f| f) || Instant::now() > deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    let shown: String = screen.split_whitespace().collect::<Vec<_>>().join(" ");
    write_report(
        &dir.join("observer"),
        &[
            format!("attached={attached}"),
            format!("found={found:?}"),
            format!("screen={}", shown.chars().take(400).collect::<String>()),
        ],
    );
    // SAFETY: detaches from the child's console before exit.
    unsafe { FreeConsole() };
    std::process::exit(0);
}

/// Writes key-down and key-up events for `text` and Enter.
fn type_line(conin: &File, text: &str) {
    let mut records = Vec::new();
    for c in text.chars().chain(std::iter::once('\r')) {
        let vk = if c == '\r' {
            0x0D
        } else {
            c.to_ascii_uppercase() as u16
        };
        for down in [1, 0] {
            // SAFETY: all-zero is a valid INPUT_RECORD.
            let mut rec: INPUT_RECORD = unsafe { std::mem::zeroed() };
            rec.EventType = KEY_EVENT as u16;
            rec.Event.KeyEvent.bKeyDown = down;
            rec.Event.KeyEvent.wRepeatCount = 1;
            rec.Event.KeyEvent.wVirtualKeyCode = vk;
            rec.Event.KeyEvent.uChar.UnicodeChar = c as u16;
            records.push(rec);
        }
    }
    let mut written = 0;
    // SAFETY: a console input handle and a slice of initialized records.
    let ok = unsafe {
        WriteConsoleInputW(
            conin.as_raw_handle(),
            records.as_ptr(),
            records.len() as u32,
            &mut written,
        )
    };
    assert_ne!(
        ok,
        0,
        "WriteConsoleInputW: {}",
        std::io::Error::last_os_error()
    );
}

/// The screen buffer text up to the cursor row.
fn read_screen(conout: &File) -> String {
    // SAFETY: all-zero is a valid out value.
    let mut info: CONSOLE_SCREEN_BUFFER_INFO = unsafe { std::mem::zeroed() };
    // SAFETY: a console output handle and a writable struct.
    if unsafe { GetConsoleScreenBufferInfo(conout.as_raw_handle(), &mut info) } == 0 {
        return String::new();
    }
    let width = info.dwSize.X.max(1) as u32;
    let rows = (info.dwCursorPosition.Y as u32 + 1).min(info.dwSize.Y.max(1) as u32);
    let mut buf = vec![0u16; (width * rows) as usize];
    let mut read = 0;
    // SAFETY: buf holds the requested number of UTF-16 units.
    let ok = unsafe {
        ReadConsoleOutputCharacterW(
            conout.as_raw_handle(),
            buf.as_mut_ptr(),
            buf.len() as u32,
            COORD { X: 0, Y: 0 },
            &mut read,
        )
    };
    if ok == 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..read as usize])
}

/// How long a cleanup guard waits for the process it ended.
const CLEANUP: Duration = Duration::from_secs(10);

/// A helper process; dropping it unexited kills it and waits a bounded time.
struct Helper(Child);

impl Helper {
    fn spawn(cmd: &mut Command) -> Helper {
        Helper(rival_core::executor::process::spawn(cmd).unwrap())
    }

    fn try_wait_for(&mut self, timeout: Duration) -> Option<std::process::ExitStatus> {
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

    fn wait(&mut self, what: &str) -> std::process::ExitStatus {
        self.try_wait_for(BOUND)
            .unwrap_or_else(|| panic!("{what} did not exit in {BOUND:?}"))
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            if self.try_wait_for(CLEANUP).is_none() {
                eprintln!("cleanup: helper {} still running after kill", self.0.id());
            }
        }
    }
}

/// The detached child, opened from its own `pid:start` report with that
/// creation time checked on the opened handle. The handle pins the process,
/// so cleanup can only reach this child. Dropping it while it runs
/// terminates it and waits a bounded time.
struct ChildProc {
    pid: u32,
    handle: std::os::windows::io::OwnedHandle,
}

impl ChildProc {
    fn open(ident: &str) -> ChildProc {
        use windows_sys::Win32::System::Threading::{PROCESS_SYNCHRONIZE, PROCESS_TERMINATE};
        let (pid, start) = ident.split_once(':').expect("pid:start");
        let (pid, start): (u32, i64) = (pid.parse().unwrap(), start.parse().unwrap());
        let handle = procinfo::windows::open(pid as i32, PROCESS_SYNCHRONIZE | PROCESS_TERMINATE)
            .unwrap_or_else(|| panic!("detached child {pid} is gone"));
        assert_eq!(
            procinfo::windows::creation_nanos(&handle),
            Some(start),
            "detached child {pid}: the PID belongs to another process"
        );
        ChildProc { pid, handle }
    }

    fn wait_exit(&self, timeout: Duration) -> bool {
        use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
        use windows_sys::Win32::System::Threading::WaitForSingleObject;
        let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
        // SAFETY: a valid handle with SYNCHRONIZE access.
        unsafe { WaitForSingleObject(self.handle.as_raw_handle(), ms) == WAIT_OBJECT_0 }
    }
}

impl Drop for ChildProc {
    fn drop(&mut self) {
        use windows_sys::Win32::System::Threading::TerminateProcess;
        if self.wait_exit(Duration::ZERO) {
            return;
        }
        // SAFETY: our verified handle pins this exact process.
        unsafe { TerminateProcess(self.handle.as_raw_handle(), 1) };
        if !self.wait_exit(CLEANUP) {
            eprintln!("cleanup: detached child {} still running", self.pid);
        }
    }
}

/// Reads a pipe to EOF on another thread.
fn drain(mut reader: std::io::PipeReader) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        let _ = reader.read_to_string(&mut text);
        let _ = tx.send(text);
    });
    rx
}

/// One case end to end. Returns the child report, the observer report (if
/// any), the stdout file text and the stderr pipe text.
struct Outcome {
    parent: Vec<(String, String)>,
    child: Vec<(String, String)>,
    observer: Option<Vec<(String, String)>>,
    stdout_file: String,
    stderr_pipe: String,
}

fn run_case(case: &str) -> Outcome {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let stdout_path = dir.join("stdout.txt");
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(helper_args())
        .env(MODE, "parent")
        .env(CASE, case)
        .env(DIR, dir)
        .env("HOME", dir)
        .env("USERPROFILE", dir)
        .env("RIVAL_HOME", dir.join(".rival"));
    let mut stderr_rx = None;
    match case {
        "console" => {
            cmd.creation_flags(CREATE_NEW_CONSOLE)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
        }
        "mixed" | "redirected" => {
            let (reader, writer) = rival_core::executor::process::pipe().unwrap();
            stderr_rx = Some(drain(reader));
            let stdin = if case == "mixed" {
                cmd.creation_flags(CREATE_NEW_CONSOLE);
                Stdio::null()
            } else {
                cmd.creation_flags(DETACHED_PROCESS);
                let input = dir.join("stdin.txt");
                fs::write(&input, format!("{LINE}\n")).unwrap();
                Stdio::from(File::open(&input).unwrap())
            };
            cmd.stdin(stdin)
                .stdout(Stdio::from(File::create(&stdout_path).unwrap()))
                .stderr(Stdio::from(writer));
        }
        other => panic!("unknown case {other}"),
    }
    let mut parent = Helper::spawn(&mut cmd);
    // Our copies of the redirected ends close with the command.
    drop(cmd);
    let parent_report = read_report(&dir.join("parent"), "parent");
    let child_pid: u32 = field(&parent_report, "child").parse().unwrap();
    let ready = read_report(&dir.join("child-ready"), "child start");
    let child = ChildProc::open(field(&ready, "child"));
    assert_eq!(
        child.pid, child_pid,
        "the parent started the reporting child"
    );
    let status = parent.wait("parent");
    assert_eq!(status.code(), Some(0), "parent exit: {status:?}");

    let observer = (case != "redirected").then(|| {
        let expect = if case == "console" {
            format!("out:{LINE},err:{LINE}")
        } else {
            String::new()
        };
        let mut obs = Command::new(std::env::current_exe().unwrap());
        obs.args(helper_args())
            .env(MODE, "observer")
            .env(DIR, dir)
            .env(TARGET, child_pid.to_string())
            .env("RIVAL_DETACH_WIN_EXPECT", expect)
            .env("HOME", dir)
            .env("USERPROFILE", dir)
            .env("RIVAL_HOME", dir.join(".rival"))
            .creation_flags(DETACHED_PROCESS)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut obs = Helper::spawn(&mut obs);
        // The child's report arrives after it read the typed line.
        let report = read_report(&dir.join("observer"), "observer");
        obs.wait("observer");
        report
    });
    let child_report = read_report(&dir.join("child"), "child");
    fs::write(dir.join("release"), "").unwrap();
    assert!(child.wait_exit(CLEANUP), "detached child did not exit");
    let stderr_pipe = stderr_rx
        .map(|rx| rx.recv_timeout(BOUND).expect("stderr pipe never closed"))
        .unwrap_or_default();
    Outcome {
        parent: parent_report,
        child: child_report,
        observer,
        stdout_file: fs::read_to_string(&stdout_path).unwrap_or_default(),
        stderr_pipe,
    }
}

fn assert_child_did_io(o: &Outcome) {
    assert_eq!(field(&o.parent, "outcome"), "Exit(0)", "{:?}", o.parent);
    assert_eq!(
        field(&o.parent, "owner_job"),
        "false",
        "the detach parent never makes the owner Job"
    );
    assert_eq!(field(&o.child, "parent_gone"), "true", "{:?}", o.child);
    assert_eq!(field(&o.child, "marker"), "1");
    assert_eq!(field(&o.child, "read"), "Ok(())", "{:?}", o.child);
    assert_eq!(field(&o.child, "line"), LINE, "{:?}", o.child);
    assert_eq!(field(&o.child, "out"), "Ok(())", "{:?}", o.child);
    assert_eq!(field(&o.child, "err"), "Ok(())", "{:?}", o.child);
}

#[test]
fn console_streams_keep_working_after_the_parent_exits() {
    let o = run_case("console");
    assert_child_did_io(&o);
    assert_eq!(field(&o.parent, "any_console"), "true");
    assert_eq!(
        field(&o.parent, "flags"),
        format!("{CREATE_NEW_PROCESS_GROUP:#x}"),
        "a console child shares the console: no DETACHED_PROCESS"
    );
    assert_eq!(field(&o.child, "has_console"), "true");
    let obs = o.observer.as_ref().unwrap();
    assert_eq!(field(obs, "attached"), "true", "{obs:?}");
    assert_eq!(field(obs, "found"), "[true, true]", "{obs:?}");
}

#[test]
fn mixed_console_file_and_pipe_streams_each_keep_their_target() {
    let o = run_case("mixed");
    assert_child_did_io(&o);
    assert_eq!(field(&o.parent, "any_console"), "true");
    assert_eq!(
        field(&o.parent, "flags"),
        format!("{CREATE_NEW_PROCESS_GROUP:#x}")
    );
    assert_eq!(field(&o.child, "has_console"), "true");
    let obs = o.observer.as_ref().unwrap();
    assert_eq!(field(obs, "attached"), "true", "{obs:?}");
    assert!(
        o.stdout_file.contains(&format!("out:{LINE}")),
        "{:?}",
        o.stdout_file
    );
    assert!(
        o.stderr_pipe.contains(&format!("err:{LINE}")),
        "{:?}",
        o.stderr_pipe
    );
}

#[test]
fn fully_redirected_streams_get_a_detached_child_without_console() {
    let o = run_case("redirected");
    assert_child_did_io(&o);
    assert_eq!(field(&o.parent, "any_console"), "false");
    assert_eq!(
        field(&o.parent, "flags"),
        format!("{:#x}", CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS)
    );
    assert_eq!(field(&o.child, "has_console"), "false");
    assert!(
        o.stdout_file.contains(&format!("out:{LINE}")),
        "{:?}",
        o.stdout_file
    );
    assert!(
        o.stderr_pipe.contains(&format!("err:{LINE}")),
        "{:?}",
        o.stderr_pipe
    );
}

#[test]
fn creation_flags_follow_the_console_rule() {
    assert_eq!(detach_creation_flags(true), CREATE_NEW_PROCESS_GROUP);
    assert_eq!(
        detach_creation_flags(false),
        CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS
    );
}
