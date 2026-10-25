//! China executor tests: the effort map, the version check, and the exact
//! request per mode.

use super::*;
use crate::executor::testutil::{Env, recorder, strings};

#[test]
fn china_effort_maps_onto_moonshot_menu() {
    for (requested, want) in [
        ("", "max"),
        ("low", "low"),
        ("medium", "high"),
        ("high", "high"),
        ("xhigh", "max"),
        ("ultra", "max"),
        ("max", "max"),
    ] {
        assert_eq!(china_effort(requested).unwrap(), want, "{requested}");
    }
    assert!(china_effort("extreme").is_err());
}

#[test]
fn opencode_major_reads_v1_and_v2_output() {
    assert_eq!(opencode_major("opencode v2.1.3\n"), Some(2));
    assert_eq!(opencode_major("2.1.2"), Some(2));
    assert_eq!(opencode_major("1.4.0\n"), Some(1));
    assert_eq!(opencode_major("v10.0.0"), Some(10));
    assert_eq!(opencode_major(""), None);
    assert_eq!(opencode_major("opencode dev"), None);
}

#[cfg(unix)]
#[test]
fn china_preflight_needs_opencode_v2() {
    let env = Env::new();
    assert_eq!(
        china_preflight(&env.config()).unwrap_err().to_string(),
        "opencode CLI not installed. Install: curl -fsSL https://opencode.ai/install | bash"
    );
    env.fake("opencode", "#!/bin/sh\necho 1.4.0\n");
    assert_eq!(
        china_preflight(&env.config()).unwrap_err().to_string(),
        r#"china needs opencode v2 or later, found "1.4.0". Update: opencode upgrade"#
    );
    env.fake("opencode", "#!/bin/sh\necho 'opencode v2.1.3'\n");
    china_preflight(&env.config()).unwrap();
}

