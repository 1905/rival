//! Codex executor tests: exact argv/request, error wrapping and preflight
//! checks against task-owned fakes.

use super::*;
#[cfg(unix)]
use crate::executor::testutil::retry_busy;
use crate::executor::testutil::{Env, Spawned, recorder, strings};

/// Both speeds normal. A test whose subject is not the tier adds it, so its
/// argv has no tier pairs (the speed is fast by default).
const TIER_OFF: &str = "speed:\n  codex: normal\n  sol: normal\n";

/// An [`Env`] whose config sets both speeds to normal.
fn tier_off_env() -> Env {
    let env = Env::new();
    add_config(&env, TIER_OFF);
    env
}

fn argv(effort: &str, workdir: &str) -> Vec<String> {
    argv_sandbox(effort, workdir, "read-only")
}

fn argv_sandbox(effort: &str, workdir: &str, sandbox: &str) -> Vec<String> {
    let effort_arg = format!("model_reasoning_effort={effort}");
    strings(&[
        "exec",
        "-C",
        workdir,
        "-m",
        config::CODEX_MODEL,
        "-c",
        &effort_arg,
        "--sandbox",
        sandbox,
        "--ephemeral",
        "--skip-git-repo-check",
        "--color",
        "never",
        "-",
    ])
}

/// The explicit model and effort reach argv (exact vector).
#[test]
fn codex_run_args_uses_explicit_model_and_effort() {
    for effort in ["high", "ultra"] {
        let args = codex_run_args(config::CODEX_MODEL, effort, "/repo", Speed::Normal, true);
        assert_eq!(args, argv(effort, "/repo"));
        let joined = args.join(" ");
        assert!(joined.contains(&format!("-m {}", config::CODEX_MODEL)));
        assert!(joined.contains(&format!("model_reasoning_effort={effort}")));
        assert!(joined.contains("--sandbox read-only"));
    }
    // An empty workdir stays an empty argument after -C.
    assert_eq!(
        codex_run_args(config::CODEX_MODEL, "", "", Speed::Normal, true),
        argv("", "")
    );
}

/// A raw prompt gets a writable sandbox, a review keeps read-only.
#[test]
fn codex_sandbox_follows_the_run_kind() {
    let raw = codex_run_args(config::CODEX_MODEL, "xhigh", "/repo", Speed::Normal, false);
    assert_eq!(raw, argv_sandbox("xhigh", "/repo", "workspace-write"));
    let review = codex_run_args(config::CODEX_MODEL, "xhigh", "/repo", Speed::Normal, true);
    assert_eq!(review, argv_sandbox("xhigh", "/repo", "read-only"));
}

#[test]
fn codex_passes_ultra_and_xhigh_through_unaliased() {
    for effort in ["xhigh", "ultra"] {
        let args = codex_run_args(config::CODEX_MODEL, effort, "/tmp", Speed::Normal, true);
        assert!(
            args.contains(&format!("model_reasoning_effort={effort}")),
            "{args:?}"
        );
    }
}

/// Unsupported, `sol` and empty models are rejected. Nothing is spawned.
#[test]
fn run_codex_model_rejects_unsupported_sol_and_empty() {
    let env = Env::new();
    let cfg = env.config();
    for model in ["retired-model", config::GPT56_SOL_MODEL, ""] {
        let mut sess = Session::default();
        let mut seen = None;
        let err = run_codex_model_with(
            &cfg,
            &mut sess,
            "review",
            "high",
            "/repo",
            model,
            true,
            None,
            recorder(&mut seen, Ok(RunResult::default())),
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("unsupported codex model {:?}", model)
        );
        assert!(seen.is_none(), "{model}: spawned");
    }
}

