//! Root command behavior: pre-run order, background joins, exit codes
//! and the plain-stderr error contract. Every hook is a recorder; no real
//! reap, update check, detach, provider or network runs.

use super::*;

use std::sync::Mutex;
use std::time::Instant;

use crate::testutil::{FakeStdin, Fixture, no_mr};

const DEFAULTS: Defaults = Defaults {
    wait_timeout: 95 * 60 * 1_000_000_000,
};

type Events = Arc<Mutex<Vec<String>>>;

fn recorder() -> Events {
    Arc::new(Mutex::new(Vec::new()))
}

fn push(events: &Events, what: impl Into<String>) {
    events.lock().unwrap().push(what.into());
}

fn hooks(events: &Events, detach: DetachOutcome) -> RootHooks {
    let (r, u, d) = (Arc::clone(events), Arc::clone(events), Arc::clone(events));
    let t = Arc::clone(events);
    RootHooks {
        reap: Arc::new(move |_| push(&r, "reap")),
        update_check: Arc::new(move |_, _| push(&u, "update")),
        detach: Box::new(move || {
            push(&d, "detach");
            detach
        }),
        tui: Box::new(move |_| {
            push(&t, "tui");
            Ok(())
        }),
    }
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
    events: Vec<String>,
}

fn run_with(
    fix: &Fixture,
    stdin: &mut FakeStdin,
    hooks: &RootHooks,
    events: &Events,
    args: &[&str],
) -> Run {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let prepare = no_mr();
    let code = {
        let mut env = CmdEnv {
            cfg: &fix.cfg,
            stdin,
            stdout: &mut stdout,
            stderr: &mut stderr,
            live_stdout: None,
            prepare_mr: &*prepare,
            signals: false,
        };
        execute_with_wait(&mut env, hooks, &DEFAULTS, &args, Duration::from_secs(5))
    };
    Run {
        code,
        stdout: String::from_utf8(stdout).unwrap(),
        stderr: String::from_utf8(stderr).unwrap(),
        events: events.lock().unwrap().clone(),
    }
}

fn run(args: &[&str]) -> Run {
    let fix = Fixture::new();
    let events = recorder();
    let hooks = hooks(&events, DetachOutcome::Continue);
    run_with(&fix, &mut FakeStdin::new(""), &hooks, &events, args)
}

// ---- PersistentPreRunE order ----

#[test]
fn pre_run_reaps_then_starts_the_update_check_then_runs() {
    let r = run(&["version"]);
    assert_eq!(r.code, 0);
    assert_eq!(r.events, ["reap", "update"]);
    assert_eq!(r.stdout, format!("{BANNER}  dev\n"));
}

#[test]
fn config_error_stops_before_detach_and_reap() {
    let fix = Fixture::with_config_yaml("efforts:\n  codex: bogus\n");
    let want = fix.cfg.user_config_error().unwrap().to_string();
    let events = recorder();
    let hooks = hooks(&events, DetachOutcome::Exit(0));
    let r = run_with(
        &fix,
        &mut FakeStdin::new(""),
        &hooks,
        &events,
        &["command", "--detach", "codex"],
    );
    assert_eq!(r.code, 1);
    assert_eq!(r.stderr, format!("{want}\n"));
    assert!(r.events.is_empty(), "{:?}", r.events);
    assert_eq!(r.stdout, "");
}

#[test]
fn detach_runs_first_and_its_exit_skips_everything_else() {
    let fix = Fixture::new();
    let events = recorder();
    for (outcome, code) in [(DetachOutcome::Exit(0), 0), (DetachOutcome::Exit(1), 1)] {
        events.lock().unwrap().clear();
        let hooks = hooks(&events, outcome);
        let r = run_with(
            &fix,
            &mut FakeStdin::new(""),
            &hooks,
            &events,
            &["command", "codex", "--detach"],
        );
        assert_eq!(r.code, code);
        assert_eq!(r.events, ["detach"]);
        assert_eq!((r.stdout.as_str(), r.stderr.as_str()), ("", ""));
    }
    // The detached child (Continue) carries on: reap, update, command.
    events.lock().unwrap().clear();
    let hooks = hooks(&events, DetachOutcome::Continue);
    let mut stdin = FakeStdin::new("");
    stdin.char_device = true;
    let r = run_with(
        &fix,
        &mut stdin,
        &hooks,
        &events,
        &["command", "--detach", "codex"],
    );
    assert_eq!(r.code, 0);
    assert_eq!(r.events, ["detach", "reap", "update"]);
    assert_eq!(r.stdout, format!("{}\n", crate::model_specs::CODEX_USAGE));
    // Without the flag nothing detaches.
    events.lock().unwrap().clear();
    let r = run_with(&fix, &mut stdin, &hooks, &events, &["command", "codex"]);
    assert_eq!(r.events, ["reap", "update"]);
}

