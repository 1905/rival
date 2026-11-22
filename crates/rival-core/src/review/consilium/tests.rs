//! Tests for 3-judge Consilium reviews: normal & heavy rosters, custom goals,
//! subsection rating breakdowns, Neon xhigh invariance, Neon preflight &
//! runtime skip notifications, and rendering.

use std::collections::HashMap;
use std::sync::Mutex;

use super::*;
use crate::config::{CLAUDE_MODEL, CODEX_MODEL, FABLE_MODEL, NEON_LABEL, NEON_MODEL, SOL_MODEL};
use crate::paths::Paths;

fn test_cfg() -> (tempfile::TempDir, Config) {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::from_home(dir.path());
    let mut env = HashMap::new();
    env.insert("RIVAL_HOME".into(), dir.path().to_string_lossy().into());
    env.insert("RIVAL_NO_QUEUE".into(), "1".into());
    let cfg = Config::new(paths, env, Some(dir.path().to_path_buf()));
    (dir, cfg)
}

fn ok_consilium_json(summary: &str, rating: u8, title: &str) -> Vec<u8> {
    format!(
        r#"{{"summary":"{summary}","rating":{rating},"breakdown":[{{"section":"Correctness","rating":{rating},"notes":"One edge case left."}},{{"section":"Clarity","rating":9,"notes":"Clean code."}}],"findings":[{{"file":"src/a.rs","line":10,"severity":"high","category":"bug","title":"{title}","body":"Null check missing.","failure_scenario":"Panics on empty input.","suggestion":"Check for empty input.","confidence":9}}]}}"#
    )
    .into_bytes()
}

#[test]
fn consilium_mode_rosters() {
    assert_eq!(
        ConsiliumMode::Normal.models(),
        [CLAUDE_MODEL, SOL_MODEL, NEON_MODEL]
    );
    assert_eq!(
        ConsiliumMode::Heavy.models(),
        [FABLE_MODEL, CODEX_MODEL, NEON_MODEL]
    );
}

#[test]
fn parse_consilium_log_extracts_rating_and_breakdown() {
    let raw =
        String::from_utf8(ok_consilium_json("Solid with one issue.", 8, "nil check")).unwrap();
    let parsed = parse_consilium_log(&raw).unwrap();
    assert_eq!(parsed.rating, 8);
    assert_eq!(parsed.summary, "Solid with one issue.");
    assert_eq!(parsed.breakdown.len(), 2);
    assert_eq!(parsed.breakdown[0].section, "Correctness");
    assert_eq!(parsed.breakdown[0].rating, 8);
    assert_eq!(parsed.breakdown[0].notes, "One edge case left.");
    assert_eq!(parsed.findings.len(), 1);
}

