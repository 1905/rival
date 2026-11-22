//! Neon (`agy`) executor tests: effort clamping, model slugs, exact argv
//! vectors for review and raw modes, error wrapping, preflight and auth hints.

use super::*;
#[cfg(unix)]
use crate::executor::testutil::retry_busy;
use crate::executor::testutil::{Env, strings};

/// `agy`'s argv for the given parts.
fn argv(slug: &str, full_prompt: &str, review: bool) -> Vec<String> {
    let mut args = strings(&[
        "--model",
        slug,
        "--disable-slash-commands",
        "--output-format",
        "text",
    ]);
    if review {
        args.extend(strings(&["--settings", NEON_READ_ONLY_SETTINGS]));
    } else {
        args.push("--dangerously-skip-permissions".to_string());
    }
    args.extend(strings(&["-p", full_prompt]));
    args
}

#[test]
fn is_neon_model_cases() {
    for id in [
        config::NEON_MODEL,
        "gemini-4-neon",
        "gemini-4-neon-low",
        "gemini-4-neon-medium",
        "gemini-4-neon-high",
        "gemini-4-neon-xhigh",
    ] {
        assert!(is_neon_model(id), "{id}");
    }
    for other in ["", "neon", "gemini-4-neon-ultra", config::CODEX_MODEL] {
        assert!(!is_neon_model(other), "{other}");
    }
}

#[test]
fn neon_effort_is_always_xhigh() {
    for effort in [
        "", "low", "medium", "high", "xhigh", "ultra", "max", "minimal", "none",
    ] {
        assert_eq!(neon_effort(effort).unwrap(), "xhigh", "{effort}");
    }
    assert_eq!(
        neon_effort("turbo").unwrap_err().to_string(),
        "unsupported neon effort \"turbo\"; neon only supports: xhigh"
    );
}

#[test]
fn neon_model_slug_is_always_xhigh() {
    assert_eq!(neon_model_slug("", "xhigh").unwrap(), "gemini-4-neon-xhigh");
    assert_eq!(
        neon_model_slug("   ", "low").unwrap(),
        "gemini-4-neon-xhigh"
    );
    assert_eq!(
        neon_model_slug(config::NEON_MODEL, "ultra").unwrap(),
        "gemini-4-neon-xhigh"
    );
    assert_eq!(
        neon_model_slug("gemini-4-neon", "low").unwrap(),
        "gemini-4-neon-xhigh"
    );
    assert_eq!(
        neon_model_slug("gemini-4-neon-medium", "high").unwrap(),
        "gemini-4-neon-xhigh"
    );
    assert!(neon_model_slug(config::NEON_MODEL, "turbo").is_err());
}

#[test]
fn neon_run_args_review_and_raw() {
    let review_args = neon_run_args(config::NEON_MODEL, "prompt", "xhigh", true).unwrap();
    assert_eq!(review_args, argv("gemini-4-neon-xhigh", "prompt", true));
    assert!(
        !review_args
            .iter()
            .any(|a| a == "--dangerously-skip-permissions")
    );

    let raw_args = neon_run_args(config::NEON_MODEL, "prompt", "ultra", false).unwrap();
    assert_eq!(raw_args, argv("gemini-4-neon-xhigh", "prompt", false));
    assert!(!raw_args.iter().any(|a| a == "--settings"));
}

#[test]
fn neon_read_only_settings_is_valid_json() {
    let v: serde_json::Value = serde_json::from_str(NEON_READ_ONLY_SETTINGS).unwrap();
    assert_eq!(v["toolPermission"], "request-review");
    assert_eq!(v["allowNonWorkspaceAccess"], false);
    let deny = v["permissions"]["deny"].as_array().unwrap();
    for rule in ["write_file(*)", "command(*)", "read_url(*)", "mcp(*)"] {
        assert!(deny.iter().any(|r| r == rule), "missing deny rule {rule}");
    }
}

#[test]
fn run_neon_model_with_threads_model_effort_and_log() {
    let env = Env::new();
    let cfg = env.config();
    let work = env.work_str();
    let mut sess = Session::default();
    let mut seen = None;
    run_neon_model_with(
        &cfg,
        &mut sess,
        "review src/",
        "xhigh",
        &work,
        "",
        true,
        Some("/tmp/repair.log"),
        crate::executor::testutil::recorder(&mut seen, Ok(RunResult::default())),
    )
    .unwrap();
    let rec = seen.unwrap();
    assert_eq!(rec.binary, "agy");
    assert_eq!(rec.prompt, "");
    assert_eq!(rec.log.as_deref(), Some("/tmp/repair.log"));
    let want_prompt = neon_full_prompt(&cfg, "review src/", &work);
    assert_eq!(rec.args, argv("gemini-4-neon-xhigh", &want_prompt, true));
}

#[test]
fn run_neon_model_errors_are_normalized() {
    let env = Env::new();
    let cfg = env.config();
    let mut sess = Session::default();
    let err = run_neon_model_with(
        &cfg,
        &mut sess,
        "p",
        "turbo",
        "/repo",
        "",
        true,
        None,
        |_, _| Ok(RunResult::default()),
    )
    .unwrap_err();
    assert_eq!(
        format!("{err:#}"),
        "neon runtime: unsupported neon effort \"turbo\"; neon only supports: xhigh"
    );

    let err = run_neon_model_with(
        &cfg,
        &mut sess,
        "p",
        "xhigh",
        "/repo",
        "",
        true,
        None,
        |_, _| Err(anyhow!("start agy: exec: \"agy\": not found")),
    )
    .unwrap_err();
    assert_eq!(
        format!("{err:#}"),
        "neon runtime: start Neon runtime: exec: \"agy\": not found"
    );
}

#[cfg(unix)]
#[test]
fn run_neon_end_to_end() {
    let env = Env::new();
    env.fake("agy", "#!/bin/sh\nprintf '%s\\n' \"$@\"\n");
    let cfg = env.config();
    let work = env.work_str();
    let mut sess = env.session("neon", "review", config::NEON_MODEL, &work);
    let mut out = Vec::new();
    let result = retry_busy(
        || {
            out.clear();
            run_neon(
                &Context::background(),
                &cfg,
                &mut sess,
                "prompt",
                "xhigh",
                &work,
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
    let want = argv(
        "gemini-4-neon-xhigh",
        &neon_full_prompt(&cfg, "prompt", &work),
        true,
    );
    assert_eq!(text, format!("{}\n", want.join("\n")));
}

#[cfg(unix)]
#[test]
fn neon_preflight_checks_path() {
    let env = Env::new();
    assert_eq!(
        neon_preflight(&env.config()).unwrap_err().to_string(),
        "neon runtime is not installed"
    );
    env.fake("agy", "#!/bin/sh\nexit 0\n");
    neon_preflight(&env.config()).unwrap();
}

#[test]
fn neon_auth_hint_detects_auth_errors() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("run.log");
    assert_eq!(neon_auth_hint(&log), "");

    std::fs::write(&log, "normal output\n").unwrap();
    assert_eq!(neon_auth_hint(&log), "");

    std::fs::write(&log, "error: authentication required\n").unwrap();
    assert_eq!(
        neon_auth_hint(&log),
        "rival: neon authentication failed — run `agy` once to sign in"
    );
}
