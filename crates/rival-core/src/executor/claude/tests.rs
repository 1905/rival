//! Claude executor tests: exact argv vectors, auth
//! env stripping, error wrapping and transport-mode checks.

use super::*;
#[cfg(unix)]
use crate::executor::testutil::retry_busy;
use crate::executor::testutil::{Env, Spawned, recorder, strings};
use crate::session::{MODE_PLAN, MODE_SECURITY};

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

/// Review transport restrictions, plus the security task mode.
/// The fake drains the prompt first, fails if CLAUDECODE or
/// ANTHROPIC_API_KEY reached it, then prints its argv one per line.
#[cfg(unix)]
#[test]
fn claude_review_transport_restrictions() {
    for mode in ["review", "plan", "security", "raw"] {
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
        let read_only = mode == "review" || crate::session::is_task_mode(mode);
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
                    read_only,
                    None,
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

/// The docker review mount is read-only (exact argv).
#[cfg(unix)]
#[test]
fn claude_docker_review_mount_is_read_only() {
    let mut env = Env::new();
    env.set(config::CLAUDE_DOCKER_TOKEN_ENV, Some("fixture-token"));
    env.fake(
        "docker",
        "#!/bin/sh\n/bin/cat >/dev/null\nprintf '%s\\n' \"$@\"\nprintf 'token=%s\\n' \"$ANTHROPIC_AUTH_TOKEN\"\n",
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
                true,
                None,
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
    let name = crate::executor::claude_docker::container_name(&sess.id);
    let mut want = strings(&[
        "run",
        "--rm",
        "-i",
        "--name",
        &name,
        "-v",
        &format!("{repo}:/workspace:ro"),
        "-w",
        "/workspace",
        "-e",
        "ANTHROPIC_AUTH_TOKEN",
        "rival-claude",
    ]);
    want.extend(read_only_argv("medium"));
    // The token reaches docker through its environment, not its argv.
    assert_eq!(text, format!("{}\ntoken=fixture-token\n", want.join("\n")));
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
        env.fake_on_path("claude");
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
            false,
            None,
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
                log: None,
            },
            "auth {auth:?}"
        );
    }
}

#[test]
fn claude_errors_are_wrapped_with_the_public_label() {
    let mut env = Env::new();
    env.fake_on_path("claude");
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
        false,
        None,
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
        false,
        None,
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
        false,
        None,
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
        false,
        None,
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
        false,
        None,
        recorder(&mut seen, Ok(RunResult::default())),
    )
    .unwrap();
    let seen = seen.unwrap();
    let name = crate::executor::claude_docker::container_name(&sess.id);
    let mut want = strings(&[
        "run",
        "--rm",
        "-i",
        "--name",
        &name,
        "-v",
        &format!("{}/./rel:/workspace", env.work_str()),
        "-w",
        "/workspace",
        "-e",
        "ANTHROPIC_AUTH_TOKEN",
        "rival-claude",
    ]);
    want.extend(raw_argv("low"));
    assert_eq!(seen.binary, "docker");
    assert_eq!(seen.args, want);
    assert_eq!(seen.env, strings(&["ANTHROPIC_AUTH_TOKEN=tok"]));
    assert!(seen.drop_env.is_empty());
    assert_eq!(
        seen.prompt,
        format!("{}\np", cfg.build_workdir_preamble(Path::new("./rel")))
    );
}

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
        let got = claude_auth_hint(&env.config(), config::CLAUDE_MODEL, &path);
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
        claude_auth_hint(&env.config(), config::CLAUDE_MODEL, &path),
        "rival: subscription auth failed — run `claude` once and /login (Pro/Max), or set RIVAL_CLAUDE_AUTH=api with a funded ANTHROPIC_API_KEY"
    );
    env.set("RIVAL_CLAUDE_AUTH", Some("api"))
        .set("ANTHROPIC_API_KEY", Some("k"));
    assert_eq!(
        claude_auth_hint(&env.config(), config::CLAUDE_MODEL, &path),
        "rival: API auth failed (RIVAL_CLAUDE_AUTH=api) — check ANTHROPIC_API_KEY and its credit balance, or unset RIVAL_CLAUDE_AUTH to use the claude CLI subscription login"
    );
    env.set("ANTHROPIC_API_KEY", None);
    assert_eq!(
        claude_auth_hint(&env.config(), config::CLAUDE_MODEL, &path),
        "RIVAL_CLAUDE_AUTH=api but ANTHROPIC_API_KEY is empty — set the key or unset RIVAL_CLAUDE_AUTH to use the claude CLI subscription login"
    );

    // missing log file
    assert_eq!(
        claude_auth_hint(
            &env.config(),
            config::CLAUDE_MODEL,
            &dir.path().join("nope.log")
        ),
        ""
    );
}

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

