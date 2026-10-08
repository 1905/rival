//! Go: `cmd/command_security_test.go`, plus the `commandSecurityAction`
//! branches with a fake opencode adapter. No provider runs; `--which`
//! looks up a never-executed `opencode` stub on a private PATH.

use super::*;

use std::cell::RefCell;
use std::io;
use std::time::{Duration, Instant};

use rival_core::config::{K3_LABEL, KIMI_MODEL, WHOLE_PROJECT};

use crate::testutil::{
    CLOSED_HANDLE, FakeStdin, Fixture, NO_SUCH_FILE, TEST_MR_URL, closed_handle_error, execute,
    no_mr, with_env,
};

fn s(p: &Path) -> String {
    p.to_str().unwrap().to_string()
}

const SECURITY_MARKER: &str = "## Role: Security Reviewer";
const BUG_HUNTER_MARKER: &str = "## Role: Implementation Bug Hunter";

// ---- TestSecurityPromptIsAlwaysTheSecurityLens ----

#[test]
fn security_prompt_is_always_the_security_lens() {
    let fix = Fixture::new();
    let bug_hunter = review::build_reviewer_prompt(&fix.cfg, "x", PromptKind::BugHunter);
    for (name, scope) in [
        ("explicit scope", "src/api/"),
        ("empty scope falls back to git or whole project", ""),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let (prompt, scope) = security_scope_and_prompt(&fix.cfg, scope, &s(dir.path()));
        assert!(prompt.contains(SECURITY_MARKER), "{name}: {prompt:.200}");
        assert!(
            !prompt.contains(BUG_HUNTER_MARKER),
            "{name}: the bug-hunter prompt leaked into a security run"
        );
        assert!(
            !prompt.contains(&bug_hunter[..80]),
            "{name}: prompt starts with the bug-hunter text"
        );
        assert!(!scope.is_empty(), "{name}: scope is empty");
    }
}

// ---- TestSecurityAutoScopeFallsBackToWholeProject ----

#[test]
fn security_auto_scope_falls_back_to_whole_project() {
    let fix = Fixture::new();
    let dir = tempfile::tempdir().unwrap();
    let (prompt, scope) = security_scope_and_prompt(&fix.cfg, " \n", &s(dir.path()));
    assert_eq!(scope, WHOLE_PROJECT);
    assert_eq!(
        prompt,
        review::build_reviewer_prompt(&fix.cfg, WHOLE_PROJECT, PromptKind::Security)
    );
}

// ---- TestSecurityScopeIsCarriedIntoThePrompt ----

#[test]
fn security_scope_is_carried_into_the_prompt() {
    let fix = Fixture::new();
    let dir = tempfile::tempdir().unwrap();
    let (prompt, scope) = security_scope_and_prompt(&fix.cfg, "  internal/auth/\n", &s(dir.path()));
    assert_eq!(scope, "internal/auth/");
    assert!(
        prompt.contains("internal/auth/"),
        "the scope never reached the prompt"
    );
}

// ---- TestSecurityPromptCoversEveryVulnerabilityClass ----

#[test]
fn security_prompt_covers_every_vulnerability_class() {
    let fix = Fixture::new();
    let prompt = review::build_reviewer_prompt(&fix.cfg, "src/", PromptKind::Security);
    for marker in [
        "njection",
        "uthorization",
        "uthentication",
        "rypto",
        "raversal",
        "SSRF",
        "eserializ",
        "ecret",
        "alidation",
        "CSRF",
        "edirect",
        "xhaustion",
    ] {
        assert!(
            prompt.contains(marker),
            "the security prompt does not cover {marker:?}"
        );
    }
}

// ---- TestBugHunterPromptUnchangedByTheSecurityLens ----

#[test]
fn bug_hunter_prompt_unchanged_by_the_security_lens() {
    let fix = Fixture::new();
    let prompt = review::build_reviewer_prompt(&fix.cfg, "src/", PromptKind::BugHunter);
    assert!(
        prompt.contains(BUG_HUNTER_MARKER),
        "the bug-hunter prompt changed"
    );
    assert!(
        !prompt.contains(SECURITY_MARKER),
        "the security prompt leaked into a bug-hunter run"
    );
}