#[test]
fn parse_errors_and_help_skip_the_pre_run() {
    for (args, code, stdout_has, stderr) in [
        (
            &["command", "codex", "--bogus"][..],
            1,
            "",
            "unknown flag: --bogus\n",
        ),
        (
            &["run", "nope"],
            1,
            "",
            "unknown command \"nope\" for \"rival run\"\n",
        ),
        (&["nope"], 1, "", "unknown command \"nope\" for \"rival\"\n"),
        (&["command", "codex", "-h"], 0, "--workdir", ""),
        (&["completion"], 0, "bash", ""),
    ] {
        let r = run(args);
        assert_eq!(r.code, code, "{args:?}");
        assert_eq!(r.stderr, stderr, "{args:?}");
        assert!(r.stdout.contains(stdout_has), "{args:?}: {}", r.stdout);
        assert!(r.events.is_empty(), "{args:?}: {:?}", r.events);
    }
}

/// `rival tui` turns the process-wide logger off. Tests that run it hold
/// this lock and turn logging back on before they release it, so their
/// background hooks never see another test's flag.
static LOGGING: Mutex<()> = Mutex::new(());

fn run_tui(fix: &Fixture, hooks: &RootHooks, events: &Events) -> Run {
    let _serial = LOGGING.lock().unwrap_or_else(|p| p.into_inner());
    let r = run_with(fix, &mut FakeStdin::new(""), hooks, events, &["tui"]);
    logging::set_enabled(true);
    r
}

#[test]
fn tui_suppresses_logging_and_reaps_in_the_background() {
    let fix = Fixture::new();
    let events = recorder();
    let (r, u, t) = (
        Arc::clone(&events),
        Arc::clone(&events),
        Arc::clone(&events),
    );
    // The reap cannot finish before the TUI has started: it waits for the
    // TUI's signal. A root that reaped before running the TUI would make the
    // reap time out instead.
    let (started_tx, started_rx) = mpsc::channel::<()>();
    let started_rx = Mutex::new(started_rx);
    let hooks = RootHooks {
        reap: Arc::new(move |_| {
            let waited = started_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10));
            let state = if waited.is_ok() {
                "after tui start"
            } else {
                "timed out"
            };
            push(
                &r,
                format!("reap {state} logging={}", logging::is_enabled()),
            );
        }),
        update_check: Arc::new(move |_, _| push(&u, "update")),
        detach: Box::new(|| DetachOutcome::Continue),
        tui: Box::new(move |_| {
            push(&t, format!("tui logging={}", logging::is_enabled()));
            let _ = started_tx.send(());
            Ok(())
        }),
    };
    let r = run_tui(&fix, &hooks, &events);
    // The TUI ran while the reap was still going, with logging already
    // off, and the root joined the reap before returning. The update check
    // is independent and may land anywhere.
    let ordered: Vec<&str> = r
        .events
        .iter()
        .map(String::as_str)
        .filter(|e| *e != "update")
        .collect();
    assert_eq!(
        ordered,
        ["tui logging=false", "reap after tui start logging=false"],
        "{:?}",
        r.events
    );
    assert_eq!((r.code, r.stdout.as_str(), r.stderr.as_str()), (0, "", ""));
}

/// A TUI error is printed as `tui: <err>`, exit 1.
#[test]
fn tui_errors_get_the_tui_prefix() {
    let fix = Fixture::new();
    let events = recorder();
    let mut hooks = hooks(&events, DetachOutcome::Continue);
    hooks.tui = Box::new(|_| Err(crate::tui::runtime::INTERRUPTED.to_string()));
    let r = run_tui(&fix, &hooks, &events);
    assert_eq!(r.code, 1);
    assert_eq!(
        r.stderr,
        "tui: program was killed: program was interrupted\n"
    );
}

