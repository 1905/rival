//! Ports of Go `internal/config/{config,codex,kimi,security,antislop}_test.go`
//! plus golden pins of the ported Go texts. Every test builds its own
//! [`Config`] from an explicit env map; nothing reads or mutates the process
//! environment or the real `~/.rival`.

use super::*;
use std::fs;

/// No `HOME`/`RIVAL_HOME` in `env`, so no config file is read.
fn cfg(env: &[(&str, &str)]) -> Config {
    Config::new(
        Paths::from_home(Path::new("/nonexistent-rival-test-home")),
        env_map(env),
        None,
    )
}

fn env_map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn efforts(pairs: &[(&str, &str)]) -> UserConfig {
    UserConfig {
        efforts: pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        ..UserConfig::default()
    }
}

fn reviewer(name: &str) -> UserConfig {
    UserConfig {
        security: SecurityConfig {
            reviewer: name.to_string(),
        },
        ..UserConfig::default()
    }
}

/// A temp HOME holding `.rival/config.yaml` with `body`, loaded through
/// [`Config::new`].
fn loaded(body: &str) -> (tempfile::TempDir, Config) {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(".rival");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("config.yaml"), body).unwrap();
    let config = home_config(home.path(), &[]);
    (home, config)
}

/// `<home>/.rival/config.yaml` with host separators, as Go's
/// `filepath.Join` writes it.
fn config_path(home: &Path) -> PathBuf {
    home.join(".rival").join("config.yaml")
}

fn home_config(home: &Path, extra: &[(&str, &str)]) -> Config {
    let mut env = env_map(extra);
    env.insert(
        paths::HOME_VAR.to_string(),
        home.to_str().unwrap().to_string(),
    );
    Config::new(Paths::from_home(home), env, None)
}

const MIN: Duration = Duration::from_secs(60);

// ---- startup order: config.yaml before .env, getenv after ----

#[test]
fn runtime_env_refresh_keeps_the_initial_user_config_and_error() {
    let (home, initial) = loaded("efforts:\n  codex: high\n");
    assert_eq!(initial.default_effort_for_model(CODEX_MODEL), "high");
    // The file changes after the first load: a refresh must not re-read it.
    fs::write(
        home.path().join(".rival/config.yaml"),
        "efforts:\n  codex: low\n",
    )
    .unwrap();
    let other = tempfile::tempdir().unwrap();
    let refreshed = initial.with_runtime_env(
        Paths::from_home(other.path()),
        env_map(&[("RIVAL_RUN_TIMEOUT", "7m"), ("FROM_DOTENV", "1")]),
        vec![OsString::from("FROM_DOTENV=1"), OsString::from("Z=2")],
        Some(other.path().to_path_buf()),
    );
    assert_eq!(refreshed.default_effort_for_model(CODEX_MODEL), "high");
    assert_eq!(refreshed.getenv("FROM_DOTENV"), "1");
    assert_eq!(
        refreshed.getenv(paths::HOME_VAR),
        "",
        "env replaced, not merged"
    );
    assert_eq!(refreshed.run_timeout(), 7 * MIN);
    assert_eq!(
        refreshed.environ(),
        [OsString::from("FROM_DOTENV=1"), OsString::from("Z=2")]
    );
    assert_eq!(refreshed.cwd(), Some(other.path()));
    assert_eq!(refreshed.paths(), &Paths::from_home(other.path()));

    let (_home, bad) = loaded("efforts:\n  codex: bogus\n");
    let err = bad.user_config_error().unwrap().clone();
    let refreshed = bad.with_runtime_env(
        Paths::from_home(other.path()),
        HashMap::new(),
        Vec::new(),
        None,
    );
    assert_eq!(refreshed.user_config_error(), Some(&err));
}

// ---- config_test.go ----

#[test]
fn max_concurrent() {
    // Go TestMaxConcurrent.
    let cases = [
        ("unset uses two", "", 2),
        ("explicit override", "3", 3),
        ("zero falls back", "0", 2),
        ("negative falls back", "-1", 2),
        ("invalid falls back", "many", 2),
        ("plus sign like Atoi", "+4", 4),
        ("overflow falls back", "99999999999999999999", 2),
    ];
    for (name, env, want) in cases {
        let got = cfg(&[("RIVAL_MAX_CONCURRENT", env)]).max_concurrent();
        assert_eq!(got, want, "{name}");
    }
}

#[test]
fn run_timeout() {
    // Go TestRunTimeout.
    let cases = [
        ("unset → default", "", DEFAULT_RUN_TIMEOUT),
        ("explicit duration", "10m", 10 * MIN),
        ("zero disables", "0", Duration::ZERO),
        ("0s disables", "0s", Duration::ZERO),
        ("garbage → default", "banana", DEFAULT_RUN_TIMEOUT),
        ("negative → default", "-5m", DEFAULT_RUN_TIMEOUT),
        ("go duration syntax", "1h30m", 90 * MIN),
    ];
    for (name, env, want) in cases {
        let got = cfg(&[("RIVAL_RUN_TIMEOUT", env)]).run_timeout();
        assert_eq!(got, want, "{name}");
    }
}

#[test]
fn queue_timeout() {
    let cases = [
        ("unset → default", "", DEFAULT_QUEUE_TIMEOUT),
        ("explicit", "10m", 10 * MIN),
        ("zero → default", "0", DEFAULT_QUEUE_TIMEOUT),
        ("negative → default", "-1m", DEFAULT_QUEUE_TIMEOUT),
        ("garbage → default", "soon", DEFAULT_QUEUE_TIMEOUT),
        ("fraction", "1.5s", Duration::from_millis(1500)),
    ];
    for (name, env, want) in cases {
        let got = cfg(&[("RIVAL_QUEUE_TIMEOUT", env)]).queue_timeout();
        assert_eq!(got, want, "{name}");
    }
}

#[test]
fn max_run_wait() {
    // Go TestMaxRunWait: queue 30m + 2*run 30m + 5m margin = 95m by default.
    let got = cfg(&[("RIVAL_QUEUE_TIMEOUT", ""), ("RIVAL_RUN_TIMEOUT", "")]).max_run_wait();
    assert_eq!(got, (95 * MIN).as_nanos() as i64, "default");
    // 10 + 2*20 + 5 = 55m
    let got = cfg(&[("RIVAL_QUEUE_TIMEOUT", "10m"), ("RIVAL_RUN_TIMEOUT", "20m")]).max_run_wait();
    assert_eq!(
        got,
        (55 * MIN).as_nanos() as i64,
        "scales with configured timeouts"
    );
    let got = cfg(&[("RIVAL_QUEUE_TIMEOUT", "30m"), ("RIVAL_RUN_TIMEOUT", "0")]).max_run_wait();
    assert_eq!(
        got,
        (35 * MIN).as_nanos() as i64,
        "run timeout disabled → queue + margin only"
    );
    assert_eq!(gostd::format_duration(95 * 60 * 1_000_000_000), "1h35m0s");
}

#[test]
fn with_run_timeout_budget() {
    // Go TestWithRunTimeout: the context deadline is the returned budget.
    assert_eq!(
        cfg(&[("RIVAL_RUN_TIMEOUT", "0")]).run_timeout_budget(1),
        None,
        "disabled returns no deadline"
    );
    assert_eq!(
        cfg(&[("RIVAL_RUN_TIMEOUT", "10m")]).run_timeout_budget(2),
        Some((20 * MIN).as_nanos() as i64),
        "mult scales the budget"
    );
    assert_eq!(
        cfg(&[("RIVAL_RUN_TIMEOUT", "10m")]).run_timeout_budget(0),
        None,
        "mult<=0 returns no deadline"
    );
    assert_eq!(
        cfg(&[]).run_timeout_budget(1),
        Some(DEFAULT_RUN_TIMEOUT.as_nanos() as i64)
    );
}