#[test]
fn run_codex_model_builds_the_exact_request() {
    let mut env = tier_off_env();
    env.set("OPENAI_API_KEY", Some("kept-for-codex"));
    let cfg = env.config();
    let work = env.work_str();
    let mut sess = env.session("codex", "review", config::CODEX_MODEL, &work);
    let mut seen = None;
    let want = RunResult {
        exit_code: 3,
        output_bytes: 7,
        output_lines: 1,
    };
    let got = run_codex_model_with(
        &cfg,
        &mut sess,
        "find the bug",
        "xhigh",
        &work,
        config::CODEX_MODEL,
        true,
        None,
        recorder(&mut seen, Ok(want)),
    )
    .unwrap();
    assert_eq!(got, want);
    let seen = seen.unwrap();
    assert_eq!(
        seen,
        Spawned {
            binary: "codex".into(),
            args: argv("xhigh", &work),
            env: vec![],
            prompt: format!(
                "{}\n\n{}\nfind the bug",
                config::SYSTEM_PROMPT,
                cfg.build_workdir_preamble(Path::new(&work))
            ),
            drop_env: vec![],
            environ: cfg.environ().to_vec(),
            mode: "review".into(),
            account: String::new(),
            log: None,
        }
    );
}

/// "Codex", "codex" and the model id all become the label.
#[test]
fn run_codex_model_wraps_errors_with_the_public_label() {
    let env = Env::new();
    let cfg = env.config();
    let mut sess = Session::default();
    let mut seen = None;
    let err = run_codex_model_with(
        &cfg,
        &mut sess,
        "p",
        "high",
        "/repo",
        config::CODEX_MODEL,
        true,
        None,
        recorder(
            &mut seen,
            Err(anyhow!("start codex: OpenAI Codex failed for gpt-6-astra")),
        ),
    )
    .unwrap_err();
    assert_eq!(
        format!("{err:#}"),
        "codex runtime: start codex: OpenAI codex failed for codex"
    );
}

#[cfg(unix)]
#[test]
fn codex_preflight_reports_missing_runtime_and_auth() {
    let env = Env::new();
    let cfg = env.config();
    assert_eq!(
        codex_preflight_for(&cfg, config::CODEX_MODEL)
            .unwrap_err()
            .to_string(),
        "codex runtime is not installed"
    );

    // Logged out: the combined output follows the message.
    env.fake(
        "codex",
        "#!/bin/sh\n[ \"$1 $2\" = 'login status' ] || exit 90\necho 'Not logged in'\necho 'warn' >&2\nexit 1\n",
    );
    let err = retry_busy(
        || codex_preflight_for(&cfg, config::CODEX_MODEL),
        |r| format!("{r:?}"),
    );
    assert_eq!(
        err.unwrap_err().to_string(),
        "codex authentication is unavailable\nNot logged in\nwarn\n"
    );

    env.fake(
        "codex",
        "#!/bin/sh\n[ \"$1 $2\" = 'login status' ] || exit 90\necho 'Logged in'\n",
    );
    retry_busy(
        || codex_preflight_for(&cfg, config::CODEX_MODEL),
        |r| format!("{r:?}"),
    )
    .unwrap();
}

/// End to end through the real subprocess runner: the fake reads the whole
/// prompt, then prints its argv and the prompt it got.
#[cfg(unix)]
#[test]
fn run_codex_model_runs_the_fake_with_argv_and_prompt() {
    let env = tier_off_env();
    let cfg = env.config();
    env.fake(
        "codex",
        "#!/bin/sh\nin=$(/bin/cat)\nprintf '%s\\n' \"$@\"\nprintf '%s\\n' \"$in\" | /usr/bin/tail -n 1\n",
    );
    let work = env.work_str();
    let mut sess = env.session("codex", "raw", config::CODEX_MODEL, &work);
    let mut out = Vec::new();
    let result = retry_busy(
        || {
            out.clear();
            run_codex_model(
                &Context::background(),
                &cfg,
                &mut sess,
                "last line",
                "ultra",
                &work,
                config::CODEX_MODEL,
                true,
                None,
                Some(&mut out),
            )
        },
        |r| format!("{r:?}"),
    )
    .unwrap();
    assert_eq!(result.exit_code, 0);
    let mut want = argv("ultra", &work).join("\n");
    want.push_str("\nlast line\n");
    assert_eq!(String::from_utf8(out).unwrap(), want);
}

