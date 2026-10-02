//! Go: `cmd/review_output_test.go` (command surface), `cmd/mr_guard_test.go`,
//! plus failure-path and ordering cases for `runModelCommand`.

use super::*;

use std::path::Path;

use crate::model_specs::CODEX_USAGE;
use crate::testutil::{
    FakeStdin, Fixture, TEST_MR_URL, TRANSCRIPT_MARKER, fake_mr, fake_run, fake_spec,
    json_answer_log, no_mr, run_command_with, with_env,
};

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

// ---- Go TestCommandReviewPrintsFormattedReview ----

#[test]
fn command_review_prints_formatted_review() {
    let fix = Fixture::new();
    let wd = tmp();
    let f = fake_run(&json_answer_log());
    let out = run_command_with(&fix, &f, "review src/", s(wd.path()), &*no_mr());
    out.result.unwrap();
    let f = f.borrow();
    assert!(f.review, "provider was not told it is a review");
    assert!(
        f.prompt.contains("## Role: Implementation Bug Hunter")
            && f.prompt.contains("Review scope: src/"),
        "provider did not get the bug-hunter review prompt:\n{:.300}",
        f.prompt
    );
    for want in [
        "═══ RIVAL REVIEW ═══",
        "Model: codex (gpt-6-astra)",
        "Scope: src/",
        "1. [high] nil deref — a.go:3",
        "\nLog: ",
    ] {
        assert!(
            out.stdout.contains(want),
            "stdout missing {want:?}:\n{}",
            out.stdout
        );
    }
    assert!(
        !out.stdout.contains(TRANSCRIPT_MARKER),
        "the transcript was printed:\n{}",
        out.stdout
    );
    let sessions = fix.sessions();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].status, "completed");
    assert_eq!(sessions[0].mode, "review");
    assert_eq!(sessions[0].review_scope, "src/");
    assert_eq!(sessions[0].effort, "xhigh");
    assert_eq!(f.status_during_run, "running");
}

// ---- Go TestCommandReviewProseFallsBackToLog ----

#[test]
fn command_review_prose_falls_back_to_log() {
    let fix = Fixture::new();
    let wd = tmp();
    let f = fake_run("I looked around and everything seems fine to me.\n");
    let out = run_command_with(&fix, &f, "review src/", s(wd.path()), &*no_mr());
    out.result.unwrap();
    for want in [
        "RIVAL REVIEW — UNPARSED OUTPUT",
        "everything seems fine to me",
        "\nLog: ",
    ] {
        assert!(
            out.stdout.contains(want),
            "stdout missing {want:?}:\n{}",
            out.stdout
        );
    }
}

// ---- Go TestCommandRawPromptPrintsLog ----

#[test]
fn command_raw_prompt_prints_log() {
    let fix = Fixture::new();
    let wd = tmp();
    let f = fake_run("the auth flow works like this\n");
    let out = run_command_with(&fix, &f, "explain the auth flow", s(wd.path()), &*no_mr());
    out.result.unwrap();
    let f = f.borrow();
    assert!(!f.review);
    assert_eq!(f.prompt, "explain the auth flow");
    assert_eq!(
        out.stdout, "the auth flow works like this\n",
        "raw output must be the log verbatim"
    );
    assert!(!f.had_mirror, "command mode has no stdout mirror");
    let sessions = fix.sessions();
    assert_eq!(sessions[0].status, "completed");
    assert_eq!(sessions[0].mode, "raw");
    // The path as given (cleaned, symlinks kept), like Go's filepath.Abs.
    assert_eq!(sessions[0].work_dir, s(wd.path()));
}

// ---- Go TestCommandReviewFailureKeepsLogAndExitCode ----

