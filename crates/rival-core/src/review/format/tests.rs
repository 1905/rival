//! Review formatting, prompt-echo detection and severity display.

use super::*;
use crate::config::{CODEX_MODEL, PromptKind};
use crate::review::testutil::temp_config;
use crate::review::{build_reviewer_prompt, format_security_console, parse_reviewer_output};

fn finding(file: &str, line: i64, severity: &str, title: &str, confidence: i64) -> ReviewerFinding {
    ReviewerFinding {
        file: file.into(),
        line,
        severity: severity.into(),
        title: title.into(),
        confidence,
        ..ReviewerFinding::default()
    }
}

fn review_fixture() -> ReviewerOutput {
    ReviewerOutput {
        summary: "One real bug in the stop path.".into(),
        findings: vec![
            ReviewerFinding {
                category: "ux".into(),
                ..finding(
                    "rival/internal/dashboard/list.go",
                    12,
                    "low",
                    "stale label",
                    4,
                )
            },
            ReviewerFinding {
                body: "Confirmation rechecks only the cached status; PIDStart is never checked."
                    .into(),
                suggestion: "compare the procinfo start time to PIDStart before signalling.".into(),
                category: "bug".into(),
                ..finding(
                    "rival/internal/dashboard/model.go",
                    597,
                    "high",
                    "Stop can signal a reused PID",
                    9,
                )
            },
        ],
    }
}

#[test]
fn format_review_console_layout() {
    let got = format_review_console(
        &review_fixture(),
        "codex (gpt-6-astra)",
        "rival/internal/dashboard/",
        "/tmp/s.log",
    );
    let want = "
═══ RIVAL REVIEW ═══

Model: codex (gpt-6-astra)
Scope: rival/internal/dashboard/

Summary: One real bug in the stop path.

1. [high] Stop can signal a reused PID — rival/internal/dashboard/model.go:597
   Confirmation rechecks only the cached status; PIDStart is never checked.
   Fix: compare the procinfo start time to PIDStart before signalling.
   (bug, confidence 9)

Low confidence (1):
- [low] stale label — rival/internal/dashboard/list.go:12 (confidence 4)

Findings: 1 total — 0 crit, 1 high, 0 med, 0 low
Log: /tmp/s.log
";
    assert_eq!(got, want);
}

#[test]
fn format_review_console_orders_by_severity_then_confidence() {
    let out = ReviewerOutput {
        summary: "s".into(),
        findings: vec![
            finding("m.go", 0, "medium", "med", 9),
            finding("h1.go", 0, "high", "high-7", 7),
            finding("c.go", 0, "critical", "crit", 6),
            finding("h2.go", 0, "high", "high-9", 9),
        ],
    };
    let got = format_review_console(&out, "m", "s", "");
    let mut last = None;
    for o in [
        "1. [crit] crit",
        "2. [high] high-9",
        "3. [high] high-7",
        "4. [med] med",
    ] {
        let i = got.find(o);
        assert!(i.is_some() && i > last, "{o:?} out of order:\n{got}");
        last = i;
    }
    assert!(
        got.contains("Findings: 4 total — 1 crit, 2 high, 1 med, 0 low\n"),
        "tally wrong:\n{got}"
    );
    assert!(
        !got.contains("Log:"),
        "empty log path must print no Log line:\n{got}"
    );
}

#[test]
fn format_review_console_empty() {
    let out = ReviewerOutput {
        summary: "No issues found.".into(),
        findings: vec![],
    };
    let got = format_review_console(&out, "m", "s", "/tmp/x.log");
    assert_eq!(
        got,
        "\n═══ RIVAL REVIEW ═══\n\nModel: m\nScope: s\n\nSummary: No issues found.\n\nNo issues found.\nLog: /tmp/x.log\n"
    );
}

/// Low-confidence findings are shown, not dropped, and the main list says why
/// it is empty.
#[test]
fn format_review_console_only_low_confidence() {
    let out = ReviewerOutput {
        summary: "s".into(),
        findings: vec![finding("a.go", 3, "medium", "maybe", 5)],
    };
    let got = format_review_console(&out, "m", "s", "");
    for want in [
        "No findings at confidence 6 or higher.",
        "Low confidence (1):\n- [med] maybe — a.go:3 (confidence 5)",
        "Findings: 0 total",
    ] {
        assert!(got.contains(want), "missing {want:?}:\n{got}");
    }
}

