//! Go: `internal/review/security_test.go`, plus the parse half of cmd
//! `TestSecurityParsesFinalAnswerNotToolOutput`.

use super::*;
use crate::config::{K3_LABEL, KIMI_MODEL, PromptKind};
use crate::review::testutil::temp_config;
use crate::review::{ReviewerFinding, build_reviewer_prompt, parse_reviewer_log};

fn out(summary: &str, findings: Vec<ReviewerFinding>) -> ReviewerOutput {
    ReviewerOutput {
        summary: summary.into(),
        findings,
    }
}

fn finding(file: &str, title: &str, severity: &str) -> ReviewerFinding {
    ReviewerFinding {
        file: file.into(),
        title: title.into(),
        severity: severity.into(),
        ..ReviewerFinding::default()
    }
}

/// Go: TestValidateSecurityResultRejectsUnusablePayloads.
#[test]
fn validate_security_result_rejects_unusable_payloads() {
    let cases = [
        ("nil", None, "no structured output"),
        ("empty summary", Some(out("  ", vec![])), "summary is empty"),
        (
            "finding with no file",
            Some(out("found things", vec![finding("", "sqli", "high")])),
            "finding 1 has no file",
        ),
        (
            "finding with no title",
            Some(out("found things", vec![finding("a.go", "", "high")])),
            "finding 1 has no title",
        ),
        (
            "unknown severity",
            Some(out("found things", vec![finding("a.go", "sqli", "spicy")])),
            r#"finding 1 has an unknown severity "spicy""#,
        ),
    ];
    for (name, payload, want) in cases {
        let err = validate_security_result(payload.as_ref(), "").expect_err(name);
        assert_eq!(err.to_string(), want, "{name}");
    }
}

/// Go: TestValidateSecurityResultAcceptsACleanReview.
#[test]
fn validate_security_result_accepts_a_clean_review() {
    validate_security_result(Some(&out("No vulnerabilities found.", vec![])), "").unwrap();
}

/// Go: TestFormatSecurityResultFallsBackOnUnusableOutput.
#[test]
fn format_security_result_falls_back_on_unusable_output() {
    let (got, result) = format_security_result(
        None,
        "provider exploded",
        "opencode",
        KIMI_MODEL,
        "src/",
        "/tmp/s.log",
    );
    assert!(
        result.is_err(),
        "unusable output returned no validation error"
    );
    assert!(
        got.ends_with("provider exploded\n\nLog: /tmp/s.log\n"),
        "fallback lacks the trailing newline or Log line:\n{got:?}"
    );
    assert!(got.contains("UNUSABLE OUTPUT"), "{got}");
    assert!(!got.contains("No vulnerabilities found"), "{got}");
}

/// Go: TestFormatSecurityConsoleOrdersBySeverity.
#[test]
fn format_security_console_orders_by_severity() {
    let mut med = finding("b.go", "open redirect", "medium");
    (med.line, med.confidence) = (2, 7);
    let mut crit = finding("a.go", "sql injection", "critical");
    (crit.line, crit.confidence) = (1, 9);
    let got = format_security_console(&out("two issues", vec![med, crit]), K3_LABEL, "src/");
    let crit_at = got.find("sql injection").unwrap();
    let med_at = got.find("open redirect").unwrap();
    assert!(crit_at < med_at, "critical rendered after medium:\n{got}");
    assert!(got.contains("1 crit, 0 high, 1 med, 0 low"), "{got}");
}

/// Go: TestFormatSecurityConsoleCleanReview.
#[test]
fn format_security_console_clean_review() {
    let got = format_security_console(&out("nothing found", vec![]), K3_LABEL, "src/");
    assert_eq!(
        got,
        "\n═══ RIVAL SECURITY REVIEW ═══\n\nModel: kimi-k3\nScope: src/\n\nSummary: nothing found\n\nNo vulnerabilities found.\n"
    );
}

/// Go: review_promptEcho — output where the model reflected the prompt
/// back instead of reviewing.
fn prompt_echo() -> String {
    let (_home, cfg) = temp_config();
    format!(
        "{}\n{{\"summary\": \"No issues found.\", \"findings\": []}}",
        build_reviewer_prompt(&cfg, "src/", PromptKind::Security)
    )
}

/// Go: TestEchoedPromptIsNotACleanReview.
#[test]
fn echoed_prompt_is_not_a_clean_review() {
    let echoed = prompt_echo();
    let parsed = out("No issues found.", vec![]);
    assert!(validate_security_result(Some(&parsed), &echoed).is_err());
    let (text, _) = format_security_result(
        Some(&parsed),
        &echoed,
        "opencode",
        "moonshotai/kimi-k3",
        "src/",
        "",
    );
    assert!(text.contains("UNUSABLE OUTPUT"), "{text}");
}

/// Go: TestGenuineCleanReviewIsStillAccepted.
#[test]
fn genuine_clean_review_is_still_accepted() {
    let parsed = out("No issues found.", vec![]);
    let raw = r#"{"summary": "No issues found.", "findings": []}"#;
    validate_security_result(Some(&parsed), raw).unwrap();
    let (text, result) = format_security_result(
        Some(&parsed),
        raw,
        "opencode",
        "moonshotai/kimi-k3",
        "src/",
        "",
    );
    result.unwrap();
    assert!(text.contains("No vulnerabilities found."), "{text}");
}

/// Go: cmd TestSecurityParsesFinalAnswerNotToolOutput, parse half. A
/// security payload printed by a tool must not pass for the review; prose
/// in the final answer is unusable output.
#[test]
fn security_parses_final_answer_not_tool_output() {
    let raw = concat!(
        "user\nreview src/\n",
        "exec\ncat old-security.json\n",
        r#"{"summary": "One injection.", "findings": [{"file": "a.go", "line": 3, "severity": "high", "category": "security", "title": "sql injection", "body": "concatenated query", "suggestion": "bind params", "confidence": 9}]}"#,
        "\n",
        "codex\nI looked around; nothing jumped out.\n"
    );
    let parsed = parse_reviewer_log(raw).ok();
    assert!(
        parsed.is_none(),
        "tool output parsed as the review: {parsed:?}"
    );
    let (text, result) = format_security_result(
        parsed.as_ref(),
        raw,
        "opencode",
        KIMI_MODEL,
        "src/",
        "/tmp/x.log",
    );
    assert!(
        result.is_err(),
        "tool output accepted as the security review:\n{text}"
    );
}