#[test]
fn command_review_failure_keeps_log_and_exit_code() {
    let fix = Fixture::new();
    let wd = tmp();
    let f = fake_run(&json_answer_log());
    f.borrow_mut().exit_code = 2;
    let out = run_command_with(&fix, &f, "review src/", s(wd.path()), &*no_mr());
    assert_eq!(
        out.result,
        Err(CmdError::exit(2, "codex exited with code 2"))
    );
    assert!(
        out.stdout.contains(TRANSCRIPT_MARKER) && !out.stdout.contains("RIVAL REVIEW"),
        "failed run must print the log, not a formatted review:\n{}",
        out.stdout
    );
    let sess = &fix.sessions()[0];
    assert_eq!(sess.status, "failed");
    assert_eq!(sess.exit_code, Some(2));
    assert_eq!(sess.error_msg, "codex exited with code 2");
}

// ---- Go TestCommandMRReviewRunsInSnapshot ----

#[test]
fn command_mr_review_runs_in_snapshot() {
    let fix = Fixture::new();
    let mr = fake_mr();
    let caller = tmp();
    let f = fake_run(&json_answer_log());
    let out = run_command_with(
        &fix,
        &f,
        &format!("review {TEST_MR_URL}"),
        s(caller.path()),
        &*mr.resolver,
    );
    out.result.unwrap();
    assert_eq!(mr.calls.get(), 1, "MR resolver calls");
    let f = f.borrow();
    assert_eq!(f.workdir, s(&mr.snapshot_dir), "run workdir");
    assert!(f.workdir_existed, "the snapshot was gone during the run");
    assert_eq!(f.cred_workdir, s(caller.path()), "credential workdir");
    assert!(
        f.prompt.contains("Review scope: PINNED-SNAPSHOT-SCOPE"),
        "prompt does not carry the snapshot scope:\n{:.300}",
        f.prompt
    );
    assert!(
        out.stdout
            .starts_with(&format!("GitLab MR: {TEST_MR_URL}\n")),
        "identity line is not first:\n{}",
        out.stdout
    );
    assert!(
        out.stdout.contains(&format!("Scope: {TEST_MR_URL}\n")),
        "formatted review should show the MR URL as scope:\n{}",
        out.stdout
    );
    assert!(
        !mr.snapshot_dir.exists(),
        "MR checkout was not closed after the run"
    );
    let sess = &fix.sessions()[0];
    assert_eq!(sess.work_dir, s(&mr.snapshot_dir));
    assert_eq!(sess.review_scope, TEST_MR_URL);
}

// ---- Go TestCommandPlainScopeSkipsMRResolver ----

#[test]
fn command_plain_scope_skips_mr_resolver() {
    let fix = Fixture::new();
    let mr = fake_mr();
    let caller = tmp();
    let f = fake_run(&json_answer_log());
    let out = run_command_with(&fix, &f, "review src/", s(caller.path()), &*mr.resolver);
    out.result.unwrap();
    let f = f.borrow();
    assert_eq!(mr.calls.get(), 0);
    assert_eq!(f.workdir, s(caller.path()));
    assert_eq!(f.cred_workdir, s(caller.path()));
}

// ---- Go TestModelCommandRejectsMRInRawPrompt ----

/// A raw prompt cannot pin an MR checkout, so an MR URL in one is rejected
/// before any reviewer starts, with a pointer to the review path that can.
#[test]
fn model_command_rejects_mr_in_raw_prompt() {
    let fix = Fixture::new();
    let mr = fake_mr();
    let f = fake_run("");
    let wd = tmp();
    let out = run_command_with(
        &fix,
        &f,
        &format!("сделай ревью МР {TEST_MR_URL}"),
        s(wd.path()),
        &*mr.resolver,
    );
    assert!(
        !f.borrow().called && mr.calls.get() == 0,
        "reviewer or MR resolver reached"
    );
    let err = out.result.unwrap_err();
    assert_eq!(err.code, 1);
    assert!(
        err.message.contains("no reviewer was started"),
        "{}",
        err.message
    );
    assert!(
        err.message.contains("rival command codex review <MR-URL>"),
        "{}",
        err.message
    );
    assert_eq!(f.borrow().preflight_calls, 0, "rejected before preflight");
    assert!(fix.sessions().is_empty());
    assert_eq!(out.stdout, "");
}

// ---- Go TestCommandReviewQuotaOrEmptyFailsTheRun ----

