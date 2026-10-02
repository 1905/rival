use super::*;
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

#[test]
fn viewer_command_is_the_go_opener() {
    let cmd = viewer_command(Path::new("/tmp/rival-log-x.txt"));
    assert_eq!(cmd.get_program(), VIEWER);
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
    let status = quiet_command("/bin/sh")
        .args(["-c", script])
        .status()
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

/// Copies live for the TTL while the TUI runs and go on exit; no thread is
/// involved.
#[test]
fn log_views_expire_copies_and_clean_up_on_close() {
    let h = harness();
    let mut views = LogViews::default();
    let first = opened(h.env.run(Job::OpenLog(solo_log(&h, "1\n"))));
    let (p1, t0) = (first.path.clone(), first.opened_at);
    views.adopt(first);
    views.sweep(t0 + LOG_VIEW_TTL - Duration::from_secs(1));
    assert!(p1.exists() && views.len() == 1, "removed before its TTL");
    views.sweep(t0 + LOG_VIEW_TTL);
    assert!(!p1.exists() && views.is_empty(), "kept past its TTL");

    let second = opened(h.env.run(Job::OpenLog(solo_log(&h, "2\n"))));
    let p2 = second.path.clone();
    views.adopt(second);
    drop(views);
    assert!(!p2.exists(), "exit left a copy whose launcher was done");
}

/// A test-owned stand-in for a launcher that has not exited: `sh` blocked
/// on its stdin pipe. The `Child` goes to the exit policy, which drops it
/// without waiting; the guard keeps the pipe, so the helper runs until the
/// guard drops. Bind the guard before the `LogViews` so it drops last.
#[cfg(unix)]
fn running_launcher() -> (Child, HelperGuard) {
    let mut child = quiet_command("/bin/sh")
        .args(["-c", "read x"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
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
struct HelperGuard {
    pid: libc::pid_t,
    stdin: Option<std::process::ChildStdin>,
}

#[cfg(unix)]
const HELPER_DEADLINE: Duration = Duration::from_secs(5);

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
fn finished_launcher() -> Child {
    let mut child = quiet_command("/bin/sh")
        .args(["-c", "exit 0"])
        .spawn()
        .unwrap();
    child.wait().unwrap();
    child
}

fn copy(h: &crate::tui::testkit::Harness, name: &str) -> PathBuf {
    let path = h.env.temp_dir.join(name);
    fs::write(&path, "copy\n").unwrap();
    path
}

/// Controller finding 5: quitting before the launcher has read the path
/// keeps the copy; a launcher that is done lets it go. Nothing is waited
/// for or killed.
#[cfg(unix)]
#[test]
fn close_keeps_a_copy_while_its_launcher_runs() {
    let h = harness();
    let (busy, done) = (copy(&h, "busy.txt"), copy(&h, "done.txt"));
    let (launcher, _helper) = running_launcher();
    let mut views = LogViews::default();
    views.adopt(OpenedLog::new(busy.clone(), Some(launcher), Instant::now()));
    views.adopt(OpenedLog::new(
        done.clone(),
        Some(finished_launcher()),
        Instant::now(),
    ));
    views.close();
    assert!(
        busy.exists(),
        "close removed a copy its launcher may still need"
    );
    assert!(!done.exists(), "close kept a copy whose launcher was done");
    assert!(views.is_empty());
}

/// An outcome dropped without being adopted follows the same policy.
#[cfg(unix)]
#[test]
fn a_dropped_outcome_follows_the_exit_policy() {
    let h = harness();
    let busy = copy(&h, "busy.txt");
    let (launcher, _helper) = running_launcher();
    drop(OpenedLog::new(busy.clone(), Some(launcher), Instant::now()));
    assert!(busy.exists());
    let out = opened(h.env.run(Job::OpenLog(solo_log(&h, "x\n"))));
    let path = out.path.clone();
    drop(out);
    assert!(!path.exists(), "a dropped outcome leaked its copy");
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