/// Setting the transport mode keeps the task modes, security included.
#[test]
fn set_claude_transport_mode_preserves_task_modes() {
    for (name, mode, want) in [
        ("plan survives", MODE_PLAN, MODE_PLAN),
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

/// The first run replaces the session mode with its transport. A second
/// read-only run on the same session must stay read-only: the caller's flag
/// decides, not the mode.
#[test]
fn read_only_survives_the_transport_replacing_the_mode() {
    let mut env = Env::new();
    env.set("RIVAL_CLAUDE_AUTH", Some("subscription"));
    env.fake_on_path("claude");
    let cfg = env.config();
    let work = env.work_str();
    let mut sess = env.session("claude", "review", config::CLAUDE_MODEL, &work);
    for _ in 0..2 {
        let mut seen = None;
        run_claude_model(
            &cfg,
            &mut sess,
            "p",
            "medium",
            &work,
            config::CLAUDE_MODEL,
            true,
            None,
            recorder(&mut seen, Ok(RunResult::default())),
        )
        .unwrap();
        assert_eq!(seen.unwrap().args, read_only_argv("medium"));
        assert_eq!(sess.mode, "native");
    }
}

/// The native adapter hands the caller's log file to the spawn step.
#[test]
fn run_claude_native_forwards_the_log_file() {
    let env = Env::new();
    env.fake_on_path("claude");
    let cfg = env.config();
    let work = env.work_str();
    let mut sess = env.session("claude", "review", config::CLAUDE_MODEL, &work);
    let mut seen = None;
    run_claude_model(
        &cfg,
        &mut sess,
        "p",
        "low",
        &work,
        config::CLAUDE_MODEL,
        true,
        Some("/home/s/sessions/x.log.repair.log"),
        recorder(&mut seen, Ok(RunResult::default())),
    )
    .unwrap();
    assert_eq!(sess.mode, "native");
    assert_eq!(
        seen.unwrap().log.as_deref(),
        Some("/home/s/sessions/x.log.repair.log")
    );
}

// ---- the proxy route ----

const PROXY_KEY: &str = "test-proxy-key-0000";
const WIRE: &str = "emcd_/claude-opus-5-5";

/// An Env whose config enables the Claude proxy route at `url` with the
/// `emcd_` prefix, and the fake key in `RIVAL_PROXY_KEY`.
fn proxy_env_at(url: &str) -> Env {
    let mut env = Env::new();
    let dir = env.home.path().join(".rival");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("config.yaml"),
        format!("proxy:\n  url: {url}\n  claude:\n    enabled: true\n    model_prefix: emcd_\n"),
    )
    .unwrap();
    env.set("RIVAL_PROXY_KEY", Some(PROXY_KEY));
    env
}

/// The argv with the wire id in place of the bare model.
fn with_wire(mut args: Vec<String>) -> Vec<String> {
    args[2] = WIRE.to_string();
    args
}

fn run_recorded(env: &Env, read_only: bool) -> (Spawned, Session) {
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
        read_only,
        None,
        recorder(&mut seen, Ok(RunResult::default())),
    )
    .unwrap();
    (seen.unwrap(), sess)
}

