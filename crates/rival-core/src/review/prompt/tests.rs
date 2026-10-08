//! Go: `internal/review/prompt_test.go` (prompt cases) and the prompt cases
//! of `cmd/command_security_test.go`, plus byte pins of every prompt text.

use std::fs;

use super::*;
use crate::config::PLAN_REVIEW_PROMPT;
use crate::review::testutil::{config_in, temp_config};

const KINDS: [PromptKind; 2] = [PromptKind::BugHunter, PromptKind::Security];

/// Go: TestReviewerPromptsCarryRubricAndFailureScenario. Both code-review
/// lenses carry the one rubric, the field and the drop rule.
#[test]
fn reviewer_prompts_carry_rubric_and_failure_scenario() {
    let (_home, cfg) = temp_config();
    for kind in KINDS {
        let prompt = build_reviewer_prompt(&cfg, "src/", kind);
        for want in [
            SEVERITY_RUBRIC,
            r#""failure_scenario": "input/state that triggers it → the wrong result""#,
            "Each finding needs a concrete failure_scenario: the input or state that triggers it and the wrong result. If you cannot state one, drop the finding.",
            CLEAN_REVIEW_EXAMPLE_LINE,
            r#""summary": "1-3 sentence reviewer summary""#,
        ] {
            assert!(
                prompt.contains(want),
                "kind {kind:?} prompt is missing {want:?}"
            );
        }
    }
    let sec = build_reviewer_prompt(&cfg, "src/", PromptKind::Security);
    assert!(
        sec.contains(
            "Put the attack (what the attacker controls,\nwhat they reach, what they get) in failure_scenario."
        ),
        "security prompt does not put the attack in failure_scenario"
    );
}