#[test]
fn queue_disabled() {
    let cases = [
        ("", false),
        ("0", false),
        ("false", false),
        ("FALSE", false),
        ("False", false),
        ("1", true),
        ("true", true),
        ("no", true),
        (" false", true),
    ];
    for (env, want) in cases {
        assert_eq!(
            cfg(&[("RIVAL_NO_QUEUE", env)]).queue_disabled(),
            want,
            "{env:?}"
        );
    }
}

#[test]
fn claude_auth() {
    // Go TestClaudeAuth.
    let cases: [(&str, &str, &str, Result<&str, &str>); 6] = [
        (
            "default is subscription",
            "",
            "sk-ant-xxx",
            Ok(CLAUDE_AUTH_SUBSCRIPTION),
        ),
        (
            "explicit subscription",
            "subscription",
            "",
            Ok(CLAUDE_AUTH_SUBSCRIPTION),
        ),
        ("sub shorthand", "sub", "", Ok(CLAUDE_AUTH_SUBSCRIPTION)),
        ("api with key", "api", "sk-ant-xxx", Ok(CLAUDE_AUTH_API)),
        (
            "api without key fails",
            "api",
            "",
            Err("ANTHROPIC_API_KEY is empty"),
        ),
        (
            "garbage fails",
            "oauth2",
            "",
            Err("invalid RIVAL_CLAUDE_AUTH"),
        ),
    ];
    for (name, auth, key, want) in cases {
        let got = cfg(&[("RIVAL_CLAUDE_AUTH", auth), ("ANTHROPIC_API_KEY", key)]).claude_auth();
        match (got, want) {
            (Ok(got), Ok(want)) => assert_eq!(got, want, "{name}"),
            (Err(err), Err(want)) => assert!(err.to_string().contains(want), "{name}: {err}"),
            (got, want) => panic!("{name}: got {got:?}, want {want:?}"),
        }
    }
    // Exact Go messages.
    let err = cfg(&[("RIVAL_CLAUDE_AUTH", "api")])
        .claude_auth()
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "RIVAL_CLAUDE_AUTH=api but ANTHROPIC_API_KEY is empty — set the key or unset RIVAL_CLAUDE_AUTH to use the claude CLI subscription login"
    );
    let err = cfg(&[("RIVAL_CLAUDE_AUTH", "oauth2")])
        .claude_auth()
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        r#"invalid RIVAL_CLAUDE_AUTH="oauth2" — use "subscription" (default) or "api""#
    );
}

#[test]
fn engine_label_table() {
    // Go TestEngineLabel.
    let cases = [
        ("codex", GPT56_SOL_MODEL, SOL_LABEL),
        ("codex", "retired-sol-id", SOL_LABEL),
        ("codex", "", SOL_LABEL),
        ("claude", CLAUDE_MODEL, CLAUDE_LABEL),
        ("claude", "retired-claude-id", "retired-model"),
        ("claude", "", "retired-model"),
        ("claude", "retired-model-id", "retired-model"),
        ("claude", CLAUDE_MODEL, CLAUDE_LABEL),
        ("claude", "", "retired-model"),
        ("opencode", KIMI_MODEL, K3_LABEL),
        ("opencode", "provider/retired-model-id", "retired-model"),
        ("opencode", "", "retired-model"),
        ("grok", GROK_MODEL, GROK_LABEL),
        ("grok", "", GROK_LABEL),
        // Beyond the Go table: the remaining branches.
        ("opencode", GROK_OPENROUTER_MODEL, GROK_OPENROUTER_LABEL),
        ("fable", "", "retired-model"),
        ("astra", "", "astra"),
        ("custom", "", "custom"),
        ("custom", "custom/model", "retired-model"),
        ("custom", GROK_OPENROUTER_LABEL, GROK_OPENROUTER_LABEL),
    ];
    for (cli, model, want) in cases {
        assert_eq!(
            engine_label(cli, model),
            want,
            "engine_label({cli:?}, {model:?})"
        );
    }
}

#[test]
fn model_label_only_exposes_supported_models() {
    // Go TestModelLabelOnlyExposesSupportedModels.
    let cases = [
        (GPT56_SOL_MODEL, SOL_LABEL),
        (SOL_LABEL, SOL_LABEL),
        (CLAUDE_MODEL, CLAUDE_LABEL),
        (CLAUDE_LABEL, CLAUDE_LABEL),
        (KIMI_MODEL, K3_LABEL),
        (K3_LABEL, K3_LABEL),
        (GROK_MODEL, GROK_LABEL),
        (GROK_LABEL, GROK_LABEL),
        ("custom/model", "retired-model"),
        ("", "retired-model"),
        (CODEX_MODEL, CODEX_LABEL),
        (CODEX_LABEL, CODEX_LABEL),
        (GROK_OPENROUTER_MODEL, GROK_OPENROUTER_LABEL),
        (GROK_OPENROUTER_LABEL, GROK_OPENROUTER_LABEL),
    ];
    for (model, want) in cases {
        assert_eq!(model_label(model), want, "model_label({model:?})");
    }
}

#[test]
fn public_runtime_log_normalizes_only_runtime_metadata() {
    // Go TestPublicRuntimeLogNormalizesOnlyRuntimeMetadata.
    let raw = "OpenAI Codex v0.130.0\n--------\nmodel: retired-sol-id\nprovider: openai\n--------\nuser\n\
               inspect rival/cmd/command_codex.go\n\
               === REVIEW FROM codex (retired-sol-id) [role: bug_hunter] ===\n";
    let got = public_runtime_log("codex", "retired-sol-id", raw);
    for want in [
        "Sol runtime v0.130.0",
        "model: sol",
        "=== REVIEW FROM sol [role: bug_hunter] ===",
        "rival/cmd/command_codex.go",
    ] {
        assert!(got.contains(want), "public log missing {want:?}:\n{got}");
    }
    for forbidden in ["OpenAI Codex", "model: retired-sol-id", "REVIEW FROM codex"] {
        assert!(
            !got.contains(forbidden),
            "public log exposes {forbidden:?}:\n{got}"
        );
    }
    // Exact output, traced through the Go algorithm.
    assert_eq!(
        got,
        "Sol runtime v0.130.0\n--------\nmodel: sol\nprovider: openai\n--------\nuser\n\
         inspect rival/cmd/command_codex.go\n\
         === REVIEW FROM sol [role: bug_hunter] ===\n"
    );
}

#[test]
fn public_runtime_log_hides_retired_runtime_identities() {
    // Go TestPublicRuntimeLogHidesRetiredRuntimeIdentities.
    let cases = [
        (
            "claude",
            "claude",
            "retired-claude-id",
            "Claude Code v1\n--------\nmodel: retired-claude-id\n--------\n=== REVIEW FROM claude (retired-claude-id) [role: bug_hunter] ===\n",
        ),
        (
            "opencode",
            "opencode",
            "custom/model",
            "=== REVIEW FROM opencode (custom/model) [role: bug_hunter] ===\n",
        ),
    ];
    for (name, cli, model, raw) in cases {
        let got = public_runtime_log(cli, model, raw);
        assert!(
            got.contains("retired-model"),
            "{name}: missing retired-model label:\n{got}"
        );
        assert!(
            !got.contains(model) && !got.contains(&format!("REVIEW FROM {cli}")),
            "{name}: exposes retired runtime identity:\n{got}"
        );
    }
    assert_eq!(
        public_runtime_log("claude", "retired-claude-id", cases[0].3),
        "Retired-model runtime v1\n--------\nmodel: retired-model\n--------\n=== REVIEW FROM retired-model [role: bug_hunter] ===\n"
    );
}

#[test]
fn public_runtime_log_labels_k3_review_header() {
    // Go TestPublicRuntimeLogLabelsK3ReviewHeader.
    let raw = format!("=== REVIEW FROM opencode ({KIMI_MODEL}) [role: bug_hunter] ===\n");
    let got = public_runtime_log("opencode", KIMI_MODEL, &raw);
    let want = format!("=== REVIEW FROM {K3_LABEL} [role: bug_hunter] ===");
    assert!(
        got.contains(&want) && !got.contains("opencode") && !got.contains(KIMI_MODEL),
        "public K3 header = {got:?}, want {want:?}"
    );
}

