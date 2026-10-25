//! Config and codex tests, plus golden pins of the prompt
//! texts. Every test builds its own
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

/// `<home>/.rival/config.yaml` with host separators, as
/// `Path::join` writes it.
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

// ---- config tests ----

#[test]
fn max_concurrent() {
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
    // Queue 30m + 2*run 30m + 5m margin = 95m by default.
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
    assert_eq!(duration::format(95 * 60 * 1_000_000_000), "1h35m0s");
}

#[test]
fn with_run_timeout_budget() {
    // The context deadline is the returned budget.
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
    // Exact messages.
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
        // The removed K3 and OpenRouter Grok ran on opencode.
        ("opencode", "moonshotai/kimi-k3", "retired-model"),
        ("opencode", "x-ai/grok-4.6", "retired-model"),
        ("opencode", "provider/retired-model-id", "retired-model"),
        ("opencode", "", "retired-model"),
        ("grok", GROK_MODEL, GROK_LABEL),
        ("grok", "", GROK_LABEL),
        // The remaining branches.
        ("fable", "", "retired-model"),
        ("astra", "", "astra"),
        ("custom", "", "custom"),
        ("custom", "custom/model", "retired-model"),
        ("custom", "grok-4.6-openrouter", "retired-model"),
        (CHINA_LABEL, CHINA_DEFAULT_MODEL, "kimi-k3.1-pro"),
        // Sol 6.1 and Fable 5.1 are current again; old ids keep their labels.
        ("codex", SOL_MODEL, SOL_LABEL),
        ("codex", "gpt-6.1-sol", "sol"),
        ("codex", "gpt-5.6-sol", "sol"),
        ("codex", CODEX_MODEL, CODEX_LABEL),
        ("claude", FABLE_MODEL, FABLE_LABEL),
        ("claude", "claude-fable-5-1", "fable"),
        ("claude", "claude-fable-5", "retired-model"),
        ("fable", FABLE_MODEL, FABLE_LABEL),
        ("", SOL_MODEL, SOL_LABEL),
        ("", FABLE_MODEL, FABLE_LABEL),
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
    let cases = [
        (GPT56_SOL_MODEL, SOL_LABEL),
        (SOL_LABEL, SOL_LABEL),
        (CLAUDE_MODEL, CLAUDE_LABEL),
        (CLAUDE_LABEL, CLAUDE_LABEL),
        ("moonshotai/kimi-k3", "retired-model"),
        ("kimi-k3", "retired-model"),
        (GROK_MODEL, GROK_LABEL),
        (GROK_LABEL, GROK_LABEL),
        ("custom/model", "retired-model"),
        ("", "retired-model"),
        (CODEX_MODEL, CODEX_LABEL),
        (CODEX_LABEL, CODEX_LABEL),
        (SOL_MODEL, SOL_LABEL),
        (FABLE_MODEL, FABLE_LABEL),
        (FABLE_LABEL, FABLE_LABEL),
    ];
    for (model, want) in cases {
        assert_eq!(model_label(model), want, "model_label({model:?})");
    }
}

#[test]
fn public_runtime_log_normalizes_only_runtime_metadata() {
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
    // Exact output.
    assert_eq!(
        got,
        "Sol runtime v0.130.0\n--------\nmodel: sol\nprovider: openai\n--------\nuser\n\
         inspect rival/cmd/command_codex.go\n\
         === REVIEW FROM sol [role: bug_hunter] ===\n"
    );
}

#[test]
fn public_runtime_log_hides_retired_runtime_identities() {
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
fn public_runtime_log_labels_the_china_model() {
    let got = public_runtime_log(
        CHINA_LABEL,
        CHINA_DEFAULT_MODEL,
        "Model unavailable: moonshot/kimi-k3.1-pro\n",
    );
    assert_eq!(got, "Model unavailable: kimi-k3.1-pro\n");
}

#[test]
fn public_runtime_error_uses_public_model_name() {
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
            CHINA_LABEL,
            CHINA_DEFAULT_MODEL,
            "opencode failed on moonshot/kimi-k3.1-pro"
        ),
        "opencode failed on kimi-k3.1-pro"
    );
}