// ---- TestSecurityParsesFinalAnswerNotToolOutput ----

const TOOL_PAYLOAD: &str = r#"{"summary": "One injection.", "findings": [{"file": "a.go", "line": 3, "severity": "high", "category": "security", "title": "sql injection", "body": "concatenated query", "suggestion": "bind params", "confidence": 9}]}"#;

fn tool_output_then_prose() -> String {
    format!(
        "user\nreview src/\nexec\ncat old-security.json\n{TOOL_PAYLOAD}\ncodex\nI looked around; nothing jumped out.\n"
    )
}

#[test]
fn security_parses_final_answer_not_tool_output() {
    let (out, err) =
        format_security_output(&tool_output_then_prose(), KIMI_MODEL, "src/", "/tmp/x.log");
    assert!(
        err.is_err(),
        "tool output accepted as the security review:\n{out}"
    );
}

// ---- commandSecurityAction ----

/// A clean, valid security review as the provider's final answer.
const GOOD_LOG: &str = r#"{"summary": "One injectable query.", "findings": [{"file": "src/api/users.go", "line": 42, "severity": "high", "category": "injection", "title": "SQL built from request input", "body": "the handler concatenates the id", "suggestion": "use a bound parameter", "confidence": 9}]}
"#;

#[derive(Debug, Default)]
struct Fake {
    /// Preflight error text; `None` passes.
    preflight_error: Option<String>,
    preflight_calls: usize,
    /// What the run writes to the session log; `None` writes nothing.
    log: Option<String>,
    exit_code: i64,
    run_error: Option<String>,
    /// Wait for the run context to end before failing.
    wait_for_cancel: bool,
    /// Make the sessions dir read-only before returning.
    lock_sessions: bool,
    run_calls: usize,
    prompt: String,
    variant: String,
    workdir: String,
    entry_name: String,
    status_during_run: String,
}

fn fake_executor(f: &RefCell<Fake>) -> SecurityExecutor<'_> {
    SecurityExecutor {
        preflight: Box::new(move |_, _, _| {
            let mut f = f.borrow_mut();
            f.preflight_calls += 1;
            match &f.preflight_error {
                Some(e) => Err(anyhow::anyhow!("{e}")),
                None => Ok(()),
            }
        }),
        run: Box::new(move |ctx, cfg, sess, prompt, variant, workdir, entry| {
            let mut f = f.borrow_mut();
            f.run_calls += 1;
            f.prompt = prompt.to_string();
            f.variant = variant.to_string();
            f.workdir = workdir.to_string();
            f.entry_name = entry.name.to_string();
            f.status_during_run = sess.status.clone();
            if f.wait_for_cancel {
                let deadline = Instant::now() + Duration::from_secs(10);
                while ctx.err().is_none() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
            if let Some(e) = &f.run_error {
                anyhow::bail!("{e}");
            }
            if let Some(log) = &f.log {
                std::fs::write(&sess.log_file, log)?;
            }
            // Only the Unix-only save-failure test sets it.
            #[cfg(unix)]
            if f.lock_sessions {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(
                    cfg.paths().sessions_dir(),
                    std::fs::Permissions::from_mode(0o500),
                )?;
            }
            #[cfg(not(unix))]
            let _ = (f.lock_sessions, cfg);
            Ok(RunResult {
                exit_code: f.exit_code,
                output_bytes: f.log.as_ref().map_or(0, |l| l.len() as i64),
                output_lines: 1,
            })
        }),
    }
}

fn good_fake() -> RefCell<Fake> {
    RefCell::new(Fake {
        log: Some(GOOD_LOG.to_string()),
        ..Fake::default()
    })
}

struct Run {
    stdout: String,
    stderr: String,
    result: Result<(), CmdError>,
}

fn run_security(
    fix: &Fixture,
    stdin: &mut FakeStdin,
    opts: SecurityOptions,
    f: &RefCell<Fake>,
) -> Run {
    let ex = fake_executor(f);
    let prepare = no_mr();
    let out = with_env(fix, stdin, &*prepare, |env| {
        run_command_security(env, &opts, &ex)
    });
    Run {
        stdout: out.stdout,
        stderr: out.stderr,
        result: out.result,
    }
}

