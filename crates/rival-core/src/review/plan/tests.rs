//! Plan output parsing, plan console formatting, and the parse half of
//! assembling plan results.

use super::*;
use crate::config::CODEX_MODEL;

/// Mimics a Codex plan-review log: the CLI echoes the prompt (with the
/// schema example, including its "rating" and placeholder finding) and then
/// streams the real answer last.
const CODEX_STYLE_PLAN_LOG: &str = r#"OpenAI Codex
user
Plan document to review: /tmp/plan.md

Output schema:
```json
{
  "summary": "1-3 sentence overall assessment of the plan",
  "rating": 7,
  "findings": [
    {
      "file": "section or heading the issue is in (or the filename)",
      "line": 0,
      "severity": "critical|high|medium|low",
      "category": "bug|gap|ambiguity|scope|verification",
      "title": "one-line description of the issue",
      "confidence": 8
    }
  ]
}
```

codex
{"summary":"Solid plan with two real gaps.","rating":6,"findings":[{"file":"Migration","line":0,"severity":"high","category":"gap","title":"no rollback step","body":"plan adds a column but never describes how to revert","suggestion":"add a down migration","confidence":9}]}
tokens used 4321
"#;

#[test]
fn ignores_echoed_schema_example() {
    let out = parse_plan_output(CODEX_STYLE_PLAN_LOG).unwrap();
    assert_eq!(out.summary, "Solid plan with two real gaps.");
    assert_eq!(out.rating, 6);
    assert_eq!(out.findings.len(), 1);
    assert_eq!(out.findings[0].title, "no rollback step");
}

#[test]
fn rejects_only_schema_example() {
    let schema_only = concat!(
        "```json\n",
        r#"{"summary":"1-3 sentence overall assessment of the plan","rating":7,"findings":[{"file":"section or heading the issue is in (or the filename)","line":0,"severity":"critical|high|medium|low","category":"bug|gap|ambiguity|scope|verification","title":"one-line description of the issue","confidence":8}]}"#,
        "\n```"
    );
    let err = parse_plan_output(schema_only).unwrap_err();
    assert_eq!(err.to_string(), "no plan JSON payload found in output");
}