#[test]
fn native_proxy_request_sets_the_wire_id_and_env() {
    for auth in [None, Some("api"), Some("subscription"), Some("bogus")] {
        let mut env = proxy_env_at("http://127.0.0.1:8999/v1");
        env.set("RIVAL_CLAUDE_AUTH", auth);
        env.fake_on_path("claude");
        let (seen, sess) = run_recorded(&env, false);
        assert_eq!(seen.binary, "claude");
        assert_eq!(seen.args, with_wire(raw_argv("low")), "{auth:?}");
        assert_eq!(
            seen.env,
            strings(&[
                "ANTHROPIC_BASE_URL=http://127.0.0.1:8999",
                "ANTHROPIC_API_KEY=test-proxy-key-0000",
            ])
        );
        assert_eq!(
            seen.drop_env,
            strings(&[
                "CLAUDECODE",
                "ANTHROPIC_AUTH_TOKEN",
                "ANTHROPIC_DEFAULT_OPUS_MODEL",
                "ANTHROPIC_DEFAULT_SONNET_MODEL",
                "ANTHROPIC_DEFAULT_HAIKU_MODEL",
                "CLAUDE_CODE_USE_BEDROCK",
                "CLAUDE_CODE_USE_VERTEX",
                "ANTHROPIC_BASE_URL",
                "ANTHROPIC_API_KEY",
            ])
        );
        assert!(!seen.args.iter().any(|a| a.contains(PROXY_KEY)));
        assert_eq!(seen.account, "proxy");
        assert_eq!(seen.mode, "native");
        // The session keeps the bare id.
        assert_eq!(sess.model, config::CLAUDE_MODEL);
        assert_eq!(
            (sess.route.as_str(), sess.wire_model.as_str()),
            ("proxy", WIRE)
        );
    }
    // Review restrictions stay.
    let env = proxy_env_at("http://127.0.0.1:8999");
    env.fake_on_path("claude");
    let (seen, _) = run_recorded(&env, true);
    assert_eq!(seen.args, with_wire(read_only_argv("low")));
}

/// The child gets the injected proxy pair and none of the inherited
/// variables that would send the run elsewhere.
#[test]
fn native_proxy_child_env_drops_the_inherited_routing() {
    let mut env = proxy_env_at("http://127.0.0.1:8999");
    for (k, v) in [
        ("ANTHROPIC_AUTH_TOKEN", "inherited-token"),
        ("ANTHROPIC_API_KEY", "inherited-key"),
        ("ANTHROPIC_BASE_URL", "https://elsewhere.example"),
        ("ANTHROPIC_DEFAULT_OPUS_MODEL", "x"),
        ("ANTHROPIC_DEFAULT_SONNET_MODEL", "x"),
        ("ANTHROPIC_DEFAULT_HAIKU_MODEL", "x"),
        ("CLAUDE_CODE_USE_BEDROCK", "1"),
        ("CLAUDE_CODE_USE_VERTEX", "1"),
        ("CLAUDECODE", "1"),
    ] {
        env.set(k, Some(v));
    }
    env.fake_on_path("claude");
    let (seen, _) = run_recorded(&env, false);
    let drop: Vec<&str> = seen.drop_env.iter().map(String::as_str).collect();
    let req = Request {
        binary: "claude",
        args: &seen.args,
        env: &seen.env,
        prompt: "",
        drop_env: &drop,
        environ: &seen.environ,
        log: None,
    };
    let child: Vec<String> =
        crate::executor::subprocess::dedup_env(&crate::executor::subprocess::child_env(&req))
            .unwrap()
            .into_iter()
            .map(|kv| kv.into_string().unwrap())
            .collect();
    let anthropic: Vec<&String> = child
        .iter()
        .filter(|kv| kv.starts_with("ANTHROPIC_") || kv.starts_with("CLAUDE"))
        .collect();
    assert_eq!(
        anthropic,
        vec![
            "ANTHROPIC_BASE_URL=http://127.0.0.1:8999",
            "ANTHROPIC_API_KEY=test-proxy-key-0000",
        ]
    );
}