fn opts(workdir: &str) -> SecurityOptions {
    SecurityOptions {
        workdir: workdir.to_string(),
        no_queue: true,
        which: false,
    }
}

#[test]
fn successful_review_prints_it_and_completes_the_session() {
    let fix = Fixture::new();
    let dir = tempfile::tempdir().unwrap();
    let w = s(dir.path());
    let f = good_fake();
    let r = run_security(&fix, &mut FakeStdin::new("  src/api/\n"), opts(&w), &f);
    assert_eq!(r.result, Ok(()));
    assert_eq!(r.stderr, "");

    let sessions = fix.sessions();
    assert_eq!(sessions.len(), 1);
    let sess = &sessions[0];
    let (want, valid) = format_security_output(GOOD_LOG, KIMI_MODEL, "src/api/", &sess.log_file);
    assert!(valid.is_ok());
    assert_eq!(r.stdout, want);
    assert!(r.stdout.contains("RIVAL SECURITY REVIEW"), "{}", r.stdout);

    assert_eq!(sess.status, "completed");
    assert_eq!(sess.exit_code, Some(0));
    assert_eq!(sess.cli, "opencode");
    assert_eq!(sess.mode, "security");
    assert_eq!(sess.model, KIMI_MODEL);
    assert_eq!(sess.effort, "max");
    assert_eq!(sess.review_scope, "src/api/");
    assert_eq!(sess.work_dir, w);
    assert_eq!(sess.output_bytes, GOOD_LOG.len() as i64);
    assert_eq!(sess.output_lines, 1);

    let f = f.borrow();
    assert_eq!(f.preflight_calls, 1);
    assert_eq!(f.run_calls, 1);
    assert_eq!(f.variant, "max");
    assert_eq!(f.workdir, w);
    assert_eq!(f.entry_name, "k3");
    assert_eq!(f.status_during_run, "running");
    let (prompt, _) = security_scope_and_prompt(&fix.cfg, "src/api/", &w);
    assert_eq!(f.prompt, prompt);
}

#[test]
fn configured_grok_runs_the_grok_entry() {
    let fix = Fixture::with_config_yaml("security:\n  reviewer: GROK\n");
    let dir = tempfile::tempdir().unwrap();
    let f = good_fake();
    let r = run_security(&fix, &mut FakeStdin::new("src/"), opts(&s(dir.path())), &f);
    assert_eq!(r.result, Ok(()));
    let f = f.borrow();
    assert_eq!(f.entry_name, "grok");
    assert_eq!(f.variant, "xhigh");
    let sess = &fix.sessions()[0];
    assert_eq!(sess.effort, "xhigh");
    assert_eq!(sess.model, "x-ai/grok-4.6");
}

#[test]
fn closed_stdin_skips_the_read_and_auto_detects() {
    let fix = Fixture::new();
    let dir = tempfile::tempdir().unwrap();
    let mut stdin = FakeStdin::new("never read");
    stdin.stat_failed = true;
    stdin.forbid_read = true;
    let f = good_fake();
    let r = run_security(&fix, &mut stdin, opts(&s(dir.path())), &f);
    assert_eq!(r.result, Ok(()));
    assert_eq!(
        f.borrow().prompt,
        review::build_reviewer_prompt(&fix.cfg, WHOLE_PROJECT, PromptKind::Security)
    );
    assert_eq!(fix.sessions()[0].review_scope, WHOLE_PROJECT);
}

#[test]
fn terminal_stdin_prints_usage_without_preflight() {
    let fix = Fixture::new();
    let dir = tempfile::tempdir().unwrap();
    let mut tty = FakeStdin::new("");
    tty.char_device = true;
    tty.forbid_read = true;
    let f = good_fake();
    let r = run_security(&fix, &mut tty, opts(&s(dir.path())), &f);
    assert_eq!(r.result, Ok(()));
    assert_eq!(r.stdout, format!("{SECURITY_USAGE}\n"));
    assert_eq!(f.borrow().preflight_calls, 0);
    assert!(fix.sessions().is_empty());
}