/// An update notice that arrives while the TUI owns the screen is held
/// back and printed after the TUI returned (its terminal restored), before
/// the TUI's error, if any. No HTTP: the check is injected.
#[test]
fn an_update_notice_during_the_tui_waits_for_the_restored_terminal() {
    const NOTICE: &str = "\n  Update available: v1.0.0 → v9.9.9 — run 'rival update'\n\n";
    for (result, want_stderr) in [
        (Ok(()), NOTICE.to_string()),
        (
            Err("program was killed: program was interrupted".to_string()),
            format!("{NOTICE}tui: program was killed: program was interrupted\n"),
        ),
    ] {
        let fix = Fixture::new();
        let events = recorder();
        let (u, t) = (Arc::clone(&events), Arc::clone(&events));
        let (printed_tx, printed_rx) = mpsc::channel();
        let printed_rx = Mutex::new(printed_rx);
        let hooks = RootHooks {
            reap: Arc::new(|_| {}),
            update_check: Arc::new(move |_, out| {
                let _ = out.write_all(NOTICE.as_bytes());
                push(&u, "notice written");
                let _ = printed_tx.send(());
            }),
            detach: Box::new(|| DetachOutcome::Continue),
            tui: Box::new(move |_| {
                // The TUI is running: wait until the check has written.
                printed_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(10))
                    .expect("the check never ran");
                push(&t, "tui restored");
                result.clone()
            }),
        };
        let r = run_tui(&fix, &hooks, &events);
        assert_eq!(r.events, ["notice written", "tui restored"]);
        assert_eq!(r.stderr, want_stderr);
        assert_eq!(r.stdout, "");
    }
}

#[test]
fn update_check_wait_is_bounded() {
    let fix = Fixture::new();
    let events = recorder();
    let u = Arc::clone(&events);
    let hooks = RootHooks {
        reap: Arc::new(|_| {}),
        update_check: Arc::new(move |_, _| {
            std::thread::sleep(Duration::from_secs(3));
            push(&u, "update done");
        }),
        detach: Box::new(|| DetachOutcome::Continue),
        tui: Box::new(|_| Ok(())),
    };
    let args = vec!["version".to_string()];
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let prepare = no_mr();
    let mut stdin = FakeStdin::new("");
    let mut env = CmdEnv {
        cfg: &fix.cfg,
        stdin: &mut stdin,
        stdout: &mut stdout,
        stderr: &mut stderr,
        live_stdout: None,
        prepare_mr: &*prepare,
        signals: false,
    };
    let start = Instant::now();
    let code = execute_with_wait(
        &mut env,
        &hooks,
        &DEFAULTS,
        &args,
        Duration::from_millis(100),
    );
    assert_eq!(code, 0);
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "{:?}",
        start.elapsed()
    );
    assert!(
        events.lock().unwrap().is_empty(),
        "returned before the slow check finished"
    );
    assert_eq!(UPDATE_CHECK_WAIT, Duration::from_secs(2));
}

#[test]
fn fast_update_check_finishes_before_exit() {
    let r = run(&["version"]);
    assert!(r.events.contains(&"update".to_string()));
}

/// A hung check holds the join for
/// its whole budget, and not much longer.
#[test]
fn wait_for_update_check_waits_out_its_budget_for_a_hung_check() {
    // The sender stays alive and never sends: a hung check.
    let (_done, wait) = mpsc::channel::<Vec<u8>>();
    let mut bg = Background {
        reap: None,
        update: Some(wait),
    };
    let limit = Duration::from_millis(300);
    let start = Instant::now();
    bg.wait_for_update_check(limit);
    let elapsed = start.elapsed();
    assert!(
        elapsed >= limit,
        "returned after {elapsed:?}, before {limit:?}"
    );
    assert!(elapsed < limit + Duration::from_secs(1), "took {elapsed:?}");
}

/// With no update check started, the wait returns at once.
#[test]
fn wait_for_update_check_without_a_check_returns_at_once() {
    let mut bg = Background::default();
    let start = Instant::now();
    bg.wait_for_update_check(Duration::from_secs(5));
    bg.wait_for_reap();
    assert!(
        start.elapsed() < Duration::from_millis(50),
        "{:?}",
        start.elapsed()
    );
}

// ---- dispatch and exit codes ----