#[test]
fn china_config_content_declares_model_and_profile() {
    let review = china_config_content("moonshot", "kimi-k3.1-pro", "max", false);
    let want = format!(
        r#"{{"$schema":"https://opencode.ai/config.json","permission":{CHINA_READ_ONLY_PERMISSION},"agent":{{"rival-china":{{"mode":"primary","permission":{CHINA_READ_ONLY_PERMISSION}}}}},"provider":{{"moonshot":{{"models":{{"kimi-k3.1-pro":{{"options":{{"reasoningEffort":"max"}}}}}}}}}}}}"#
    );
    // The deny-all rule comes first, as tested live: later, more specific
    // rules then allow the native reads.
    assert!(review.contains(r#""permission":{"*":"deny","#), "{review}");
    let parsed: serde_json::Value = serde_json::from_str(&review).unwrap();
    assert_eq!(
        parsed,
        serde_json::from_str::<serde_json::Value>(&want).unwrap()
    );

    let raw: serde_json::Value =
        serde_json::from_str(&china_config_content("p", "a/b", "low", true)).unwrap();
    assert_eq!(
        raw["permission"],
        serde_json::from_str::<serde_json::Value>(OPENCODE_FULL_AUTO_PERMISSION).unwrap()
    );
    assert_eq!(raw["agent"]["rival-china"]["permission"], raw["permission"]);
    assert_eq!(
        raw["provider"]["p"]["models"]["a/b"]["options"]["reasoningEffort"],
        "low"
    );
}

/// The exact request per mode: v2 argv, no key, the profile by mode, and
/// the credential strip in raw mode only.
#[test]
fn run_china_request_per_mode() {
    let env = Env::new();
    let cfg = env.config();
    let work = env.work_str();
    let selector = config::CHINA_DEFAULT_MODEL;

    for (mode, raw, extra_drop) in [
        ("review", false, &[][..]),
        ("raw", true, &RAW_DROP_ENV[..]),
        ("security", false, &[][..]),
    ] {
        let mut sess = env.session(config::CHINA_LABEL, mode, selector, &work);
        let mut seen = None;
        run_china_with(
            &cfg,
            &mut sess,
            "look",
            "high",
            &work,
            None,
            recorder(&mut seen, Ok(RunResult::default())),
        )
        .unwrap();
        let seen = seen.unwrap();
        assert_eq!(seen.binary, "opencode");
        assert_eq!(
            seen.args,
            strings(&[
                "run",
                "--standalone",
                "--agent",
                "rival-china",
                "-m",
                "moonshot/kimi-k3.1-pro"
            ])
        );
        assert_eq!(
            seen.env,
            vec![
                format!(
                    "OPENCODE_CONFIG_CONTENT={}",
                    china_config_content("moonshot", "kimi-k3.1-pro", "high", raw)
                ),
                "OPENCODE_DISABLE_PROJECT_CONFIG=1".to_string(),
            ],
            "{mode}"
        );
        assert_eq!(seen.drop_env, strings(extra_drop), "{mode}");
        assert_eq!(
            seen.prompt,
            format!(
                "{}\n\n{}\nlook",
                config::SYSTEM_PROMPT,
                cfg.build_workdir_preamble(Path::new(&work))
            )
        );
    }
}

#[test]
fn run_china_rejects_a_bad_selector() {
    let env = Env::new();
    let work = env.work_str();
    let mut sess = env.session(config::CHINA_LABEL, "review", "no-slash", &work);
    let err = run_china_with(&env.config(), &mut sess, "p", "max", &work, None, |_, _| {
        panic!("must not spawn")
    })
    .unwrap_err();
    assert!(err.to_string().contains("invalid china.model"), "{err}");
}

/// The version check gets the filtered env: a blocked variable from a
/// reviewed repo's .env (here NODE_OPTIONS) never reaches it.
#[cfg(unix)]
#[test]
fn china_preflight_filters_the_env() {
    let mut env = Env::new();
    env.set("NODE_OPTIONS", Some("--require ./evil.js"));
    env.fake(
        "opencode",
        "#!/bin/sh\nif [ -n \"$NODE_OPTIONS\" ]; then echo leaked; exit 1; fi\necho 'opencode v2.1.3'\n",
    );
    china_preflight(&env.config()).unwrap();
}

#[test]
fn parse_opencode_models_keeps_selectors_sorted() {
    let text = "WARN something\nopenrouter/z-ai/glm-5.3\nmoonshot/kimi-k3.1-pro\n\nopenrouter/z-ai/glm-5.3\nnot a model\n";
    assert_eq!(
        parse_opencode_models(text),
        strings(&["moonshot/kimi-k3.1-pro", "openrouter/z-ai/glm-5.3"])
    );
}

#[cfg(unix)]
#[test]
fn china_models_runs_opencode_models() {
    let env = Env::new();
    env.fake(
        "opencode",
        "#!/bin/sh\n[ \"$1 $2\" = 'models --standalone' ] || exit 3\n[ \"$OPENCODE_DISABLE_PROJECT_CONFIG\" = 1 ] || exit 4\nprintf 'openrouter/qwen/qwen4\\nmoonshot/kimi-k3.1-pro\\n'\n",
    );
    assert_eq!(
        china_models(&env.config()).unwrap(),
        strings(&["moonshot/kimi-k3.1-pro", "openrouter/qwen/qwen4"])
    );
}

/// A cancelled caller context (a model check the user cancelled) ends the
/// version query at once, not after its own 30 s bound.
#[cfg(unix)]
#[test]
fn china_preflight_honours_the_caller_context() {
    let env = Env::new();
    env.fake("opencode", "#!/bin/sh\nsleep 20\n");
    let (ctx, cancel) = Context::background().with_cancel();
    cancel.cancel();
    let started = std::time::Instant::now();
    let err = oscmd::with_context(&ctx, || china_preflight(&env.config())).unwrap_err();
    assert!(err.to_string().contains("context canceled"), "{err}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}
