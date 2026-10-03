use super::*;
#[cfg(unix)]
use crate::tui::testkit::{exiting_launcher, finished_launcher, running_launcher};
use crate::tui::testkit::{harness, launched, set_launch_fails};

fn solo_log(h: &crate::tui::testkit::Harness, body: &str) -> OpenLogRequest {
    OpenLogRequest {
        sessions: vec![Arc::new(Session {
            model: "gpt-6-astra".into(),
            log_file: h.log("raw.log", body),
            ..Session::default()
        })],
        group: false,
    }
}

fn opened(out: JobOutput) -> OpenedLog {
    match out {
        JobOutput::Opened(o) => o,
        other => panic!("want an opened log, got {other:?}"),
    }
}

/// Windows opens through the shell with the path as data: metacharacters,
/// spaces and percent signs arrive unchanged, nothing is interpolated, and
/// no launcher process is kept.
#[cfg(windows)]
#[test]
fn windows_opener_passes_the_path_as_data() {
    use std::cell::RefCell;
    fn decode(p: *const u16) -> String {
        let mut n = 0;
        // SAFETY: the opener passes NUL-terminated strings.
        while unsafe { *p.add(n) } != 0 {
            n += 1;
        }
        // SAFETY: n units were just read.
        String::from_utf16(unsafe { std::slice::from_raw_parts(p, n) }).unwrap()
    }
    let path = Path::new(r"C:\Temp\rival log & (x) ^ %PATH% !.txt");
    let seen = RefCell::new(None);
    let got = shell_open(path, |verb, file| {
        *seen.borrow_mut() = Some((decode(verb), decode(file)));
        42
    });
    assert!(matches!(got, Ok(None)), "{got:?}");
    assert_eq!(
        seen.into_inner(),
        Some(("open".to_string(), path.to_str().unwrap().to_string()))
    );
    // A shell error code (2 = file not found) is a failed launch.
    let err = shell_open(path, |_, _| 2).unwrap_err();
    assert_eq!(err.to_string(), "ShellExecute failed with code 2");
}

#[cfg(unix)]
#[test]
fn viewer_command_is_the_go_opener() {
    let cmd = viewer_command(Path::new("/tmp/rival-log-x.txt"));
    assert_eq!(cmd.get_program(), VIEWER);
    let want = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    assert_eq!(VIEWER, want);
    let args: Vec<_> = cmd.get_args().collect();
    assert_eq!(args, ["/tmp/rival-log-x.txt"]);
}

/// The launcher's stdin, stdout and stderr are all the null device, so it
/// cannot read the TUI's keys or print over the screen. Checked with the
/// same construction on /bin/sh, never the real viewer.
#[cfg(unix)]
#[test]
fn quiet_command_uses_null_streams() {
    let script = "for f in 0 1 2; do [ /dev/fd/$f -ef /dev/null ] || exit 1$f; done";
    let mut cmd = quiet_command("/bin/sh");
    cmd.args(["-c", script]);
    let status = process::spawn(&mut cmd)
        .and_then(|mut child| child.wait())
        .unwrap();
    assert!(status.success(), "a stream is not /dev/null: {status}");
}

#[test]
fn open_log_copies_the_raw_log_and_launches_it() {
    let h = harness();
    let out = opened(
        h.env
            .run(Job::OpenLog(solo_log(&h, "model: gpt-6-astra\n"))),
    );
    assert!(out.path.starts_with(&h.env.temp_dir));
    assert_eq!(
        fs::read_to_string(&out.path).unwrap(),
        "model: gpt-6-astra\n"
    );
    assert_eq!(launched(), std::slice::from_ref(&out.path));
    assert!(!out.has_launcher());
}

#[test]
fn open_log_without_a_log_file_does_nothing() {
    let h = harness();
    let req = OpenLogRequest {
        sessions: vec![Arc::new(Session::default())],
        group: false,
    };
    assert!(matches!(h.env.run(Job::OpenLog(req)), JobOutput::Nothing));
    assert!(launched().is_empty());
}

/// Go `openLogPath`: a copy the viewer cannot be started for is removed.
#[test]
fn a_failed_launch_removes_the_copy() {
    let h = harness();
    set_launch_fails(true);
    let out = h.env.run(Job::OpenLog(solo_log(&h, "x\n")));
    assert!(matches!(out, JobOutput::Nothing));
    let copy = &launched()[0];
    assert!(
        !copy.exists(),
        "{} survived a failed launch",
        copy.display()
    );
    assert_eq!(fs::read_dir(&h.env.temp_dir).unwrap().count(), 0);
}

#[test]
fn group_open_writes_every_member() {
    let h = harness();
    let req = OpenLogRequest {
        sessions: vec![
            Arc::new(Session {
                model: "gpt-5.5".into(),
                log_file: h.log("a.log", "A\n"),
                ..Session::default()
            }),
            Arc::new(Session {
                model: "gpt-6-astra".into(),
                mode: "consilium".into(),
                log_file: h.log("b.log", "B\n"),
                ..Session::default()
            }),
        ],
        group: true,
    };
    let out = opened(h.env.run(Job::OpenLog(req)));
    let text = fs::read_to_string(&out.path).unwrap();
    assert_eq!(
        text,
        "=== gpt-5.5 REVIEW ===\nA\n\n=== gpt-6-astra JUDGE ===\nB\n\n"
    );
}