#[test]
fn read_errors_and_mr_scopes_fail_plainly_before_preflight() {
    let fix = Fixture::new();
    let dir = tempfile::tempdir().unwrap();
    let mut stdin = FakeStdin::new("");
    stdin.read_error = Some("read /dev/stdin: input/output error".into());
    let f = good_fake();
    let r = run_security(&fix, &mut stdin, opts(&s(dir.path())), &f);
    assert_eq!(
        r.result,
        Err(CmdError::plain(
            "read stdin: read /dev/stdin: input/output error"
        ))
    );

    let r = run_security(
        &fix,
        &mut FakeStdin::new(&format!("review {TEST_MR_URL}")),
        opts(&s(dir.path())),
        &f,
    );
    assert_eq!(
        r.result,
        Err(CmdError::plain(
            "GitLab MR URLs need a pinned review: use rival command codex review <MR-URL> (or /rival-codex review <MR-URL>) from a repository with the MR's remote; no reviewer was started"
        ))
    );
    assert_eq!(r.stdout, "");
    assert_eq!(f.borrow().preflight_calls, 0);
    assert!(fix.sessions().is_empty());
}

#[test]
fn preflight_failure_prints_a_hint_and_starts_nothing() {
    let fix = Fixture::new();
    let dir = tempfile::tempdir().unwrap();
    let f = RefCell::new(Fake {
        preflight_error: Some(
            "model kimi-k3 requires MOONSHOT_API_KEY — add it to the project .env or export it"
                .into(),
        ),
        ..Fake::default()
    });
    let r = run_security(&fix, &mut FakeStdin::new("src/"), opts(&s(dir.path())), &f);
    assert_eq!(
        r.result,
        Err(CmdError::exit(
            1,
            "model kimi-k3 requires MOONSHOT_API_KEY — add it to the project .env or export it"
        ))
    );
    assert_eq!(
        r.stdout,
        "model kimi-k3 requires MOONSHOT_API_KEY — add it to the project .env or export it\n\nsecurity.reviewer selects the model; accepted values: k3, grok\n"
    );
    assert_eq!(f.borrow().run_calls, 0);
    assert!(fix.sessions().is_empty());
}

#[test]
fn run_error_fails_the_session_and_the_command() {
    let fix = Fixture::new();
    let dir = tempfile::tempdir().unwrap();
    let f = RefCell::new(Fake {
        run_error: Some("start opencode: exec format error".into()),
        ..Fake::default()
    });
    let r = run_security(&fix, &mut FakeStdin::new("src/"), opts(&s(dir.path())), &f);
    assert_eq!(
        r.result,
        Err(CmdError::plain("start opencode: exec format error"))
    );
    assert_eq!(r.stdout, "");
    let sess = &fix.sessions()[0];
    assert_eq!(sess.status, "failed");
    assert_eq!(sess.exit_code, Some(1));
    assert_eq!(sess.error_msg, "start opencode: exec format error");
}

#[test]
fn run_timeout_is_recorded_as_the_reason() {
    let fix = Fixture::with(&[("RIVAL_RUN_TIMEOUT", "50ms")], None);
    let dir = tempfile::tempdir().unwrap();
    let f = RefCell::new(Fake {
        run_error: Some("signal: killed".into()),
        wait_for_cancel: true,
        ..Fake::default()
    });
    let r = run_security(&fix, &mut FakeStdin::new("src/"), opts(&s(dir.path())), &f);
    assert_eq!(r.result, Err(CmdError::plain("signal: killed")));
    assert_eq!(
        fix.sessions()[0].error_msg,
        format!("{K3_LABEL} run timeout after 50ms (RIVAL_RUN_TIMEOUT) — model did not finish")
    );
}

