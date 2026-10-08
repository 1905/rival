//! Review output of the run surface, plus model run order and failure
//! cases.

use super::*;

use std::path::Path;

use crate::testutil::{
    FakeStdin, Fixture, SharedRun, TEST_MR_URL, fake_mr, fake_run, fake_spec, json_answer_log,
    no_mr, with_env,
};

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

fn run(
    fix: &Fixture,
    f: &SharedRun,
    stdin: &mut FakeStdin,
    prepare: &crate::root::PrepareMr,
    opts: RunOptions,
) -> crate::testutil::Outcome {
    let spec = fake_spec(f);
    with_env(fix, stdin, prepare, |env| run_model_run(env, &spec, opts))
}

fn review_opts(workdir: &Path, scope: &str) -> RunOptions {
    RunOptions {
        workdir: s(workdir).into(),
        no_queue: true,
        review_scope: scope.into(),
        is_review: true,
        ..RunOptions::default()
    }
}

#[test]
fn run_mr_review_runs_in_snapshot() {
    let fix = Fixture::new();
    let mr = fake_mr();
    let caller = tempfile::tempdir().unwrap();
    let f = fake_run(&json_answer_log());
    f.borrow_mut().mirror_text = "LIVE-MIRROR\n".into();
    let out = run(
        &fix,
        &f,
        &mut FakeStdin::new(""),
        &*mr.resolver,
        review_opts(caller.path(), TEST_MR_URL),
    );
    out.result.unwrap();
    {
        let f = f.borrow();
        assert_eq!(f.workdir, s(&mr.snapshot_dir));
        assert!(f.workdir_existed);
        assert_eq!(f.cred_workdir, s(caller.path()));
        assert!(
            f.prompt.contains("PINNED-SNAPSHOT-SCOPE"),
            "{:.300}",
            f.prompt
        );
        assert!(f.had_mirror, "run mode mirrors provider output");
    }
    assert!(out.stdout.starts_with("GitLab MR: "), "{}", out.stdout);
    assert!(
        out.stdout.contains("═══ RIVAL REVIEW ═══"),
        "{}",
        out.stdout
    );
    // Identity, then the live mirror, then the formatted review.
    let identity = out.stdout.find("GitLab MR: ").unwrap();
    let mirror = out.stdout.find("LIVE-MIRROR").unwrap();
    let review = out.stdout.find("═══ RIVAL REVIEW ═══").unwrap();
    assert!(identity < mirror && mirror < review, "{}", out.stdout);
    assert!(
        !mr.snapshot_dir.exists(),
        "MR checkout was not closed after the run"
    );
    assert_eq!(mr.calls.get(), 1);
}

/// `rival run <model> --review` with no scope auto-detects like command
/// mode; outside a git repo that falls back to the whole project.
#[test]
fn run_review_empty_scope_auto_detects() {
    let fix = Fixture::new();
    let wd = tempfile::tempdir().unwrap();
    let f = fake_run(&json_answer_log());
    let out = run(
        &fix,
        &f,
        &mut FakeStdin::new(""),
        &*no_mr(),
        review_opts(wd.path(), ""),
    );
    out.result.unwrap();
    assert!(
        f.borrow()
            .prompt
            .contains("Review scope: the entire project"),
        "{:.300}",
        f.borrow().prompt
    );
    assert!(
        out.stdout.contains("Scope: the entire project\n"),
        "{}",
        out.stdout
    );
    assert_eq!(fix.sessions()[0].review_scope, "the entire project");
}

#[test]
fn prompt_stdin_runs_raw_and_completes_without_reading_the_log() {
    let fix = Fixture::new();
    let wd = tempfile::tempdir().unwrap();
    let f = fake_run("ignored log\n");
    f.borrow_mut().mirror_text = "mirrored answer\n".into();
    let mut stdin = FakeStdin::new("explain the auth flow");
    let out = run(
        &fix,
        &f,
        &mut stdin,
        &*no_mr(),
        RunOptions {
            workdir: s(wd.path()).into(),
            no_queue: true,
            prompt_stdin: true,
            ..RunOptions::default()
        },
    );
    out.result.unwrap();
    assert_eq!(f.borrow().prompt, "explain the auth flow");
    assert!(!f.borrow().review);
    assert_eq!(out.stdout, "mirrored answer\n", "only the live mirror");
    let sess = &fix.sessions()[0];
    assert_eq!(
        (sess.status.as_str(), sess.mode.as_str()),
        ("completed", "raw")
    );
}