/// Copies live for the TTL while the TUI runs; no thread is involved. On
/// exit a fresh copy stays even with no launcher (finding 10: the viewer may
/// not have read it), and an expired one goes.
#[test]
fn log_views_expire_copies_and_keep_fresh_ones_on_close() {
    let h = harness();
    let mut views = LogViews::default();
    let first = opened(h.env.run(Job::OpenLog(solo_log(&h, "1\n"))));
    let (p1, t0) = (first.path.clone(), first.opened_at);
    views.adopt(first);
    views.sweep(t0 + LOG_VIEW_TTL - Duration::from_secs(1));
    assert!(p1.exists() && views.len() == 1, "removed before its TTL");
    views.sweep(t0 + LOG_VIEW_TTL);
    assert!(!p1.exists() && views.is_empty(), "kept past its TTL");

    let old = opened(h.env.run(Job::OpenLog(solo_log(&h, "2\n"))));
    let (p2, t2) = (old.path.clone(), old.opened_at);
    views.adopt(old);
    views.close_at(t2 + LOG_VIEW_TTL);
    assert!(!p2.exists(), "exit kept an expired copy");

    let fresh = opened(h.env.run(Job::OpenLog(solo_log(&h, "3\n"))));
    let p3 = fresh.path.clone();
    views.adopt(fresh);
    drop(views);
    assert!(p3.exists(), "exit removed a fresh copy");
}

#[cfg(unix)]
fn copy(h: &crate::tui::testkit::Harness, name: &str) -> PathBuf {
    let path = h.env.temp_dir.join(name);
    fs::write(&path, "copy\n").unwrap();
    path
}

/// Blocks until `pid` has exited, without reaping it.
#[cfg(unix)]
fn wait_exited_unreaped(pid: libc::pid_t) {
    // SAFETY: zeroed siginfo_t is a valid out buffer for waitid.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    // SAFETY: waitid on this test's own child with a valid out pointer;
    // WNOWAIT leaves the child waitable.
    let r = unsafe {
        libc::waitid(
            libc::P_PID,
            libc::id_t::try_from(pid).unwrap(),
            &mut info,
            libc::WEXITED | libc::WNOWAIT,
        )
    };
    assert_eq!(r, 0, "waitid: {}", io::Error::last_os_error());
}

/// Whether `pid` is still an unreaped child of this test.
#[cfg(unix)]
fn unreaped(pid: libc::pid_t) -> bool {
    let mut status = 0;
    // SAFETY: waitpid on this test's own child PID with a valid status
    // pointer; WNOHANG never blocks.
    let r = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
    r != -1
}

/// Controller findings 5 and 10: quitting keeps every fresh copy, whether
/// its launcher still runs or is done (plain `open` returns before the app
/// reads the path). A done launcher is reaped; nothing is waited for or
/// killed.
#[cfg(unix)]
#[test]
fn close_keeps_fresh_copies_and_reaps_done_launchers() {
    let h = harness();
    let (busy, done) = (copy(&h, "busy.txt"), copy(&h, "done.txt"));
    let (launcher, _helper) = running_launcher();
    let finished = exiting_launcher();
    let pid = libc::pid_t::try_from(finished.id()).unwrap();
    wait_exited_unreaped(pid);
    let mut views = LogViews::default();
    views.adopt(OpenedLog::new(busy.clone(), Some(launcher), Instant::now()));
    views.adopt(OpenedLog::new(done.clone(), Some(finished), Instant::now()));
    views.close();
    assert!(
        busy.exists(),
        "close removed a copy its launcher may still need"
    );
    assert!(
        done.exists(),
        "close removed a fresh copy the viewer may not have read"
    );
    assert!(!unreaped(pid), "close left the exited launcher a zombie");
    assert!(views.is_empty());
}

/// An outcome dropped without being adopted follows the same policy: a
/// fresh copy stays, with a running launcher, a done one, or none.
#[cfg(unix)]
#[test]
fn a_dropped_outcome_follows_the_exit_policy() {
    let h = harness();
    let (busy, done) = (copy(&h, "busy.txt"), copy(&h, "done.txt"));
    let (launcher, _helper) = running_launcher();
    drop(OpenedLog::new(busy.clone(), Some(launcher), Instant::now()));
    drop(OpenedLog::new(
        done.clone(),
        Some(finished_launcher()),
        Instant::now(),
    ));
    assert!(busy.exists() && done.exists());
    let out = opened(h.env.run(Job::OpenLog(solo_log(&h, "x\n"))));
    assert!(!out.has_launcher());
    let path = out.path.clone();
    drop(out);
    assert!(path.exists(), "a dropped fresh outcome lost its copy");
}

/// Past the TTL the copy goes even while the launcher runs (Go's timer did
/// the same); the entry stays until the launcher is reaped.
#[cfg(unix)]
#[test]
fn sweep_expires_a_copy_but_holds_a_running_launcher() {
    let h = harness();
    let busy = copy(&h, "busy.txt");
    let t0 = Instant::now();
    let (launcher, _helper) = running_launcher();
    let mut views = LogViews::default();
    views.adopt(OpenedLog::new(busy.clone(), Some(launcher), t0));
    views.sweep(t0 + LOG_VIEW_TTL);
    assert!(!busy.exists());
    assert_eq!(
        views.len(),
        1,
        "the running launcher is still held for reaping"
    );
    assert!(views.views[0].has_launcher());
}