#[test]
fn nonzero_exit_prints_the_log_and_keeps_the_code() {
    let fix = Fixture::new();
    let dir = tempfile::tempdir().unwrap();
    let log = "provider crashed\n";
    let f = RefCell::new(Fake {
        log: Some(log.into()),
        exit_code: 3,
        ..Fake::default()
    });
    let r = run_security(&fix, &mut FakeStdin::new("src/"), opts(&s(dir.path())), &f);
    assert_eq!(
        r.result,
        Err(CmdError::exit(3, "kimi-k3 exited with code 3"))
    );
    assert_eq!(r.stdout, public_runtime_log("opencode", KIMI_MODEL, log));
    let sess = &fix.sessions()[0];
    assert_eq!(sess.status, "failed");
    assert_eq!(sess.exit_code, Some(3));
    assert_eq!(sess.error_msg, "kimi-k3 exited with code 3");
}

#[test]
fn unusable_output_prints_it_and_fails() {
    let fix = Fixture::new();
    let dir = tempfile::tempdir().unwrap();
    for log in [
        "I looked around and everything seems fine.\n".to_string(),
        tool_output_then_prose(),
    ] {
        let f = RefCell::new(Fake {
            log: Some(log.clone()),
            ..Fake::default()
        });
        let r = run_security(&fix, &mut FakeStdin::new("src/"), opts(&s(dir.path())), &f);
        let sessions = fix.sessions();
        let sess = sessions.iter().find(|s| s.status != "completed").unwrap();
        let (want, valid) = format_security_output(&log, KIMI_MODEL, "src/", &sess.log_file);
        let reason = format!("{:#}", valid.unwrap_err());
        assert_eq!(r.stdout, want);
        assert!(want.contains("UNUSABLE OUTPUT"), "{want}");
        assert_eq!(
            r.result,
            Err(CmdError::exit(
                1,
                format!("security review produced no usable findings: {reason}")
            ))
        );
        assert_eq!(sess.status, "failed");
        assert_eq!(
            sess.error_msg,
            format!("unusable security output: {reason}")
        );
        // Clean up so the next case finds its own session.
        let _ = std::fs::rename(
            fix.cfg
                .paths()
                .sessions_dir()
                .join(format!("{}.json", sess.id)),
            dir.path().join(format!("{}.json", sess.id)),
        );
    }
}

#[test]
fn missing_log_fails_the_session_and_the_command() {
    let fix = Fixture::new();
    let dir = tempfile::tempdir().unwrap();
    let f = RefCell::new(Fake::default());
    let r = run_security(&fix, &mut FakeStdin::new("src/"), opts(&s(dir.path())), &f);
    let sess = &fix.sessions()[0];
    let err = format!("open {}: {NO_SUCH_FILE}", sess.log_file);
    assert_eq!(
        r.result,
        Err(CmdError::plain(format!("read log file: {err}")))
    );
    assert_eq!(sess.status, "failed");
    assert_eq!(sess.error_msg, format!("read log: {err}"));
}

