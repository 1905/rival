//! Go: `internal/executor/claude_test.go`, plus exact argv vectors, auth
//! env stripping, error wrapping and transport-mode checks.

use super::*;
#[cfg(unix)]
use crate::executor::testutil::retry_busy;
use crate::executor::testutil::{Env, Spawned, recorder, strings};
use crate::session::{MODE_ANTISLOP, MODE_PLAN, MODE_SECURITY};

/// The read-only (review and task modes) argv, including the empty
/// `--setting-sources` value and the repeated tool list.
fn read_only_argv(effort: &str) -> Vec<String> {
    strings(&[
        "-p",
        "--model",
        config::CLAUDE_MODEL,
        "--effort",
        effort,
        "--output-format",
        "text",
        "--no-session-persistence",
        "--system-prompt",
        config::SYSTEM_PROMPT,
        "--safe-mode",
        "--setting-sources",
        "",
        "--settings",
        r#"{"disableAllHooks":true}"#,
        "--strict-mcp-config",
        "--mcp-config",
        r#"{"mcpServers":{}}"#,
        "--tools",
        "Read,Glob,Grep",
        "--allowedTools",
        "Read,Glob,Grep",
        "--disallowedTools",
        "mcp__*",
        "--permission-mode",
        "dontAsk",
    ])
}

fn raw_argv(effort: &str) -> Vec<String> {
    strings(&[
        "-p",
        "--model",
        config::CLAUDE_MODEL,
        "--effort",
        effort,
        "--output-format",
        "text",
        "--no-session-persistence",
        "--system-prompt",
        config::SYSTEM_PROMPT,
        "--dangerously-skip-permissions",
    ])
}

#[test]
fn claude_args_exact_vectors_and_effort_mapping() {
    assert_eq!(
        claude_args(config::CLAUDE_MODEL, "medium", true),
        read_only_argv("medium")
    );
    assert_eq!(
        claude_args(config::CLAUDE_MODEL, "medium", false),
        raw_argv("medium")
    );
    for (effort, want) in [
        ("low", "low"),
        ("medium", "medium"),
        ("high", "max"),
        ("xhigh", "max"),
        ("ultra", "max"),
        ("", "max"),
        ("bogus", "max"),
    ] {
        assert_eq!(
            claude_args(config::CLAUDE_MODEL, effort, false),
            raw_argv(want),
            "{effort}"
        );
    }
}

/// Go: TestClaudeReviewTransportRestrictions, plus the security task mode.
/// The fake drains the prompt first, fails if CLAUDECODE or
/// ANTHROPIC_API_KEY reached it, then prints its argv one per line.
#[cfg(unix)]
#[test]
fn claude_review_transport_restrictions() {
    for mode in ["review", "plan", "antislop", "security", "raw"] {
        let mut env = Env::new();
        env.set("CLAUDECODE", Some("1"))
            .set("RIVAL_CLAUDE_AUTH", Some("subscription"))
            .set("ANTHROPIC_API_KEY", Some("should-not-reach-child"));
        env.fake(
            "claude",
            "#!/bin/sh\n/bin/cat >/dev/null\n[ -z \"$CLAUDECODE\" ] || exit 10\n[ -z \"$ANTHROPIC_API_KEY\" ] || exit 11\nprintf '%s\\n' \"$@\"\n",
        );
        let cfg = env.config();
        let repo = env.work_str();
        let mut sess = env.session("claude", mode, config::CLAUDE_MODEL, &repo);
        let mut out = Vec::new();
        let result = retry_busy(
            || {
                out.clear();
                run_claude(
                    &Context::background(),
                    &cfg,
                    &mut sess,
                    "review",
                    "medium",
                    &repo,
                    Some(&mut out),
                )
            },
            |r| format!("{r:?}"),
        )
        .unwrap();
        assert_eq!(result.exit_code, 0, "{mode}: child exit");
        let text = String::from_utf8(out).unwrap();
        let args: Vec<String> = text
            .strip_suffix('\n')
            .unwrap()
            .split('\n')
            .map(str::to_string)
            .collect();
        if mode == "raw" {
            assert_eq!(args, raw_argv("medium"), "raw prompt behavior changed");
            assert_eq!(sess.mode, "native");
        } else {
            assert_eq!(
                args,
                read_only_argv("medium"),
                "{mode}: review restrictions"
            );
            let want_mode = if mode == "review" { "native" } else { mode };
            assert_eq!(sess.mode, want_mode);
        }
        assert_eq!(sess.account, config::CLAUDE_AUTH_SUBSCRIPTION);
    }
}