#[test]
fn rejects_unrelated_json() {
    // Has neither findings nor rating.
    assert!(parse_plan_output(r#"{"event":"done","ok":true}"#).is_err());
    // Has findings but no rating — a reviewer payload, not a plan payload.
    assert!(parse_plan_output(r#"{"summary":"x","findings":[]}"#).is_err());
}

#[test]
fn accepts_clean_plan() {
    let out = parse_plan_output(r#"prose {"summary":"Airtight.","rating":9,"findings":[]} prose"#)
        .unwrap();
    assert_eq!(
        (out.rating, out.summary.as_str(), out.findings.len()),
        (9, "Airtight.", 0)
    );
}

#[test]
fn drops_only_placeholder_findings() {
    let raw = concat!(
        r#"{"summary":"real","rating":5,"findings":["#,
        r#"{"file":"path/to/file","line":0,"severity":"critical|high|medium|low","category":"bug|gap|ambiguity|scope|verification","title":"x","confidence":8},"#,
        r#"{"file":"Section 2","line":0,"severity":"medium","category":"gap","title":"real gap","body":"b","confidence":7}]}"#
    );
    let out = parse_plan_output(raw).unwrap();
    assert_eq!(out.findings.len(), 1);
    assert_eq!(out.findings[0].title, "real gap");
}

#[test]
fn keeps_real_dual_category_finding() {
    let raw = concat!(
        r#"{"summary":"real","rating":6,"findings":["#,
        r#"{"file":"a.go","line":3,"severity":"high","category":"bug|security","#,
        r#""title":"real dual-category finding","body":"b","confidence":8}]}"#
    );
    let out = parse_plan_output(raw).unwrap();
    assert_eq!(out.findings.len(), 1);
    assert_eq!(out.findings[0].title, "real dual-category finding");
}

#[test]
fn drops_partially_echoed_slop_example() {
    let raw = concat!(
        r#"{"summary":"lean enough","rating":9,"findings":["#,
        r#"{"file":"cmd/main.go","line":10,"severity":"high","#,
        r#""category":"reuse|simplify|efficiency|altitude|compat|reinvention|slop|yagni","#,
        r#""title":"echoed example","body":"copied from the schema","confidence":8},"#,
        r#"{"file":"cmd/main.go","line":20,"severity":"medium","category":"slop","#,
        r#""title":"real finding","body":"a real cut","confidence":7}]}"#
    );
    let out = parse_plan_output(raw).unwrap();
    assert_eq!(out.findings.len(), 1);
    assert_eq!(out.findings[0].title, "real finding");
}

/// A rating outside 1-10 skips the candidate; a decode error is
/// reported with serde_json's text.
#[test]
fn rating_bounds_and_decode_errors() {
    for rating in ["0", "11", "-3"] {
        let raw = format!(r#"{{"summary":"s","rating":{rating},"findings":[]}}"#);
        let err = parse_plan_output(&raw).unwrap_err();
        assert_eq!(
            err.to_string(),
            "no plan JSON payload found in output",
            "{rating}"
        );
    }
    let err = parse_plan_output(r#"{"summary":"s","rating":"7","findings":[]}"#).unwrap_err();
    assert_eq!(
        err.to_string(),
        "no valid plan JSON payload (last decode error: invalid type: string \"7\", expected i64 at line 1 column 27)"
    );
    let err =
        parse_plan_output(r#"{"summary":"s","rating":7,"findings":[{"line":true}]}"#).unwrap_err();
    assert_eq!(
        err.to_string(),
        "no valid plan JSON payload (last decode error: invalid type: boolean `true`, expected i64 at line 1 column 50)"
    );
    // A null rating stays 0 and is skipped; keys are exact.
    assert!(parse_plan_output(r#"{"summary":"s","rating":null,"findings":[]}"#).is_err());
    assert!(parse_plan_output(r#"{"summary":"s","Rating":7,"findings":[]}"#).is_err());
    let out = parse_plan_output(r#"{"summary":"s","rating":1,"RATING":7,"findings":[]}"#).unwrap();
    assert_eq!(out.rating, 1);
}

/// A codex log where an `exec` tool printed a valid plan assessment and the
/// model's final answer, after the last "codex" header, is `final_answer`.
fn codex_tool_plan_transcript(final_answer: &str) -> String {
    format!(
        "user\nreview the plan\nexec\ncat old-review.json\n{}\ncodex\n{final_answer}\n",
        r#"{"summary":"tool output, not the answer","rating":10,"findings":[]}"#
    )
}

/// A plan assessment printed by a tool is never the model's review.
#[test]
fn parses_final_answer_not_tool_output() {
    let raw = codex_tool_plan_transcript("The plan looks broadly fine, a few nits.");
    assert!(
        parse_plan_log(&raw).is_err(),
        "tool output parsed as the review"
    );
    // Without final_answer the tool's 10/10 would win.
    assert_eq!(parse_plan_output(&raw).unwrap().rating, 10);
    let raw = codex_tool_plan_transcript(r#"{"summary":"real","rating":4,"findings":[]}"#);
    assert_eq!(parse_plan_log(&raw).unwrap().rating, 4);
}

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

#[test]
fn format_plan_console_numbers_and_orders_by_severity() {
    let out = PlanOutput {
        summary: "two issues".into(),
        rating: 4,
        findings: vec![
            finding("B", 0, "low", "minor", 5),
            ReviewerFinding {
                body: "bad".into(),
                suggestion: "fix it".into(),
                category: "bug".into(),
                ..finding("A", 12, "critical", "broken", 9)
            },
            finding("C", 0, "medium", "vague", 6),
        ],
    };
    let s = format_plan_console(&out, "/tmp/plan.md");
    assert!(s.contains("Rating: 4/10"), "{s}");
    let crit = s.find("1. [crit] broken — A:12").expect(&s);
    let med = s.find("2. [med] vague").expect(&s);
    let low = s.find("3. [low] minor").expect(&s);
    assert!(crit < med && med < low, "{s}");
    assert!(s.contains("Fix: fix it"), "{s}");
    assert!(
        s.contains("Findings: 3 total — 1 crit, 0 high, 1 med, 1 low"),
        "{s}"
    );
}

#[test]
fn format_plan_console_clean_plan() {
    let out = PlanOutput {
        summary: "Airtight.".into(),
        rating: 10,
        findings: vec![],
    };
    assert_eq!(
        format_plan_console(&out, "/tmp/plan.md"),
        "\n═══ RIVAL PLAN REVIEW ═══\n\nFile: /tmp/plan.md\nRating: 10/10\n\nSummary: Airtight.\n\nNo bugs or gaps found.\n"
    );
}

/// A volunteered failure_scenario never prints in a doc review.
#[test]
fn format_plan_body_drops_failure_scenario() {
    let out = PlanOutput {
        summary: String::new(),
        rating: 3,
        findings: vec![ReviewerFinding {
            failure_scenario: "x → y".into(),
            ..finding("S", 0, "high", "t", 8)
        }],
    };
    assert_eq!(
        format_plan_console(&out, "f"),
        "\n═══ RIVAL PLAN REVIEW ═══\n\nFile: f\nRating: 3/10\n\n1. [high] t — S\n   (confidence 8)\n\nFindings: 1 total — 0 crit, 1 high, 0 med, 0 low\n"
    );
}

fn result(cli: &str, model: &str, parsed: Option<PlanOutput>, raw: &str) -> PlanCLIResult {
    PlanCLIResult {
        cli: cli.into(),
        model: model.into(),
        parsed,
        raw: raw.into(),
    }
}

fn plan(summary: &str, rating: i64) -> PlanOutput {
    PlanOutput {
        summary: summary.into(),
        rating,
        findings: vec![],
    }
}

/// The empty, single-unparsed and multi layouts byte for byte.
#[test]
fn format_plan_result_layouts() {
    assert_eq!(format_plan_result(None, "f"), "No plan review output.\n");
    assert_eq!(
        format_plan_result(Some(&PlanRunResult::default()), "f"),
        "No plan review output.\n"
    );

    let single = PlanRunResult {
        results: vec![result("codex", CODEX_MODEL, None, "raw text")],
        skipped: vec![],
    };
    assert_eq!(
        format_plan_result(Some(&single), "f"),
        config::public_runtime_log("codex", CODEX_MODEL, "raw text")
    );

    let multi = PlanRunResult {
        results: vec![
            result("codex", CODEX_MODEL, Some(plan("ok", 7)), ""),
            result("codex", CODEX_MODEL, None, "  prose  \n"),
        ],
        skipped: vec![SkippedCLI {
            cli: "grok".into(),
            model: "grok-4.6".into(),
            reason: "unavailable".into(),
        }],
    };
    assert_eq!(
        format_plan_result(Some(&multi), "/p.md"),
        "\n═══ RIVAL PLAN REVIEW (codex + codex) ═══\n\nFile: /p.md\n\n── codex ──\n\nRating: 7/10\n\nSummary: ok\n\nNo bugs or gaps found.\n\n\n── codex ──\n\n(could not parse structured output — raw output below)\n\nprose\n\nSkipped: grok — unavailable\n"
    );
    // One result plus a skipped model uses the multi layout.
    let one_skipped = PlanRunResult {
        results: vec![result("codex", CODEX_MODEL, Some(plan("ok", 7)), "")],
        skipped: multi.skipped.clone(),
    };
    assert!(
        format_plan_result(Some(&one_skipped), "f")
            .starts_with("\n═══ RIVAL PLAN REVIEW (codex) ═══\n")
    );
}
