//! Tests for `rival command consilium`: stdin grammar (modes, quoted `--goal`,
//! `-re`, `--` escape), terminal usage output, ratings/breakdown output, and
//! Neon-skipped notice forwarding.

use std::sync::Mutex;

use rival_core::config::{CLAUDE_MODEL, NEON_LABEL, NEON_MODEL, SOL_MODEL};
use rival_core::result::RatingBreakdown;
use rival_core::review::{
    ConsiliumCLIResult, ConsiliumMode, ConsiliumOutput, ConsiliumRunResult, SkippedCLI,
};

use super::*;
use crate::testutil::{FakeStdin, Fixture, no_mr, with_env};

#[test]
fn parse_consilium_input_variants() {
    assert_eq!(
        parse_consilium_input("").unwrap(),
        ParsedConsiliumInput {
            mode: None,
            effort: String::new(),
            goal: String::new(),
            scope: WHOLE_PROJECT.to_string(),
            auto_scope: true,
        }
    );
    assert_eq!(
        parse_consilium_input("src/api/").unwrap(),
        ParsedConsiliumInput {
            mode: None,
            effort: String::new(),
            goal: String::new(),
            scope: "src/api/".to_string(),
            auto_scope: false,
        }
    );
    assert_eq!(
        parse_consilium_input("heavy src/api/").unwrap(),
        ParsedConsiliumInput {
            mode: Some(ConsiliumMode::Heavy),
            effort: String::new(),
            goal: String::new(),
            scope: "src/api/".to_string(),
            auto_scope: false,
        }
    );
    assert_eq!(
        parse_consilium_input("heavy --goal \"design fixes and clarity\" src/api/").unwrap(),
        ParsedConsiliumInput {
            mode: Some(ConsiliumMode::Heavy),
            effort: String::new(),
            goal: "design fixes and clarity".to_string(),
            scope: "src/api/".to_string(),
            auto_scope: false,
        }
    );
    assert_eq!(
        parse_consilium_input("src/api/ --goal 'API ergonomics' -re high").unwrap(),
        ParsedConsiliumInput {
            mode: None,
            effort: "high".to_string(),
            goal: "API ergonomics".to_string(),
            scope: "src/api/".to_string(),
            auto_scope: false,
        }
    );
    assert_eq!(
        parse_consilium_input("-g=\"clean abstractions\"").unwrap(),
        ParsedConsiliumInput {
            mode: None,
            effort: String::new(),
            goal: "clean abstractions".to_string(),
            scope: WHOLE_PROJECT.to_string(),
            auto_scope: true,
        }
    );
    assert_eq!(
        parse_consilium_input("-- heavy --goal literal").unwrap(),
        ParsedConsiliumInput {
            mode: None,
            effort: String::new(),
            goal: String::new(),
            scope: "heavy --goal literal".to_string(),
            auto_scope: false,
        }
    );
    assert!(parse_consilium_input("normal heavy").is_err());
    assert!(parse_consilium_input("--bogus src/").is_err());
    assert!(parse_consilium_input("-re turbo src/").is_err());
    assert!(parse_consilium_input("--goal").is_err());
    assert!(parse_consilium_input("--goal \"unclosed").is_err());
}

#[test]
fn terminal_stdin_prints_usage() {
    let fix = Fixture::new();
    let workdir = tempfile::tempdir().unwrap();
    let mut stdin = FakeStdin::new("");
    stdin.char_device = true;
    stdin.forbid_read = true;
    let opts = ConsiliumOptions {
        workdir: workdir.path().to_str().unwrap().to_string(),
        ..ConsiliumOptions::default()
    };
    let out = with_env(&fix, &mut stdin, &*no_mr(), |env| {
        run_command_consilium(env, &opts, &|_, _, _, _| {
            panic!("runner must not execute on terminal stdin")
        })
    });
    out.result.unwrap();
    assert!(
        out.stdout.contains("/consilium heavy src/api/"),
        "{}",
        out.stdout
    );
    assert!(out.stdout.contains("Neon (xhigh)"), "{}", out.stdout);
}

#[test]
fn run_command_consilium_passes_goal_mode_and_prints_neon_skip_notice() {
    let fix = Fixture::new();
    let workdir = tempfile::tempdir().unwrap();
    let mut stdin = FakeStdin::new("heavy --goal \"design quality\" src/lib.rs");
    let seen: Mutex<Option<(ConsiliumMode, String, String, String)>> = Mutex::new(None);
    let runner = |_: &Context, _: &Config, req: &ConsiliumRequest<'_>, _: &mut dyn Write| {
        assert!(req.prompt.contains("Review goal: design quality"));
        *seen.lock().unwrap() = Some((
            req.mode,
            req.scope.to_string(),
            req.goal.to_string(),
            req.effort.to_string(),
        ));
        Ok(ConsiliumRunResult {
            mode: req.mode,
            results: vec![
                ConsiliumCLIResult {
                    cli: "claude".into(),
                    model: CLAUDE_MODEL.into(),
                    parsed: Some(ConsiliumOutput {
                        summary: "Good design.".into(),
                        rating: 9,
                        breakdown: vec![RatingBreakdown {
                            section: "Modularity".into(),
                            rating: 9,
                            notes: "Clean boundaries.".into(),
                        }],
                        findings: Vec::new(),
                    }),
                    raw: String::new(),
                    log_path: "/tmp/claude.log".into(),
                },
                ConsiliumCLIResult {
                    cli: "codex".into(),
                    model: SOL_MODEL.into(),
                    parsed: Some(ConsiliumOutput {
                        summary: "Nearly there.".into(),
                        rating: 8,
                        breakdown: vec![RatingBreakdown {
                            section: "Modularity".into(),
                            rating: 8,
                            notes: "One module mixes concerns.".into(),
                        }],
                        findings: Vec::new(),
                    }),
                    raw: String::new(),
                    log_path: "/tmp/sol.log".into(),
                },
            ],
            skipped: vec![SkippedCLI {
                cli: NEON_LABEL.into(),
                model: NEON_MODEL.into(),
                reason: "neon runtime is not installed".into(),
            }],
            notices: vec![
                "rival: ⚠ neon failed and was skipped — neon runtime is not installed\n".into(),
            ],
        })
    };
    let opts = ConsiliumOptions {
        workdir: workdir.path().to_str().unwrap().to_string(),
        no_queue: true,
        ..ConsiliumOptions::default()
    };
    let out = with_env(&fix, &mut stdin, &*no_mr(), |env| {
        run_command_consilium(env, &opts, &runner)
    });
    out.result.unwrap();

    assert_eq!(
        seen.lock().unwrap().clone(),
        Some((
            ConsiliumMode::Heavy,
            "src/lib.rs".into(),
            "design quality".into(),
            String::new()
        ))
    );
    assert!(
        out.stdout
            .contains("═══ RIVAL CONSILIUM [heavy] (claude + sol) ═══"),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout.contains("Goal: design quality"),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout
            .contains("Ratings: claude 9/10 · sol 8/10 · min 8/10"),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout
            .contains("- Modularity: 9/10 — Clean boundaries."),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout
            .contains("Skipped: neon — neon runtime is not installed"),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout
            .contains("rival: ⚠ neon failed and was skipped — neon runtime is not installed"),
        "{}",
        out.stdout
    );
    assert!(
        out.stderr
            .contains("rival: ⚠ neon failed and was skipped — neon runtime is not installed"),
        "{}",
        out.stderr
    );
}
