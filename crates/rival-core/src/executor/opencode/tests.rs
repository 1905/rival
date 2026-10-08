//! Opencode executor checks: the provider config key order and escaping,
//! option overrides and the
//! exact request.

use super::*;
use crate::executor::testutil::{Env, recorder, strings};

fn k3() -> SecurityModel {
    config::open_code_entry_for(config::KIMI_MODEL).expect("K3 missing from the registry")
}

fn grok() -> SecurityModel {
    config::open_code_entry_for(config::GROK_OPENROUTER_MODEL)
        .expect("OpenRouter Grok missing from the registry")
}

/// Captured from the tree before the registry existed. Generalizing the
/// adapter must not change one byte of what K3 already sends.
const K3_PROVIDER_BASELINE: &str = r#"{"$schema":"https://opencode.ai/config.json","provider":{"moonshotai":{"options":{"apiKey":"TESTKEY"}}}}"#;

#[test]
fn k3_provider_config_unchanged() {
    assert_eq!(
        opencode_provider_config(&k3(), "TESTKEY"),
        K3_PROVIDER_BASELINE
    );
}

/// The Grok provider config names OpenRouter (exact bytes).
#[test]
fn grok_provider_config_names_openrouter() {
    let got = opencode_provider_config(&grok(), "TESTKEY");
    assert_eq!(
        got,
        r#"{"$schema":"https://opencode.ai/config.json","provider":{"openrouter":{"options":{"apiKey":"TESTKEY","baseURL":"https://openrouter.ai/api/v1"}}}}"#
    );
    assert!(!got.contains("moonshotai"));
}