/// Go: TestReviewerContractOrdersFailureScenario. The field sits between
/// body and suggestion in the contract.
#[test]
fn reviewer_contract_orders_failure_scenario() {
    let c = REVIEWER_JSON_CONTRACT;
    let body = c.find(r#""body""#).unwrap();
    let scen = c.find(r#""failure_scenario""#).unwrap();
    let fix = c.find(r#""suggestion""#).unwrap();
    assert!(
        body < scen && scen < fix,
        "failure_scenario not between body and suggestion: body={body} scenario={scen} suggestion={fix}"
    );
}

fn write_user_config(home: &std::path::Path, yaml: &str) {
    let dir = home.join(".rival");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("config.yaml"), yaml).unwrap();
}

/// Go: TestBugHunterOverrideKeepsContractWithoutRubric. A user override
/// replaces the lens text but still gets the contract, and the rubric is
/// not injected into it.
#[test]
fn bug_hunter_override_keeps_contract_without_rubric() {
    let home = tempfile::tempdir().unwrap();
    write_user_config(home.path(), "roles:\n  bug_hunter: \"CUSTOM LENS\\n\"\n");
    let cfg = config_in(home.path(), &[]);
    assert!(
        cfg.user_config_error().is_none(),
        "load config: {:?}",
        cfg.user_config_error()
    );

    let got = build_reviewer_prompt(&cfg, "X", PromptKind::BugHunter);
    let want = format!("Review scope: X\n\nCUSTOM LENS\n{REVIEWER_JSON_CONTRACT}");
    assert_eq!(got, want);
    assert!(
        got.contains(r#""failure_scenario""#),
        "override prompt lost failure_scenario"
    );
    assert!(
        !got.contains("Severity:\n- critical"),
        "the rubric was injected into the override"
    );
}

/// Rust-only: each lens reads only its own role key, and a blank override
/// falls through to the built-in text (Go `strings.TrimSpace(override) != ""`).
#[test]
fn overrides_are_per_lens_and_blank_falls_through() {
    let home = tempfile::tempdir().unwrap();
    write_user_config(
        home.path(),
        "roles:\n  security: \"SEC LENS\"\n  bug_hunter: \" \\n\\t\"\n",
    );
    let cfg = config_in(home.path(), &[]);
    assert!(
        cfg.user_config_error().is_none(),
        "{:?}",
        cfg.user_config_error()
    );

    assert_eq!(
        build_reviewer_prompt(&cfg, "s", PromptKind::Security),
        format!("Review scope: s\n\nSEC LENS{REVIEWER_JSON_CONTRACT}")
    );
    assert_eq!(
        build_reviewer_prompt(&cfg, "s", PromptKind::BugHunter),
        format!("Review scope: s\n\n{BUG_HUNTER_INSTRUCTIONS}{REVIEWER_JSON_CONTRACT}")
    );
}

/// Go: TestNoPromptUsesAPersona.
#[test]
fn no_prompt_uses_a_persona() {
    let (_home, cfg) = temp_config();
    let prompts = [
        ("plan", PLAN_REVIEW_PROMPT.to_string()),
        (
            "bug",
            build_reviewer_prompt(&cfg, "x", PromptKind::BugHunter),
        ),
        (
            "security",
            build_reviewer_prompt(&cfg, "x", PromptKind::Security),
        ),
    ];
    for (name, p) in prompts {
        let lower = p.to_lowercase();
        for banned in ["ruthless", "senior staff"] {
            assert!(
                !lower.contains(banned),
                "{name} prompt contains persona text {banned:?}"
            );
        }
    }
}

/// Go: TestPlanPromptVerifiesCodeClaims.
#[test]
fn plan_prompt_verifies_code_claims() {
    assert!(
        PLAN_REVIEW_PROMPT.contains("open the repo and check them"),
        "PlanReviewPrompt lacks the repo-verification instruction"
    );
    assert!(
        !PLAN_REVIEW_PROMPT.contains("(as described)"),
        "PlanReviewPrompt still judges the system only as described"
    );
}

/// Go: cmd TestSecurityPromptCoversEveryVulnerabilityClass.
#[test]
fn security_prompt_covers_every_vulnerability_class() {
    let (_home, cfg) = temp_config();
    let prompt = build_reviewer_prompt(&cfg, "src/", PromptKind::Security);
    for marker in [
        "njection",
        "uthorization",
        "uthentication",
        "rypto",
        "raversal",
        "SSRF",
        "eserializ",
        "ecret",
        "alidation",
        "CSRF",
        "edirect",
        "xhaustion",
    ] {
        assert!(
            prompt.contains(marker),
            "the security prompt does not cover {marker:?}"
        );
    }
}

/// Go: cmd TestBugHunterPromptUnchangedByTheSecurityLens.
#[test]
fn bug_hunter_prompt_unchanged_by_the_security_lens() {
    let (_home, cfg) = temp_config();
    let prompt = build_reviewer_prompt(&cfg, "src/", PromptKind::BugHunter);
    assert!(
        prompt.contains("## Role: Implementation Bug Hunter"),
        "the bug-hunter prompt changed"
    );
    assert!(
        !prompt.contains("## Role: Security Reviewer"),
        "the security prompt leaked into a bug-hunter run"
    );
}

/// Rust-only: the exact assembled prompt for each lens.
#[test]
fn built_prompt_is_scope_lens_then_contract() {
    let (_home, cfg) = temp_config();
    assert_eq!(
        build_reviewer_prompt(&cfg, "a.go\nb.go", PromptKind::BugHunter),
        format!("Review scope: a.go\nb.go\n\n{BUG_HUNTER_INSTRUCTIONS}{REVIEWER_JSON_CONTRACT}")
    );
    assert_eq!(
        build_reviewer_prompt(&cfg, "", PromptKind::Security),
        format!("Review scope: \n\n{SECURITY_INSTRUCTIONS}{REVIEWER_JSON_CONTRACT}")
    );
}

// ---- byte pins ----

fn rust_prompts() -> [(&'static str, &'static str); 6] {
    [
        ("severityRubric", SEVERITY_RUBRIC),
        ("failureScenarioRule", FAILURE_SCENARIO_RULE),
        ("cleanReviewExampleLine", CLEAN_REVIEW_EXAMPLE_LINE),
        ("bugHunterInstructions", BUG_HUNTER_INSTRUCTIONS),
        ("securityInstructions", SECURITY_INSTRUCTIONS),
        ("reviewerJSONContract", REVIEWER_JSON_CONTRACT),
    ]
}

fn sha256_hex(s: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Pins the prompt bytes independently of the Go tree: byte length and
/// SHA-256 of each Go text, evaluated from `prompt.go`. This stays when the
/// Go tree is removed.
#[test]
fn prompts_sha256_golden() {
    let want = [
        (
            "severityRubric",
            327,
            "6f06ee8f4493e6671d8a8843f122b20f19e99f5021e15e7bebe4608081cb344e",
        ),
        (
            "failureScenarioRule",
            148,
            "7e3c4338a1822653bf7e56f3b55094990ff14e1212e49617eb916254c9a0b0cd",
        ),
        (
            "cleanReviewExampleLine",
            77,
            "789d5a7eef287c0acae13fcb0965290df4e816342531b7be1524bb08d0551ea7",
        ),
        (
            "bugHunterInstructions",
            1821,
            "76c96ef88b0641152af87852779981f3f4d91a364431d18fc0c1afe6ca64d830",
        ),
        (
            "securityInstructions",
            3108,
            "8b23546ff7656c1f9ce37f6175c9c2ce8bf3008ab56a0c976de65f2cca21bde6",
        ),
        (
            "reviewerJSONContract",
            1550,
            "1e280fb0f448687e305b3930adddfd54a9fdec25bd2a134330a3ace95182923f",
        ),
    ];
    let got: Vec<_> = rust_prompts()
        .into_iter()
        .map(|(name, text)| (name, text.len(), sha256_hex(text)))
        .collect();
    let want: Vec<_> = want
        .into_iter()
        .map(|(name, len, hash)| (name, len, hash.to_string()))
        .collect();
    assert_eq!(got, want);
}

/// Pins the two assembled default prompts for one scope, as the commands
/// send them.
#[test]
fn built_prompts_sha256_golden() {
    let (_home, cfg) = temp_config();
    let got: Vec<_> = KINDS
        .into_iter()
        .map(|kind| {
            let p = build_reviewer_prompt(&cfg, "src/", kind);
            (p.len(), sha256_hex(&p))
        })
        .collect();
    let want = [
        (
            3391,
            "088e42a71aa260abf4123d756d2ac5e9a03232e4d28c84a3724a9db56cca9f5d",
        ),
        (
            4678,
            "6e1b1f2c2b692b61cd3b10a501670c631b03e03db448eeda4d11c79915c95531",
        ),
    ]
    .map(|(len, hash)| (len, hash.to_string()));
    assert_eq!(got, want);
}