/// Go: TestClaudeDockerReviewMountIsReadOnly (exact argv here).
#[cfg(unix)]
#[test]
fn claude_docker_review_mount_is_read_only() {
    let mut env = Env::new();
    env.set(config::CLAUDE_DOCKER_TOKEN_ENV, Some("fixture-token"));
    env.fake(
        "docker",
        "#!/bin/sh\n/bin/cat >/dev/null\nprintf '%s\\n' \"$@\"\n",
    );
    let cfg = env.config();
    let repo = env.work_str();
    let mut sess = env.session("claude", "review", config::CLAUDE_MODEL, &repo);
    let mut out = Vec::new();
    let result = retry_busy(
        || {
            out.clear();
            run_claude(
                &Context::background(),
                &cfg,
                &mut sess,
                "review",
                "medium",
                &repo,
                Some(&mut out),
            )
        },
        |r| format!("{r:?}"),
    )
    .unwrap();
    assert_eq!(result.exit_code, 0);
    let text = String::from_utf8(out).unwrap();
    assert!(
        text.contains(&format!("{repo}:/workspace:ro\n")),
        "unsafe Docker review: {text}"
    );
    assert!(
        text.contains("Read,Glob,Grep"),
        "unsafe Docker review: {text}"
    );
    let mut want = strings(&[
        "run",
        "--rm",
        "-i",
        "-v",
        &format!("{repo}:/workspace:ro"),
        "-w",
        "/workspace",
        "-e",
        "ANTHROPIC_AUTH_TOKEN=fixture-token",
        "rival-claude",
    ]);
    want.extend(read_only_argv("medium"));
    assert_eq!(text, format!("{}\n", want.join("\n")));
    assert_eq!(sess.mode, "docker");
    // Docker runs never record a native auth mode.
    assert_eq!(sess.account, "");
}

#[test]
fn native_request_strips_subscription_credentials_only() {
    for (auth, key, want_drop) in [
        (
            "",
            "",
            vec!["CLAUDECODE", "ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"],
        ),
        (
            "sub",
            "",
            vec!["CLAUDECODE", "ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"],
        ),
        ("api", "sk-x", vec!["CLAUDECODE"]),
    ] {
        let mut env = Env::new();
        env.set("RIVAL_CLAUDE_AUTH", Some(auth))
            .set("ANTHROPIC_API_KEY", Some(key));
        env.fake("claude", "#!/bin/sh\nexit 0\n");
        let cfg = env.config();
        let work = env.work_str();
        let mut sess = env.session("claude", "raw", config::CLAUDE_MODEL, &work);
        let mut seen = None;
        run_claude_model(
            &cfg,
            &mut sess,
            "do it",
            "low",
            &work,
            config::CLAUDE_MODEL,
            recorder(&mut seen, Ok(RunResult::default())),
        )
        .unwrap();
        let want_account = if auth == "api" { "api" } else { "subscription" };
        assert_eq!(
            seen.unwrap(),
            Spawned {
                binary: "claude".into(),
                args: raw_argv("low"),
                env: vec![],
                prompt: format!("{}\ndo it", cfg.build_workdir_preamble(Path::new(&work))),
                drop_env: strings(&want_drop),
                environ: cfg.environ().to_vec(),
                mode: "native".into(),
                account: want_account.into(),
            },
            "auth {auth:?}"
        );
    }
}