/// A zero exit is not a review: a run that only hit a quota or wrote
/// nothing fails.
#[test]
fn command_review_quota_or_empty_fails_the_run() {
    for (name, log, reason) in [
        (
            "quota",
            "ERROR: insufficient_quota: You exceeded your current quota\n",
            "codex hit provider quota/rate limit (429)",
        ),
        (
            "empty",
            "",
            "codex produced no output (empty result); likely an auth/session failure",
        ),
    ] {
        let fix = Fixture::new();
        let wd = tmp();
        let f = fake_run(log);
        let out = run_command_with(&fix, &f, "review src/", s(wd.path()), &*no_mr());
        assert_eq!(out.result, Err(CmdError::exit(1, reason)), "{name}");
        assert!(
            !out.stdout.contains("═══ RIVAL REVIEW ═══"),
            "{name}: a failed run printed a review"
        );
        // The log is still printed, as Go does.
        assert_eq!(out.stdout, log, "{name}");
        let sess = &fix.sessions()[0];
        assert_eq!(sess.status, "failed", "{name}");
        assert_eq!(sess.error_msg, reason, "{name}");
    }
}

// ---- runModelCommand order and failure paths ----

#[test]
fn terminal_stdin_prints_usage_without_reading() {
    let fix = Fixture::new();
    let f = fake_run("");
    let spec = fake_spec(&f);
    let mut stdin = FakeStdin::new("");
    stdin.char_device = true;
    stdin.forbid_read = true;
    let out = with_env(&fix, &mut stdin, &*no_mr(), |env| {
        run_model_command(env, &spec, "/nonexistent", true)
    });
    out.result.unwrap();
    assert_eq!(out.stdout, format!("{CODEX_USAGE}\n"));
    assert!(fix.sessions().is_empty());
}

#[test]
fn empty_input_prints_usage() {
    let fix = Fixture::new();
    let f = fake_run("");
    for input in ["", "   \n"] {
        let out = run_command_with(&fix, &f, input, "/nonexistent", &*no_mr());
        out.result.unwrap();
        assert_eq!(out.stdout, format!("{CODEX_USAGE}\n"), "{input:?}");
    }
    assert!(!f.borrow().called);
}

#[test]
fn stdin_read_error_is_plain() {
    let fix = Fixture::new();
    let f = fake_run("");
    let spec = fake_spec(&f);
    let mut stdin = FakeStdin::new("");
    stdin.read_error = Some("read /dev/stdin: bad file descriptor".into());
    let out = with_env(&fix, &mut stdin, &*no_mr(), |env| {
        run_model_command(env, &spec, ".", true)
    });
    assert_eq!(
        out.result,
        Err(CmdError::plain(
            "read stdin: read /dev/stdin: bad file descriptor"
        ))
    );
    assert_eq!(out.stdout, "");
}

#[test]
fn parse_error_prints_to_stdout_and_exits_one() {
    let fix = Fixture::new();
    let f = fake_run("");
    let out = run_command_with(&fix, &f, "-re bogus review", "/nonexistent", &*no_mr());
    let err = out.result.unwrap_err();
    assert_eq!(err.code, 1);
    assert_eq!(out.stdout, format!("{}\n", err.message));
    assert!(err.message.contains("bogus"), "{}", err.message);
    assert!(fix.sessions().is_empty());
}

#[test]
fn workdir_then_preflight_order() {
    let fix = Fixture::new();
    let f = fake_run("");
    // A bad workdir is printed on stdout, exits 1, and skips preflight.
    let out = run_command_with(&fix, &f, "-re ultra hi", "/nonexistent-dir", &*no_mr());
    assert_eq!(
        out.result,
        Err(CmdError::exit(1, "workdir not found: /nonexistent-dir"))
    );
    assert_eq!(out.stdout, "workdir not found: /nonexistent-dir\n");
    assert_eq!(f.borrow().preflight_calls, 0);

    // A failing preflight is a plain error and creates no session.
    let mut spec = fake_spec(&f);
    spec.preflight = Box::new(|_, _| anyhow::bail!("codex runtime is not installed"));
    let wd = tmp();
    let mut stdin = FakeStdin::new("hi");
    let out = with_env(&fix, &mut stdin, &*no_mr(), |env| {
        run_model_command(env, &spec, s(wd.path()), true)
    });
    assert_eq!(
        out.result,
        Err(CmdError::plain("codex runtime is not installed"))
    );
    assert!(fix.sessions().is_empty());
}