#[cfg(unix)]
#[test]
fn completion_save_failure_fails_the_command() {
    use std::os::unix::fs::PermissionsExt;
    // SAFETY: plain getter.
    if unsafe { libc::geteuid() } == 0 {
        return; // root writes anywhere
    }
    let fix = Fixture::new();
    let dir = tempfile::tempdir().unwrap();
    let f = RefCell::new(Fake {
        log: Some(GOOD_LOG.into()),
        lock_sessions: true,
        ..Fake::default()
    });
    let r = run_security(&fix, &mut FakeStdin::new("src/"), opts(&s(dir.path())), &f);
    let sessions_dir = fix.cfg.paths().sessions_dir();
    std::fs::set_permissions(&sessions_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let err = r.result.unwrap_err();
    assert_eq!(err.code, 1);
    assert!(
        err.message.starts_with("save session completion: "),
        "{}",
        err.message
    );
    assert!(
        r.stdout.contains("RIVAL SECURITY REVIEW"),
        "the review still printed"
    );
    // Neither the completion nor the failure could be saved.
    assert_eq!(fix.sessions()[0].status, "running");
}

/// A stdout whose writes fail like a closed fd 1.
struct ClosedStdout;

impl io::Write for ClosedStdout {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(closed_handle_error())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn stdout_write_error_leaves_the_session_interrupted() {
    let fix = Fixture::new();
    let dir = tempfile::tempdir().unwrap();
    let f = good_fake();
    let ex = fake_executor(&f);
    let prepare = no_mr();
    let mut stdin = FakeStdin::new("src/");
    let mut stdout = ClosedStdout;
    let mut stderr: Vec<u8> = Vec::new();
    let result = {
        let mut env = CmdEnv {
            cfg: &fix.cfg,
            stdin: &mut stdin,
            stdout: &mut stdout,
            stderr: &mut stderr,
            live_stdout: None,
            prepare_mr: &*prepare,
            signals: false,
        };
        run_command_security(&mut env, &opts(&s(dir.path())), &ex)
    };
    assert_eq!(
        result,
        Err(CmdError::plain(format!(
            "write stdout: write /dev/stdout: {CLOSED_HANDLE}"
        )))
    );
    let sess = &fix.sessions()[0];
    assert_eq!(sess.status, "failed");
    assert_eq!(sess.error_msg, "interrupted");
}

// ---- --which ----

/// A PATH dir holding a never-executed `opencode` stub.
#[cfg(unix)]
fn opencode_stub() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let bin = tempfile::tempdir().unwrap();
    let stub = bin.path().join("opencode");
    std::fs::write(&stub, "#!/bin/sh\nexit 97\n").unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

/// `command security --which` with the real preflight. Stdin is never read.
fn run_which(fix: &Fixture, workdir: &str) -> Run {
    let ex = SecurityExecutor::production();
    let prepare = no_mr();
    let mut stdin = FakeStdin::new("");
    stdin.forbid_read = true;
    let out = with_env(fix, &mut stdin, &*prepare, |env| {
        run_command_security(
            env,
            &SecurityOptions {
                workdir: workdir.to_string(),
                no_queue: false,
                which: true,
            },
            &ex,
        )
    });
    Run {
        stdout: out.stdout,
        stderr: out.stderr,
        result: out.result,
    }
}

#[cfg(unix)]
#[test]
fn which_reports_a_ready_default_model() {
    let bin = opencode_stub();
    let fix = Fixture::with(
        &[
            ("PATH", bin.path().to_str().unwrap()),
            ("MOONSHOT_API_KEY", "sk-test"),
        ],
        None,
    );
    let dir = tempfile::tempdir().unwrap();
    let r = run_which(&fix, &s(dir.path()));
    assert_eq!(r.result, Ok(()));
    assert_eq!(
        r.stdout,
        "Security reviewer: k3 (moonshotai/kimi-k3 via moonshotai)\n\
         Config: security.reviewer = unset (default)\n\
         OpenCode selector: moonshotai/kimi-k3\n\
         Reasoning variant: max\n\
         MOONSHOT_API_KEY: set\n\
         \n\
         Ready.\n"
    );
    assert_eq!(r.stderr, "");
    assert!(fix.sessions().is_empty(), "--which starts no review");
}

#[cfg(unix)]
#[test]
fn which_shows_the_validated_configured_value() {
    // Config loading lowercases a valid security.reviewer, so "GROK" shows
    // as "grok".
    let bin = opencode_stub();
    let fix = Fixture::with_config_and_env(
        "security:\n  reviewer: GROK\n",
        &[
            ("PATH", bin.path().to_str().unwrap()),
            ("OPENROUTER_API_KEY", "sk-or"),
        ],
    );
    let dir = tempfile::tempdir().unwrap();
    let r = run_which(&fix, &s(dir.path()));
    assert_eq!(r.result, Ok(()));
    assert_eq!(
        r.stdout,
        "Security reviewer: grok (x-ai/grok-4.6 via openrouter)\n\
         Config: security.reviewer = grok\n\
         OpenCode selector: openrouter/x-ai/grok-4.6\n\
         Reasoning variant: xhigh\n\
         OPENROUTER_API_KEY: set\n\
         \n\
         Ready.\n"
    );
}

#[cfg(unix)]
#[test]
fn which_reports_a_missing_key_only_through_the_error() {
    let bin = opencode_stub();
    let fix = Fixture::with(&[("PATH", bin.path().to_str().unwrap())], None);
    let dir = tempfile::tempdir().unwrap();
    let r = run_which(&fix, &s(dir.path()));
    assert_eq!(
        r.result,
        Err(CmdError::exit(
            1,
            format!(
                "model {K3_LABEL} requires MOONSHOT_API_KEY — add it to the project .env or export it"
            )
        ))
    );
    assert!(
        r.stdout
            .ends_with("Reasoning variant: max\nMOONSHOT_API_KEY: MISSING\n\nNot usable.\n"),
        "{}",
        r.stdout
    );
    assert_eq!(r.stderr, "", "the root prints the error once");
}

#[test]
fn which_reports_a_missing_binary_even_with_a_key() {
    let empty = tempfile::tempdir().unwrap();
    let fix = Fixture::with_config_and_env(
        "security:\n  reviewer: grok\n",
        &[
            ("PATH", empty.path().to_str().unwrap()),
            ("OPENROUTER_API_KEY", "sk-or"),
        ],
    );
    let dir = tempfile::tempdir().unwrap();
    let r = run_which(&fix, &s(dir.path()));
    assert_eq!(
        r.result,
        Err(CmdError::exit(
            1,
            "opencode CLI not installed. Install: curl -fsSL https://opencode.ai/install | bash"
        ))
    );
    assert_eq!(
        r.stdout,
        "Security reviewer: grok (x-ai/grok-4.6 via openrouter)\n\
         Config: security.reviewer = grok\n\
         OpenCode selector: openrouter/x-ai/grok-4.6\n\
         Reasoning variant: xhigh\n\
         OPENROUTER_API_KEY: set\n\
         \n\
         Not usable.\n"
    );
}

// ---- Through the root ----

#[test]
fn root_which_prints_its_error_once() {
    let empty = tempfile::tempdir().unwrap();
    let fix = Fixture::with(&[("PATH", empty.path().to_str().unwrap())], None);
    let dir = tempfile::tempdir().unwrap();
    let mut stdin = FakeStdin::new("");
    stdin.forbid_read = true;
    let (code, stdout, stderr) = execute(
        &fix,
        &mut stdin,
        &[
            "command",
            "security",
            "--which",
            "--workdir",
            &s(dir.path()),
        ],
    );
    assert_eq!(code, 1);
    assert!(stdout.ends_with("\nNot usable.\n"), "{stdout}");
    assert_eq!(
        stderr,
        "opencode CLI not installed. Install: curl -fsSL https://opencode.ai/install | bash\n"
    );
}

#[test]
fn root_preflight_failure_prints_hint_on_stdout_and_error_on_stderr() {
    let empty = tempfile::tempdir().unwrap();
    let fix = Fixture::with(&[("PATH", empty.path().to_str().unwrap())], None);
    let dir = tempfile::tempdir().unwrap();
    let (code, stdout, stderr) = execute(
        &fix,
        &mut FakeStdin::new("src/"),
        &["command", "security", "--workdir", &s(dir.path())],
    );
    let err = "opencode CLI not installed. Install: curl -fsSL https://opencode.ai/install | bash";
    assert_eq!(code, 1);
    assert_eq!(
        stdout,
        format!("{err}\n\nsecurity.reviewer selects the model; accepted values: k3, grok\n")
    );
    assert_eq!(stderr, format!("{err}\n"));
    assert!(fix.sessions().is_empty());
}

#[test]
fn root_invalid_reviewer_is_caught_by_the_pre_run_first() {
    // The root's config check runs before the action, so the action's own
    // stderr line never prints through the root.
    let fix = Fixture::with_config_yaml("security:\n  reviewer: gpt\n");
    let pre_run = fix.cfg.user_config_error().unwrap().to_string();
    assert!(
        pre_run.contains("invalid security.reviewer \"gpt\""),
        "{pre_run}"
    );
    let dir = tempfile::tempdir().unwrap();
    let (code, stdout, stderr) = execute(
        &fix,
        &mut FakeStdin::new("src/"),
        &["command", "security", "--workdir", &s(dir.path())],
    );
    assert_eq!(code, 1);
    assert_eq!(stdout, "");
    assert_eq!(stderr, format!("{pre_run}\n"));
}