#[test]
fn run_consilium_normal_runs_all_three_judges_with_breakdown() {
    let (_dir, cfg) = test_cfg();
    let seen: Mutex<Vec<(String, String, String)>> = Mutex::new(Vec::new());
    let ex = ConsiliumExecutor {
        preflight: Box::new(|_, _| Ok(())),
        run: Box::new(|_, sess, cli, _, effort, _, log| {
            if log.is_none() {
                seen.lock()
                    .unwrap()
                    .push((cli.into(), sess.model.clone(), effort.into()));
            }
            let rating = match cli {
                "claude" => 8,
                "codex" => 9,
                _ => 7,
            };
            let payload = ok_consilium_json(
                &format!("{} verdict", config::engine_label(cli, &sess.model)),
                rating,
                &format!("issue from {}", config::engine_label(cli, &sess.model)),
            );
            Ok((payload, 0))
        }),
    };
    let req = ConsiliumRequest {
        mode: ConsiliumMode::Normal,
        prompt: "review src/",
        scope: "src/",
        goal: "design cleanliness",
        effort: "",
        workdir: "/repo",
        group_id: "grp-1",
        no_queue: true,
    };
    let mut stderr = Vec::new();
    let result =
        run_consilium_review_with(&Context::background(), &cfg, &ex, &req, &mut stderr).unwrap();

    assert_eq!(result.mode, ConsiliumMode::Normal);
    assert!(result.skipped.is_empty());
    assert!(result.notices.is_empty());
    let models: Vec<&str> = result.results.iter().map(|r| r.model.as_str()).collect();
    assert_eq!(models, [CLAUDE_MODEL, SOL_MODEL, NEON_MODEL]);

    let calls = seen.lock().unwrap();
    assert!(calls.contains(&("claude".into(), CLAUDE_MODEL.into(), "medium".into())));
    assert!(calls.contains(&("codex".into(), SOL_MODEL.into(), "xhigh".into())));
    assert!(calls.contains(&(NEON_LABEL.into(), NEON_MODEL.into(), "xhigh".into())));

    let formatted = format_consilium_result(&result, "src/", "design cleanliness");
    assert!(formatted.contains("═══ RIVAL CONSILIUM [normal] (claude + sol + neon) ═══"));
    assert!(formatted.contains("Goal: design cleanliness"));
    assert!(formatted.contains("Ratings: claude 8/10 · sol 9/10 · neon 7/10 · min 7/10"));
    assert!(formatted.contains("── claude (claude-opus-5-5) — 8/10 ──"));
    assert!(formatted.contains("Rating breakdown:"));
    assert!(formatted.contains("- Correctness: 8/10 — One edge case left."));
    assert!(formatted.contains("- Clarity: 9/10 — Clean code."));
}

#[test]
fn run_consilium_heavy_keeps_neon_at_xhigh_under_effort_override() {
    let (_dir, cfg) = test_cfg();
    let seen: Mutex<Vec<(String, String, String)>> = Mutex::new(Vec::new());
    let ex = ConsiliumExecutor {
        preflight: Box::new(|_, _| Ok(())),
        run: Box::new(|_, sess, cli, _, effort, _, log| {
            if log.is_none() {
                seen.lock()
                    .unwrap()
                    .push((cli.into(), sess.model.clone(), effort.into()));
            }
            Ok((ok_consilium_json("Clean check.", 9, "minor nit"), 0))
        }),
    };
    let req = ConsiliumRequest {
        mode: ConsiliumMode::Heavy,
        prompt: "review src/",
        scope: "src/",
        goal: "",
        effort: "low",
        workdir: "/repo",
        group_id: "grp-2",
        no_queue: true,
    };
    let mut stderr = Vec::new();
    let result =
        run_consilium_review_with(&Context::background(), &cfg, &ex, &req, &mut stderr).unwrap();

    let models: Vec<&str> = result.results.iter().map(|r| r.model.as_str()).collect();
    assert_eq!(models, [FABLE_MODEL, CODEX_MODEL, NEON_MODEL]);

    let calls = seen.lock().unwrap();
    assert!(calls.contains(&("claude".into(), FABLE_MODEL.into(), "low".into())));
    assert!(calls.contains(&("codex".into(), CODEX_MODEL.into(), "low".into())));
    // Neon stays xhigh even when -re low is passed.
    assert!(calls.contains(&(NEON_LABEL.into(), NEON_MODEL.into(), "xhigh".into())));

    let formatted = format_consilium_result(&result, "src/", "");
    assert!(formatted.contains("═══ RIVAL CONSILIUM [heavy] (fable + codex + neon) ═══"));
    assert!(formatted.contains("Ratings: fable 9/10 · codex 9/10 · neon 9/10 · min 9/10"));
}