#[test]
fn root_prints_banner_and_usage() {
    let r = run(&[]);
    assert_eq!(r.code, 0);
    assert!(
        r.stdout.starts_with(&format!(
            "{BANNER}  dev — multi-model AI reviews from your terminal\n\n"
        )),
        "{}",
        r.stdout
    );
    for name in ["command", "completion", "help", "wait"] {
        assert!(r.stdout.contains(name), "{}", r.stdout);
    }
    assert_eq!(r.events, ["reap", "update"]);
}

#[test]
fn parents_print_help() {
    for args in [&["command"][..], &["run"]] {
        let r = run(args);
        assert_eq!(r.code, 0);
        assert!(r.stdout.contains("claude"), "{}", r.stdout);
        assert_eq!(
            r.events,
            ["reap", "update"],
            "parents are runnable: the pre-run runs"
        );
    }
}

#[test]
fn wait_errors_use_their_exit_codes() {
    let r = run(&["wait"]);
    assert_eq!(r.code, 64);
    assert_eq!(
        r.stderr,
        "provide --log <file> or one or more session IDs\n"
    );
    let r = run(&["wait", "--poll", "0s", "x"]);
    assert_eq!((r.code, r.stderr.as_str()), (64, "--poll must be > 0\n"));
    let r = run(&["wait", "../x"]);
    assert_eq!(
        (r.code, r.stderr.as_str()),
        (64, "invalid session ID \"../x\" (expected a UUID)\n")
    );
    // An unknown ID fails fast on the first poll.
    let r = run(&[
        "wait",
        "--poll",
        "10ms",
        "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
    ]);
    assert_eq!(r.code, 64);
    assert_eq!(
        r.stdout,
        "no such session (file not found) — check the ID\n"
    );
    assert_eq!(r.stderr, "rival wait: exit 64\n");
}

#[test]
fn wait_summarizes_finished_sessions_from_the_configured_home() {
    let fix = Fixture::new();
    let dir = fix.cfg.paths().sessions_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let id = "0b7c6a43-1111-2222-3333-444455556666";
    std::fs::write(
        dir.join(format!("{id}.json")),
        r#"{"status":"failed","exit_code":2,"duration":"3s","error":"boom"}"#,
    )
    .unwrap();
    let events = recorder();
    let hooks = hooks(&events, DetachOutcome::Continue);
    let r = run_with(
        &fix,
        &mut FakeStdin::new(""),
        &hooks,
        &events,
        &["wait", id, "--poll=10ms"],
    );
    assert_eq!(r.code, 2);
    assert_eq!(r.stdout, "0b7c6a43 failed exit=2 3s — boom\n");
    assert_eq!(r.stderr, "rival wait: exit 2\n");
}

/// The telemetry flush runs only on a normal return: every error exits the
/// process first. A detach parent exits inside the pre-run hook.
#[test]
fn only_a_normal_return_reaches_the_telemetry_flush() {
    let fix = Fixture::new();
    let events = recorder();
    for (args, detach, code, returned) in [
        (&["version"][..], DetachOutcome::Continue, 0, true),
        (&["--help"], DetachOutcome::Continue, 0, true),
        (&["nope"], DetachOutcome::Continue, 1, false),
        (&["wait"], DetachOutcome::Continue, 64, false),
        (
            &["command", "codex", "--detach"],
            DetachOutcome::Exit(0),
            0,
            false,
        ),
    ] {
        let hooks = hooks(&events, detach);
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        let prepare = no_mr();
        let mut stdin = FakeStdin::new("");
        let mut env = CmdEnv {
            cfg: &fix.cfg,
            stdin: &mut stdin,
            stdout: &mut stdout,
            stderr: &mut stderr,
            live_stdout: None,
            prepare_mr: &*prepare,
            signals: false,
        };
        let exit = execute_inner(&mut env, &hooks, &DEFAULTS, &args, Duration::from_secs(5));
        assert_eq!((exit.code, exit.returned), (code, returned), "{args:?}");
    }
}

#[test]
fn help_command_prints_help_or_an_unknown_topic() {
    let r = run(&["help", "command", "codex"]);
    assert_eq!(r.code, 0);
    assert!(r.stdout.contains("--workdir"), "{}", r.stdout);
    assert_eq!(r.events, ["reap", "update"], "help is a runnable command");

    let r = run(&["help", "nope", "x"]);
    assert_eq!(r.code, 0);
    assert_eq!(r.stdout, "");
    assert!(
        r.stderr.starts_with("Unknown help topic [`nope` `x`]\n"),
        "{}",
        r.stderr
    );
    assert!(
        r.stderr.contains("completion"),
        "the root usage follows on stderr"
    );
}