#[test]
fn input_validation_order_and_messages() {
    let fix = Fixture::new();
    let f = fake_run("");
    let missing = fix.missing_dir("nonexistent");
    // Neither --review nor --prompt-stdin.
    let mut stdin = FakeStdin::new("x");
    stdin.forbid_read = true;
    let out = run(
        &fix,
        &f,
        &mut stdin,
        &*no_mr(),
        RunOptions {
            workdir: missing.clone(),
            ..RunOptions::default()
        },
    );
    assert_eq!(
        out.result,
        Err(CmdError::plain("provide --prompt-stdin or --review"))
    );

    // Empty stdin prompt, before the workdir is checked.
    let mut stdin = FakeStdin::new("");
    let opts = RunOptions {
        workdir: missing.clone(),
        prompt_stdin: true,
        ..RunOptions::default()
    };
    let out = run(&fix, &f, &mut stdin, &*no_mr(), opts.clone());
    assert_eq!(out.result, Err(CmdError::plain("empty prompt")));

    // An MR URL in a raw prompt is rejected before anything starts.
    let mut stdin = FakeStdin::new(&format!("look at {TEST_MR_URL}"));
    let out = run(&fix, &f, &mut stdin, &*no_mr(), opts.clone());
    assert!(
        out.result
            .unwrap_err()
            .message
            .contains("no reviewer was started")
    );

    // Read errors are plain.
    let mut stdin = FakeStdin::new("");
    stdin.read_error = Some("read /dev/stdin: is a directory".into());
    let out = run(&fix, &f, &mut stdin, &*no_mr(), opts.clone());
    assert_eq!(
        out.result,
        Err(CmdError::plain(
            "read stdin: read /dev/stdin: is a directory"
        ))
    );

    // A bad workdir is a plain error here (stderr), not stdout + exit 1.
    let mut stdin = FakeStdin::new("hi");
    let out = run(&fix, &f, &mut stdin, &*no_mr(), opts);
    assert_eq!(
        out.result,
        Err(CmdError::plain(format!("workdir not found: {missing}")))
    );
    assert_eq!(out.stdout, "");
    assert_eq!(f.borrow().preflight_calls, 0);
    assert!(fix.sessions().is_empty());
}

/// --review wins over --prompt-stdin when both are given: stdin is not read.
#[test]
fn review_wins_over_prompt_stdin() {
    let fix = Fixture::new();
    let wd = tempfile::tempdir().unwrap();
    let f = fake_run(&json_answer_log());
    let mut stdin = FakeStdin::new("");
    stdin.forbid_read = true;
    let mut opts = review_opts(wd.path(), "src/");
    opts.prompt_stdin = true;
    let out = run(&fix, &f, &mut stdin, &*no_mr(), opts);
    out.result.unwrap();
    assert!(f.borrow().review);
}

#[test]
fn nonzero_exit_records_the_code_and_returns_it() {
    let fix = Fixture::new();
    let wd = tempfile::tempdir().unwrap();
    let f = fake_run("partial\n");
    f.borrow_mut().exit_code = 7;
    let out = run(
        &fix,
        &f,
        &mut FakeStdin::new("hi"),
        &*no_mr(),
        RunOptions {
            workdir: s(wd.path()).into(),
            no_queue: true,
            prompt_stdin: true,
            ..RunOptions::default()
        },
    );
    assert_eq!(
        out.result,
        Err(CmdError::exit(7, "codex exited with code 7"))
    );
    let sess = &fix.sessions()[0];
    assert_eq!(sess.status, "failed");
    assert_eq!(sess.exit_code, Some(7));
    assert_eq!(sess.error_msg, "codex exited with code 7");
}

/// The run prints the no-review reason on stderr and returns it, so the root
/// prints it a second time.
#[test]
fn quota_review_prints_reason_on_stderr() {
    let fix = Fixture::new();
    let wd = tempfile::tempdir().unwrap();
    let f = fake_run("");
    let out = run(
        &fix,
        &f,
        &mut FakeStdin::new(""),
        &*no_mr(),
        review_opts(wd.path(), "src/"),
    );
    let reason = "codex produced no output (empty result); likely an auth/session failure";
    assert_eq!(out.result, Err(CmdError::exit(1, reason)));
    assert_eq!(out.stderr, format!("{reason}\n"));
    assert!(!out.stdout.contains("RIVAL REVIEW"));
    assert_eq!(fix.sessions()[0].error_msg, reason);
}

#[test]
fn mr_checkout_is_removed_when_the_run_fails() {
    let fix = Fixture::new();
    let mr = fake_mr();
    let caller = tempfile::tempdir().unwrap();
    let f = fake_run("");
    f.borrow_mut().exit_code = 1;
    let out = run(
        &fix,
        &f,
        &mut FakeStdin::new(""),
        &*mr.resolver,
        review_opts(caller.path(), TEST_MR_URL),
    );
    assert!(out.result.is_err());
    assert!(f.borrow().workdir_existed);
    assert!(!mr.snapshot_dir.exists());
    assert!(
        out.stdout
            .starts_with(&format!("GitLab MR: {TEST_MR_URL}\n\n"))
    );
}