#[test]
fn format_review_console_joins_multi_line_scope() {
    let out = ReviewerOutput {
        summary: "s".into(),
        findings: vec![],
    };
    let got = format_review_console(&out, "m", "a.go\nb.go\n", "");
    assert!(
        got.contains("Scope: a.go, b.go\n"),
        "scope not joined:\n{got}"
    );
    assert_eq!(one_line("  a \n\n\t b\r\n"), "a, b");
}

#[test]
fn format_review_result_model_line() {
    let got = format_review_result(
        Some(&review_fixture()),
        r#"{"summary":"x","findings":[]}"#,
        "codex",
        CODEX_MODEL,
        "src/",
        "/tmp/l.log",
    );
    assert!(got.contains("═══ RIVAL REVIEW ═══"), "{got}");
    assert!(got.contains("Model: codex (gpt-6-astra)\n"), "{got}");
}

#[test]
fn format_review_result_falls_back_to_raw_log() {
    let (_home, cfg) = temp_config();
    let echoed = format!(
        "user\n{}\ntokens used: 10\n",
        build_reviewer_prompt(&cfg, "src/", PromptKind::BugHunter)
    );
    let blank = ReviewerOutput {
        summary: "  ".into(),
        findings: vec![],
    };
    let clean = ReviewerOutput {
        summary: "No issues found.".into(),
        findings: vec![],
    };
    let cases = [
        (
            "nil",
            None,
            "the model wrote prose instead",
            "no structured output",
        ),
        (
            "empty summary",
            Some(&blank),
            "the model wrote prose instead",
            "summary is empty",
        ),
        (
            "echoed example",
            Some(&clean),
            echoed.as_str(),
            "output repeats the prompt's own example rather than a review",
        ),
    ];
    for (name, parsed, raw, problem) in cases {
        let got = format_review_result(parsed, raw, "codex", CODEX_MODEL, "src/", "/tmp/l.log");
        assert!(
            got.contains("═══ RIVAL REVIEW — UNPARSED OUTPUT ═══"),
            "{name}: {got}"
        );
        assert!(
            got.contains(&format!("Problem: {problem}\n")),
            "{name}: {got}"
        );
        assert!(got.contains("Log: /tmp/l.log"), "{name}: {got}");
        assert!(
            !got.contains("No issues found.\n\nLog"),
            "{name}: rendered as clean:\n{got}"
        );
    }
    let got = format_review_result(
        None,
        "the model wrote prose instead",
        "codex",
        CODEX_MODEL,
        "src/",
        "",
    );
    assert!(
        got.contains("the model wrote prose instead"),
        "raw log dropped:\n{got}"
    );
}

/// Rust-only: the exact unusable layout, with and without a trailing
/// newline on the raw log and a log path.
#[test]
fn format_unusable_layout() {
    let got = format_review_result(None, "prose", "codex", CODEX_MODEL, "a\nb", "/tmp/l.log");
    assert_eq!(
        got,
        "\n═══ RIVAL REVIEW — UNPARSED OUTPUT ═══\n\nModel: codex (gpt-6-astra)\nScope: a, b\nProblem: no structured output\n\nThe model ran but its answer is not a structured review.\nRaw output follows.\n\nprose\n\nLog: /tmp/l.log\n"
    );
    let got = format_review_result(None, "", "codex", CODEX_MODEL, "s", "");
    assert!(got.ends_with("Raw output follows.\n\n"), "{got:?}");
}

/// Codex writes the whole prompt into its log; a genuine clean answer after
/// it passes.
#[test]
fn bug_hunter_clean_review_after_echoed_prompt_is_accepted() {
    let (_home, cfg) = temp_config();
    let raw = format!(
        "user\n{}\ncodex\n{{\"summary\": \"No issues found.\", \"findings\": []}}\ntokens used: 10\n",
        build_reviewer_prompt(&cfg, "src/", PromptKind::BugHunter)
    );
    let parsed = parse_reviewer_output(&raw).unwrap();
    validate_review_result(Some(&parsed), &raw).unwrap();
}