/// `--no-descriptions` drops the command and flag descriptions; the
/// commands and flags stay.
#[test]
fn completion_no_descriptions_drops_descriptions() {
    for shell in ["zsh", "fish", "powershell"] {
        let with = run(&["completion", shell]);
        let without = run(&["completion", shell, "--no-descriptions"]);
        assert_eq!((with.code, without.code), (0, 0), "{shell}");
        for text in ["Skill-facing Codex executor", "working directory"] {
            assert!(with.stdout.contains(text), "{shell}: descriptions missing");
            assert!(
                !without.stdout.contains(text),
                "{shell}: {text} still present"
            );
        }
        for word in ["codex", "workdir", "detach"] {
            assert!(without.stdout.contains(word), "{shell}: lost {word}");
        }
    }
}

#[test]
fn completion_prints_a_script() {
    for shell in ["bash", "zsh", "fish", "powershell"] {
        let r = run(&["completion", shell, "--no-descriptions"]);
        assert_eq!(r.code, 0, "{shell}");
        assert!(r.stdout.contains("rival"), "{shell}");
        assert!(r.stdout.contains("codex"), "{shell}");
    }
}

#[test]
fn model_command_errors_reach_stderr_with_their_code() {
    let fix = Fixture::new();
    let events = recorder();
    let hooks = hooks(&events, DetachOutcome::Continue);
    let mut stdin = FakeStdin::new("-re bogus hi");
    let r = run_with(
        &fix,
        &mut stdin,
        &hooks,
        &events,
        &["command", "codex", "--no-queue"],
    );
    assert_eq!(r.code, 1);
    // The parse error is printed on stdout for the skill and on stderr from
    // the root.
    assert!(!r.stdout.is_empty());
    assert_eq!(r.stdout, r.stderr);

    let mut stdin = FakeStdin::new("hi");
    let missing = fix.missing_dir("nonexistent-rival");
    let r = run_with(
        &fix,
        &mut stdin,
        &hooks,
        &events,
        &["command", "codex", "--workdir", &missing],
    );
    let want = format!("workdir not found: {missing}\n");
    assert_eq!(r.code, 1);
    assert_eq!(r.stdout, want);
    assert_eq!(r.stderr, want);

    let mut stdin = FakeStdin::new("hi");
    let r = run_with(
        &fix,
        &mut stdin,
        &hooks,
        &events,
        &["run", "claude", "--effort", "nope", "--prompt-stdin"],
    );
    assert_eq!(r.code, 1);
    assert_eq!(r.stderr, "invalid effort \"nope\" for claude\n");
    assert_eq!(stdin.reads, 0);
}

#[test]
fn cmd_error_constructors() {
    assert_eq!(
        CmdError::plain("x"),
        CmdError {
            code: 1,
            message: "x".into()
        }
    );
    assert_eq!(CmdError::exit(64, "y").to_string(), "y");
}

// ---- standard descriptors closed at startup ----

/// Set on the helper child only. Names the report file.
#[cfg(unix)]
const FD_HELPER_OUT: &str = "RIVAL_ROOT_FD_HELPER_OUT";
#[cfg(unix)]
const FD_HELPER_TEST: &str = "root::tests::std_fd_helper_child";

/// Runs only inside the helper process: reports what the process stdin and
/// stdout adapters do.
#[cfg(unix)]
#[test]
#[ignore = "helper process for the closed-descriptor tests"]
fn std_fd_helper_child() {
    let Some(out) = std::env::var_os(FD_HELPER_OUT) else {
        return;
    };
    let mut stdin = ProcessStdin;
    let char_device = stdin.is_char_device();
    let read = match stdin.read_all() {
        Ok(data) => format!("ok:{}", data.len()),
        Err(e) => e,
    };
    let write = match ProcessStdout.write(b"") {
        Ok(n) => format!("ok:{n}"),
        Err(e) => e.to_string(),
    };
    std::fs::write(
        &out,
        format!("char_device={char_device}\nread={read}\nwrite={write}"),
    )
    .unwrap();
    // SAFETY: ends the helper without libtest's summary.
    unsafe { libc::_exit(0) };
}