#[test]
fn public_runtime_error_uses_public_model_name() {
    // Go TestPublicRuntimeErrorUsesPublicModelName.
    let got = public_runtime_error(
        "codex",
        GPT56_SOL_MODEL,
        "Codex CLI failed for gpt-5.6-sol; run codex login",
    );
    let lower = got.to_lowercase();
    assert!(
        !lower.contains("codex") && !got.contains(GPT56_SOL_MODEL) && lower.contains(SOL_LABEL),
        "public error was not normalized: {got:?}"
    );
    assert_eq!(
        got,
        "Sol runtime failed for sol; authenticate the Sol runtime"
    );
}

#[test]
fn public_runtime_error_rewrites_every_runtime_phrase() {
    let got = public_runtime_error(
        "codex",
        CODEX_MODEL,
        "OpenAI Codex: start codex: x; subprocess codex: y; codex exited 1; codex CLI; Codex",
    );
    assert_eq!(
        got,
        "Codex runtime: start Codex runtime: x; Codex runtime: y; codex exited 1; Codex runtime; Codex"
    );
    let got = public_runtime_error(
        "claude",
        CLAUDE_MODEL,
        "Claude Code CLI; Claude CLI; claude CLI; claude requires Docker; claude exited 2; start claude: a; subprocess claude: b; claude-opus-5-5",
    );
    assert_eq!(
        got,
        "Claude runtime; Claude runtime; claude runtime; Claude runtime requires Docker; claude exited 2; start Claude runtime: a; Claude runtime: b; claude"
    );
    // Other adapters only get their model ids scrubbed.
    assert_eq!(
        public_runtime_error(
            "opencode",
            KIMI_MODEL,
            "opencode failed on moonshotai/kimi-k3"
        ),
        "opencode failed on kimi-k3"
    );
}

#[test]
fn replace_ordered_follows_go_replacer_priority() {
    // Earlier patterns win at a position even when a later one is longer.
    let pairs = [
        ("ab", "1".to_string()),
        ("abc", "2".to_string()),
        ("c", "3".to_string()),
    ];
    assert_eq!(replace_ordered("abcabc", &pairs), "1313");
    // No overlapping or re-scanned output.
    let pairs = [("a", "aa".to_string())];
    assert_eq!(replace_ordered("aéa", &pairs), "aaéaa");
}

#[test]
fn resolve_effort_precedence_and_model_defaults() {
    // Go TestResolveEffortPrecedenceAndModelDefaults.
    let config = cfg(&[]);
    for (model, want) in [
        (CODEX_MODEL, "xhigh"),
        (KIMI_MODEL, "max"),
        (CLAUDE_MODEL, "medium"),
    ] {
        assert_eq!(
            config.resolve_effort(model, "", "").unwrap(),
            want,
            "{model}"
        );
    }

    let config = cfg(&[]).with_user_config(Some(efforts(&[
        (GROK_LABEL, "low"),
        ("kimi-k3", "max"),
        (CLAUDE_LABEL, "high"),
    ])));
    assert_eq!(
        config.resolve_effort(GROK_MODEL, "", "high").unwrap(),
        "low"
    );
    assert_eq!(
        config.resolve_effort(GROK_MODEL, "medium", "low").unwrap(),
        "medium"
    );
    assert_eq!(
        config.resolve_effort(KIMI_MODEL, "low", "low").unwrap(),
        "max"
    );

    let config = cfg(&[]).with_user_config(Some(UserConfig::default()));
    assert_eq!(config.resolve_effort(GROK_MODEL, "", "low").unwrap(), "low");
    // Claude's medium pin outranks a surface fallback; -re still wins.
    assert_eq!(
        config.resolve_effort(CLAUDE_MODEL, "", "high").unwrap(),
        "medium"
    );
    assert_eq!(
        config
            .resolve_effort(CLAUDE_MODEL, "xhigh", "high")
            .unwrap(),
        "xhigh"
    );
}

#[test]
fn resolve_effort_normalizes_and_reports_go_errors() {
    let config = cfg(&[]);
    assert_eq!(
        config.resolve_effort(GROK_MODEL, "  HIGH ", "").unwrap(),
        "high"
    );
    assert_eq!(
        config
            .resolve_effort(GROK_MODEL, "Max", "")
            .unwrap_err()
            .to_string(),
        r#"invalid effort "max" for grok"#
    );
    assert_eq!(
        config
            .resolve_effort("custom/model", "", "huge")
            .unwrap_err()
            .to_string(),
        r#"invalid fallback effort "huge" for retired-model"#
    );
    // A pinned model ignores even an invalid fallback.
    assert_eq!(
        config.resolve_effort(CLAUDE_MODEL, "", "huge").unwrap(),
        "medium"
    );
    // Unknown models fall back to the review default.
    assert_eq!(
        config.resolve_effort("custom/model", "", "").unwrap(),
        "high"
    );
}

#[test]
fn resolve_antislop_effort_skips_only_the_codex_pin() {
    let config = cfg(&[]);
    assert_eq!(
        config.resolve_antislop_effort(CODEX_MODEL, "").unwrap(),
        "high"
    );
    assert_eq!(
        config.resolve_antislop_effort(CODEX_MODEL, "low").unwrap(),
        "low"
    );
    assert_eq!(
        config.resolve_antislop_effort(CLAUDE_MODEL, "").unwrap(),
        "medium"
    );
    assert_eq!(
        config.resolve_antislop_effort(KIMI_MODEL, "").unwrap(),
        "max"
    );
    assert_eq!(
        config.resolve_antislop_effort(GROK_MODEL, "").unwrap(),
        "high"
    );
    let config = config.with_user_config(Some(efforts(&[(CODEX_LABEL, "ultra")])));
    assert_eq!(
        config.resolve_antislop_effort(CODEX_MODEL, "").unwrap(),
        "ultra"
    );
}

#[test]
fn grok_effort_defaults_and_configured_override() {
    // Go TestGrokEffortDefaultsAndConfiguredOverride.
    let config = cfg(&[]);
    assert_eq!(config.default_effort_for_model(GROK_MODEL), "high");
    assert_eq!(
        config
            .resolve_effort(GROK_MODEL, "", DEFAULT_REVIEW_EFFORT)
            .unwrap(),
        "high"
    );
    assert_eq!(
        config.resolve_effort(GROK_MODEL, "medium", "").unwrap(),
        "medium"
    );
    assert!(
        config.resolve_effort(GROK_MODEL, "max", "").is_err(),
        "grok accepted max, want invalid effort error"
    );

    let config = config.with_user_config(Some(efforts(&[(GROK_LABEL, "low")])));
    assert_eq!(
        config
            .resolve_effort(GROK_MODEL, "", DEFAULT_REVIEW_EFFORT)
            .unwrap(),
        "low"
    );
    assert_eq!(config.default_effort_for_model(GROK_MODEL), "low");
}

#[test]
fn grok_is_a_valid_configured_effort_model() {
    // Go TestGrokIsAValidConfiguredEffortModel.
    let (_home, config) = loaded("efforts:\n  grok: low\n");
    assert_eq!(config.user_config_error(), None);
    assert_eq!(config.default_effort_for_model(GROK_MODEL), "low");
}

#[test]
fn grok_is_enumerated_in_effort_model_errors() {
    // Go TestGrokIsEnumeratedInEffortModelErrors.
    let (home, config) = loaded("efforts:\n  mystery: high\n");
    let err = config
        .user_config_error()
        .expect("unknown effort model was accepted");
    assert!(err.to_string().contains(GROK_LABEL), "{err}");
    let path = config_path(home.path());
    assert_eq!(
        err.to_string(),
        format!(
            r#"invalid effort model "mystery" in {}; use one of: codex, kimi-k3, claude, grok"#,
            path.display()
        )
    );
    assert_eq!(
        config.user_config(),
        None,
        "Go leaves userConfig nil on error"
    );
}