#[test]
fn preflight_gets_the_caller_workdir_for_an_mr_review() {
    let fix = Fixture::new();
    let mr = fake_mr();
    let caller = tmp();
    let f = fake_run(&json_answer_log());
    let mut spec = fake_spec(&f);
    let seen = std::rc::Rc::new(std::cell::RefCell::new(String::new()));
    let seen2 = std::rc::Rc::clone(&seen);
    spec.preflight = Box::new(move |_, wd| {
        *seen2.borrow_mut() = wd.to_string();
        Ok(())
    });
    let mut stdin = FakeStdin::new(&format!("review {TEST_MR_URL}"));
    let out = with_env(&fix, &mut stdin, &*mr.resolver, |env| {
        run_model_command(env, &spec, s(caller.path()), true)
    });
    out.result.unwrap();
    assert_eq!(*seen.borrow(), s(caller.path()));
}

#[test]
fn mr_checkout_is_removed_when_the_run_fails() {
    for case in ["run error", "nonzero exit", "quota"] {
        let fix = Fixture::new();
        let mr = fake_mr();
        let caller = tmp();
        let f = fake_run(&json_answer_log());
        match case {
            "run error" => f.borrow_mut().error = Some("boom".into()),
            "nonzero exit" => f.borrow_mut().exit_code = 3,
            _ => f.borrow_mut().log = "ERROR: insufficient_quota\n".into(),
        }
        let out = run_command_with(
            &fix,
            &f,
            &format!("review {TEST_MR_URL}"),
            s(caller.path()),
            &*mr.resolver,
        );
        assert!(out.result.is_err(), "{case}");
        assert!(
            f.borrow().workdir_existed,
            "{case}: snapshot gone during the run"
        );
        assert!(!mr.snapshot_dir.exists(), "{case}: MR checkout left behind");
        assert!(
            out.stdout
                .starts_with(&format!("GitLab MR: {TEST_MR_URL}\n\n")),
            "{case}: identity first:\n{}",
            out.stdout
        );
        assert_eq!(fix.sessions()[0].status, "failed", "{case}");
    }
}

#[test]
fn mr_resolver_error_starts_nothing() {
    let fix = Fixture::new();
    let f = fake_run("");
    let resolver: Box<crate::root::PrepareMr> =
        Box::new(|_, _, _, _| Err("glab: merge request not found".to_string()));
    let wd = tmp();
    let out = run_command_with(
        &fix,
        &f,
        &format!("review {TEST_MR_URL}"),
        s(wd.path()),
        &*resolver,
    );
    assert_eq!(
        out.result,
        Err(CmdError::plain("glab: merge request not found"))
    );
    assert!(!f.borrow().called);
    assert!(fix.sessions().is_empty());
    assert_eq!(out.stdout, "");
}

#[test]
fn run_error_fails_the_session_with_the_error() {
    let fix = Fixture::new();
    let wd = tmp();
    let f = fake_run("");
    f.borrow_mut().error = Some("codex runtime: start failed".into());
    let out = run_command_with(&fix, &f, "hi", s(wd.path()), &*no_mr());
    assert_eq!(
        out.result,
        Err(CmdError::plain("codex runtime: start failed"))
    );
    let sess = &fix.sessions()[0];
    assert_eq!(sess.status, "failed");
    assert_eq!(sess.exit_code, Some(1));
    assert_eq!(sess.error_msg, "codex runtime: start failed");
}