/// With no proxy route the request is byte-identical to the direct one:
/// an absent block, a disabled route, and `RIVAL_PROXY=off`.
#[test]
fn direct_request_is_unchanged_when_the_proxy_is_off() {
    let mut base = Env::new();
    base.set("ANTHROPIC_API_KEY", Some("sk-x"));
    base.fake_on_path("claude");
    let (want, want_sess) = run_recorded(&base, true);
    assert_eq!(want.args, read_only_argv("low"));
    assert_eq!(
        (want_sess.route.as_str(), want_sess.wire_model.as_str()),
        ("", "")
    );

    let disabled = |extra: &[(&str, &str)], yaml: &str| {
        let mut env = Env::new();
        // The same temp dirs would differ, so compare against a twin env.
        env.set("ANTHROPIC_API_KEY", Some("sk-x"));
        for (k, v) in extra {
            env.set(k, Some(v));
        }
        let dir = env.home.path().join(".rival");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.yaml"), yaml).unwrap();
        env.fake_on_path("claude");
        let (got, sess) = run_recorded(&env, true);
        (got, sess, env)
    };
    let on = "proxy:\n  url: http://127.0.0.1:8999\n  claude:\n    enabled: true\n    model_prefix: emcd_\n";
    let off = "proxy:\n  url: http://127.0.0.1:8999\n  claude:\n    enabled: false\n    model_prefix: emcd_\n";
    for (name, extra, yaml) in [
        ("disabled", vec![("RIVAL_PROXY_KEY", PROXY_KEY)], off),
        (
            "RIVAL_PROXY=off",
            vec![("RIVAL_PROXY_KEY", PROXY_KEY), ("RIVAL_PROXY", "off")],
            on,
        ),
    ] {
        let (got, sess, env) = disabled(&extra, yaml);
        assert_eq!(got.binary, want.binary, "{name}");
        assert_eq!(got.args, want.args, "{name}");
        assert_eq!(got.env, want.env, "{name}");
        assert_eq!(got.drop_env, want.drop_env, "{name}");
        assert_eq!(got.account, want.account, "{name}");
        assert_eq!(
            got.prompt,
            format!(
                "{}\ndo it",
                env.config().build_workdir_preamble(env.work.path())
            ),
            "{name}"
        );
        assert_eq!(
            (sess.route.as_str(), sess.wire_model.as_str()),
            ("", ""),
            "{name}"
        );
    }
}

#[test]
fn docker_proxy_passes_names_and_maps_a_loopback_host() {
    for (url, inside, add_host) in [
        (
            "http://127.0.0.1:8317",
            "http://host.docker.internal:8317",
            true,
        ),
        (
            "http://localhost:8317",
            "http://host.docker.internal:8317",
            true,
        ),
        (
            "http://[::1]:8317",
            "http://host.docker.internal:8317",
            true,
        ),
        (
            "https://proxy.example:9000",
            "https://proxy.example:9000",
            false,
        ),
    ] {
        // No claude on PATH and no RIVAL_CLAUDE_TOKEN: Docker on the proxy.
        let env = proxy_env_at(url);
        let (seen, sess) = run_recorded(&env, false);
        let name = crate::executor::claude_docker::container_name(&sess.id);
        let mut want = strings(&[
            "run",
            "--rm",
            "-i",
            "--name",
            &name,
            "-v",
            &format!("{}:/workspace", env.work_str()),
            "-w",
            "/workspace",
        ]);
        if add_host {
            want.push("--add-host=host.docker.internal:host-gateway".into());
        }
        want.extend(strings(&[
            "-e",
            "ANTHROPIC_BASE_URL",
            "-e",
            "ANTHROPIC_API_KEY",
            "rival-claude",
        ]));
        want.extend(with_wire(raw_argv("low")));
        assert_eq!(seen.binary, "docker");
        assert_eq!(seen.args, want, "{url}");
        assert_eq!(
            seen.env,
            vec![
                format!("ANTHROPIC_BASE_URL={inside}"),
                format!("ANTHROPIC_API_KEY={PROXY_KEY}"),
            ]
        );
        assert!(!seen.args.iter().any(|a| a.contains(PROXY_KEY)));
        assert_eq!(seen.mode, "docker");
        assert_eq!(seen.account, "proxy");
        assert_eq!(sess.wire_model, WIRE);
    }
}

#[test]
fn proxy_route_errors_stop_the_run() {
    // The route is on but no key is set.
    let mut env = proxy_env_at("http://127.0.0.1:8999");
    env.set("RIVAL_PROXY_KEY", None);
    env.fake_on_path("claude");
    let cfg = env.config();
    let work = env.work_str();
    let mut sess = env.session("claude", "raw", config::CLAUDE_MODEL, &work);
    let mut seen = None;
    let err = run_claude_model(
        &cfg,
        &mut sess,
        "p",
        "low",
        &work,
        config::CLAUDE_MODEL,
        false,
        None,
        recorder(&mut seen, Ok(RunResult::default())),
    )
    .unwrap_err();
    assert_eq!(
        format!("{err:#}"),
        "claude runtime: proxy key missing — run rival config key set"
    );
    assert!(seen.is_none());
}