#[test]
fn grok_concrete_model_id_is_never_exposed() {
    // Go TestGrokConcreteModelIDIsNeverExposed.
    let raw = format!(
        "=== REVIEW FROM grok ({GROK_MODEL}) [role: bug_hunter] ===\nchecked {GROK_MODEL}\n"
    );
    let got = public_runtime_log("grok", GROK_MODEL, &raw);
    let want = format!("=== REVIEW FROM {GROK_LABEL} [role: bug_hunter] ===");
    assert!(
        got.contains(&want),
        "public grok header = {got:?}, want {want:?}"
    );
    assert!(
        !got.contains(GROK_MODEL),
        "exposes the concrete grok id:\n{got}"
    );
}

#[test]
fn load_user_config_validates_effort_map() {
    // Go TestLoadUserConfigValidatesEffortMap.
    let cases = [
        // sol is a removed model: its old entry is dropped, not an error.
        (
            "valid",
            "efforts:\n  sol: low\n  codex: ultra\n  kimi-k3: max\n  claude: medium\n",
            None,
        ),
        (
            "unknown model",
            "efforts:\n  mystery: high\n",
            Some("invalid effort model"),
        ),
        (
            "invalid k3 level",
            "efforts:\n  kimi-k3: high\n",
            Some("use one of: max"),
        ),
        ("invalid yaml", "efforts: [", Some("parse ")),
    ];
    for (name, body, want_err) in cases {
        let (_home, config) = loaded(body);
        match want_err {
            None => {
                assert_eq!(config.user_config_error(), None, "{name}");
                assert_eq!(
                    config.default_effort_for_model(CODEX_MODEL),
                    "ultra",
                    "{name}"
                );
                assert!(
                    !config
                        .user_config()
                        .unwrap()
                        .efforts
                        .contains_key(SOL_LABEL),
                    "{name}: removed sol effort entry was kept"
                );
            }
            Some(want) => {
                let err = config
                    .user_config_error()
                    .unwrap_or_else(|| panic!("{name}: no error"));
                assert!(err.to_string().contains(want), "{name}: {err}");
            }
        }
    }
}

#[test]
fn load_user_config_reports_go_messages() {
    let (home, config) = loaded("efforts:\n  claude: Huge\n");
    let path = config_path(home.path());
    assert_eq!(
        config.user_config_error().unwrap().to_string(),
        format!(
            r#"invalid effort "Huge" for claude in {}; use one of: low, medium, high, xhigh, ultra"#,
            path.display()
        )
    );
    let (home, config) = loaded("efforts:\n  kimi-k3: high\n");
    let path = config_path(home.path());
    assert_eq!(
        config.user_config_error().unwrap().to_string(),
        format!(
            r#"invalid effort "high" for kimi-k3 in {}; use one of: max"#,
            path.display()
        )
    );
    let (home, config) = loaded("security:\n  reviewer: ' GPT5 '\n");
    let path = config_path(home.path());
    assert_eq!(
        config.user_config_error().unwrap().to_string(),
        format!(
            r#"invalid security.reviewer "GPT5" in {}; use one of: k3, grok"#,
            path.display()
        )
    );
    let (home, config) = loaded("efforts: [");
    let path = config_path(home.path());
    let message = config.user_config_error().unwrap().to_string();
    assert!(
        message.starts_with(&format!("parse {}: ", path.display())),
        "{message}"
    );
}

#[test]
fn load_user_config_normalizes_like_go() {
    let (_home, config) = loaded(
        "claude:\n  subscription: team\nsecurity:\n  reviewer: ' Grok '\nefforts:\n  codex: ' XHigh '\n  grok: LOW\nroles:\n  bug_hunter: hunt\n  empty:\n",
    );
    assert_eq!(config.user_config_error(), None);
    let user = config.user_config().unwrap();
    assert_eq!(user.security.reviewer, "grok");
    assert_eq!(user.efforts["codex"], "xhigh");
    assert_eq!(user.efforts["grok"], "low");
    assert_eq!(config.claude_subscription(), "team");
    assert_eq!(config.configured_security_reviewer(), "grok");
    assert_eq!(config.role_prompt_override("bug_hunter"), Some("hunt"));
    // yaml.v3 decodes a null map value into a Go string as "".
    assert_eq!(config.role_prompt_override("empty"), Some(""));
    assert_eq!(config.role_prompt_override("missing"), None);
    assert_eq!(
        config.resolve_security_model().unwrap().name,
        SECURITY_REVIEWER_GROK
    );
}

#[test]
fn load_user_config_yaml_v3_decoding_edges() {
    // Empty and comment-only files decode to the zero config.
    for body in ["", "\n", "# nothing here\n", "~\n"] {
        let (_home, config) = loaded(body);
        assert_eq!(config.user_config_error(), None, "{body:?}");
        assert_eq!(
            config.user_config(),
            Some(&UserConfig::default()),
            "{body:?}"
        );
    }
    // Null sections are empty, like nil maps and zero structs.
    let (_home, config) = loaded("efforts:\nroles: ~\nclaude:\nsecurity:\n");
    assert_eq!(config.user_config(), Some(&UserConfig::default()));
    // yaml.v3 decodes any scalar into a string field as its literal text.
    let (_home, config) = loaded("roles:\n  n: 5\n  b: true\nclaude:\n  subscription: 7\n");
    assert_eq!(config.role_prompt_override("n"), Some("5"));
    assert_eq!(config.role_prompt_override("b"), Some("true"));
    assert_eq!(config.claude_subscription(), "7");
    // A null effort becomes "" and fails validation.
    let (_home, config) = loaded("efforts:\n  codex:\n");
    let err = config.user_config_error().unwrap().to_string();
    assert!(
        err.starts_with(r#"invalid effort "" for codex in "#),
        "{err}"
    );
    // Type mismatches and duplicate keys are parse errors in yaml.v3 too.
    for body in [
        "efforts: high\n",
        "claude: 5\n",
        "roles:\n  a: x\n  a: y\n",
        "efforts:\n  codex: [a]\n",
    ] {
        let (_home, config) = loaded(body);
        let err = config.user_config_error().map(ToString::to_string);
        assert!(
            err.as_deref().is_some_and(|e| e.starts_with("parse ")),
            "{body:?}: {err:?}"
        );
    }
}

#[test]
fn load_user_config_missing_file_is_silent() {
    let home = tempfile::tempdir().unwrap();
    let config = home_config(home.path(), &[]);
    assert_eq!(config.user_config_error(), None);
    assert_eq!(config.user_config(), None);
    assert_eq!(load_user_config(&home.path().join("nope.yaml")), Ok(None));
}

#[test]
fn load_user_config_needs_a_known_root() {
    // Go skips the file when os.UserHomeDir fails; RIVAL_HOME (a Rust-port
    // addition) is a known root by itself.
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("config.yaml"), "efforts:\n  grok: low\n").unwrap();
    let paths = Paths {
        root: root.path().to_path_buf(),
    };
    let without = Config::new(paths.clone(), HashMap::new(), None);
    assert_eq!(without.user_config(), None);
    let with = Config::new(
        paths,
        env_map(&[("RIVAL_HOME", root.path().to_str().unwrap())]),
        None,
    );
    assert_eq!(with.default_effort_for_model(GROK_MODEL), "low");
}

#[test]
fn load_user_config_reports_unreadable_config_path() {
    // Go TestLoadUserConfigReportsUnreadableConfigPath.
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join(".rival").join("config.yaml");
    fs::create_dir_all(&path).unwrap();
    let config = home_config(home.path(), &[]);
    let err = config
        .user_config_error()
        .expect("directory at config path was silently ignored");
    let shown = path.display().to_string();
    assert!(err.to_string().contains(&format!("read {shown}")), "{err}");
    // Go: os.ReadFile wraps the read(2) failure in a *PathError. On Windows
    // Go's syscall.Open opens the directory too; ReadFile then fails.
    assert_eq!(
        err.to_string(),
        format!(
            "read {shown}: read {shown}: {}",
            crate::gostd::errtext::IS_A_DIRECTORY
        )
    );
}