#[test]
fn neon_preflight_failure_notifies_and_skips() {
    let (_dir, cfg) = test_cfg();
    let ex = ConsiliumExecutor {
        preflight: Box::new(|cli, _| {
            if cli == NEON_LABEL {
                bail!("neon runtime is not installed");
            }
            Ok(())
        }),
        run: Box::new(|_, _, _, _, _, _, _| Ok((ok_consilium_json("Clean check.", 9, "nit"), 0))),
    };
    let req = ConsiliumRequest {
        mode: ConsiliumMode::Normal,
        prompt: "review src/",
        scope: "src/",
        goal: "",
        effort: "",
        workdir: "/repo",
        group_id: "grp-3",
        no_queue: true,
    };
    let mut stderr = Vec::new();
    let result =
        run_consilium_review_with(&Context::background(), &cfg, &ex, &req, &mut stderr).unwrap();

    let models: Vec<&str> = result.results.iter().map(|r| r.model.as_str()).collect();
    assert_eq!(models, [CLAUDE_MODEL, SOL_MODEL]);
    assert_eq!(result.skipped.len(), 1);
    assert_eq!(result.skipped[0].cli, NEON_LABEL);
    assert_eq!(
        result.notices,
        ["rival: ⚠ neon failed and was skipped — neon runtime is not installed\n"]
    );

    let formatted = format_consilium_result(&result, "src/", "");
    assert!(formatted.contains("═══ RIVAL CONSILIUM [normal] (claude + sol) ═══"));
    assert!(formatted.contains("Skipped: neon — neon runtime is not installed"));
}

#[test]
fn neon_runtime_failure_notifies_and_skips() {
    let (_dir, cfg) = test_cfg();
    let ex = ConsiliumExecutor {
        preflight: Box::new(|_, _| Ok(())),
        run: Box::new(|_, _, cli, _, _, _, _| {
            if cli == NEON_LABEL {
                return Ok((b"resource_exhausted (code 429)".to_vec(), 0));
            }
            Ok((ok_consilium_json("Clean check.", 8, "bug"), 0))
        }),
    };
    let req = ConsiliumRequest {
        mode: ConsiliumMode::Heavy,
        prompt: "review src/",
        scope: "src/",
        goal: "",
        effort: "",
        workdir: "/repo",
        group_id: "grp-4",
        no_queue: true,
    };
    let mut stderr = Vec::new();
    let result =
        run_consilium_review_with(&Context::background(), &cfg, &ex, &req, &mut stderr).unwrap();

    let models: Vec<&str> = result.results.iter().map(|r| r.model.as_str()).collect();
    assert_eq!(models, [FABLE_MODEL, CODEX_MODEL]);
    assert_eq!(result.skipped.len(), 1);
    assert_eq!(result.skipped[0].cli, NEON_LABEL);
    assert_eq!(
        result.notices,
        ["rival: ⚠ neon failed and was skipped — hit provider quota/rate limit (429)\n"]
    );

    let sessions = Session::load_all(cfg.paths());
    let neon_sess = sessions.iter().find(|s| s.cli == NEON_LABEL).unwrap();
    assert_eq!(neon_sess.status, "failed");
    assert_eq!(neon_sess.mode, session::MODE_CONSILIUM);
}

#[test]
fn all_judges_failing_returns_error() {
    let (_dir, cfg) = test_cfg();
    let ex = ConsiliumExecutor {
        preflight: Box::new(|_, _| Ok(())),
        run: Box::new(|_, _, _, _, _, _, _| Ok((Vec::new(), 1))),
    };
    let req = ConsiliumRequest {
        mode: ConsiliumMode::Normal,
        prompt: "review src/",
        scope: "src/",
        goal: "",
        effort: "",
        workdir: "/repo",
        group_id: "grp-5",
        no_queue: true,
    };
    let mut stderr = Vec::new();
    let err = run_consilium_review_with(&Context::background(), &cfg, &ex, &req, &mut stderr)
        .unwrap_err()
        .to_string();
    assert!(err.contains("all consilium judges failed"), "{err}");
    assert!(err.contains("claude: exited with code 1"), "{err}");
    assert!(err.contains("sol: exited with code 1"), "{err}");
    assert!(err.contains("neon: exited with code 1"), "{err}");
}