/// The run context carries RIVAL_RUN_TIMEOUT; a run that hits it records
/// the timeout reason.
#[test]
fn run_timeout_is_reported_on_the_session() {
    let fix = Fixture::with(&[("RIVAL_RUN_TIMEOUT", "20ms")], None);
    let wd = tmp();
    let f = fake_run("");
    let mut spec = fake_spec(&f);
    spec.run = Box::new(|c| {
        c.ctx.wait_timeout(std::time::Duration::from_secs(10));
        anyhow::bail!("{}", c.ctx.err().map(|e| e.to_string()).unwrap_or_default())
    });
    let mut stdin = FakeStdin::new("hi");
    let out = with_env(&fix, &mut stdin, &*no_mr(), |env| {
        run_model_command(env, &spec, s(wd.path()), true)
    });
    assert_eq!(
        out.result,
        Err(CmdError::plain("context deadline exceeded"))
    );
    assert_eq!(
        fix.sessions()[0].error_msg,
        "codex run timeout after 20ms (RIVAL_RUN_TIMEOUT) — model did not finish"
    );
}

/// A panic inside the provider still leaves no session running (Go's
/// deferred interrupted check) and removes the MR checkout.
#[test]
fn panic_marks_the_session_interrupted_and_cleans_up() {
    let fix = Fixture::new();
    let mr = fake_mr();
    let caller = tmp();
    let f = fake_run("");
    let mut spec = fake_spec(&f);
    spec.run = Box::new(|_| panic!("provider exploded"));
    let mut stdin = FakeStdin::new(&format!("review {TEST_MR_URL}"));
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        with_env(&fix, &mut stdin, &*mr.resolver, |env| {
            run_model_command(env, &spec, s(caller.path()), true)
        })
    }));
    assert!(caught.is_err());
    let sess = &fix.sessions()[0];
    assert_eq!(sess.status, "failed");
    assert_eq!(sess.error_msg, "interrupted");
    assert!(!mr.snapshot_dir.exists());
}

#[test]
fn claude_records_the_configured_subscription() {
    let fix = Fixture::with_config_yaml("claude:\n  subscription: team\n");
    let f = fake_run("ok\n");
    let mut spec = fake_spec(&f);
    spec.command_name = rival_core::config::CLAUDE_LABEL;
    spec.cli = "claude";
    spec.model = rival_core::config::CLAUDE_MODEL;
    let wd = tmp();
    let mut stdin = FakeStdin::new("hi");
    let out = with_env(&fix, &mut stdin, &*no_mr(), |env| {
        run_model_command(env, &spec, s(wd.path()), true)
    });
    out.result.unwrap();
    let sess = &fix.sessions()[0];
    assert_eq!(sess.account, "team");
    assert_eq!(sess.effort, "medium");

    // Codex never records an account.
    let fix = Fixture::with_config_yaml("claude:\n  subscription: team\n");
    let out = run_command_with(&fix, &fake_run("ok\n"), "hi", s(wd.path()), &*no_mr());
    out.result.unwrap();
    assert_eq!(fix.sessions()[0].account, "");
}

/// Go's final stdout write error is reported (a closed fd 1 gives EBADF);
/// the session is already complete.
#[test]
fn stdout_write_error_is_reported() {
    struct Closed;
    impl std::io::Write for Closed {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::from_raw_os_error(libc::EBADF))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let fix = Fixture::new();
    let wd = tmp();
    let f = fake_run("answer\n");
    let spec = fake_spec(&f);
    let mut stdin = FakeStdin::new("hi");
    let mut stdout = Closed;
    let mut stderr: Vec<u8> = Vec::new();
    let prepare = no_mr();
    let mut env = crate::root::CmdEnv {
        cfg: &fix.cfg,
        stdin: &mut stdin,
        stdout: &mut stdout,
        stderr: &mut stderr,
        prepare_mr: &*prepare,
        signals: false,
    };
    let result = run_model_command(&mut env, &spec, s(wd.path()), true);
    assert_eq!(
        result,
        Err(CmdError::plain(
            "write stdout: write /dev/stdout: bad file descriptor"
        ))
    );
    assert_eq!(fix.sessions()[0].status, "completed");
}
