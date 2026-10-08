//! Native Windows checks of the overlapped pipe IO, the owner Job and the
//! Windows `LookPath`. Every wait is bounded; a hang fails the test.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::windows::{self, LookEnv, hooks};
use super::{Abort, Io, LookPathError};

const BOUND: Duration = Duration::from_secs(10);

/// Runs `f` on its own thread and returns its result, or panics after
/// [`BOUND`]. A thread-local hook must be set inside `f`.
fn bounded<T: Send + 'static>(name: &str, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name(name.to_string())
        .spawn(move || {
            let _ = tx.send(f());
        })
        .unwrap();
    rx.recv_timeout(BOUND)
        .unwrap_or_else(|_| panic!("{name}: no result after {BOUND:?}"))
}

static HOOK_RUNS: AtomicUsize = AtomicUsize::new(0);

/// The race boundary: the abort fires after the worker checked it and
/// before it issues the IO. A one-shot cancel would miss this IO; the
/// level-triggered abort event must still end it.
fn fire_between_check_and_io(abort: &Abort) {
    HOOK_RUNS.fetch_add(1, Ordering::SeqCst);
    abort.fire();
}

#[test]
fn abort_between_check_and_read_still_ends_the_read() {
    let (reader, writer) = windows::overlapped_pipe(true).unwrap();
    let before = HOOK_RUNS.load(Ordering::SeqCst);
    let (got, elapsed) = bounded("read-boundary", move || {
        hooks::BEFORE_IO.with(|h| h.set(Some(fire_between_check_and_io)));
        let abort = Abort::new().unwrap();
        let mut buf = [0u8; 16];
        let start = Instant::now();
        let got = super::read_some(&reader, &mut buf, &abort);
        hooks::BEFORE_IO.with(|h| h.set(None));
        // The writer stays open, so no EOF can end the read instead.
        drop(writer);
        (got, start.elapsed())
    });
    assert!(HOOK_RUNS.load(Ordering::SeqCst) > before, "hook never ran");
    assert!(matches!(got, Io::Aborted), "{got:?}");
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
}

#[test]
fn abort_between_check_and_write_still_ends_the_write() {
    let (reader, writer) = windows::overlapped_pipe(false).unwrap();
    let (got, then) = bounded("write-boundary", move || {
        hooks::BEFORE_IO.with(|h| h.set(Some(fire_between_check_and_io)));
        let abort = Abort::new().unwrap();
        // Far more than the 4 KiB pipe buffer, with no reader: it pends.
        let big = vec![b'x'; 1 << 20];
        let got = super::write_some(&writer, &big, &abort);
        hooks::BEFORE_IO.with(|h| h.set(None));
        // Every later call sees the abort before any IO.
        let then = super::write_some(&writer, b"y", &abort);
        drop(reader);
        (got, then)
    });
    // A write may also complete in part before the cancel lands.
    assert!(matches!(got, Io::Aborted | Io::Done(_)), "{got:?}");
    assert!(matches!(then, Io::Aborted), "{then:?}");
}

#[test]
fn abort_during_a_pending_read_ends_it_and_keeps_ownership() {
    let (reader, writer) = windows::overlapped_pipe(true).unwrap();
    let abort = std::sync::Arc::new(Abort::new().unwrap());
    let worker_abort = abort.clone();
    let (tx, rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let mut buf = [0u8; 16];
        let got = super::read_some(&reader, &mut buf, &worker_abort);
        let _ = tx.send(());
        (got, reader)
    });
    std::thread::sleep(Duration::from_millis(200));
    assert!(rx.try_recv().is_err(), "the read ended before the abort");
    abort.fire();
    rx.recv_timeout(BOUND).expect("read not ended by the abort");
    // The worker returns only after the cancelled IO completed; joining it
    // proves no IO is left in flight on its buffer.
    let (got, _reader) = worker.join().unwrap();
    assert!(matches!(got, Io::Aborted), "{got:?}");
    drop(writer);
}