/// The adapter hands the caller's log file to the spawn step.
#[test]
fn run_codex_model_forwards_the_log_file() {
    let env = Env::new();
    let cfg = env.config();
    let work = env.work_str();
    let mut sess = env.session("codex", "review", config::CODEX_MODEL, &work);
    let mut seen = None;
    run_codex_model_with(
        &cfg,
        &mut sess,
        "p",
        "low",
        &work,
        config::CODEX_MODEL,
        true,
        Some("/home/s/sessions/x.log.repair.log"),
        recorder(&mut seen, Ok(RunResult::default())),
    )
    .unwrap();
    assert_eq!(
        seen.unwrap().log.as_deref(),
        Some("/home/s/sessions/x.log.repair.log")
    );
}

// ---- the proxy route ----

const PROXY_KEY: &str = "test-proxy-key-0000";

/// An Env whose config enables the Codex proxy route at `url` with an
/// empty prefix, and the fake key in `RIVAL_PROXY_KEY`.
fn proxy_env_at(url: &str) -> Env {
    let mut env = Env::new();
    let dir = env.home.path().join(".rival");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("config.yaml"),
        format!("proxy:\n  url: {url}\n  codex:\n    enabled: true\n"),
    )
    .unwrap();
    env.set("RIVAL_PROXY_KEY", Some(PROXY_KEY));
    env
}

/// Appends `yaml` to the env's `~/.rival/config.yaml`.
fn add_config(env: &Env, yaml: &str) {
    let dir = env.home.path().join(".rival");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.yaml");
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    std::fs::write(&path, old + yaml).unwrap();
}

/// The direct argv with the two provider `-c` pairs after `-m <model>`,
/// exactly as P0 ran them against Codex CLI 0.161.0.
fn proxy_argv(effort: &str, workdir: &str, base: &str) -> Vec<String> {
    let mut args = argv(effort, workdir);
    let provider = format!(
        "model_providers.rival_proxy={{ name = \"rival proxy\", base_url = \"{base}/v1\", env_key = \"RIVAL_PROXY_KEY\", wire_api = \"responses\" }}"
    );
    args.splice(
        5..5,
        strings(&["-c", "model_provider=\"rival_proxy\"", "-c", &provider]),
    );
    args
}

fn run_recorded(env: &Env) -> anyhow::Result<(Spawned, Session)> {
    let cfg = env.config();
    let work = env.work_str();
    let mut sess = env.session("codex", "review", config::CODEX_MODEL, &work);
    let mut seen = None;
    run_codex_model_with(
        &cfg,
        &mut sess,
        "find the bug",
        "xhigh",
        &work,
        config::CODEX_MODEL,
        true,
        None,
        recorder(&mut seen, Ok(RunResult::default())),
    )?;
    Ok((seen.expect("spawned"), sess))
}

/// The provider `-c` strings are pinned byte for byte.
#[test]
fn codex_proxy_args_pin_the_provider_values() {
    let args = codex_proxy_run_args(
        "gpt-6-astra",
        "high",
        "/repo",
        Speed::Normal,
        true,
        "http://127.0.0.1:8999",
    )
    .unwrap();
    assert_eq!(
        args,
        strings(&[
            "exec",
            "-C",
            "/repo",
            "-m",
            "gpt-6-astra",
            "-c",
            "model_provider=\"rival_proxy\"",
            "-c",
            r#"model_providers.rival_proxy={ name = "rival proxy", base_url = "http://127.0.0.1:8999/v1", env_key = "RIVAL_PROXY_KEY", wire_api = "responses" }"#,
            "-c",
            "model_reasoning_effort=high",
            "--sandbox",
            "read-only",
            "--ephemeral",
            "--skip-git-repo-check",
            "--color",
            "never",
            "-",
        ])
    );
}