/// The provider config rejects an empty key and unregistered models.
#[test]
fn provider_config_rejects_empty_key_and_unregistered_models() {
    assert_eq!(opencode_provider_config(&k3(), ""), "");
    assert!(config::open_code_entry_for("custom/example-model").is_none());
    assert!(opencode_provider_config(&k3(), "sk-moon").contains(r#""moonshotai""#));
}

/// Keys sorted by bytes; strings escape quotes, backslashes and control
/// bytes only. `<`, `>`, `&` and U+2028 are written as they are.
#[test]
fn provider_config_sorts_keys_and_escapes_only_what_json_needs() {
    let entry = SecurityModel {
        provider: "Z<&>",
        base_url: "http://x/?a=1&b=<2>",
        ..k3()
    };
    assert_eq!(
        opencode_provider_config(&entry, "k\"\\\n\u{1}\u{2028}\u{e9}"),
        concat!(
            r#"{"$schema":"https://opencode.ai/config.json","provider":{"Z<&>":"#,
            r#"{"options":{"apiKey":"k\"\\\n\u0001"#,
            "\u{2028}\u{e9}",
            r#"","baseURL":"http://x/?a=1&b=<2>"}}}}"#
        )
    );
}

/// Run args use the selector, not the model ID (exact vector).
#[test]
fn run_args_use_the_selector_not_the_model_id() {
    let args = opencode_run_args(&grok(), "xhigh", "/tmp");
    assert_eq!(
        args,
        strings(&[
            "run",
            "--pure",
            "-m",
            config::GROK_OPENROUTER_SELECTOR,
            "--variant",
            "xhigh",
            "--dir",
            "/tmp"
        ])
    );
    assert!(
        !args
            .join(" ")
            .contains(&format!("-m {} ", config::GROK_OPENROUTER_MODEL))
    );
}

/// K3 pins max whatever effort was requested, and only supported variants
/// are used.
#[test]
fn k3_run_args_keep_max_variant() {
    for effort in ["high", "max", "low", ""] {
        assert_eq!(
            opencode_run_args(&k3(), effort, "/repo"),
            strings(&[
                "run",
                "--pure",
                "-m",
                config::KIMI_MODEL,
                "--variant",
                "max",
                "--dir",
                "/repo"
            ]),
            "{effort}"
        );
    }
    // No variant: no flag. An empty workdir stays an empty argument.
    let plain = SecurityModel {
        variant: "",
        ..k3()
    };
    assert_eq!(
        opencode_run_args(&plain, "high", ""),
        strings(&["run", "--pure", "-m", config::KIMI_MODEL, "--dir", ""])
    );
}

/// A fake opencode is on
/// PATH, so the key branch is always asserted.
#[cfg(unix)]
#[test]
fn preflight_names_the_right_variable_per_model() {
    let mut env = Env::new();
    env.set("MOONSHOT_API_KEY", Some(""))
        .set("KIMI_API", Some(""))
        .set("OPENROUTER_API_KEY", Some(""));
    let cfg = env.config();
    let work = env.work_str();
    assert_eq!(
        opencode_preflight_entry(&cfg, &k3(), &work)
            .unwrap_err()
            .to_string(),
        "opencode CLI not installed. Install: curl -fsSL https://opencode.ai/install | bash"
    );
    env.fake("opencode", "#!/bin/sh\nexit 97\n");
    let cfg = env.config();
    assert_eq!(
        opencode_preflight_entry(&cfg, &k3(), &work)
            .unwrap_err()
            .to_string(),
        "model kimi-k3 requires MOONSHOT_API_KEY — add it to the project .env or export it"
    );
    assert_eq!(
        opencode_preflight_entry(&cfg, &grok(), &work)
            .unwrap_err()
            .to_string(),
        "model grok-4.6-openrouter requires OPENROUTER_API_KEY — add it to the project .env or export it"
    );
}

/// The K3 preflight requires a key. A task-owned fake opencode replaces the
/// real one, so the test never skips when opencode is not installed.
#[cfg(unix)]
#[test]
fn opencode_preflight_k3_requires_key() {
    let mut env = Env::new();
    env.fake("opencode", "#!/bin/sh\nexit 97\n");
    env.set("MOONSHOT_API_KEY", Some(""))
        .set("KIMI_API", Some(""));
    let err = opencode_preflight_model(&env.config(), config::KIMI_MODEL, "").unwrap_err();
    assert!(err.to_string().contains("MOONSHOT_API_KEY"), "{err}");

    env.set("MOONSHOT_API_KEY", Some("sk-test"));
    opencode_preflight_model(&env.config(), config::KIMI_MODEL, "").unwrap();
}

#[test]
fn opencode_preflight_rejects_unsupported_model() {
    let env = Env::new();
    assert_eq!(
        opencode_preflight_model(&env.config(), "custom/example-model", "")
            .unwrap_err()
            .to_string(),
        "unsupported OpenCode model \"custom/example-model\""
    );
}

#[test]
fn opencode_run_env_isolates_session_databases() {
    let mut env = Env::new();
    env.set("MOONSHOT_API_KEY", Some("sk-test"));
    let cfg = env.config();
    let opts = OpencodeRunOpts::default();
    let first = opencode_run_env_with(&cfg, "session-a", &k3(), "", &opts).join("\n");
    let second = opencode_run_env_with(&cfg, "session-b", &k3(), "", &opts).join("\n");
    assert!(first.contains("OPENCODE_DB=rival-session-a.db"), "{first}");
    assert!(
        second.contains("OPENCODE_DB=rival-session-b.db"),
        "{second}"
    );
    assert_ne!(first, second);
}

#[test]
fn run_env_options_and_key_sources() {
    let mut env = Env::new();
    let cfg = env.config();
    // No key anywhere: no provider config at all.
    assert_eq!(
        opencode_run_env_with(&cfg, "s", &k3(), "", &OpencodeRunOpts::default()),
        vec![
            format!("OPENCODE_PERMISSION={OPENCODE_READ_ONLY_PERMISSION}"),
            "OPENCODE_DB=rival-s.db".to_string(),
        ]
    );

    // An explicit key and permission win over the env.
    env.set("OPENROUTER_API_KEY", Some(" sk-env "));
    let cfg = env.config();
    let opts = OpencodeRunOpts {
        permission: "{\"read\":\"allow\"}".into(),
        api_key: "sk-explicit".into(),
        drop_env: vec![],
    };
    let got = opencode_run_env_with(&cfg, "s", &grok(), "", &opts);
    assert_eq!(got[0], "OPENCODE_PERMISSION={\"read\":\"allow\"}");
    assert!(got[2].contains(r#""apiKey":"sk-explicit""#), "{got:?}");

    // Otherwise the entry's own variable, trimmed.
    let got = opencode_run_env_with(&cfg, "s", &grok(), "", &OpencodeRunOpts::default());
    assert!(got[2].contains(r#""apiKey":"sk-env""#), "{got:?}");
    assert_eq!(got.len(), 3);
}

#[test]
fn run_opencode_resolves_the_model_and_drops_extra_vars() {
    let mut env = Env::new();
    env.set("MOONSHOT_API_KEY", Some("sk-k3"));
    let cfg = env.config();
    let work = env.work_str();
    let mut sess = env.session("opencode", "security", config::KIMI_MODEL, &work);
    let opts = OpencodeRunOpts {
        drop_env: strings(&["AWS_", "GH_TOKEN"]),
        ..OpencodeRunOpts::default()
    };

    // An empty model means K3.
    let mut seen = None;
    run_opencode_model_with(
        &cfg,
        &mut sess,
        "p",
        "high",
        &work,
        "",
        &opts,
        recorder(&mut seen, Ok(RunResult::default())),
    )
    .unwrap();
    let seen = seen.unwrap();
    assert_eq!(seen.binary, "opencode");
    assert_eq!(
        seen.args,
        strings(&[
            "run",
            "--pure",
            "-m",
            config::KIMI_MODEL,
            "--variant",
            "max",
            "--dir",
            &work
        ])
    );
    assert_eq!(
        seen.drop_env,
        strings(&[
            "OPENCODE_PERMISSION",
            "OPENCODE_CONFIG_CONTENT",
            "OPENCODE_DB",
            "AWS_",
            "GH_TOKEN"
        ])
    );
    assert_eq!(
        seen.env[0],
        format!("OPENCODE_PERMISSION={OPENCODE_READ_ONLY_PERMISSION}")
    );
    assert_eq!(seen.mode, "security");

    let mut seen = None;
    let err = run_opencode_model_with(
        &cfg,
        &mut sess,
        "p",
        "high",
        &work,
        "custom/example-model",
        &opts,
        recorder(&mut seen, Ok(RunResult::default())),
    )
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "unsupported OpenCode model \"custom/example-model\""
    );
    assert!(seen.is_none());

    // Spawn errors pass through unwrapped.
    let err = run_opencode_entry_with(
        &cfg,
        &mut sess,
        "p",
        "high",
        &work,
        &grok(),
        &opts,
        |_, _| Err(anyhow::anyhow!("start opencode: boom")),
    )
    .unwrap_err();
    assert_eq!(format!("{err:#}"), "start opencode: boom");
}

#[test]
fn run_opts_debug_never_prints_the_key() {
    let opts = OpencodeRunOpts {
        permission: String::new(),
        api_key: "sk-secret".into(),
        drop_env: vec![],
    };
    let text = format!("{opts:?}");
    assert!(!text.contains("sk-secret"), "{text}");
    assert!(text.contains("<redacted>"), "{text}");
}