#[test]
fn overlapped_pipes_carry_data_and_eof() {
    // Parent reads: data, then EOF once the child end closes.
    let (reader, mut writer) = windows::overlapped_pipe(true).unwrap();
    writer.write_all(b"hello").unwrap();
    drop(writer);
    let abort = Abort::new().unwrap();
    let mut buf = [0u8; 16];
    let mut got = Vec::new();
    loop {
        match super::read_some(&reader, &mut buf, &abort) {
            Io::Done(0) => break,
            Io::Done(n) => got.extend_from_slice(&buf[..n]),
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(got, b"hello");

    // Parent writes: the child end reads it; an empty write is one call.
    let (mut reader, writer) = windows::overlapped_pipe(false).unwrap();
    assert!(matches!(
        super::write_some(&writer, b"", &abort),
        Io::Done(0)
    ));
    assert!(matches!(
        super::write_some(&writer, b"abc", &abort),
        Io::Done(3)
    ));
    drop(writer);
    let mut text = String::new();
    reader.read_to_string(&mut text).unwrap();
    assert_eq!(text, "abc");

    // A write after the reader closed is an error, not a hang.
    let (reader, writer) = windows::overlapped_pipe(false).unwrap();
    drop(reader);
    assert!(matches!(
        super::write_some(&writer, b"z", &abort),
        Io::Err(_)
    ));
}

#[test]
fn owner_job_is_created_once_under_concurrency() {
    let handles: Vec<_> = (0..8)
        .map(|_| std::thread::spawn(windows::ensure_owner_job))
        .collect();
    for h in handles {
        h.join().unwrap().expect("owner job");
    }
    assert!(windows::owner_job_active());
    use windows_sys::Win32::System::JobObjects::IsProcessInJob;
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut in_job = 0;
    // SAFETY: the pseudo handle and NULL (any Job) are valid inputs.
    assert_ne!(
        unsafe { IsProcessInJob(GetCurrentProcess(), std::ptr::null_mut(), &mut in_job) },
        0
    );
    assert_ne!(in_job, 0, "this process is in a Job");
    // Later calls change nothing.
    windows::ensure_owner_job().unwrap();
}

fn touch(path: &Path) {
    std::fs::write(path, b"@echo off\r\n").unwrap();
}

fn look(file: &str, path_env: &str) -> Result<PathBuf, LookPathError> {
    windows::look_path_exts(
        file,
        &windows::path_ext(None),
        Some(std::ffi::OsStr::new(path_env)),
        true,
        windows::same_file,
    )
}

#[test]
fn look_path_follows_windows_rules() {
    let tmp = tempfile::tempdir().unwrap();
    // A directory with a space and a quoted `;` in %PATH%.
    let bin = tmp.path().join("npm bin;x");
    std::fs::create_dir(&bin).unwrap();
    let codex = bin.join("codex.cmd");
    touch(&codex);
    std::fs::create_dir(bin.join("dir.exe")).unwrap();
    let other = tmp.path().join("other");
    std::fs::create_dir(&other).unwrap();
    touch(&other.join("tool.exe"));
    touch(&other.join("tool.cmd"));

    let path_env = format!("{};;\"{}\"", other.display(), bin.display());
    // PATHEXT order: .exe before .cmd; a bare name gets its extension.
    assert_eq!(look("tool", &path_env).unwrap(), other.join("tool.exe"));
    assert_eq!(look("codex", &path_env).unwrap(), codex);
    // A name with an extension resolves as given; case does not matter.
    assert_eq!(look("codex.cmd", &path_env).unwrap(), codex);
    assert_eq!(look("CODEX", &path_env).unwrap(), bin.join("CODEX.cmd"));
    // A directory is never an executable.
    assert_eq!(
        look("dir", &path_env).unwrap_err().err,
        windows::ERR_NOT_FOUND
    );
    let err = look("missing", &path_env).unwrap_err();
    assert_eq!(
        err.to_string(),
        r#"exec: "missing": executable file not found in %PATH%"#
    );

    // A name with a separator skips %PATH%.
    let direct = bin.join("codex");
    let direct_s = direct.to_str().unwrap();
    assert_eq!(look(direct_s, "").unwrap(), codex);
    let missing = bin.join("nope.cmd");
    let missing_s = missing.to_str().unwrap();
    assert_eq!(
        look(missing_s, "").unwrap_err().to_string(),
        format!("exec: {:?}: file does not exist", missing_s)
    );
    let no_ext = bin.join("nope");
    assert_eq!(
        look(no_ext.to_str().unwrap(), "").unwrap_err().err,
        windows::ERR_NOT_FOUND
    );

    // The process-level entry point validates the same way.
    assert_eq!(
        super::look_path("", None).unwrap_err().err,
        windows::ERR_NOT_FOUND
    );

    // lookExtensions: an absolute name gains its extension; one that has
    // a PATHEXT extension is taken as resolved.
    let env = LookEnv {
        path_ext: None,
        no_dot: true,
    };
    assert_eq!(windows::look_extensions(direct_s, "", &env).unwrap(), codex);
    assert_eq!(
        windows::look_extensions(missing_s, "", &env).unwrap(),
        missing,
        "an extension in PATHEXT is not checked"
    );
    // A relative name against cmd.Dir keeps its relative spelling.
    assert_eq!(
        windows::look_extensions(r"npm bin;x\codex", tmp.path().to_str().unwrap(), &env).unwrap(),
        PathBuf::from(r"npm bin;x\codex.cmd")
    );
}

#[test]
fn path_ext_defaults_and_parsing() {
    let ext = |v: &str| windows::path_ext(Some(std::ffi::OsStr::new(v)));
    assert_eq!(windows::path_ext(None), [".com", ".exe", ".bat", ".cmd"]);
    assert_eq!(ext(""), [".com", ".exe", ".bat", ".cmd"]);
    assert_eq!(ext(".EXE;;CMD;.Ps1"), [".exe", ".cmd", ".ps1"]);
    assert!(ext(";;").is_empty());
}

#[test]
fn same_file_compares_identity_not_spelling() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a.exe");
    touch(&a);
    let b = tmp.path().join("b.exe");
    touch(&b);
    let spelled = tmp.path().join(".").join("A.EXE");
    assert!(windows::same_file(&a, &spelled));
    assert!(!windows::same_file(&a, &b));
    assert!(!windows::same_file(&a, &tmp.path().join("missing")));
}

#[test]
fn provider_flags_hide_a_new_console_only_without_one() {
    use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED};
    assert_eq!(windows::provider_creation_flags(true), CREATE_SUSPENDED);
    assert_eq!(
        windows::provider_creation_flags(false),
        CREATE_SUSPENDED | CREATE_NO_WINDOW
    );
}