#[test]
fn load_user_config_ignores_obsolete_megareview_keys() {
    // Go TestLoadUserConfigIgnoresObsoleteMegareviewKeys.
    let (_home, config) = loaded(
        "review:\n  models: [codex, k3]\nroles:\n  consilium: judge prompt\n  bug_hunter: custom hunter\n",
    );
    assert_eq!(
        config.user_config_error(),
        None,
        "obsolete megareview keys broke config loading"
    );
    assert_eq!(
        config.role_prompt_override("bug_hunter"),
        Some("custom hunter")
    );
}

#[test]
fn config_debug_hides_env_values() {
    let config = cfg(&[("MOONSHOT_API_KEY", "secret-key")]);
    let shown = format!("{config:?}");
    assert!(!shown.contains("secret-key"), "{shown}");
    assert!(shown.contains("<1 vars>"), "{shown}");
}

// ---- codex_test.go ----

#[test]
fn codex_is_not_labelled_sol() {
    // Go TestCodexIsNotLabelledSol.
    assert_eq!(engine_label("codex", CODEX_MODEL), CODEX_LABEL);
    assert_eq!(engine_label("codex", GPT56_SOL_MODEL), SOL_LABEL);
    assert_eq!(model_label(CODEX_MODEL), CODEX_LABEL);
}

#[test]
fn codex_id_normalizes_to_its_label() {
    // Go TestCodexIDNormalizesToItsLabel.
    let raw = format!("banner from {CODEX_MODEL} done");
    let once = public_runtime_log("codex", CODEX_MODEL, &raw);
    assert!(
        !once.contains(CODEX_MODEL),
        "concrete codex id leaked: {once:?}"
    );
    assert!(once.contains(CODEX_LABEL), "codex not normalized: {once:?}");
    assert_eq!(
        public_runtime_log("codex", CODEX_MODEL, &once),
        once,
        "not idempotent"
    );
}

#[test]
fn codex_defaults_to_xhigh() {
    // Go TestCodexDefaultsToXhigh.
    assert_eq!(cfg(&[]).default_effort_for_model(CODEX_MODEL), "xhigh");
    assert!(
        known_effort_model(CODEX_LABEL),
        "efforts.codex would be rejected"
    );
}

#[test]
fn codex_command_path_keeps_xhigh() {
    // Go TestCodexCommandPathKeepsXhigh.
    assert_eq!(
        cfg(&[]).resolve_effort(CODEX_MODEL, "", "").unwrap(),
        "xhigh"
    );
}

#[test]
fn codex_pin_does_not_override_user_intent() {
    // Go TestCodexPinDoesNotOverrideUserIntent.
    let config = cfg(&[]);
    assert_eq!(
        config
            .resolve_effort(CODEX_MODEL, "medium", "high")
            .unwrap(),
        "medium"
    );
    let config = config.with_user_config(Some(efforts(&[(CODEX_LABEL, "high")])));
    assert_eq!(
        config.resolve_effort(CODEX_MODEL, "", "medium").unwrap(),
        "high"
    );
}

#[test]
fn codex_xhigh_on_every_surface() {
    // Go TestCodexXhighOnEverySurface.
    let config = cfg(&[]);
    for fallback in ["", DEFAULT_REVIEW_EFFORT, DEFAULT_PLAN_EFFORT] {
        assert_eq!(
            config.resolve_effort(CODEX_MODEL, "", fallback).unwrap(),
            "xhigh",
            "{fallback:?}"
        );
    }
}

#[test]
fn codex_banner_names_the_running_model() {
    // Go TestCodexBannerNamesTheRunningModel.
    const RAW: &str = "OpenAI Codex v1.0\nready";
    let sol = public_runtime_log("codex", GPT56_SOL_MODEL, RAW);
    assert!(sol.contains("Sol runtime"), "sol banner regressed: {sol:?}");
    let codex = public_runtime_log("codex", CODEX_MODEL, RAW);
    assert!(
        codex.contains("Codex runtime"),
        "codex banner still says sol: {codex:?}"
    );
    assert!(
        !codex.contains("OpenAI Codex"),
        "raw runtime name leaked: {codex:?}"
    );
    assert_eq!(codex, "Codex runtime v1.0\nready");
}

#[test]
fn public_runtime_log_header_rules() {
    // Old Sol banner on the first line only; model lines rewrite only after a
    // banner and before the header closes; indentation is kept.
    let raw = "Codex v0.1\n  Model: gpt-x\n--------\n--------\nmodel: later\n";
    assert_eq!(
        public_runtime_log("codex", "", raw),
        "Sol runtime v0.1\n  model: sol\n--------\n--------\nmodel: later\n"
    );
    let raw = "intro\nCodex v0.1\nmodel: gpt-x\n";
    assert_eq!(public_runtime_log("codex", "", raw), raw);
    let raw = "Claude v2\nmodel: x\nUSER\nmodel: y";
    assert_eq!(
        public_runtime_log("claude", CLAUDE_MODEL, raw),
        "Claude runtime v2\nmodel: claude\nUSER\nmodel: y"
    );
    assert_eq!(public_runtime_log("codex", CODEX_MODEL, ""), "");
}

#[test]
fn public_review_header_cases() {
    let cases = [
        (
            "=== REVIEW FROM codex [role: x] ===",
            "=== REVIEW FROM codex [role: x] ===",
        ),
        (
            "=== REVIEW FROM codex (astra) [role: x] ===",
            "=== REVIEW FROM codex [role: x] ===",
        ),
        (
            "=== REVIEW FROM codex (old) [role: x] ===",
            "=== REVIEW FROM sol [role: x] ===",
        ),
        (
            "=== REVIEW FROM claude [role: x] ===",
            "=== REVIEW FROM claude [role: x] ===",
        ),
        (
            "=== REVIEW FROM claude (fable) [role: x] ===",
            "=== REVIEW FROM retired-model [role: x] ===",
        ),
        (
            "=== REVIEW FROM opencode (kimi-k3) [role: x] ===",
            "=== REVIEW FROM kimi-k3 [role: x] ===",
        ),
        (
            "=== REVIEW FROM (gpt-5.6-sol) [role: x] ===",
            "=== REVIEW FROM sol [role: x] ===",
        ),
        (
            "=== REVIEW FROM GROK [role: x] ===",
            "=== REVIEW FROM grok [role: x] ===",
        ),
        (
            "=== REVIEW FROM custom (y) [role: x] ===",
            "=== REVIEW FROM custom [role: x] ===",
        ),
        ("=== REVIEW FROM codex ===", "=== REVIEW FROM codex ==="),
        (
            "=== REVIEW FROM  [role: x] ===",
            "=== REVIEW FROM  [role: x] ===",
        ),
        (
            "=== REVIEW FROM x (Retired-Model) [role: x]",
            "=== REVIEW FROM retired-model [role: x]",
        ),
    ];
    for (line, want) in cases {
        assert_eq!(public_review_header(line), want, "{line:?}");
    }
}

// ---- kimi_test.go ----

#[test]
fn kimi_api_key_from_prefers_env() {
    // Go TestKimiAPIKeyFromPrefersEnv.
    let home = tempfile::tempdir().unwrap();
    fs::write(home.path().join(".env"), "MOONSHOT_API_KEY=file-key\n").unwrap();
    let config = home_config(
        home.path(),
        &[("MOONSHOT_API_KEY", "env-key"), ("KIMI_API", "legacy-key")],
    );
    assert_eq!(
        config.kimi_api_key_from(home.path()),
        "env-key",
        "env must win over workdir .env"
    );
}