#[test]
fn claude_errors_are_wrapped_with_the_public_label() {
    let mut env = Env::new();
    env.fake("claude", "#!/bin/sh\nexit 0\n");
    let work = env.work_str();

    // An auth config error stops before any spawn.
    env.set("RIVAL_CLAUDE_AUTH", Some("bogus"));
    let cfg = env.config();
    let mut sess = env.session("claude", "raw", config::CLAUDE_MODEL, &work);
    let mut seen = None;
    let err = run_claude_model(
        &cfg,
        &mut sess,
        "p",
        "low",
        &work,
        config::CLAUDE_MODEL,
        recorder(&mut seen, Ok(RunResult::default())),
    )
    .unwrap_err();
    assert_eq!(
        format!("{err:#}"),
        "claude runtime: invalid RIVAL_CLAUDE_AUTH=\"bogus\" — use \"subscription\" (default) or \"api\""
    );
    assert!(seen.is_none());

    // A spawn error goes through PublicRuntimeError.
    env.set("RIVAL_CLAUDE_AUTH", None);
    let cfg = env.config();
    let err = run_claude_model(
        &cfg,
        &mut sess,
        "p",
        "low",
        &work,
        config::CLAUDE_MODEL,
        recorder(&mut seen, Err(anyhow!("subprocess claude: signal: killed"))),
    )
    .unwrap_err();
    assert_eq!(
        format!("{err:#}"),
        "claude runtime: Claude runtime: signal: killed"
    );

    let err = run_claude_model(
        &cfg,
        &mut sess,
        "p",
        "low",
        &work,
        "claude-opus-5",
        recorder(&mut seen, Ok(RunResult::default())),
    )
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "unsupported Claude Code model \"claude-opus-5\""
    );
}

#[test]
fn docker_transport_needs_the_token_and_absolutizes_the_workdir() {
    // No claude on PATH: the docker transport is chosen.
    let mut env = Env::new();
    let cfg = env.config();
    let mut sess = env.session("claude", "raw", config::CLAUDE_MODEL, "rel");
    let mut seen = None;
    let err = run_claude_model(
        &cfg,
        &mut sess,
        "p",
        "low",
        "rel",
        config::CLAUDE_MODEL,
        recorder(&mut seen, Ok(RunResult::default())),
    )
    .unwrap_err();
    assert_eq!(
        format!("{err:#}"),
        "claude runtime: RIVAL_CLAUDE_TOKEN env var not set"
    );
    assert_eq!(sess.mode, "docker");
    assert!(seen.is_none());

    // A relative workdir is joined to the cwd with a bare "/", uncleaned;
    // a raw run mounts read-write.
    env.set(config::CLAUDE_DOCKER_TOKEN_ENV, Some("tok"));
    let cfg = env.config();
    let mut sess = env.session("claude", "raw", config::CLAUDE_MODEL, "rel");
    run_claude_model(
        &cfg,
        &mut sess,
        "p",
        "low",
        "./rel",
        config::CLAUDE_MODEL,
        recorder(&mut seen, Ok(RunResult::default())),
    )
    .unwrap();
    let seen = seen.unwrap();
    let mut want = strings(&[
        "run",
        "--rm",
        "-i",
        "-v",
        &format!("{}/./rel:/workspace", env.work_str()),
        "-w",
        "/workspace",
        "-e",
        "ANTHROPIC_AUTH_TOKEN=tok",
        "rival-claude",
    ]);
    want.extend(raw_argv("low"));
    assert_eq!(seen.binary, "docker");
    assert_eq!(seen.args, want);
    assert!(seen.drop_env.is_empty());
    assert_eq!(
        seen.prompt,
        format!("{}\np", cfg.build_workdir_preamble(Path::new("./rel")))
    );
}