#[cfg(unix)]
#[test]
fn claude_preflight_runs_the_proxy_preflight() {
    use crate::proxy::testserver::serve;
    let (url, seen) = serve(
        200,
        r#"{"data":[{"id":"emcd_/claude-opus-5-5"},{"id":"emcd2_/claude-opus-5-5"}]}"#,
    );
    let env = proxy_env_at(&url);
    env.fake("claude", "#!/bin/sh\nexit 97\n");
    claude_preflight(&env.config()).unwrap();
    assert_eq!(seen.lock().unwrap()[0].1, format!("Bearer {PROXY_KEY}"));

    let (url, _) = serve(200, r#"{"data":[{"id":"emcd2_/claude-opus-5-5"}]}"#);
    let env = proxy_env_at(&url);
    env.fake("claude", "#!/bin/sh\nexit 97\n");
    assert_eq!(
        claude_preflight(&env.config()).unwrap_err().to_string(),
        "proxy does not serve emcd_/claude-opus-5-5; it serves claude-opus-5-5 as: emcd2_/claude-opus-5-5 — set proxy.claude.model_prefix"
    );

    let (url, _) = serve(401, "{}");
    let env = proxy_env_at(&url);
    env.fake("claude", "#!/bin/sh\nexit 97\n");
    assert_eq!(
        claude_preflight(&env.config()).unwrap_err().to_string(),
        "proxy rejected the key (401) — run rival config key set"
    );
}

#[test]
fn claude_auth_hint_proxy_branch() {
    use crate::proxy::testserver::serve;
    let (url, _) = serve(
        200,
        r#"{"data":[{"id":"emcd_/claude-opus-5-5"},{"id":"emcd2_/claude-opus-5-5"}]}"#,
    );
    let env = proxy_env_at(&url);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run.log");
    std::fs::write(
        &path,
        "API Error: 429 {\"type\":\"rate_limit_error\",\"message\":\"All credentials for model emcd_/claude-opus-5-5 are cooling down\"}",
    )
    .unwrap();
    assert_eq!(
        claude_auth_hint(&env.config(), config::CLAUDE_MODEL, &path),
        "rival: proxy account emcd_ is at its limit (429); other prefixes that serve claude-opus-5-5: emcd2_ — set proxy.claude.model_prefix"
    );
    std::fs::write(&path, "API Error: 401 Invalid API key").unwrap();
    assert_eq!(
        claude_auth_hint(&env.config(), config::CLAUDE_MODEL, &path),
        "rival: proxy request failed for emcd_/claude-opus-5-5 — check the proxy key (rival config key set) and proxy.claude.model_prefix (now \"emcd_\"); a 429 means the account is at its limit"
    );
    // Not /login on the proxy route; a plain model failure has no hint.
    std::fs::write(&path, "model overloaded, retry later").unwrap();
    assert_eq!(
        claude_auth_hint(&env.config(), config::CLAUDE_MODEL, &path),
        ""
    );
}

/// The leak guard: a provider that echoes the proxy key on stdout and
/// stderr leaves no trace of it in the session log, the mirror, or the
/// session record.
#[cfg(unix)]
#[test]
fn proxy_key_never_reaches_the_log_mirror_or_record() {
    let env = proxy_env_at("http://127.0.0.1:8999");
    env.fake(
        "claude",
        "#!/bin/sh\n/bin/cat >/dev/null\nprintf 'key=%s\\n' \"$ANTHROPIC_API_KEY\"\nprintf 'err %s tail' \"$ANTHROPIC_API_KEY\" >&2\nexit 1\n",
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
                true,
                None,
                Some(&mut out),
            )
        },
        |r| format!("{r:?}"),
    )
    .unwrap();
    assert_eq!(result.exit_code, 1);
    let mirror = String::from_utf8(out).unwrap();
    assert_eq!(mirror, "key=<redacted>\n");
    let log = std::fs::read_to_string(&sess.log_file).unwrap();
    assert!(!log.contains(PROXY_KEY), "{log}");
    assert!(log.contains("key=<redacted>\n"), "{log}");
    assert!(log.contains("err <redacted> tail"), "{log}");
    // An error text that quotes the key is scrubbed from the record.
    sess.fail(&env.paths(), 1, &format!("claude said {PROXY_KEY}"))
        .unwrap();
    let record =
        std::fs::read_to_string(env.paths().sessions_dir().join(format!("{}.json", sess.id)))
            .unwrap();
    assert!(!record.contains(PROXY_KEY), "{record}");
    assert!(record.contains("\"route\": \"proxy\""), "{record}");
    assert!(
        record.contains(&format!("\"wire_model\": \"{WIRE}\"")),
        "{record}"
    );
}