#[test]
fn kimi_api_key_from_falls_back_to_workdir_env_file() {
    // Go TestKimiAPIKeyFromFallsBackToWorkdirEnvFile. HOME bounds the walk so
    // no real directory above the temp tree is read.
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join("project");
    let empty = home.path().join("other");
    fs::create_dir_all(&dir).unwrap();
    fs::create_dir_all(&empty).unwrap();
    fs::write(dir.join(".env"), "MOONSHOT_API_KEY=file-key\n").unwrap();
    let config = home_config(home.path(), &[("MOONSHOT_API_KEY", ""), ("KIMI_API", "")]);
    assert_eq!(config.kimi_api_key_from(&dir), "file-key");
    assert_eq!(config.kimi_api_key_from(&empty), "", "no .env");
}

#[test]
fn kimi_api_key_from_supports_legacy_env_alias() {
    // Go TestKimiAPIKeyFromSupportsLegacyEnvAlias.
    let home = tempfile::tempdir().unwrap();
    let config = home_config(
        home.path(),
        &[("MOONSHOT_API_KEY", ""), ("KIMI_API", "legacy-key")],
    );
    assert_eq!(config.kimi_api_key_from(home.path()), "legacy-key");
}

#[test]
fn opencode_variant_kimi_k3_pins_max() {
    // Go TestOpencodeVariantKimiK3PinsMax.
    for effort in ["low", "medium", "high", "xhigh", "ultra", "max", ""] {
        assert_eq!(opencode_variant(KIMI_MODEL, effort), "max", "{effort:?}");
    }
    assert_eq!(opencode_variant(GROK_OPENROUTER_MODEL, "high"), "");
}

#[test]
fn kimi_api_key_from_walks_up_to_parent_env_file() {
    // Go TestKimiAPIKeyFromWalksUpToParentEnvFile.
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    let sub = root.join("rival").join("internal");
    fs::create_dir_all(&sub).unwrap();
    fs::write(root.join(".env"), "MOONSHOT_API_KEY=parent-key\n").unwrap();
    let config = home_config(home.path(), &[]);
    assert_eq!(config.kimi_api_key_from(&sub), "parent-key");
}

#[test]
fn dotenv_walk_bounds_match_go() {
    let top = tempfile::tempdir().unwrap();
    let home = top.path().join("home");
    let project = home.join("p");
    fs::create_dir_all(&project).unwrap();
    // Above HOME: never read.
    fs::write(top.path().join(".env"), "MOONSHOT_API_KEY=above-home\n").unwrap();
    let config = home_config(&home, &[]);
    assert_eq!(config.kimi_api_key_from(&project), "");
    // HOME itself is read before the walk stops.
    fs::write(home.join(".env"), "KIMI_API=at-home\n").unwrap();
    assert_eq!(config.kimi_api_key_from(&project), "at-home");

    // At most 8 directories: workdir plus 7 parents.
    let deep_root = tempfile::tempdir().unwrap();
    let mut deep = deep_root.path().to_path_buf();
    for i in 0..8 {
        deep.push(format!("d{i}"));
    }
    fs::create_dir_all(&deep).unwrap();
    fs::write(deep_root.path().join(".env"), "MOONSHOT_API_KEY=root-key\n").unwrap();
    let config = home_config(deep_root.path(), &[]);
    assert_eq!(
        config.kimi_api_key_from(&deep),
        "",
        "the 9th directory is out of reach"
    );
    assert_eq!(
        config.kimi_api_key_from(deep.parent().unwrap()),
        "root-key",
        "8th is read"
    );

    // A relative workdir resolves against the snapshotted cwd; a bad .env is
    // skipped, and blank values do not count.
    fs::write(project.join(".env"), "MOONSHOT_API_KEY='unterminated\n").unwrap();
    fs::write(
        home.join(".env"),
        "MOONSHOT_API_KEY=  \nKIMI_API=' spaced '\n",
    )
    .unwrap();
    let mut env = env_map(&[]);
    env.insert(
        paths::HOME_VAR.to_string(),
        home.to_str().unwrap().to_string(),
    );
    let config = Config::new(Paths::from_home(&home), env, Some(home.clone()));
    assert_eq!(config.kimi_api_key_from(Path::new("p")), "spaced");
    assert_eq!(config.kimi_api_key_from(Path::new("")), "");
    let no_cwd = home_config(&home, &[]);
    assert_eq!(
        no_cwd.kimi_api_key_from(Path::new("p")),
        "",
        "Abs fails without a cwd"
    );
}

#[test]
fn build_workdir_preamble_injects_absolute_path() {
    // filepath.Abs output is host-shaped.
    let (cwd, want) = if cfg!(windows) {
        (r"C:\work", r"C:\work\app")
    } else {
        ("/work", "/work/app")
    };
    let config = Config::new(
        Paths::from_home(Path::new("/nonexistent-rival-test-home")),
        HashMap::new(),
        Some(PathBuf::from(cwd)),
    );
    assert_eq!(
        config.build_workdir_preamble(Path::new("repo/../app")),
        format!(
            "You are working in project directory: {want}\nUse your tools to read files, run git commands, and explore the codebase as needed.\n"
        )
    );
    assert!(
        cfg(&[])
            .build_workdir_preamble(Path::new("rel"))
            .starts_with("You are working in project directory: \nUse your tools")
    );
}

// ---- Windows startup: environment names and a fresh profile ----

#[test]
fn getenv_case_rule_follows_the_platform() {
    let env = env_map(&[
        ("Path", r"C:\bin"),
        ("HOME", "/h"),
        ("zz", "1"),
        ("ZZ", "2"),
    ]);
    // Windows (Go os.Getenv via GetEnvironmentVariableW): case-insensitive.
    assert_eq!(getenv_in(true, &env, "PATH"), r"C:\bin");
    assert_eq!(getenv_in(true, &env, "path"), r"C:\bin");
    assert_eq!(getenv_in(true, &env, "Home"), "/h");
    assert_eq!(getenv_in(true, &env, "zz"), "1", "an exact match wins");
    assert_eq!(
        getenv_in(true, &env, "Zz"),
        "2",
        "the smallest name, stably"
    );
    assert_eq!(getenv_in(true, &env, "PATHX"), "");
    // Unix: exact names only.
    assert_eq!(getenv_in(false, &env, "PATH"), "");
    assert_eq!(getenv_in(false, &env, "Path"), r"C:\bin");
    assert_eq!(getenv_in(false, &env, "Zz"), "");
    // The Config getter uses the host rule.
    let config = cfg(&[("Path", r"C:\bin")]);
    let want = if cfg!(windows) { r"C:\bin" } else { "" };
    assert_eq!(config.getenv("PATH"), want);
}

/// A fresh home has no `.rival` directory at all. Opening
/// `<home>/.rival/config.yaml` then fails on its missing parent
/// (Windows `ERROR_PATH_NOT_FOUND`, Unix `ENOENT`); Go's `ErrNotExist`
/// covers both, so there is no config and no error.
#[test]
fn fresh_home_without_rival_dir_has_no_config_error() {
    let home = tempfile::tempdir().unwrap();
    assert!(!home.path().join(".rival").exists());
    let config = home_config(home.path(), &[]);
    assert!(
        config.user_config_error().is_none(),
        "{:?}",
        config.user_config_error()
    );
    assert!(config.user_config().is_none());
    assert_eq!(
        load_user_config(&home.path().join(".rival").join("config.yaml")).unwrap(),
        None
    );
    // A missing deeper parent too.
    let deep = home.path().join("a").join("b").join("config.yaml");
    assert_eq!(load_user_config(&deep).unwrap(), None);
    // Any other open error still reports: a directory in place of the file.
    fs::create_dir_all(home.path().join("dir.yaml")).unwrap();
    assert!(load_user_config(&home.path().join("dir.yaml")).is_err());
}

// ---- security_test.go ----

#[test]
fn resolve_security_model_defaults_to_k3() {
    // Go TestResolveSecurityModelDefaultsToK3.
    let entry = cfg(&[]).resolve_security_model().unwrap();
    assert_eq!(entry.name, SECURITY_REVIEWER_K3);
    assert_eq!(entry.model, KIMI_MODEL);
    assert_eq!(entry.variant, "max", "its provider exposes no other level");
    assert_eq!(
        entry,
        SecurityModel {
            name: "k3",
            model: "moonshotai/kimi-k3",
            selector: "moonshotai/kimi-k3",
            provider: "moonshotai",
            base_url: "",
            key_env: "MOONSHOT_API_KEY",
            label: "kimi-k3",
            variant: "max",
        }
    );
}