#[test]
fn replace_ordered_follows_replacer_priority() {
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
    let config = cfg(&[]);
    for (model, want) in [(CODEX_MODEL, "xhigh"), (CLAUDE_MODEL, "medium")] {
        assert_eq!(
            config.resolve_effort(model, "", "").unwrap(),
            want,
            "{model}"
        );
    }

    let config = cfg(&[]).with_user_config(Some(efforts(&[
        (GROK_LABEL, "low"),
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
fn resolve_effort_normalizes_and_reports_errors() {
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
fn grok_effort_defaults_and_configured_override() {
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
    let (_home, config) = loaded("efforts:\n  grok: low\n");
    assert_eq!(config.user_config_error(), None);
    assert_eq!(config.default_effort_for_model(GROK_MODEL), "low");
}

#[test]
fn grok_is_enumerated_in_effort_model_errors() {
    let (home, config) = loaded("efforts:\n  mystery: high\n");
    let err = config
        .user_config_error()
        .expect("unknown effort model was accepted");
    assert!(err.to_string().contains(GROK_LABEL), "{err}");
    let path = config_path(home.path());
    assert_eq!(
        err.to_string(),
        format!(
            r#"invalid effort model "mystery" in {}; use one of: codex, sol, claude, fable, grok"#,
            path.display()
        )
    );
    assert_eq!(
        config.user_config(),
        None,
        "user config stays unset on error"
    );
}

#[test]
fn grok_concrete_model_id_is_never_exposed() {
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
    let cases = [
        (
            "valid",
            "efforts:\n  sol: low\n  codex: ultra\n  claude: medium\n  fable: high\n",
            None,
        ),
        // The removed K3's key and security block are ignored, any value.
        (
            "retired k3 keys",
            "efforts:\n  sol: low\n  codex: ultra\n  kimi-k3: whatever\n  fable: high\nsecurity:\n  reviewer: k3\n",
            None,
        ),
        (
            "unknown model",
            "efforts:\n  mystery: high\n",
            Some("invalid effort model"),
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
                let efforts = &config.user_config().unwrap().efforts;
                assert_eq!(efforts.get(SOL_LABEL).map(String::as_str), Some("low"));
                assert_eq!(efforts.get(FABLE_LABEL).map(String::as_str), Some("high"));
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
fn load_user_config_reports_error_messages() {
    let (home, config) = loaded("efforts:\n  claude: Huge\n");
    let path = config_path(home.path());
    assert_eq!(
        config.user_config_error().unwrap().to_string(),
        format!(
            r#"invalid effort "Huge" for claude in {}; use one of: low, medium, high, xhigh, ultra"#,
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
fn load_user_config_normalizes_values() {
    let (_home, config) = loaded(
        "claude:\n  subscription: team\nefforts:\n  codex: ' XHigh '\n  grok: LOW\nroles:\n  bug_hunter: hunt\n  empty:\n",
    );
    assert_eq!(config.user_config_error(), None);
    let user = config.user_config().unwrap();
    assert_eq!(user.efforts["codex"], "xhigh");
    assert_eq!(user.efforts["grok"], "low");
    assert_eq!(config.claude_subscription(), "team");
    assert_eq!(config.role_prompt_override("bug_hunter"), Some("hunt"));
    // A null map value decodes into a string as "".
    assert_eq!(config.role_prompt_override("empty"), Some(""));
    assert_eq!(config.role_prompt_override("missing"), None);
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
    // The file is skipped when the home directory is unknown; RIVAL_HOME
    // (an extra root) is a known root by itself.
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
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join(".rival").join("config.yaml");
    fs::create_dir_all(&path).unwrap();
    let config = home_config(home.path(), &[]);
    let err = config
        .user_config_error()
        .expect("directory at config path was silently ignored");
    let shown = path.display().to_string();
    assert!(err.to_string().contains(&format!("read {shown}")), "{err}");
    // On Unix the directory opens and the read fails. On Windows the open
    // fails.
    let (op, text) = crate::errtext::DIR_AS_FILE;
    assert_eq!(
        err.to_string(),
        format!("read {shown}: {op} {shown}: {text}")
    );
}

#[test]
fn load_user_config_ignores_obsolete_megareview_keys() {
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

// ---- codex tests ----

#[test]
fn codex_is_not_labelled_sol() {
    assert_eq!(engine_label("codex", CODEX_MODEL), CODEX_LABEL);
    assert_eq!(engine_label("codex", GPT56_SOL_MODEL), SOL_LABEL);
    assert_eq!(model_label(CODEX_MODEL), CODEX_LABEL);
}

#[test]
fn codex_id_normalizes_to_its_label() {
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
    assert_eq!(cfg(&[]).default_effort_for_model(CODEX_MODEL), "xhigh");
    assert!(
        known_effort_model(CODEX_LABEL),
        "efforts.codex would be rejected"
    );
}

#[test]
fn codex_command_path_keeps_xhigh() {
    assert_eq!(
        cfg(&[]).resolve_effort(CODEX_MODEL, "", "").unwrap(),
        "xhigh"
    );
}

#[test]
fn codex_pin_does_not_override_user_intent() {
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
            "=== REVIEW FROM retired-model [role: x] ===",
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
    // Windows (GetEnvironmentVariableW): case-insensitive.
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
/// (Windows `ERROR_PATH_NOT_FOUND`, Unix `ENOENT`); both count as
/// "not found", so there is no config and no error.
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

#[test]
fn runtime_log_normalization_is_idempotent() {
    let cases = [
        (
            "xai id becomes its label",
            GROK_LABEL,
            GROK_MODEL,
            GROK_LABEL,
        ),
        (
            "china selector becomes its label",
            CHINA_LABEL,
            CHINA_DEFAULT_MODEL,
            "kimi-k3.1-pro",
        ),
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
    // An already-normalized label survives next to its id.
    let text = format!("{GROK_LABEL} and {GROK_MODEL}");
    assert_eq!(
        replace_concrete_model_ids(GROK_LABEL, GROK_MODEL, &text),
        "grok and grok"
    );
    assert_eq!(
        replace_concrete_model_ids("", "", "gpt-6-astra claude-opus-5-5 grok-4.6 gpt-5.6-sol"),
        "codex claude grok sol"
    );
}

// ---- golden pins of the texts and constants ----

fn rust_prompts() -> [(&'static str, &'static str); 5] {
    [
        ("SystemPrompt", SYSTEM_PROMPT),
        ("WorkdirPreamble", WORKDIR_PREAMBLE),
        ("DiffReviewPreamble", DIFF_REVIEW_PREAMBLE),
        ("PlanReviewPrompt", PLAN_REVIEW_PROMPT),
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

/// Pins the prompt bytes: byte length and SHA-256 of each prompt constant.
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
            4477,
            "9cd3d3e21e2fab1920c89e50514d775c088df341efe9ba7d9b6eeaa0e85ebc7b",
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

/// The 25 string constants of the config module, with their pinned values.
#[test]
fn string_constants_golden() {
    let got = [
        GPT56_SOL_MODEL,
        CODEX_MODEL,
        CODEX_LABEL,
        CLAUDE_MODEL,
        SOL_LABEL,
        CLAUDE_LABEL,
        GROK_MODEL,
        GROK_LABEL,
        CLAUDE_DOCKER_IMAGE,
        CLAUDE_DOCKER_TOKEN_ENV,
        DEFAULT_REVIEW_EFFORT,
        DEFAULT_PLAN_EFFORT,
        SESSION_DIR,
        QUEUE_DIR,
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
        "grok-4.6",
        "grok",
        "rival-claude",
        "RIVAL_CLAUDE_TOKEN",
        "high",
        "high",
        ".rival/sessions",
        ".rival/queue",
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
fn claude_effort_level_map() {
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

/// A huge `RIVAL_RUN_TIMEOUT` saturates at the maximum, never wraps negative.
#[test]
fn overflowing_budgets_saturate() {
    let c = cfg(&[("RIVAL_RUN_TIMEOUT", "2000000h")]);
    assert_eq!(c.run_timeout_budget(2), Some(i64::MAX));
    assert_eq!(c.max_run_wait(), i64::MAX);
    let c = cfg(&[
        ("RIVAL_RUN_TIMEOUT", "2000000h"),
        ("RIVAL_QUEUE_TIMEOUT", "2000000h"),
    ]);
    assert_eq!(c.run_timeout_budget(i32::MAX), Some(i64::MAX));
    assert_eq!(c.max_run_wait(), i64::MAX);
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

// ---- proxy and plan settings ----

const FULL_PROXY: &str = "proxy:
  url: http://127.0.0.1:8317/v1/
  key_file: ~/keys/proxy.key
  claude:
    enabled: true
    model_prefix: emcd_/
  codex:
    enabled: false
    model_prefix: \"\"
plan:
  models: [codex, Opus, fable]
";

/// Writes `key` to `path` with owner-only permissions.
fn write_key(path: &Path, key: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, key).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
}

#[test]
fn proxy_and_plan_blocks_parse_and_normalize() {
    let (_home, config) = loaded(FULL_PROXY);
    assert_eq!(config.user_config_error(), None);
    let user = config.user_config().unwrap();
    assert_eq!(
        user.proxy,
        ProxyConfig {
            url: "http://127.0.0.1:8317".into(),
            key_file: "~/keys/proxy.key".into(),
            claude: ProxyRoute {
                enabled: true,
                model_prefix: "emcd_".into(),
            },
            codex: ProxyRoute {
                enabled: false,
                model_prefix: String::new(),
            },
        }
    );
    assert_eq!(user.plan.models, ["codex", "opus", "fable"]);
}

#[test]
fn no_proxy_block_means_proxy_off_for_both() {
    for body in ["", "efforts:\n  codex: high\n", "proxy:\n", "plan:\n"] {
        let (_home, config) = loaded(body);
        assert_eq!(config.user_config_error(), None, "{body:?}");
        let user = config.user_config().unwrap();
        assert_eq!(user.proxy, ProxyConfig::default(), "{body:?}");
        assert!(user.plan.models.is_empty(), "{body:?}");
        for provider in [ProxyProvider::Claude, ProxyProvider::Codex] {
            assert_eq!(config.proxy_route(provider), Ok(None), "{body:?}");
        }
    }
    // No config file at all.
    assert_eq!(cfg(&[]).proxy_route(ProxyProvider::Claude), Ok(None));
}

#[test]
fn proxy_route_reads_url_prefix_and_key_file() {
    let (home, config) = loaded(FULL_PROXY);
    write_key(&home.path().join("keys/proxy.key"), "  sk-test-0001\n");
    let route = config.proxy_route(ProxyProvider::Claude).unwrap().unwrap();
    assert_eq!(route.url, "http://127.0.0.1:8317");
    assert_eq!(route.prefix, "emcd_");
    assert_eq!(route.key(), "sk-test-0001");
    // Codex is not enabled.
    assert_eq!(config.proxy_route(ProxyProvider::Codex), Ok(None));
}

#[test]
fn proxy_env_overrides_url_and_key_and_switches_off() {
    let body = "proxy:\n  url: http://127.0.0.1:8317\n  codex:\n    enabled: true\n";
    let (home, _) = loaded(body);
    // RIVAL_PROXY_KEY wins over the key file; RIVAL_PROXY_URL over proxy.url.
    write_key(&home.path().join(".rival/proxy.key"), "from-file-key");
    let config = home_config(
        home.path(),
        &[
            ("RIVAL_PROXY_KEY", " env-key-1234 "),
            ("RIVAL_PROXY_URL", "https://proxy.example:9000/v1"),
        ],
    );
    let route = config.proxy_route(ProxyProvider::Codex).unwrap().unwrap();
    assert_eq!(route.url, "https://proxy.example:9000");
    assert_eq!(route.key(), "env-key-1234");
    assert_eq!(route.prefix, "");
    // RIVAL_PROXY=off sends the run direct.
    for off in ["off", "OFF"] {
        let config = home_config(home.path(), &[("RIVAL_PROXY", off)]);
        assert_eq!(config.proxy_route(ProxyProvider::Codex), Ok(None), "{off}");
    }
    // Without the env key, the default key file is read.
    let config = home_config(home.path(), &[]);
    let route = config.proxy_route(ProxyProvider::Codex).unwrap().unwrap();
    assert_eq!(route.key(), "from-file-key");
    // An invalid RIVAL_PROXY_URL is an error naming the variable.
    let config = home_config(home.path(), &[("RIVAL_PROXY_URL", "ftp://x")]);
    let err = config.proxy_route(ProxyProvider::Codex).unwrap_err();
    assert!(err.to_string().contains("RIVAL_PROXY_URL"), "{err}");
}

#[test]
fn proxy_key_missing_or_empty_names_the_fix() {
    let body = "proxy:\n  url: http://127.0.0.1:8317\n  claude:\n    enabled: true\n    model_prefix: emcd_\n";
    let (home, config) = loaded(body);
    // A missing key is not a load error: the key loads at the first use.
    assert_eq!(config.user_config_error(), None);
    let err = config.proxy_route(ProxyProvider::Claude).unwrap_err();
    assert_eq!(
        err.to_string(),
        "proxy key missing — run rival config key set"
    );
    assert_eq!(config.proxy_key(), Ok(None));
    write_key(&home.path().join(".rival/proxy.key"), " \n");
    let config = home_config(home.path(), &[]);
    assert_eq!(config.proxy_key(), Ok(None));
}

#[test]
fn proxy_route_needs_a_url() {
    let (home, config) = loaded("proxy:\n  claude:\n    enabled: true\n");
    write_key(&home.path().join(".rival/proxy.key"), "k");
    let err = config.proxy_route(ProxyProvider::Claude).unwrap_err();
    assert!(err.to_string().contains("proxy.url"), "{err}");
}

#[test]
fn proxy_key_source_and_mask() {
    let (home, _) = loaded("");
    let file = home.path().join(".rival/proxy.key");
    write_key(&file, "sk-abcdef-a91f\n");
    let config = home_config(home.path(), &[]);
    let key = config.proxy_key().unwrap().unwrap();
    assert_eq!(key.secret(), "sk-abcdef-a91f");
    assert_eq!(key.source, ProxyKeySource::File(file.clone()));
    assert_eq!(key.masked(), "set (…a91f)");
    assert!(!format!("{key:?}").contains("sk-abcdef"));
    let config = home_config(home.path(), &[("RIVAL_PROXY_KEY", "env-secret-0042")]);
    let key = config.proxy_key().unwrap().unwrap();
    assert_eq!(key.source, ProxyKeySource::Env);
    assert_eq!(key.masked(), "set (…0042)");
    // A short key shows no characters.
    let config = home_config(home.path(), &[("RIVAL_PROXY_KEY", "abc")]);
    assert_eq!(config.proxy_key().unwrap().unwrap().masked(), "set");
    assert_eq!(config.proxy_key_file(), file);
}

#[test]
fn proxy_key_file_path_expands_home() {
    let (home, config) = loaded("proxy:\n  key_file: ~/k/p.key\n");
    assert_eq!(config.proxy_key_file(), home.path().join("k").join("p.key"));
    let (_home, config) = loaded("proxy:\n  key_file: /etc/rival.key\n");
    assert_eq!(config.proxy_key_file(), PathBuf::from("/etc/rival.key"));
    let (home, config) = loaded("");
    assert_eq!(
        config.proxy_key_file(),
        home.path().join(".rival").join("proxy.key")
    );
}

#[cfg(unix)]
#[test]
fn proxy_key_file_readable_by_others_is_an_error() {
    use std::os::unix::fs::PermissionsExt as _;
    let (home, _) = loaded("");
    let file = home.path().join(".rival/proxy.key");
    write_key(&file, "secret-key-9999");
    for mode in [0o644, 0o640, 0o604] {
        fs::set_permissions(&file, fs::Permissions::from_mode(mode)).unwrap();
        let config = home_config(home.path(), &[]);
        let err = config.proxy_key().unwrap_err().to_string();
        assert!(err.contains(&file.display().to_string()), "{err}");
        assert!(err.contains("chmod 600"), "{err}");
        assert!(!err.contains("secret-key"), "{err}");
    }
    fs::set_permissions(&file, fs::Permissions::from_mode(0o400)).unwrap();
    let config = home_config(home.path(), &[]);
    assert!(config.proxy_key().unwrap().is_some());
}

#[test]
fn proxy_url_normalize() {
    let ok = [
        ("http://127.0.0.1:8317", "http://127.0.0.1:8317"),
        ("http://127.0.0.1:8317/", "http://127.0.0.1:8317"),
        ("http://127.0.0.1:8317/v1", "http://127.0.0.1:8317"),
        ("http://127.0.0.1:8317/v1/", "http://127.0.0.1:8317"),
        (
            " https://proxy.example/base/v1 ",
            "https://proxy.example/base",
        ),
        ("HTTPS://[::1]:8317", "HTTPS://[::1]:8317"),
        ("http://user@host", "http://user@host"),
    ];
    for (raw, want) in ok {
        assert_eq!(normalize_proxy_url(raw).as_deref(), Ok(want), "{raw:?}");
    }
    for raw in [
        "",
        "127.0.0.1:8317",
        "ftp://host",
        "http://",
        "http:///v1",
        "http://:8317",
        "http://user@",
        "http://[]:1",
    ] {
        assert!(normalize_proxy_url(raw).is_err(), "{raw:?}");
    }
}

#[test]
fn model_prefix_normalize() {
    for (raw, want) in [
        ("emcd_", "emcd_"),
        ("emcd_/", "emcd_"),
        (" emcd2_// ", "emcd2_"),
        ("", ""),
        ("/", ""),
    ] {
        assert_eq!(normalize_model_prefix(raw), want, "{raw:?}");
    }
}

#[test]
fn proxy_and_plan_validation_errors() {
    let cases = [
        ("proxy:\n  key: sk-secret\n", "rival config key set"),
        ("proxy:\n  key:\n", "rival config key set"),
        ("proxy:\n  url: ftp://x\n", "invalid proxy.url"),
        ("proxy:\n  url: localhost:8317\n", "invalid proxy.url"),
        ("plan:\n  models: [codex, gpt]\n", "invalid plan.models"),
        ("proxy:\n  claude:\n    enabled: maybe\n", "parse "),
        ("plan:\n  models: codex\n", "parse "),
    ];
    for (body, want) in cases {
        let (home, config) = loaded(body);
        let err = config
            .user_config_error()
            .unwrap_or_else(|| panic!("{body:?}: no error"))
            .to_string();
        assert!(err.contains(want), "{body:?}: {err}");
        assert!(
            err.contains(&config_path(home.path()).display().to_string()),
            "{body:?}: {err}"
        );
        assert!(!err.contains("sk-secret"), "{body:?}: {err}");
    }
    let (_home, config) = loaded("plan:\n  models: [codex, gpt]\n");
    assert_eq!(
        config.user_config_error().unwrap().to_string(),
        format!(
            r#"invalid plan.models entry "gpt" in {}; use one of: codex, sol, claude, opus, fable"#,
            config_path(_home.path()).display()
        )
    );
}

#[test]
fn route_debug_hides_the_key() {
    let route = Route {
        url: "http://127.0.0.1:8317".into(),
        prefix: "emcd_".into(),
        key: "sk-very-secret".into(),
    };
    let shown = format!("{route:?}");
    assert!(!shown.contains("sk-very-secret"), "{shown}");
    assert!(shown.contains("emcd_"), "{shown}");
}

/// Sol 6.1 and Fable 5.1 logs show their own labels; a Codex log's bare
/// "Codex" banner names Codex, not Sol.
#[test]
fn public_runtime_log_names_sol_and_fable() {
    assert_eq!(
        public_runtime_log("codex", SOL_MODEL, "Codex v1\nmodel: gpt-6.1-sol\n"),
        "Sol runtime v1\nmodel: sol\n"
    );
    assert_eq!(
        public_runtime_log("codex", CODEX_MODEL, "Codex v1\nmodel: gpt-6-astra\n"),
        "Codex runtime v1\nmodel: codex\n"
    );
    assert_eq!(
        public_runtime_log("codex", "", "Codex v1\n"),
        "Sol runtime v1\n"
    );
    assert_eq!(
        public_runtime_log(
            "claude",
            FABLE_MODEL,
            "Claude Code v2\nmodel: claude-fable-5-1\n"
        ),
        "Fable runtime v2\nmodel: fable\n"
    );
}

#[test]
fn a_draft_proxy_key_stands_in_for_the_key_file_but_not_the_env() {
    let body = "proxy:\n  url: http://127.0.0.1:8317\n  claude:\n    enabled: true\n";
    let (home, config) = loaded(body);
    write_key(&home.path().join(".rival/proxy.key"), "saved-key-0001");
    // A config that already read the file still takes the draft key.
    assert!(config.proxy_key().unwrap().is_some());
    let draft = config.clone().with_draft_proxy_key(" draft-key-0002 ");
    let route = draft.proxy_route(ProxyProvider::Claude).unwrap().unwrap();
    assert_eq!(route.key(), "draft-key-0002");
    let key = draft.proxy_key().unwrap().unwrap();
    assert_eq!(key.source, ProxyKeySource::File(config.proxy_key_file()));
    // The saved config still reads the file.
    let route = config.proxy_route(ProxyProvider::Claude).unwrap().unwrap();
    assert_eq!(route.key(), "saved-key-0001");
    // RIVAL_PROXY_KEY wins over a draft key, as it wins over the file.
    let env = home_config(home.path(), &[("RIVAL_PROXY_KEY", "env-key-0003")])
        .with_draft_proxy_key("draft-key-0002");
    let route = env.proxy_route(ProxyProvider::Claude).unwrap().unwrap();
    assert_eq!(route.key(), "env-key-0003");
}

/// `speed.codex` and `speed.sol` are fast unless set, also with no config
/// file; each key covers its own model only, and no other model ever gets
/// a tier.
#[test]
fn speed_is_per_codex_runtime_model_and_fast_by_default() {
    let (_home, missing) = loaded("");
    assert_eq!(missing.speed(CODEX_MODEL), Speed::Fast);
    assert_eq!(missing.speed(SOL_MODEL), Speed::Fast);
    let no_file = cfg(&[]);
    assert!(no_file.user_config().is_none());
    assert_eq!(no_file.speed(CODEX_MODEL), Speed::Fast);
    assert_eq!(SpeedConfig::default().sol, Speed::Fast);
    use Speed::*;
    for (body, codex, sol) in [
        ("efforts:\n  codex: high\n", Fast, Fast),
        ("speed:\n", Fast, Fast),
        ("speed:\n  codex: normal\n", Normal, Fast),
        ("speed:\n  sol: ULTRAFAST\n", Fast, Ultrafast),
        (
            "speed:\n  codex: ultrafast\n  sol: normal\n",
            Ultrafast,
            Normal,
        ),
        ("speed:\n  codex: \" fast \"\n", Fast, Fast),
    ] {
        let (_home, config) = loaded(body);
        assert!(config.user_config_error().is_none(), "{body:?}");
        assert_eq!(config.speed(CODEX_MODEL), codex, "{body:?}");
        assert_eq!(config.speed(SOL_MODEL), sol, "{body:?}");
        for other in [
            CLAUDE_MODEL,
            FABLE_MODEL,
            GPT56_SOL_MODEL,
            "",
            "codex",
            "sol",
        ] {
            assert_eq!(config.speed(other), Normal, "{body:?} {other:?}");
        }
    }
}

/// The 5.2 `ultrafast.<model>: true` reads as ultrafast while
/// `speed.<model>` is absent; `false` falls through to the default fast;
/// `speed.<model>` always wins.
#[test]
fn old_ultrafast_switches_read_as_a_speed() {
    use Speed::*;
    for (body, codex, sol) in [
        ("ultrafast:\n  codex: true\n", Ultrafast, Fast),
        ("ultrafast:\n  codex: false\n  sol: true\n", Fast, Ultrafast),
        (
            "ultrafast:\n  sol: true\nspeed:\n  sol: normal\n",
            Fast,
            Normal,
        ),
        (
            "ultrafast:\n  codex: false\nspeed:\n  codex: ultrafast\n",
            Ultrafast,
            Fast,
        ),
    ] {
        let (_home, config) = loaded(body);
        assert!(config.user_config_error().is_none(), "{body:?}");
        assert_eq!(config.speed(CODEX_MODEL), codex, "{body:?}");
        assert_eq!(config.speed(SOL_MODEL), sol, "{body:?}");
    }
    let (_home, bad) = loaded("ultrafast:\n  codex: fast\n");
    assert!(bad.user_config_error().is_some());
}

/// A bad `speed.<model>` value is a config error that names the key and the
/// accepted values.
#[test]
fn speed_rejects_unknown_values() {
    let (home, config) = loaded("speed:\n  sol: turbo\n");
    assert_eq!(
        config.user_config_error().expect("error").to_string(),
        format!(
            r#"invalid speed.sol "turbo" in {}; use one of: normal, fast, ultrafast"#,
            config_path(home.path()).display()
        )
    );
}

/// Each speed's tier and name.
#[test]
fn speed_tiers_and_names() {
    assert_eq!(Speed::Normal.tier(), None);
    assert_eq!(Speed::Fast.tier(), Some("fast"));
    assert_eq!(Speed::Ultrafast.tier(), Some("ultrafast"));
    assert_eq!(speed_names(), ["normal", "fast", "ultrafast"]);
    assert_eq!(Speed::parse(" Fast "), Some(Speed::Fast));
    assert_eq!(Speed::parse("priority"), None);
    assert_eq!(Speed::from_tier("ultrafast"), Speed::Ultrafast);
    assert_eq!(Speed::from_tier(""), Speed::Normal);
}

/// `skill.claude` defaults to sol and `skill.codex` to claude, with or
/// without a config file.
#[test]
fn skill_models_default_to_the_other_vendor() {
    let no_file = cfg(&[]);
    assert_eq!(no_file.skill_model(SkillHost::Claude), SOL_LABEL);
    assert_eq!(no_file.skill_model(SkillHost::Codex), CLAUDE_LABEL);
    assert!(!no_file.skill_model_configured(SkillHost::Claude));
    for body in ["", "skill:\n", "skill:\n  claude: \"\"\n  codex:\n"] {
        let (_home, config) = loaded(body);
        assert!(config.user_config_error().is_none(), "{body:?}");
        assert_eq!(config.skill_model(SkillHost::Claude), SOL_LABEL, "{body:?}");
        assert_eq!(
            config.skill_model(SkillHost::Codex),
            CLAUDE_LABEL,
            "{body:?}"
        );
        assert!(!config.skill_model_configured(SkillHost::Codex), "{body:?}");
    }
}

/// Every model name is accepted for each host, in any case; `opus` reads
/// as `claude`.
#[test]
fn skill_models_take_every_command_model_and_the_opus_alias() {
    for (raw, want) in [
        ("codex", "codex"),
        ("SOL", "sol"),
        (" claude ", "claude"),
        ("Opus", "claude"),
        ("fable", "fable"),
        ("grok", "grok"),
    ] {
        let (_home, config) = loaded(&format!("skill:\n  claude: {raw:?}\n  codex: {raw:?}\n"));
        assert!(config.user_config_error().is_none(), "{raw:?}");
        let user = config.user_config().unwrap();
        assert_eq!(user.skill.claude, want, "{raw:?}");
        assert_eq!(user.skill.codex, want, "{raw:?}");
        for host in SkillHost::ALL {
            assert_eq!(config.skill_model(host), want, "{raw:?}");
            assert!(config.skill_model_configured(host));
        }
    }
}

/// A bad `skill.<host>` value is a config error that names the key and the
/// accepted values.
#[test]
fn skill_model_rejects_unknown_names() {
    for (key, body) in [
        ("skill.claude", "skill:\n  claude: gpt5\n"),
        ("skill.codex", "skill:\n  codex: gpt5\n"),
    ] {
        let (home, config) = loaded(body);
        let err = config.user_config_error().expect("error").to_string();
        assert_eq!(
            err,
            format!(
                r#"invalid {key} "gpt5" in {}; use one of: codex, sol, claude, fable, grok"#,
                config_path(home.path()).display()
            )
        );
    }
}

/// `--host` takes `claude` or `codex` in any case, nothing else.
#[test]
fn skill_host_parses_both_hosts() {
    assert_eq!(SkillHost::parse("claude"), Some(SkillHost::Claude));
    assert_eq!(SkillHost::parse(" CODEX "), Some(SkillHost::Codex));
    assert_eq!(SkillHost::parse("sol"), None);
    assert_eq!(SkillHost::parse(""), None);
    assert_eq!(SkillHost::Claude.key(), "skill.claude");
    assert_eq!(SkillHost::Codex.key(), "skill.codex");
}

/// `china.model` defaults to Kimi K3.1 Pro on the moonshot provider; a
/// configured `provider/model` wins, trimmed.
#[test]
fn china_model_defaults_and_overrides() {
    let no_file = cfg(&[]);
    assert_eq!(no_file.china_model(), CHINA_DEFAULT_MODEL);
    assert!(!no_file.china_model_configured());
    for body in ["", "china:\n", "china:\n  model: \"\"\n"] {
        let (_home, config) = loaded(body);
        assert!(config.user_config_error().is_none(), "{body:?}");
        assert_eq!(config.china_model(), CHINA_DEFAULT_MODEL, "{body:?}");
    }
    let (_home, config) = loaded("china:\n  model: \" openrouter/moonshotai/kimi-k3.1-pro \"\n");
    assert!(config.user_config_error().is_none());
    assert_eq!(config.china_model(), "openrouter/moonshotai/kimi-k3.1-pro");
    assert!(config.china_model_configured());
}

/// A `china.model` without a provider or a model is a config error that
/// names the key.
#[test]
fn china_model_rejects_bad_selectors() {
    for raw in [
        "kimi-k3.1-pro",
        "/kimi",
        "moonshot/",
        "moonshot/a/",
        "moon shot/kimi",
        "moonshot/kimi-k3.1-pro#max",
    ] {
        let (home, config) = loaded(&format!("china:\n  model: {raw:?}\n"));
        let err = config.user_config_error().expect("error").to_string();
        assert_eq!(
            err,
            format!(
                "invalid china.model {raw:?} in {}; use an opencode provider/model, for example moonshot/kimi-k3.1-pro",
                config_path(home.path()).display()
            )
        );
    }
}

#[test]
fn china_label_is_the_model_name() {
    assert_eq!(
        parse_china_model("moonshot/kimi-k3.1-pro"),
        Some(("moonshot", "kimi-k3.1-pro"))
    );
    assert_eq!(
        parse_china_model("openrouter/moonshotai/kimi-k3.1-pro"),
        Some(("openrouter", "moonshotai/kimi-k3.1-pro"))
    );
    assert_eq!(china_label("moonshot/kimi-k3.1-pro"), "kimi-k3.1-pro");
    assert_eq!(
        china_label("openrouter/moonshotai/kimi-k3.1-pro"),
        "kimi-k3.1-pro"
    );
    assert_eq!(china_label(""), CHINA_LABEL);
    assert_eq!(
        engine_label(CHINA_LABEL, CHINA_DEFAULT_MODEL),
        "kimi-k3.1-pro"
    );
    // A known id under china keeps the china label.
    assert_eq!(engine_label(CHINA_LABEL, "x-ai/grok-4.6"), "grok-4.6");
}