/// Go: TestClaudeAuthHint.
#[test]
fn claude_auth_hint_cases() {
    let dir = tempfile::tempdir().unwrap();
    let cases = [
        (
            "credit balance, sub mode",
            "Credit balance is too low",
            "",
            "",
            "subscription auth failed",
        ),
        (
            "credit balance, api mode",
            "Credit balance is too low",
            "api",
            "sk-x",
            "API auth failed",
        ),
        (
            "login prompt",
            "Please run /login to continue",
            "",
            "",
            "subscription auth failed",
        ),
        (
            "invalid key",
            "Invalid API key provided",
            "api",
            "sk-x",
            "API auth failed",
        ),
        (
            "model failure is not auth",
            "model overloaded, retry later",
            "",
            "",
            "",
        ),
    ];
    for (i, (name, log, auth, key, want)) in cases.into_iter().enumerate() {
        let mut env = Env::new();
        env.set("RIVAL_CLAUDE_AUTH", Some(auth))
            .set("ANTHROPIC_API_KEY", Some(key));
        let path = dir.path().join(format!("run{i}.log"));
        std::fs::write(&path, log).unwrap();
        let got = claude_auth_hint(&env.config(), &path);
        if want.is_empty() {
            assert_eq!(got, "", "{name}");
        } else {
            assert!(
                got.contains(want),
                "{name}: hint {got:?} does not contain {want:?}"
            );
        }
    }

    // Exact texts, and an invalid auth mode returns its config error.
    let path = dir.path().join("auth.log");
    std::fs::write(&path, "x authentication_error y").unwrap();
    let mut env = Env::new();
    assert_eq!(
        claude_auth_hint(&env.config(), &path),
        "rival: subscription auth failed — run `claude` once and /login (Pro/Max), or set RIVAL_CLAUDE_AUTH=api with a funded ANTHROPIC_API_KEY"
    );
    env.set("RIVAL_CLAUDE_AUTH", Some("api"))
        .set("ANTHROPIC_API_KEY", Some("k"));
    assert_eq!(
        claude_auth_hint(&env.config(), &path),
        "rival: API auth failed (RIVAL_CLAUDE_AUTH=api) — check ANTHROPIC_API_KEY and its credit balance, or unset RIVAL_CLAUDE_AUTH to use the claude CLI subscription login"
    );
    env.set("ANTHROPIC_API_KEY", None);
    assert_eq!(
        claude_auth_hint(&env.config(), &path),
        "RIVAL_CLAUDE_AUTH=api but ANTHROPIC_API_KEY is empty — set the key or unset RIVAL_CLAUDE_AUTH to use the claude CLI subscription login"
    );

    // missing log file
    assert_eq!(
        claude_auth_hint(&env.config(), &dir.path().join("nope.log")),
        ""
    );
}

/// Go: TestSetClaudeTransportModePreservesPlanTask.
#[test]
fn set_claude_transport_mode_preserves_plan_task() {
    let mut plan = Session {
        mode: "plan".into(),
        ..Session::default()
    };
    set_claude_transport_mode(&mut plan, "native");
    assert_eq!(plan.mode, "plan");

    let mut standalone = Session {
        mode: "raw".into(),
        ..Session::default()
    };
    set_claude_transport_mode(&mut standalone, "docker");
    assert_eq!(standalone.mode, "docker");
}

/// Go: TestSetClaudeTransportModePreservesTaskModes, plus security.
#[test]
fn set_claude_transport_mode_preserves_task_modes() {
    for (name, mode, want) in [
        ("plan survives", MODE_PLAN, MODE_PLAN),
        ("antislop survives", MODE_ANTISLOP, MODE_ANTISLOP),
        ("security survives", MODE_SECURITY, MODE_SECURITY),
        ("raw records transport", "raw", "native"),
        ("review records transport", "review", "native"),
    ] {
        let mut sess = Session {
            mode: mode.into(),
            ..Session::default()
        };
        set_claude_transport_mode(&mut sess, "native");
        assert_eq!(sess.mode, want, "{name}");
    }
}