/// A program for a child in another directory, through the real
/// `GetFullPathNameW` of `std::path::absolute`.
#[test]
fn program_in_dir_uses_the_os_full_path() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let drive = &dir.to_str().unwrap()[..2];
    assert_eq!(
        drive.as_bytes()[1],
        b':',
        "temp dir {dir:?} is not on a drive"
    );
    let p = |s: &str| windows::program_in_dir(Path::new(s), dir).unwrap();
    // Root-relative: the directory's drive.
    assert_eq!(p(r"\tool.exe"), PathBuf::from(format!(r"{drive}\tool.exe")));
    assert_eq!(p("/tool.exe"), PathBuf::from(format!(r"{drive}\tool.exe")));
    // Relative: under the directory.
    assert_eq!(p(r"bin\..\tool.exe"), dir.join("tool.exe"));
    assert_eq!(p(r"..\tool.exe"), dir.parent().unwrap().join("tool.exe"));
    // Drive-relative: the OS resolves it from the drive's own directory of
    // this process, also on the drive of `dir` (was `dir\tool.exe`).
    let same_drive = format!("{drive}tool.exe");
    assert_eq!(p(&same_drive), std::path::absolute(&same_drive).unwrap());
    // Absolute and UNC programs are kept.
    assert_eq!(p(r"Z:\x\tool.exe"), PathBuf::from(r"Z:\x\tool.exe"));
    assert_eq!(p("Z:/x/tool.exe"), PathBuf::from("Z:/x/tool.exe"));
    assert_eq!(p(r"\\srv\share\t.exe"), PathBuf::from(r"\\srv\share\t.exe"));
    // No directory: unchanged.
    assert_eq!(
        windows::program_in_dir(Path::new("rel.exe"), Path::new("")).unwrap(),
        PathBuf::from("rel.exe")
    );
    // An empty program is an invalid input. A bare drive is the drive's own
    // directory of this process (was an invalid input).
    let err = windows::program_in_dir(Path::new(""), dir).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    assert_eq!(err.to_string(), "invalid input parameter");
    assert_eq!(p(drive), std::path::absolute(drive).unwrap());
    // A relative directory is resolved against this process's directory.
    let cwd = std::env::current_dir().unwrap();
    assert_eq!(
        windows::program_in_dir(Path::new("t.exe"), Path::new("sub")).unwrap(),
        cwd.join("sub").join("t.exe")
    );
}

#[test]
fn bare_name_has_no_directory_and_no_drive() {
    for name in ["codex", "codex.cmd", "foo:bar"] {
        assert!(windows::is_bare_name(name), "{name:?}");
    }
    // `.` and `..` have no file name in std (were bare names).
    for name in [
        "",
        ".",
        "..",
        r"x\codex",
        "x/codex",
        r"codex\",
        "C:codex",
        "a:b",
        r"C:\codex",
        r"\\host\share\codex",
    ] {
        assert!(!windows::is_bare_name(name), "{name:?}");
    }
}

#[test]
fn close_file_reports_success() {
    let tmp = tempfile::tempdir().unwrap();
    let f = std::fs::File::create(tmp.path().join("f")).unwrap();
    super::close_file(f).unwrap();
}