#[test]
fn bug_hunter_echo_only_is_rejected() {
    let (_home, cfg) = temp_config();
    let raw = format!(
        "user\n{}\ntokens used: 10\n",
        build_reviewer_prompt(&cfg, "src/", PromptKind::BugHunter)
    );
    let parsed = parse_reviewer_output(&raw).expect("the echoed clean example should parse");
    assert!(validate_review_result(Some(&parsed), &raw).is_err());
}

#[test]
fn clean_review_quoting_prompt_markers_is_not_an_echo() {
    let (_home, cfg) = temp_config();
    let answer = r#"{"summary": "No issues found.", "findings": []}"#;
    let line = super::super::prompt::CLEAN_REVIEW_EXAMPLE_LINE;
    for raw in [
        format!(
            "user\n{}\nexec cat prompt.go\n## Role: Security Reviewer\nWork through every class below\n{line}\ncodex\n{answer}",
            build_reviewer_prompt(&cfg, "x", PromptKind::BugHunter)
        ),
        format!("tool: ## Role: Security Reviewer\n{answer}"),
    ] {
        let out = parse_reviewer_output(&raw).unwrap();
        validate_review_result(Some(&out), &raw).unwrap();
    }
}

#[test]
fn prompt_echo_without_answer_is_rejected() {
    let (_home, cfg) = temp_config();
    for kind in [PromptKind::BugHunter, PromptKind::Security] {
        let raw = build_reviewer_prompt(&cfg, "x", kind);
        let Ok(out) = parse_reviewer_output(&raw) else {
            continue; // the echo did not even parse: already UNPARSED
        };
        assert!(
            validate_review_result(Some(&out), &raw).is_err(),
            "kind {kind:?}: echoed prompt accepted as a review"
        );
    }
}

#[test]
fn formatters_show_scenario_only_when_set() {
    let with = |s: &str| ReviewerOutput {
        summary: "x".into(),
        findings: vec![ReviewerFinding {
            category: "bug".into(),
            body: "the body".into(),
            failure_scenario: s.into(),
            suggestion: "the fix".into(),
            ..finding("a.go", 3, "high", "t", 9)
        }],
    };
    type Formatter = fn(&ReviewerOutput) -> String;
    let formatters: [(&str, Formatter); 2] = [
        ("review", |o| format_review_console(o, "m", "src/", "")),
        ("security", |o| format_security_console(o, "m", "src/")),
    ];
    for (name, format) in formatters {
        let got = format(&with("nil map → panic on write"));
        assert!(
            got.contains("   the body\n   Scenario: nil map → panic on write\n   Fix: the fix\n"),
            "{name}: Scenario line missing or misplaced:\n{got}"
        );
        for empty in ["", "   "] {
            let got = format(&with(empty));
            assert!(
                !got.contains("Scenario:"),
                "{name}: Scenario shown for {empty:?}:\n{got}"
            );
        }
    }
}

#[test]
fn severity_tally_counts_unknown_as_low() {
    let fs = [
        finding("", 0, "critical", "", 0),
        finding("", 0, "HIGH", "", 0),
        finding("", 0, "spicy", "", 0),
    ];
    assert_eq!(
        severity_tally(&fs),
        "Findings: 3 total — 1 crit, 1 high, 0 med, 1 low"
    );
}

#[test]
fn sorted_findings_does_not_mutate_input() {
    let input = [
        finding("", 0, "low", "l", 0),
        finding("", 0, "critical", "c", 0),
    ];
    let out = sorted_findings(&input);
    assert_eq!((input[0].title.as_str(), out[0].title.as_str()), ("l", "c"));
}

#[test]
fn display_severity_labels() {
    for (input, want) in [
        ("critical", "crit"),
        ("CRITICAL", "crit"),
        ("high", "high"),
        ("medium", "med"),
        ("low", "low"),
        ("weird", "weird"),
        ("WEIRD", "weird"),
    ] {
        assert_eq!(display_severity(input), want, "{input}");
    }
}

/// Rust-only: a finding with no file and no line has no location, and one
/// with no category drops it from the confidence line.
#[test]
fn write_finding_without_location_or_category() {
    let mut sb = String::new();
    write_finding(&mut sb, 3, &finding("", 7, "medium", "t", 2));
    assert_eq!(sb, "3. [med] t — :7\n   (confidence 2)\n\n");
    let mut sb = String::new();
    write_finding(&mut sb, 1, &finding("", 0, "x", "t", 2));
    assert_eq!(sb, "1. [x] t\n   (confidence 2)\n\n");
}
