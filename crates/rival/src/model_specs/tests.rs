//! Model spec, effort and usage cases; the tree metadata cases live in
//! `tree/tests.rs`.

use super::*;

use rival_core::config::{CLAUDE_LABEL, CODEX_LABEL, GROK_LABEL, K3_LABEL, UserConfig};

use crate::model_run::{RunOptions, run_model_run};
use crate::testutil::{FakeStdin, Fixture, fake_run, fake_spec, no_mr, with_env};

fn cfg() -> Config {
    Fixture::new().cfg
}

/// The command word and the display label differ for K3, so one field
/// cannot serve both. Error text and logs use the label; the tree uses the
/// command name.
#[test]
fn spec_labels_and_command_names() {
    let cases = [
        (codex_spec(), "codex", CODEX_LABEL, "codex"),
        (claude_spec(), "claude", CLAUDE_LABEL, "claude"),
        (k3_spec(), "k3", K3_LABEL, "opencode"),
        (grok_spec(), "grok", GROK_LABEL, GROK_LABEL),
    ];
    for (spec, command_name, label, cli) in cases {
        assert_eq!(spec.command_name, command_name);
        assert_eq!(spec.label(), label, "{command_name}");
        assert_eq!(spec.cli, cli, "{command_name}");
    }
    assert_eq!(codex_spec().model, config::CODEX_MODEL);
    assert_eq!(claude_spec().model, config::CLAUDE_MODEL);
    assert_eq!(k3_spec().model, config::KIMI_MODEL);
    assert_eq!(grok_spec().model, config::GROK_MODEL);
}

#[test]
fn k3_effort_is_pinned_to_max() {
    let cfg = cfg();
    for requested in ["", "low", "high", "xhigh", "ultra", "bogus"] {
        assert_eq!(
            k3_spec().resolve_effort(&cfg, requested).unwrap(),
            "max",
            "{requested:?}"
        );
    }
}

#[test]
fn grok_effort_clamps_to_its_own_menu() {
    let cfg = cfg();
    for requested in ["xhigh", "ultra"] {
        assert_eq!(grok_spec().resolve_effort(&cfg, requested).unwrap(), "high");
    }
}

/// Codex receives the level verbatim: ultra is its own level, not an xhigh
/// alias.
#[test]
fn codex_effort_is_not_aliased() {
    let cfg = cfg();
    for requested in ["xhigh", "ultra"] {
        assert_eq!(
            codex_spec().resolve_effort(&cfg, requested).unwrap(),
            requested
        );
    }
}