/// A URL that would need TOML escaping is refused, not escaped.
#[test]
fn codex_proxy_args_refuse_a_toml_unsafe_url() {
    for url in [
        "http://h:1/a\"b",
        "http://h:1/a\\b",
        "http://h:1/a\nb",
        "http://h:1/a\u{7f}b",
        "http://h:1/a\u{1}b",
    ] {
        let err =
            codex_proxy_run_args("gpt-6-astra", "high", "/repo", Speed::Normal, true, url)
                .unwrap_err();
        assert_eq!(
            err.to_string(),
            format!(
                "proxy URL {url:?} cannot go in a codex -c value: it has a quote, a backslash or a control character — run rival config set proxy.url <url>"
            ),
        );
    }
}

#[test]
fn proxy_request_sets_the_provider_args_and_the_key_env() {
    let mut env = proxy_env_at("http://127.0.0.1:8999/v1");
    add_config(&env, TIER_OFF);
    env.set("OPENAI_API_KEY", Some("kept-for-codex"));
    let (seen, sess) = run_recorded(&env).unwrap();
    assert_eq!(seen.binary, "codex");
    assert_eq!(
        seen.args,
        proxy_argv("xhigh", &env.work_str(), "http://127.0.0.1:8999")
    );
    assert_eq!(seen.env, strings(&["RIVAL_PROXY_KEY=test-proxy-key-0000"]));
    assert!(seen.drop_env.is_empty());
    assert!(!seen.args.iter().any(|a| a.contains(PROXY_KEY)));
    assert_eq!(seen.account, "proxy");
    // Empty prefix: the wire id is the bare model, and the session keeps it.
    assert_eq!(sess.model, config::CODEX_MODEL);
    assert_eq!(
        (sess.route.as_str(), sess.wire_model.as_str()),
        ("proxy", config::CODEX_MODEL)
    );
    assert_eq!(sess.account, "proxy");
}

/// A model prefix goes on the `-m` value only.
#[test]
fn proxy_request_puts_the_prefix_on_the_wire_id() {
    let env = Env::new();
    let dir = env.home.path().join(".rival");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("config.yaml"),
        "proxy:\n  url: http://127.0.0.1:8999\n  codex:\n    enabled: true\n    model_prefix: team_\n",
    )
    .unwrap();
    let mut env = env;
    env.set("RIVAL_PROXY_KEY", Some(PROXY_KEY));
    let (seen, sess) = run_recorded(&env).unwrap();
    let wire = format!("team_/{}", config::CODEX_MODEL);
    assert_eq!(seen.args[4], wire);
    assert_eq!(sess.wire_model, wire);
}

