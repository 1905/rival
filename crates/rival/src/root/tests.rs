//! Go: `cmd/root.go` behavior — pre-run order, background joins, exit codes
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
    RootHooks {
        reap: Arc::new(move |_| push(&r, "reap")),
        update_check: Arc::new(move |_| push(&u, "update")),
        detach: Box::new(move || {
            push(&d, "detach");
            detach
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

#[test]
fn tui_suppresses_logging_and_reaps_in_the_background() {
    let fix = Fixture::new();
    let events = recorder();
    let (r, u) = (Arc::clone(&events), Arc::clone(&events));
    let hooks = RootHooks {
        reap: Arc::new(move |_| {
            std::thread::sleep(Duration::from_millis(150));
            push(&r, format!("reap logging={}", logging::is_enabled()));
        }),
        update_check: Arc::new(move |_| push(&u, "update")),
        detach: Box::new(|| DetachOutcome::Continue),
    };
    let r = run_with(&fix, &mut FakeStdin::new(""), &hooks, &events, &["tui"]);
    logging::set_enabled(true);
    // The command (pending here) ran without waiting for the reap, and the
    // root joined the reap before returning.
    assert!(
        r.events.contains(&"reap logging=false".to_string()),
        "{:?}",
        r.events
    );
    assert_eq!(r.code, 1);
    assert_eq!(
        r.stderr,
        "rival tui: not available in this build yet (Task 4.5 of the Rust port)\n"
    );
}

#[test]
fn update_check_wait_is_bounded() {
    let fix = Fixture::new();
    let events = recorder();
    let u = Arc::clone(&events);
    let hooks = RootHooks {
        reap: Arc::new(|_| {}),
        update_check: Arc::new(move |_| {
            std::thread::sleep(Duration::from_secs(3));
            push(&u, "update done");
        }),
        detach: Box::new(|| DetachOutcome::Continue),
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

/// Go `TestWaitForUpdateCheckIsBounded`: a hung check holds the join for
/// its whole budget, and not much longer.
#[test]
fn wait_for_update_check_waits_out_its_budget_for_a_hung_check() {
    // The sender stays alive and never sends: a hung check.
    let (_done, wait) = mpsc::channel::<()>();
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

/// Go `TestWaitForUpdateCheckWithoutStartReturnsImmediately`.
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

#[test]
fn pending_commands_fail_plainly_after_the_pre_run() {
    for (args, path, task) in [
        (&["queue"][..], "rival queue", "Task 3.4"),
        (&["queue", "clear"], "rival queue clear", "Task 3.4"),
        (&["sessions"], "rival sessions", "Task 3.4"),
        (&["update"], "rival update", "Task 3.4"),
    ] {
        let r = run(args);
        assert_eq!(r.code, 1, "{args:?}");
        assert_eq!(
            r.stderr,
            format!("{path}: not available in this build yet ({task} of the Rust port)\n")
        );
        assert_eq!(r.events, ["reap", "update"], "{args:?}");
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
    // Go prints the parse error on stdout for the skill and on stderr from
    // the root.
    assert!(!r.stdout.is_empty());
    assert_eq!(r.stdout, r.stderr);

    let mut stdin = FakeStdin::new("hi");
    let r = run_with(
        &fix,
        &mut stdin,
        &hooks,
        &events,
        &["command", "codex", "--workdir", "/nonexistent-rival"],
    );
    assert_eq!(r.code, 1);
    assert_eq!(r.stdout, "workdir not found: /nonexistent-rival\n");
    assert_eq!(r.stderr, "workdir not found: /nonexistent-rival\n");

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
        Err(e) => rival_core::gostd::os_error_text(&e),
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
    let status = cmd.status().unwrap();
    assert!(status.success(), "helper failed: {status}");
    std::fs::read_to_string(&out).unwrap()
}

/// Go's os.Stdin.Stat fails on a closed fd 0 and the read fails with EBADF;
/// a closed fd 1 fails writes with EBADF. Rust reopened both on /dev/null,
/// where the stat says "character device" and writes succeed.
#[cfg(unix)]
#[test]
fn closed_stdin_and_stdout_fail_like_go() {
    assert_eq!(
        run_fd_helper(&[]),
        "char_device=true\nread=ok:0\nwrite=ok:0",
        "/dev/null stdin is a character device, as in Go"
    );
    assert_eq!(
        run_fd_helper(&[0]),
        "char_device=false\nread=read /dev/stdin: bad file descriptor\nwrite=ok:0"
    );
    assert_eq!(
        run_fd_helper(&[1]),
        "char_device=true\nread=ok:0\nwrite=bad file descriptor"
    );
}