#[cfg(unix)]
fn run_fd_helper(close: &'static [i32]) -> String {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("report");
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", FD_HELPER_TEST, "--ignored", "--quiet"])
        .env(FD_HELPER_OUT, &out)
        .env("HOME", dir.path())
        .env("RIVAL_HOME", dir.path().join(".rival"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: close is async-signal-safe; these are the child's copies.
    unsafe {
        cmd.pre_exec(move || {
            for &fd in close {
                libc::close(fd);
            }
            Ok(())
        });
    }
    let status = rival_core::executor::process::spawn(&mut cmd)
        .and_then(|mut child| child.wait())
        .unwrap();
    assert!(status.success(), "helper failed: {status}");
    std::fs::read_to_string(&out).unwrap()
}

/// A closed fd 0 fails the stdin stat and the read with EBADF; a closed fd 1
/// fails writes with EBADF. std would reopen both on /dev/null,
/// where the stat says "character device" and writes succeed.
#[cfg(unix)]
#[test]
fn closed_stdin_and_stdout_fail_with_ebadf() {
    assert_eq!(
        run_fd_helper(&[]),
        "char_device=true\nread=ok:0\nwrite=ok:0",
        "/dev/null stdin is a character device"
    );
    assert_eq!(
        run_fd_helper(&[0]),
        "char_device=false\nread=read /dev/stdin: Bad file descriptor (os error 9)\nwrite=ok:0"
    );
    assert_eq!(
        run_fd_helper(&[1]),
        "char_device=true\nread=ok:0\nwrite=Bad file descriptor (os error 9)"
    );
}

// ---- Windows standard handles ----

#[cfg(windows)]
const WIN_STDIN_OUT: &str = "RIVAL_ROOT_WIN_STDIN_OUT";
#[cfg(windows)]
const WIN_STDIN_MODE: &str = "RIVAL_ROOT_WIN_STDIN_MODE";
#[cfg(windows)]
const WIN_STDIN_TEST: &str = "root::tests::win_stdin_helper_child";

/// Runs only inside the helper process: reports what the process stdin and
/// stdout adapters see. Modes `invalid`/`nullhandle` first set the standard
/// input slot to INVALID_HANDLE_VALUE (no stdin at all) or NULL (a
/// `File` on handle 0); `out-invalid`/`out-nullhandle` do that to stdout.
#[cfg(windows)]
#[test]
#[ignore = "helper process for the Windows stdin tests"]
fn win_stdin_helper_child() {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle};
    let Some(out) = std::env::var_os(WIN_STDIN_OUT) else {
        return;
    };
    let set = |which, handle| {
        // SAFETY: changes only this helper's standard handle slot.
        unsafe { SetStdHandle(which, handle) };
    };
    match std::env::var(WIN_STDIN_MODE).as_deref() {
        Ok("invalid") => set(STD_INPUT_HANDLE, INVALID_HANDLE_VALUE),
        Ok("nullhandle") => set(STD_INPUT_HANDLE, std::ptr::null_mut()),
        Ok("out-invalid") => set(STD_OUTPUT_HANDLE, INVALID_HANDLE_VALUE),
        Ok("out-nullhandle") => set(STD_OUTPUT_HANDLE, std::ptr::null_mut()),
        _ => {}
    }
    let mut stdin = ProcessStdin;
    let char_device = stdin.is_char_device();
    let stat_failed = stdin.stat_failed();
    let read = match stdin.read_all() {
        Ok(data) => format!("ok:{}", String::from_utf8_lossy(&data)),
        Err(e) => e,
    };
    let write = match ProcessStdout.write(b"x") {
        Ok(n) => format!("ok:{n}"),
        Err(e) => crate::command_plan::write_stdout_error(&e).message,
    };
    std::fs::write(
        &out,
        format!("char_device={char_device}\nstat_failed={stat_failed}\nread={read}\nwrite={write}"),
    )
    .unwrap();
    std::process::exit(0);
}

#[cfg(windows)]
fn run_win_stdin_helper(mode: &str) -> String {
    use std::process::{Command, Stdio};
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("report");
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", WIN_STDIN_TEST, "--ignored", "--quiet"])
        .env(WIN_STDIN_OUT, &out)
        .env(WIN_STDIN_MODE, mode)
        .env("HOME", dir.path())
        .env("USERPROFILE", dir.path())
        .env("RIVAL_HOME", dir.path().join(".rival"))
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut file_input = None;
    match mode {
        "file" => {
            let input = dir.path().join("in.txt");
            std::fs::write(&input, "from file").unwrap();
            cmd.stdin(Stdio::from(std::fs::File::open(&input).unwrap()));
        }
        "pipe" => {
            let (reader, writer) = rival_core::executor::process::pipe().unwrap();
            cmd.stdin(Stdio::from(reader));
            file_input = Some(writer);
        }
        _ => {
            cmd.stdin(Stdio::null());
        }
    }
    /// Owns the helper from its spawn: dropping it unexited kills it and
    /// waits a bounded time, so no assertion leaves it running.
    struct Guard(std::process::Child);
    impl Guard {
        fn wait_for(&mut self, timeout: std::time::Duration) -> Option<std::process::ExitStatus> {
            let deadline = std::time::Instant::now() + timeout;
            loop {
                if let Ok(Some(status)) = self.0.try_wait() {
                    return Some(status);
                }
                if std::time::Instant::now() >= deadline {
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            if matches!(self.0.try_wait(), Ok(None)) {
                let _ = self.0.kill();
                if self.wait_for(std::time::Duration::from_secs(10)).is_none() {
                    eprintln!("cleanup: stdin helper {} still running", self.0.id());
                }
            }
        }
    }
    let mut child = Guard(rival_core::executor::process::spawn(&mut cmd).unwrap());
    drop(cmd);
    if let Some(mut writer) = file_input {
        writer.write_all(b"from pipe").unwrap();
    }
    let status = child
        .wait_for(std::time::Duration::from_secs(30))
        .unwrap_or_else(|| panic!("stdin helper ({mode}) did not exit in 30s"));
    assert!(status.success(), "helper failed: {status}");
    std::fs::read_to_string(&out).unwrap()
}

/// Windows: `NUL` is a character device (so it reads as no piped input), a
/// pipe and a file are not. An INVALID_HANDLE_VALUE stdin means no stdin:
/// `Stat` fails and the read is the bare `invalid argument`.
/// A NULL stdin is a `File` on handle 0: `Stat` (GetFileType) fails and the
/// read fails with ERROR_INVALID_HANDLE. std would read EOF from both.
#[cfg(windows)]
#[test]
fn windows_stdin_kinds_and_missing_handles() {
    let ok_write = "write=ok:1";
    assert_eq!(
        run_win_stdin_helper("null"),
        format!("char_device=true\nstat_failed=false\nread=ok:\n{ok_write}")
    );
    assert_eq!(
        run_win_stdin_helper("pipe"),
        format!("char_device=false\nstat_failed=false\nread=ok:from pipe\n{ok_write}")
    );
    assert_eq!(
        run_win_stdin_helper("file"),
        format!("char_device=false\nstat_failed=false\nread=ok:from file\n{ok_write}")
    );
    assert_eq!(
        run_win_stdin_helper("invalid"),
        format!("char_device=false\nstat_failed=true\nread=invalid argument\n{ok_write}")
    );
    let invalid_handle = std::io::Error::from_raw_os_error(
        windows_sys::Win32::Foundation::ERROR_INVALID_HANDLE as i32,
    )
    .to_string();
    assert_eq!(
        run_win_stdin_helper("nullhandle"),
        format!(
            "char_device=false\nstat_failed=true\nread=read /dev/stdin: {invalid_handle}\n{ok_write}"
        )
    );
}

/// Windows: an INVALID_HANDLE_VALUE stdout means no stdout, and a write
/// fails with the bare `invalid argument`; a NULL stdout is a `File` whose
/// WriteFile fails with ERROR_INVALID_HANDLE. std would report success
/// for both.
#[cfg(windows)]
#[test]
fn windows_stdout_missing_handles_fail_writes() {
    let head = "char_device=true\nstat_failed=false\nread=ok:";
    assert_eq!(
        run_win_stdin_helper("out-invalid"),
        format!("{head}\nwrite=write stdout: invalid argument")
    );
    let invalid_handle = std::io::Error::from_raw_os_error(
        windows_sys::Win32::Foundation::ERROR_INVALID_HANDLE as i32,
    )
    .to_string();
    assert_eq!(
        run_win_stdin_helper("out-nullhandle"),
        format!("{head}\nwrite=write stdout: write /dev/stdout: {invalid_handle}")
    );
}