/// The key reaches the child through `Request.env`, although the inherited
/// `RIVAL_PROXY_KEY` is blocked for every child.
#[test]
fn proxy_key_reaches_the_child_env() {
    let env = proxy_env_at("http://127.0.0.1:8999");
    let (seen, _) = run_recorded(&env).unwrap();
    let drop: Vec<&str> = seen.drop_env.iter().map(String::as_str).collect();
    let req = Request {
        binary: "codex",
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
    let keys: Vec<&String> = child
        .iter()
        .filter(|kv| kv.starts_with("RIVAL_PROXY_KEY="))
        .collect();
    assert_eq!(keys, vec!["RIVAL_PROXY_KEY=test-proxy-key-0000"]);
}

/// With no proxy route the request is byte-identical to the direct one:
/// a disabled route and `RIVAL_PROXY=off`.
#[test]
fn direct_request_is_unchanged_when_the_proxy_is_off() {
    let base = tier_off_env();
    let (want, want_sess) = run_recorded(&base).unwrap();
    assert_eq!(want.args, argv("xhigh", &base.work_str()));
    assert_eq!(
        (want_sess.route.as_str(), want_sess.wire_model.as_str()),
        ("", "")
    );
    let on = "proxy:\n  url: http://127.0.0.1:8999\n  codex:\n    enabled: true\n";
    let off = "proxy:\n  url: http://127.0.0.1:8999\n  codex:\n    enabled: false\n";
    for (name, extra, yaml) in [
        ("disabled", vec![("RIVAL_PROXY_KEY", PROXY_KEY)], off),
        (
            "RIVAL_PROXY=off",
            vec![("RIVAL_PROXY_KEY", PROXY_KEY), ("RIVAL_PROXY", "off")],
            on,
        ),
    ] {
        let mut env = Env::new();
        for (k, v) in &extra {
            env.set(k, Some(v));
        }
        let dir = env.home.path().join(".rival");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.yaml"), format!("{yaml}{TIER_OFF}")).unwrap();
        let (got, sess) = run_recorded(&env).unwrap();
        assert_eq!(got.args, argv("xhigh", &env.work_str()), "{name}");
        assert_eq!(got.env, want.env, "{name}");
        assert_eq!(got.drop_env, want.drop_env, "{name}");
        assert_eq!(got.account, want.account, "{name}");
        assert_eq!(
            (sess.route.as_str(), sess.wire_model.as_str()),
            ("", ""),
            "{name}"
        );
    }
}

#[test]
fn proxy_route_errors_stop_the_run() {
    let mut env = proxy_env_at("http://127.0.0.1:8999");
    env.set("RIVAL_PROXY_KEY", None);
    let err = run_recorded(&env).unwrap_err();
    assert_eq!(
        format!("{err:#}"),
        "codex runtime: proxy key missing — run rival config key set"
    );
}

/// On the proxy route the preflight checks the binary, then the proxy; it
/// never runs `codex login status`.
#[cfg(unix)]
#[test]
fn codex_preflight_runs_the_proxy_preflight_not_login_status() {
    use crate::proxy::testserver::serve;
    // Not installed: the binary check comes first, before any request.
    let (url, seen) = serve(200, r#"{"data":[{"id":"gpt-6-astra"}]}"#);
    let env = proxy_env_at(&url);
    assert_eq!(
        codex_preflight_for(&env.config(), config::CODEX_MODEL)
            .unwrap_err()
            .to_string(),
        "codex runtime is not installed"
    );
    assert!(seen.lock().unwrap().is_empty());

    // A fake that fails any call: login status would fail the preflight.
    env.fake("codex", "#!/bin/sh\nexit 97\n");
    codex_preflight_for(&env.config(), config::CODEX_MODEL).unwrap();
    assert_eq!(seen.lock().unwrap()[0].1, format!("Bearer {PROXY_KEY}"));

    let (url, _) = serve(401, "{}");
    let env = proxy_env_at(&url);
    env.fake("codex", "#!/bin/sh\nexit 97\n");
    assert_eq!(
        codex_preflight_for(&env.config(), config::CODEX_MODEL)
            .unwrap_err()
            .to_string(),
        "proxy rejected the key (401) — run rival config key set"
    );

    let (url, _) = serve(200, r#"{"data":[{"id":"emcd_/claude-opus-5-5"}]}"#);
    let env = proxy_env_at(&url);
    env.fake("codex", "#!/bin/sh\nexit 97\n");
    assert_eq!(
        codex_preflight_for(&env.config(), config::CODEX_MODEL)
            .unwrap_err()
            .to_string(),
        "proxy has no codex account — log in on the proxy (-codex-device-login)"
    );
}

#[test]
fn codex_auth_hint_is_proxy_only() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("x.log");
    std::fs::write(
        &log,
        "Reconnecting... 5/5\nunexpected status 401 Unauthorized\n",
    )
    .unwrap();
    // Direct route: no hint.
    let env = Env::new();
    assert_eq!(
        codex_auth_hint(&env.config(), config::CODEX_MODEL, &log),
        ""
    );
    // Proxy route: names the key and the prefix.
    let env = proxy_env_at("http://127.0.0.1:1");
    assert_eq!(
        codex_auth_hint(&env.config(), config::CODEX_MODEL, &log),
        format!(
            "rival: proxy request failed for {} — check the proxy key (rival config key set) and proxy.codex.model_prefix (now \"\"); a 429 means the account is at its limit",
            config::CODEX_MODEL
        )
    );
    // Other failures: no hint.
    std::fs::write(&log, "panic: something else\n").unwrap();
    assert_eq!(
        codex_auth_hint(&env.config(), config::CODEX_MODEL, &log),
        ""
    );
    // An account at its limit (429). The list call fails here (nothing
    // listens), so no other prefix is named.
    std::fs::write(&log, "429 Too Many Requests: rate_limit_error\n").unwrap();
    assert_eq!(
        codex_auth_hint(&env.config(), config::CODEX_MODEL, &log),
        format!(
            "rival: proxy account (no prefix) is at its limit (429); no other prefix serves {} — wait for the limit to reset",
            config::CODEX_MODEL
        )
    );
}

/// Sol 6.1 runs on the codex runtime, direct and through the proxy.
#[test]
fn sol_runs_direct_and_through_the_proxy() {
    let run = |env: &Env| {
        let cfg = env.config();
        let work = env.work_str();
        let mut sess = env.session("codex", "review", config::SOL_MODEL, &work);
        let mut seen = None;
        run_codex_model_with(
            &cfg,
            &mut sess,
            "p",
            "ultra",
            &work,
            config::SOL_MODEL,
            true,
            None,
            recorder(&mut seen, Ok(RunResult::default())),
        )
        .unwrap();
        (seen.unwrap(), sess)
    };
    let env = tier_off_env();
    let (seen, _) = run(&env);
    let mut want = argv("ultra", &env.work_str());
    want[4] = config::SOL_MODEL.to_string();
    assert_eq!(seen.args, want);

    let env = proxy_env_at("http://127.0.0.1:8999");
    add_config(&env, TIER_OFF);
    let (seen, sess) = run(&env);
    let mut want = proxy_argv("ultra", &env.work_str(), "http://127.0.0.1:8999");
    want[4] = config::SOL_MODEL.to_string();
    assert_eq!(seen.args, want);
    assert_eq!(sess.wire_model, config::SOL_MODEL);
    assert!(is_codex_model(config::SOL_MODEL));
    assert!(!is_codex_model(config::GPT56_SOL_MODEL));
}

/// The tier pairs of `tier`, as they follow `-m <model>`.
fn tier_pairs(tier: &str) -> Vec<String> {
    vec![
        "-c".to_string(),
        format!("service_tier=\"{tier}\""),
        "-c".to_string(),
        format!("features.{tier}_mode=true"),
    ]
}

/// A model's `speed.<label>` (fast by default) puts its tier pairs after
/// `-m <model>`, after the proxy's provider pairs, on both routes, and the
/// session records the tier. Normal adds no pairs; the other model's speed
/// does not count.
#[test]
fn speed_reaches_argv_direct_and_through_the_proxy() {
    let run = |env: &Env, model: &str| {
        let cfg = env.config();
        let work = env.work_str();
        let mut sess = env.session("codex", "review", model, &work);
        let mut seen = None;
        run_codex_model_with(
            &cfg,
            &mut sess,
            "p",
            "xhigh",
            &work,
            model,
            true,
            None,
            recorder(&mut seen, Ok(RunResult::default())),
        )
        .unwrap();
        (seen.unwrap(), sess)
    };
    for (model, on, other) in [
        (config::CODEX_MODEL, "codex", "sol"),
        (config::SOL_MODEL, "sol", "codex"),
    ] {
        for (config, tier) in [
            (String::new(), "fast"),
            ("speed:\n".to_string(), "fast"),
            (format!("speed:\n  {on}: fast\n  {other}: normal\n"), "fast"),
            (format!("speed:\n  {on}: ultrafast\n"), "ultrafast"),
            (format!("ultrafast:\n  {on}: true\n"), "ultrafast"),
        ] {
            let env = Env::new();
            add_config(&env, &config);
            let (seen, sess) = run(&env, model);
            let mut want = argv("xhigh", &env.work_str());
            want[4] = model.to_string();
            want.splice(5..5, tier_pairs(tier));
            assert_eq!(seen.args, want, "{model} {config:?}");
            assert_eq!(sess.service_tier, tier);

            let env = proxy_env_at("http://127.0.0.1:8999");
            add_config(&env, &config);
            let (seen, sess) = run(&env, model);
            let mut want = proxy_argv("xhigh", &env.work_str(), "http://127.0.0.1:8999");
            want[4] = model.to_string();
            want.splice(9..9, tier_pairs(tier));
            assert_eq!(seen.args, want, "{model} proxy {config:?}");
            assert_eq!(sess.service_tier, tier);
        }

        // Its own speed normal, the other one ultrafast: no tier.
        let env = Env::new();
        add_config(
            &env,
            &format!("speed:\n  {on}: normal\n  {other}: ultrafast\n"),
        );
        let (seen, sess) = run(&env, model);
        let mut want = argv("xhigh", &env.work_str());
        want[4] = model.to_string();
        assert_eq!(seen.args, want, "{model} normal, {other} ultrafast");
        assert_eq!(sess.service_tier, "");
    }
}

/// No tier: the argv has no tier pairs and nothing is flagged.
#[test]
fn no_tier_leaves_argv_and_session_unchanged() {
    let env = tier_off_env();
    let (seen, sess) = run_recorded(&env).unwrap();
    assert_eq!(seen.args, argv("xhigh", &env.work_str()));
    assert!(!sess.service_tier_omitted);
}

const TIER_WARNING: &str = "warning: Configured service tier `ultrafast` is not advertised as supported for model `gpt-6.1-sol` and will be omitted from requests.";

/// [`TIER_WARNING`] for another tier name.
fn warning_for(tier: &str) -> String {
    TIER_WARNING.replace("`ultrafast`", &format!("`{tier}`"))
}

/// A Codex log: banner, the echoed prompt, `between`, then the reply.
fn tier_log(prompt: &str, between: &str, reply: &str) -> String {
    format!(
        "OpenAI Codex v0.162.1\n--------\nuser\n{prompt}\n{between}codex\n{reply}\ntokens used\n9\n{reply}\n"
    )
}

/// Only Codex's warning lines right after the echoed prompt count. The
/// full warning line inside the prompt or the reply does not.
#[test]
fn tier_omitted_reads_only_the_warnings_after_the_prompt_echo() {
    let prompt = "review the tier code";
    let warned = format!("{TIER_WARNING}\n");
    assert!(tier_omitted(
        &tier_log(prompt, &warned, "ok"),
        prompt,
        "ultrafast"
    ));
    // Other config warnings may come first; blank lines are skipped.
    let mixed = format!("\nwarning: something else\n  {TIER_WARNING}  \n");
    assert!(tier_omitted(
        &tier_log(prompt, &mixed, "ok"),
        prompt,
        "ultrafast"
    ));
    // Codex maps fast to priority: a fast run counts either name, and only
    // the tier it asked for.
    for name in ["fast", "priority"] {
        let warned = format!("{}\n", warning_for(name));
        let log = tier_log(prompt, &warned, "ok");
        assert!(tier_omitted(&log, prompt, "fast"), "{name}");
        assert!(!tier_omitted(&log, prompt, "ultrafast"), "{name}");
    }
    assert!(!tier_omitted(
        &tier_log(prompt, &warned, "ok"),
        prompt,
        "fast"
    ));
    assert!(!tier_omitted(&tier_log(prompt, &warned, "ok"), prompt, ""));

    let quoted = format!("explain this line:\n{TIER_WARNING}");
    for (log, prompt) in [
        // The full warning line inside the prompt.
        (tier_log(&quoted, "", "ok"), quoted.as_str()),
        // The full warning line inside the reply.
        (tier_log(prompt, "", TIER_WARNING), prompt),
        // No echo of the prompt.
        (format!("{TIER_WARNING}\n"), prompt),
        // Another tier, an empty slug, a cut-off line.
        (
            tier_log(
                prompt,
                &format!("{}\n", TIER_WARNING.replace("`ultrafast`", "`priority`")),
                "ok",
            ),
            prompt,
        ),
        (
            tier_log(
                prompt,
                &format!("{}\n", TIER_WARNING.replace("`gpt-6.1-sol`", "``")),
                "ok",
            ),
            prompt,
        ),
        (
            tier_log(
                prompt,
                "warning: Configured service tier `ultrafast` is not advertised as supported for model `gpt-6.1-sol`\n",
                "ok",
            ),
            prompt,
        ),
    ] {
        assert!(!tier_omitted(&log, prompt, "ultrafast"), "{log:?}");
    }
}

/// The run reads its own log after the spawn and sets the session flag
/// only when its speed asked for a tier and Codex warned about that tier.
#[test]
fn run_sets_service_tier_omitted_from_its_log() {
    let run = |speed: &str, between: &str| {
        let env = Env::new();
        add_config(&env, &format!("speed:\n  sol: {speed}\n"));
        let cfg = env.config();
        let work = env.work_str();
        let log = std::path::Path::new(&work).join("call.log");
        let mut sess = env.session("codex", "review", config::SOL_MODEL, &work);
        run_codex_model_with(
            &cfg,
            &mut sess,
            "p",
            "low",
            &work,
            config::SOL_MODEL,
            true,
            Some(log.to_str().unwrap()),
            |_, req| {
                std::fs::write(&log, tier_log(req.prompt, between, "ok")).unwrap();
                Ok(RunResult::default())
            },
        )
        .unwrap();
        sess.service_tier_omitted
    };
    let warned = format!("{TIER_WARNING}\n");
    assert!(run("ultrafast", &warned));
    assert!(!run("ultrafast", ""));
    assert!(!run("normal", &warned));
    assert!(!run("fast", &warned), "an ultrafast warning on a fast run");
    assert!(run("fast", &format!("{}\n", warning_for("priority"))));
}

/// A session on the ultrafast tier, `omitted` as Codex's warning sets it,
/// on `route`.
fn tier_session(route: &str, omitted: bool) -> Session {
    Session {
        cli: "codex".to_string(),
        model: config::SOL_MODEL.to_string(),
        route: route.to_string(),
        service_tier: "ultrafast".to_string(),
        service_tier_omitted: omitted,
        ..Session::default()
    }
}

/// An omitted tier gives the notice under the public label; the plan line
/// shows for ultrafast on the direct route only.
#[test]
fn tier_notice_names_the_plan_for_ultrafast_on_the_direct_route_only() {
    assert_eq!(
        tier_notice(&tier_session("", true)).as_deref(),
        Some(
            "rival: ⚠ ultrafast not applied — Codex omitted service_tier=ultrafast for sol because its model catalog does not list the tier; the run used the standard tier.\n\
             rival: Ultrafast needs the Pro $500 plan, or an Enterprise/Edu workspace with Ultrafast on.\n"
        )
    );
    let mut fast = tier_session("", true);
    fast.service_tier = "fast".to_string();
    assert_eq!(
        tier_notice(&fast).as_deref(),
        Some(
            "rival: ⚠ fast not applied — Codex omitted service_tier=fast for sol because its model catalog does not list the tier; the run used the standard tier.\n"
        )
    );
    let mut proxied = tier_session(ROUTE_PROXY, true);
    proxied.model = config::CODEX_MODEL.to_string();
    let notice = tier_notice(&proxied).expect("notice");
    assert!(notice.contains("for codex because"), "{notice}");
    assert!(
        !notice.contains("Pro $500"),
        "proxy route names no account: {notice}"
    );
    assert_eq!(tier_notice(&tier_session("", false)), None);
}

/// Only the head of the log is read: a warning past the window is not.
#[test]
fn log_head_is_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.log");
    let prompt = "p";
    let padding = "x".repeat(TIER_WARNING_WINDOW + 10);
    std::fs::write(
        &path,
        format!("user\n{prompt}\n{padding}\n{TIER_WARNING}\n"),
    )
    .unwrap();
    let head = log_head(path.to_str().unwrap(), prompt).unwrap();
    assert_eq!(head.len(), prompt.len() + TIER_WARNING_WINDOW);
    assert!(!head.contains(TIER_WARNING));
}