#[test]
fn resolve_security_model_grok() {
    // Go TestResolveSecurityModelGrok.
    let entry = cfg(&[])
        .with_user_config(Some(reviewer(SECURITY_REVIEWER_GROK)))
        .resolve_security_model()
        .unwrap();
    assert_eq!(entry.model, GROK_OPENROUTER_MODEL);
    // OpenCode splits the -m value at the first slash to pick the provider,
    // so the selector must name openrouter.
    assert_eq!(entry.selector, GROK_OPENROUTER_SELECTOR);
    assert_eq!(entry.key_env, "OPENROUTER_API_KEY");
    assert_eq!(entry.variant, "xhigh");
    assert_eq!(entry.provider, "openrouter");
    assert_eq!(entry.base_url, "https://openrouter.ai/api/v1");
    assert_eq!(entry.label, GROK_OPENROUTER_LABEL);
}

#[test]
fn resolve_security_model_rejects_unknown() {
    // Go TestResolveSecurityModelRejectsUnknown.
    let err = cfg(&[])
        .with_user_config(Some(reviewer("gpt5")))
        .resolve_security_model()
        .expect_err("expected an unknown reviewer to error");
    for want in security_reviewer_names() {
        assert!(
            err.to_string().contains(want),
            "{err} does not name {want:?}"
        );
    }
    assert_eq!(
        err.to_string(),
        r#"invalid security.reviewer "gpt5", must be one of: k3, grok"#
    );
    // Trimmed and lowered, like Go.
    let entry = cfg(&[])
        .with_user_config(Some(reviewer("  GROK ")))
        .resolve_security_model()
        .unwrap();
    assert_eq!(entry.name, SECURITY_REVIEWER_GROK);
}

#[test]
fn grok_labels_do_not_collide() {
    // Go TestGrokLabelsDoNotCollide.
    assert_ne!(GROK_OPENROUTER_LABEL, GROK_LABEL);
    assert_ne!(GROK_OPENROUTER_LABEL, GROK_MODEL);
}

#[test]
fn open_code_entry_for_ignores_config() {
    // Go TestOpenCodeEntryForIgnoresConfig: a free function cannot consult
    // the security config at all.
    let entry = open_code_entry_for(KIMI_MODEL).expect("K3 model not found in the registry");
    assert_eq!(entry.key_env, "MOONSHOT_API_KEY");
    assert_eq!(open_code_entry_for("no-such-model"), None);
    assert_eq!(
        open_code_entry_for(GROK_OPENROUTER_MODEL).unwrap().name,
        SECURITY_REVIEWER_GROK
    );
}

#[test]
fn both_groks_normalize_to_their_own_label() {
    // Go TestBothGroksNormalizeToTheirOwnLabel.
    let xai_log = format!("runtime banner from {GROK_MODEL} finished");
    let got = public_runtime_log(GROK_LABEL, GROK_MODEL, &xai_log);
    assert!(got.contains(GROK_LABEL), "xAI log did not normalize: {got}");
    let or_log = format!("runtime banner from {GROK_OPENROUTER_MODEL} finished");
    let got = public_runtime_log("opencode", GROK_OPENROUTER_MODEL, &or_log);
    assert!(
        got.contains(GROK_OPENROUTER_LABEL),
        "OpenRouter log did not normalize: {got}"
    );
    assert_eq!(got, "runtime banner from grok-4.6-openrouter finished");
    assert_ne!(model_label(GROK_MODEL), model_label(GROK_OPENROUTER_MODEL));
}

#[test]
fn k3_key_still_reads_the_legacy_env_alias() {
    // Go TestK3KeyStillReadsTheLegacyEnvAlias.
    let home = tempfile::tempdir().unwrap();
    fs::write(home.path().join(".env"), "KIMI_API=from-dotenv\n").unwrap();
    let config = home_config(home.path(), &[("MOONSHOT_API_KEY", ""), ("KIMI_API", "")]);
    let entry = open_code_entry_for(KIMI_MODEL).expect("K3 missing from the registry");
    assert_eq!(
        config.security_api_key_from(&entry, home.path()),
        "from-dotenv"
    );
}

#[test]
fn security_api_key_from_reads_the_entry_key() {
    let home = tempfile::tempdir().unwrap();
    fs::write(
        home.path().join(".env"),
        "OPENROUTER_API_KEY=or-file\nKIMI_API=not-for-grok\n",
    )
    .unwrap();
    let grok = open_code_entry_for(GROK_OPENROUTER_MODEL).unwrap();
    let config = home_config(home.path(), &[("OPENROUTER_API_KEY", " or-env ")]);
    assert_eq!(config.security_api_key_from(&grok, home.path()), "or-env");
    let config = home_config(home.path(), &[]);
    assert_eq!(config.security_api_key_from(&grok, home.path()), "or-file");
    assert_eq!(config.security_api_key_from(&grok, Path::new("")), "");
}

#[test]
fn runtime_log_normalization_is_idempotent() {
    // Go TestRuntimeLogNormalizationIsIdempotent.
    let cases = [
        (
            "openrouter id becomes its label",
            "opencode",
            GROK_OPENROUTER_MODEL,
            GROK_OPENROUTER_LABEL,
        ),
        (
            "xai id becomes its label",
            GROK_LABEL,
            GROK_MODEL,
            GROK_LABEL,
        ),
        ("k3 id becomes its label", "opencode", KIMI_MODEL, K3_LABEL),
    ];
    for (name, cli, model, want) in cases {
        let raw = format!("banner from {model} done");
        let once = public_runtime_log(cli, model, &raw);
        assert!(once.contains(want), "{name}: first pass = {once:?}");
        assert_eq!(
            public_runtime_log(cli, model, &once),
            once,
            "{name}: not idempotent"
        );
    }
}

#[test]
fn claude_id_normalizes_to_its_label() {
    // Go TestClaudeIDNormalizesToItsLabel.
    let raw = format!("banner from {CLAUDE_MODEL} done");
    let once = public_runtime_log("claude", CLAUDE_MODEL, &raw);
    assert!(
        once.contains(CLAUDE_LABEL),
        "claude id not normalized: {once:?}"
    );
    assert!(
        !once.contains(CLAUDE_MODEL),
        "concrete claude id leaked: {once:?}"
    );
    assert_eq!(
        public_runtime_log("claude", CLAUDE_MODEL, &once),
        once,
        "not idempotent"
    );
}

#[test]
fn replace_concrete_model_ids_protects_labels() {
    // An already-normalized OpenRouter label survives a second pass even
    // though it contains the xAI id.
    let text = format!("{GROK_OPENROUTER_LABEL} and {GROK_MODEL}");
    assert_eq!(
        replace_concrete_model_ids(GROK_LABEL, GROK_MODEL, &text),
        "grok-4.6-openrouter and grok"
    );
    assert_eq!(
        replace_concrete_model_ids(
            "",
            "",
            "gpt-6-astra claude-opus-5-5 moonshotai/kimi-k3 x-ai/grok-4.6 gpt-5.6-sol"
        ),
        "codex claude kimi-k3 grok-4.6-openrouter sol"
    );
}

// ---- antislop_test.go ----

#[test]
fn antislop_prompt_templates() {
    // Go TestAntislopPromptTemplates.
    const EXAMPLE_SUMMARY: &str = r#""summary": "1-3 sentence overall assessment of the plan""#;
    const CATEGORY_ENUM: &str = "reuse|simplify|efficiency|altitude|compat|reinvention|slop|yagni";
    let prompt = ANTISLOP_CODE_PROMPT;
    assert!(prompt.contains("{SCOPE}"), "missing placeholder {{SCOPE}}");
    assert!(
        !prompt.contains("{FILE}"),
        "contains foreign placeholder {{FILE}}"
    );
    for key in [
        r#""summary""#,
        r#""rating""#,
        r#""findings""#,
        EXAMPLE_SUMMARY,
        CATEGORY_ENUM,
    ] {
        assert!(prompt.contains(key), "missing {key:?}");
    }
    // The echo-skip only works if the example summary matches the plan prompt's.
    assert!(
        PLAN_REVIEW_PROMPT.contains(EXAMPLE_SUMMARY),
        "PLAN_REVIEW_PROMPT example summary diverged from the antislop contract"
    );
}