/// Defaults come from the config constants: codex xhigh, claude medium,
/// grok high; a configured effort wins; an invalid one is an error.
#[test]
fn defaults_configured_efforts_and_invalid_efforts() {
    let cfg = cfg();
    assert_eq!(codex_spec().resolve_effort(&cfg, "").unwrap(), "xhigh");
    assert_eq!(claude_spec().resolve_effort(&cfg, "").unwrap(), "medium");
    assert_eq!(grok_spec().resolve_effort(&cfg, "").unwrap(), "high");

    let configured = cfg.clone().with_user_config(Some(UserConfig {
        efforts: [("codex", "low"), ("claude", "high"), ("grok", "ultra")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        ..UserConfig::default()
    }));
    assert_eq!(codex_spec().resolve_effort(&configured, "").unwrap(), "low");
    assert_eq!(
        claude_spec().resolve_effort(&configured, "").unwrap(),
        "high"
    );
    assert_eq!(grok_spec().resolve_effort(&configured, "").unwrap(), "high");
    assert_eq!(
        codex_spec().resolve_effort(&configured, "medium").unwrap(),
        "medium"
    );

    for spec in [codex_spec(), claude_spec(), grok_spec()] {
        let err = spec.resolve_effort(&cfg, "not-a-level").unwrap_err();
        assert_eq!(
            err,
            format!("invalid effort \"not-a-level\" for {}", spec.label())
        );
    }
    // The ladder is case-insensitive and trimmed before validation.
    assert_eq!(codex_spec().resolve_effort(&cfg, " HIGH ").unwrap(), "high");
}

#[test]
fn only_claude_reports_an_auth_hint() {
    let cfg = cfg();
    for spec in [codex_spec(), k3_spec(), grok_spec()] {
        assert_eq!(
            spec.auth_hint(&cfg, "/nonexistent.log"),
            "",
            "{}",
            spec.command_name
        );
    }
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("x.log");
    std::fs::write(&log, "Invalid API key · Please run /login\n").unwrap();
    let log = log.to_str().unwrap();
    assert_eq!(codex_spec().auth_hint(&cfg, log), "");
    let hint = claude_spec().auth_hint(&cfg, log);
    assert!(!hint.is_empty());
    assert_eq!(
        hint,
        executor::claude_auth_hint(&cfg, config::CLAUDE_MODEL, Path::new(log))
    );
}

#[test]
fn session_mode_names_the_run() {
    assert_eq!(session_mode(true), "review");
    assert_eq!(session_mode(false), "raw");
}

/// An invalid effort must fail before the workflow touches the provider or
/// blocks on stdin. Ordering this wrong hides the real error behind an auth
/// failure, or hangs forever waiting for input that will never come.
#[test]
fn run_model_run_validates_effort_before_anything_else() {
    let fix = Fixture::new();
    let f = fake_run("");
    let spec = fake_spec(&f);
    let mut stdin = FakeStdin::new("");
    // A read would panic: the effort must fail first.
    stdin.forbid_read = true;
    let workdir = tempfile::tempdir().unwrap();
    let out = with_env(&fix, &mut stdin, &*no_mr(), |env| {
        run_model_run(
            env,
            &spec,
            RunOptions {
                workdir: workdir.path().to_str().unwrap().into(),
                no_queue: true,
                effort: "not-a-level".into(),
                prompt_stdin: true,
                ..RunOptions::default()
            },
        )
    });
    let err = out.result.unwrap_err();
    assert_eq!(err.message, "invalid effort \"not-a-level\" for codex");
    assert_eq!(err.code, 1);
    assert_eq!(
        f.borrow().preflight_calls,
        0,
        "preflight ran before the effort was validated"
    );
    assert!(fix.sessions().is_empty());
}

#[test]
fn claude_usage_names_the_public_command_and_effort_fallback() {
    let lower = CLAUDE_USAGE.to_lowercase();
    assert!(lower.contains("/rival-claude"), "{lower}");
    assert!(lower.contains("built-in default: medium"), "{lower}");
}

/// The sandbox is selected by the review boolean handed to the grok
/// adapter, so a review-mode session that reported raw would run grok
/// unsandboxed with --yolo. Pin both the mode derivation and the boolean
/// that follows from it.
#[test]
fn grok_session_mode_drives_review_sandbox() {
    let cases = [
        ("plain prompt", "explain the auth flow", "raw", false),
        ("review without scope", "review", "review", true),
        ("review with scope", "review src/api/", "review", true),
        (
            "review with effort",
            "-re high review src/api/",
            "review",
            true,
        ),
        (
            "prompt with effort",
            "-re low explain the auth flow",
            "raw",
            false,
        ),
    ];
    for (name, raw, want_mode, want_review) in cases {
        let parsed = (grok_spec().parse)(raw).unwrap();
        let mode = session_mode(parsed.is_review);
        assert_eq!(mode, want_mode, "{name}");
        assert_eq!(mode == "review", want_review, "{name}");
    }
}

#[test]
fn grok_uses_configured_grok_effort_default() {
    let parsed = (grok_spec().parse)("review").unwrap();
    let effort = cfg()
        .resolve_effort(
            config::GROK_MODEL,
            &parsed.effort,
            config::DEFAULT_REVIEW_EFFORT,
        )
        .unwrap();
    assert_eq!(effort, "high");
}

/// The session must record the effort grok is actually handed. rival's
/// ladder goes past what grok exposes, so an ultra request lands as high.
#[test]
fn grok_effort_recorded_after_clamp() {
    for (resolved, want) in [
        ("low", "low"),
        ("medium", "medium"),
        ("high", "high"),
        ("ultra", "high"),
        ("xhigh", "high"),
        ("minimal", "low"),
    ] {
        assert_eq!(executor::grok_effort(resolved).unwrap(), want, "{resolved}");
    }
}

/// ResolveEffort feeding straight into the clamp is the exact chain both
/// `rival command grok` and `rival run grok` use before the session.
#[test]
fn grok_resolved_ultra_effort_is_sent_as_high() {
    let resolved = cfg()
        .resolve_effort(config::GROK_MODEL, "ultra", config::DEFAULT_REVIEW_EFFORT)
        .unwrap();
    assert_eq!(executor::grok_effort(&resolved).unwrap(), "high");
    assert_eq!(grok_spec().resolve_effort(&cfg(), "ultra").unwrap(), "high");
}

#[test]
fn grok_usage_uses_only_public_naming() {
    let lower = GROK_USAGE.to_lowercase();
    assert!(lower.contains("/rival-grok"));
    assert!(lower.contains("built-in: high"), "{lower}");
}

/// The usage texts are pinned byte for byte (spot checks on each).
#[test]
fn usage_texts_are_pinned() {
    assert!(CODEX_USAGE.starts_with(
        "Usage:\n  /rival-codex 'explain the auth flow' — run any prompt with Codex\n"
    ));
    assert!(
        CODEX_USAGE
            .ends_with("Omitted uses efforts.codex from ~/.rival/config.yaml (built-in: xhigh).")
    );
    assert!(K3_USAGE.starts_with("Usage:\n  echo 'explain the auth flow' | rival command k3\n"));
    assert!(K3_USAGE.ends_with(
        "raw prompts run full auto and can edit files and run commands in the workdir."
    ));
    assert!(GROK_USAGE.ends_with(
        "Review mode runs read-only sandboxed; raw prompts can edit files in the workdir."
    ));
    assert_eq!(codex_spec().usage, CODEX_USAGE);
    assert_eq!(claude_spec().usage, CLAUDE_USAGE);
    assert_eq!(k3_spec().usage, K3_USAGE);
    assert_eq!(grok_spec().usage, GROK_USAGE);
}

/// Fable runs like Claude and Sol like Codex: runtime, label, effort
/// default, and their own `efforts.fable` / `efforts.sol` keys.
#[test]
fn fable_and_sol_specs_follow_their_runtimes() {
    let fable = fable_spec();
    assert_eq!(
        (fable.command_name, fable.cli, fable.model, fable.label()),
        ("fable", "claude", config::FABLE_MODEL, "fable".to_string())
    );
    assert_eq!(fable.usage, FABLE_USAGE);
    let sol = sol_spec();
    assert_eq!(
        (sol.command_name, sol.cli, sol.model, sol.label()),
        ("sol", "codex", config::SOL_MODEL, "sol".to_string())
    );
    assert_eq!(sol.usage, SOL_USAGE);

    let cfg = cfg();
    assert_eq!(fable_spec().resolve_effort(&cfg, "").unwrap(), "medium");
    assert_eq!(sol_spec().resolve_effort(&cfg, "").unwrap(), "xhigh");
    assert_eq!(sol_spec().resolve_effort(&cfg, "ultra").unwrap(), "ultra");

    let configured = cfg.clone().with_user_config(Some(UserConfig {
        efforts: [("fable", "low"), ("sol", "high"), ("claude", "xhigh")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        ..UserConfig::default()
    }));
    assert_eq!(fable_spec().resolve_effort(&configured, "").unwrap(), "low");
    assert_eq!(sol_spec().resolve_effort(&configured, "").unwrap(), "high");
    assert_eq!(
        claude_spec().resolve_effort(&configured, "").unwrap(),
        "xhigh"
    );

    // Fable's auth hint is Claude's, for its own model.
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("x.log");
    std::fs::write(&log, "Invalid API key · Please run /login\n").unwrap();
    let log = log.to_str().unwrap();
    assert_eq!(
        fable_spec().auth_hint(&cfg, log),
        executor::claude_auth_hint(&cfg, config::FABLE_MODEL, Path::new(log))
    );
    assert!(!fable_spec().auth_hint(&cfg, log).is_empty());
    assert_eq!(sol_spec().auth_hint(&cfg, log), "");
}