// ---- golden pins of the texts and constants ported from Go `config.go` ----

fn rust_prompts() -> [(&'static str, &'static str); 7] {
    [
        ("SystemPrompt", SYSTEM_PROMPT),
        ("WorkdirPreamble", WORKDIR_PREAMBLE),
        ("DiffReviewPreamble", DIFF_REVIEW_PREAMBLE),
        ("PlanReviewPrompt", PLAN_REVIEW_PROMPT),
        ("antislopJSONContract", antislop_json_contract!()),
        ("AntislopCodePrompt", ANTISLOP_CODE_PROMPT),
        ("WholeProject", WHOLE_PROJECT),
    ]
}

fn sha256_hex(s: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Pins the prompt bytes: byte length and SHA-256 of each Go constant as
/// evaluated from `config.go` before the Go tree was removed.
#[test]
fn prompts_sha256_golden() {
    let want = [
        (
            "SystemPrompt",
            174,
            "3b3dfdc7068c5920e266e6cd7ef3bd744418bbdcaf8f4f388b5eecc064827597",
        ),
        (
            "WorkdirPreamble",
            132,
            "6b244fa0f96ca0b50326c37b8f68e68347ee798ffcf0ecde71affebb20f229f1",
        ),
        (
            "DiffReviewPreamble",
            212,
            "c111dbdd3eef08f59e22db0ac95406ca001d7a1d455e37655c90f4509590afd0",
        ),
        (
            "PlanReviewPrompt",
            2587,
            "8c532f2b42d55282aee13a1eb216046b5d2ebe3946ab225359ef9e1317ac9fcc",
        ),
        (
            "antislopJSONContract",
            1020,
            "812ac30dcd653182def8a2cc78eb24c40bb3b38b6a12d4b3bff21a05287c2969",
        ),
        (
            "AntislopCodePrompt",
            4476,
            "032e130637a1d5448da3c67544f61a96466b7e055615067655e2e8f86e9ef553",
        ),
        (
            "WholeProject",
            18,
            "0e30295534e780eada78822c170a57c0ac174fae0f86ca76b41bcf811e0b95ea",
        ),
    ];
    for ((name, rust), (want_name, want_len, want_hash)) in rust_prompts().into_iter().zip(want) {
        assert_eq!(name, want_name);
        assert_eq!(
            (rust.len(), sha256_hex(rust).as_str()),
            (want_len, want_hash),
            "{name}"
        );
    }
}

/// The 26 string constants of Go `config.go`, with the values it held
/// before the Go tree was removed.
#[test]
fn string_constants_golden() {
    let got = [
        GPT56_SOL_MODEL,
        CODEX_MODEL,
        CODEX_LABEL,
        CLAUDE_MODEL,
        SOL_LABEL,
        CLAUDE_LABEL,
        K3_LABEL,
        K3_COMMAND_NAME,
        KIMI_MODEL,
        GROK_MODEL,
        GROK_LABEL,
        CLAUDE_DOCKER_IMAGE,
        CLAUDE_DOCKER_TOKEN_ENV,
        DEFAULT_REVIEW_EFFORT,
        DEFAULT_PLAN_EFFORT,
        DEFAULT_ANTISLOP_EFFORT,
        SESSION_DIR,
        QUEUE_DIR,
        SECURITY_REVIEWER_K3,
        SECURITY_REVIEWER_GROK,
        GROK_OPENROUTER_MODEL,
        GROK_OPENROUTER_SELECTOR,
        GROK_OPENROUTER_LABEL,
        OPENROUTER_BASE_URL,
        CLAUDE_AUTH_SUBSCRIPTION,
        CLAUDE_AUTH_API,
    ];
    let want = [
        "gpt-5.6-sol",
        "gpt-6-astra",
        "codex",
        "claude-opus-5-5",
        "sol",
        "claude",
        "kimi-k3",
        "k3",
        "moonshotai/kimi-k3",
        "grok-4.6",
        "grok",
        "rival-claude",
        "RIVAL_CLAUDE_TOKEN",
        "high",
        "high",
        "high",
        ".rival/sessions",
        ".rival/queue",
        "k3",
        "grok",
        "x-ai/grok-4.6",
        "openrouter/x-ai/grok-4.6",
        "grok-4.6-openrouter",
        "https://openrouter.ai/api/v1",
        "subscription",
        "api",
    ];
    assert_eq!(got, want);
    assert_eq!(
        (
            PROMPT_PREVIEW_LEN,
            PROMPT_DETAIL_MAX_LINES,
            DEFAULT_MAX_CONCURRENT
        ),
        (100, 10, 2)
    );
    assert_eq!(DEFAULT_QUEUE_TIMEOUT, 30 * MIN);
    assert_eq!(DEFAULT_RUN_TIMEOUT, 30 * MIN);
    assert_eq!(QUEUE_POLL_INTERVAL, Duration::from_secs(2));
    assert_eq!(VALID_EFFORTS, ["low", "medium", "high", "xhigh", "ultra"]);
}

#[test]
fn claude_effort_level_matches_go_map() {
    let cases = [
        ("low", Some("low")),
        ("medium", Some("medium")),
        ("high", Some("max")),
        ("xhigh", Some("max")),
        ("ultra", Some("max")),
        ("max", None),
        ("", None),
    ];
    for (effort, want) in cases {
        assert_eq!(claude_effort_level(effort), want, "{effort:?}");
    }
    assert!(VALID_EFFORTS.iter().all(|e| is_valid_effort(e)));
    assert!(!is_valid_effort("max") && !is_valid_effort("HIGH"));
    assert_eq!(
        (PromptKind::BugHunter as i32, PromptKind::Security as i32),
        (0, 1)
    );
}

#[test]
fn overflowing_budgets_keep_go_signed_wrap() {
    let c = cfg(&[("RIVAL_RUN_TIMEOUT", "2000000h")]);
    assert_eq!(c.run_timeout_budget(2), Some(-4_046_744_073_709_551_616));
    assert_eq!(c.max_run_wait(), -4_046_741_973_709_551_616);
}

#[test]
fn environ_comes_from_env_sorted_and_can_be_replaced() {
    let c = cfg(&[("B", "2"), ("A", "x=y"), ("PATH", "/bin")]);
    let want: Vec<OsString> = ["A=x=y", "B=2", "PATH=/bin"].map(OsString::from).into();
    assert_eq!(c.environ(), want.as_slice());

    let ordered: Vec<OsString> = ["Z=1", "A=1"].map(OsString::from).into();
    let c = c.with_environ(ordered.clone());
    assert_eq!(c.environ(), ordered.as_slice());
    // The getters still read the original snapshot.
    assert_eq!(c.getenv("B"), "2");
    assert!(format!("{c:?}").contains("environ: <2 entries>"));
    assert!(!format!("{c:?}").contains("Z=1"));
}

#[test]
fn auto_fix_critical_high_defaults_off() {
    for (body, want) in [
        ("efforts:\n  codex: high\n", "off"),
        ("auto_fix_critical_high: false\n", "off"),
        ("auto_fix_critical_high:\n", "off"),
        ("auto_fix_critical_high: true\n", "critical+high"),
    ] {
        let (_home, config) = loaded(body);
        assert!(config.user_config_error().is_none(), "{body:?}");
        assert_eq!(config.auto_fix_policy(), want, "{body:?}");
    }
    let (_home, missing) = loaded("");
    assert_eq!(missing.auto_fix_policy(), "off");
}
