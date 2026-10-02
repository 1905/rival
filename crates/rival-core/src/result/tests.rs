//! Port of the app's `ResultParserTests.swift` (41 cases), plus grouping
//! checks for the Rust `groups` field.

use std::path::Path;

use super::*;

// Helpers

/// The findings of a `Findings` result in group order, or a panic.
fn findings(r: &RunResult) -> (&str, Option<u8>, Vec<&Finding>) {
    match r {
        RunResult::Findings {
            summary,
            rating,
            groups,
        } => (
            summary,
            *rating,
            groups.iter().flat_map(|g| &g.findings).collect(),
        ),
        other => panic!("want Findings, got {other:?}"),
    }
}

fn failed(r: &RunResult) -> &str {
    match r {
        RunResult::Failed { reason } => reason,
        other => panic!("want Failed, got {other:?}"),
    }
}

fn markdown(text: &str) -> RunResult {
    RunResult::Markdown {
        text: text.to_string(),
    }
}

fn empty_findings(summary: &str, rating: Option<u8>) -> RunResult {
    RunResult::Findings {
        summary: summary.to_string(),
        rating,
        groups: Vec::new(),
    }
}

fn finding(file: &str, severity: &str, confidence: i64) -> Finding {
    Finding {
        file: file.to_string(),
        severity: severity.to_string(),
        confidence,
        ..Finding::default()
    }
}

fn files<'a>(fs: &[&'a Finding]) -> Vec<&'a str> {
    fs.iter().map(|f| f.file.as_str()).collect()
}

fn fake_log(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/logs")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// A codex transcript with an echoed prompt that carries the schema example.
const CODEX_HEADER: &str = r#"OpenAI Codex v0.155.1
--------
workdir: /Users/dev/src/acme-api
model: gpt-6-astra
--------
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

exec
/bin/zsh -lc "cat internal/billing/ledger.go" in /Users/dev/src/acme-api
func Post(e Entry) error {
    if e.Amount == 0 {"#;

// Codex transcripts

#[test]
fn codex_plan_double_answer_json() {
    let answer = r#"{"summary":"Sound plan, one gap.","rating":6,"findings":[{"file":"Rollout","line":12,"severity":"high","category":"gap","title":"no rollback","body":"b","confidence":9}]}"#;
    let raw = format!(
        "{CODEX_HEADER}\ncodex\n{answer}\nhook: Stop\nhook: Stop Completed\ntokens used\n117.735\n{answer}\n"
    );
    let r = parse_run_result(&raw);
    let (summary, rating, fs) = findings(&r);
    assert_eq!(summary, "Sound plan, one gap.");
    assert_eq!(rating, Some(6));
    let titles: Vec<&str> = fs.iter().map(|f| f.title.as_str()).collect();
    assert_eq!(
        titles,
        ["no rollback"],
        "decoded once, schema example skipped"
    );
}

#[test]
fn codex_markdown_double_answer_has_no_hook_lines() {
    let md = "## Review\n\n- `ledger.go:14` drops zero entries.\n- Tests pass.";
    let streamed = "## Review\nhook: PreToolUse\nhook: PreToolUse Completed\n\n- `ledger.go:14` drops zero entries.\nhook: Stop\n- Tests pass.";
    let raw = format!("{CODEX_HEADER}\ncodex\n{streamed}\ntokens used\n78,402\n{md}\n");
    assert_eq!(final_answer(&raw).trim(), md);
    assert_eq!(parse_run_result(&raw), markdown(md));
}

#[test]
fn codex_footer_inside_second_copy() {
    let md = "Two issues.\n\n- one\n- two\n- three";
    let raw = format!(
        "{CODEX_HEADER}\ncodex\n{md}\nhook: Stop\nhook: Stop Completed\nTwo issues.\n\n- one\ntokens used\n69.540\n- two\n- three\n"
    );
    assert_eq!(parse_run_result(&raw), markdown(md));
}

#[test]
fn codex_footer_last_uses_streamed_answer_without_hooks() {
    let raw = format!(
        "{CODEX_HEADER}\ncodex\nAll good.\nhook: Stop\nhook: Stop Completed\ntokens used\n99.836\n"
    );
    assert_eq!(parse_run_result(&raw), markdown("All good."));
}

#[test]
fn codex_without_footer_uses_text_after_last_header() {
    assert_eq!(final_answer("codex\nfirst\ncodex\nsecond").trim(), "second");
    assert_eq!(
        final_answer("plain log"),
        "plain log",
        "no header: the whole text"
    );
}

#[test]
fn tool_output_json_before_answer_is_ignored() {
    let raw = concat!(
        "user\nprompt…\nexec cat saved-review.json\n",
        r#"{"summary": "Saved: nothing wrong.", "findings": []}"#,
        "\ncodex\nI could not finish the review.\ntokens used\n1234\n",
    );
    assert_eq!(
        parse_run_result(raw),
        markdown("I could not finish the review.")
    );
}

#[test]
fn prompt_echo_only_is_no_answer() {
    assert_eq!(
        failed(&parse_run_result(CODEX_HEADER)),
        "no answer in the log"
    );
    // A tail that cut the banner still has exec lines.
    assert_eq!(
        failed(&parse_run_result("exec\nls -la\ntotal 0\n")),
        "no answer in the log"
    );
}

#[test]
fn empty_log_is_no_answer() {
    assert_eq!(failed(&parse_run_result("")), "no answer in the log");
    assert_eq!(
        failed(&parse_run_result("\n\u{1B}[0m  \n")),
        "no answer in the log"
    );
    let raw = format!("{CODEX_HEADER}\ncodex\n\ntokens used\n12\n");
    assert_eq!(failed(&parse_run_result(&raw)), "no answer in the log");
}

// Plain logs

#[test]
fn plain_json_log() {
    let raw = r#"{"summary":"Lean.","rating":8,"findings":[{"file":"cmd/run.go","line":3,"severity":"low","category":"slop","title":"t","body":"b","confidence":6}]}"#;
    let r = parse_run_result(raw);
    let (_, rating, fs) = findings(&r);
    assert_eq!(rating, Some(8));
    assert_eq!(fs.first().map(|f| f.file.as_str()), Some("cmd/run.go"));
}

#[test]
fn fenced_json_log() {
    let raw = "```json\n{\"summary\":\"Fenced.\",\"findings\":[]}\n```\n";
    assert_eq!(parse_run_result(raw), empty_findings("Fenced.", None));
}

#[test]
fn review_payload_without_rating() {
    let raw = r#"{"summary":"One bug.","findings":[{"file":"a.go","line":1,"severity":"high","failure_scenario":"x=0","suggestion":"guard","confidence":8}]}"#;
    let r = parse_run_result(raw);
    let (_, rating, fs) = findings(&r);
    assert_eq!(rating, None);
    let want = Finding {
        file: "a.go".into(),
        line: 1,
        severity: "high".into(),
        failure_scenario: Some("x=0".into()),
        suggestion: Some("guard".into()),
        confidence: 8,
        ..Finding::default()
    };
    assert_eq!(fs, [&want]);
}

#[test]
fn markdown_answer() {
    let md = "# Verdict\n\nShip it.\n\n1. one\n2. two";
    assert_eq!(parse_run_result(&format!("{md}\n")), markdown(md));
}

#[test]
fn markdown_is_sanitized() {
    assert_eq!(
        parse_run_result("\u{1B}[1mbold\u{1B}[0m\tx"),
        markdown("bold    x")
    );
}

#[test]
fn broken_json_answer_fails() {
    let r = parse_run_result(r#"{"summary":"cut off","findings":[{"file":"a.go""#);
    let msg = failed(&r);
    let prefix = "JSON answer did not decode: ";
    assert!(msg.starts_with(prefix), "{msg}");
    assert!(msg.len() > prefix.len(), "{msg}");
}

#[test]
fn wrong_type_reports_decode_error() {
    let r = parse_run_result(r#"{"summary":"s","findings":[{"file":"a.go","line":"42"}]}"#);
    let msg = failed(&r);
    assert!(msg.starts_with("JSON answer did not decode: "), "{msg}");
    assert!(msg.contains("line"), "{msg}");
}

#[test]
fn json_without_payload_keys_fails() {
    assert_eq!(
        failed(&parse_run_result(r#"{"event":"done","ok":true}"#)),
        "JSON answer did not decode: no summary/findings keys"
    );
}

#[test]
fn rating_out_of_range_is_rejected() {
    for rating in [0, 11] {
        let raw = format!(r#"{{"summary":"s","rating":{rating},"findings":[]}}"#);
        let r = parse_run_result(&raw);
        assert!(
            matches!(r, RunResult::Failed { .. }),
            "rating {rating}: {r:?}"
        );
    }
}

#[test]
fn plan_payload_wins_over_later_review() {
    let raw = r#"{"summary":"plan","rating":5,"findings":[]} {"summary":"review","findings":[]}"#;
    assert_eq!(parse_run_result(raw), empty_findings("plan", Some(5)));
}

#[test]
fn placeholders_dropped_and_severity_order() {
    let raw = concat!(
        r#"{"summary":"s","findings":["#,
        r#"{"file":"f1","severity":"low","confidence":9},"#,
        r#"{"file":"path/to/file","severity":"high","confidence":9},"#,
        r#"{"file":"f2","severity":"weird","confidence":9},"#,
        r#"{"file":"f3","severity":"High","confidence":5},"#,
        r#"{"file":"f4","severity":"critical","confidence":1},"#,
        r#"{"file":"f5","severity":"medium","confidence":7},"#,
        r#"{"file":"f6","severity":"high","confidence":8},"#,
        r#"{"file":"f7","severity":"high","category":"bug|gap|ambiguity|scope|verification","confidence":9}"#,
        "]}",
    );
    let r = parse_run_result(raw);
    let (_, _, fs) = findings(&r);
    assert_eq!(files(&fs), ["f4", "f6", "f3", "f5", "f1", "f2"]);
    // Rust: the flat order is the group order; unknown severities rank last.
    let RunResult::Findings { groups, .. } = &r else {
        unreachable!()
    };
    let names: Vec<&str> = groups.iter().map(|g| g.severity.as_str()).collect();
    assert_eq!(names, ["critical", "high", "medium", "low", "other"]);
}

#[test]
fn severity_rank_and_stable_sort() {
    let ranks: Vec<usize> = ["critical", "HIGH", "Medium", "low", "info", ""]
        .iter()
        .map(|s| severity_rank(s))
        .collect();
    assert_eq!(ranks, [0, 1, 2, 3, 4, 4]);
    let fs = vec![
        finding("a", "low", 5),
        finding("b", "low", 5),
        finding("c", "low", 6),
    ];
    let sorted: Vec<String> = sorted_findings(fs).into_iter().map(|f| f.file).collect();
    assert_eq!(sorted, ["c", "a", "b"]);
}

#[test]
fn missing_and_null_fields_default() {
    let raw = r#"{"summary":null,"findings":[{"file":"a.go","line":null,"suggestion":""}]}"#;
    let r = parse_run_result(raw);
    let (summary, _, fs) = findings(&r);
    assert_eq!(summary, "");
    let want = Finding {
        file: "a.go".into(),
        ..Finding::default()
    };
    assert_eq!(fs, [&want]);
}

// Go parity (rival/internal/review/parse_test.go, plan_test.go)

#[test]
fn go_reviewer_ignores_echoed_schema_example() {
    let raw = r#"OpenAI Codex
user
Review scope: rival/

```json
{
  "summary": "1-3 sentence reviewer summary",
  "findings": [
    {"file": "path/to/file", "line": 42, "severity": "critical|high|medium|low", "title": "brief title", "confidence": 8}
  ]
}
```

exec /bin/zsh -lc "nl -ba main.go"
codex
{"summary":"Found a real issue.","findings":[{"file":"rival/main.go","line":10,"severity":"high","category":"bug","title":"real bug","body":"real explanation","confidence":9}]}
tokens used 1234"#;
    let r = parse_run_result(raw);
    let (summary, _, fs) = findings(&r);
    assert_eq!(summary, "Found a real issue.");
    assert_eq!(files(&fs), ["rival/main.go"]);
}

#[test]
fn go_bare_object_with_prose() {
    assert_eq!(
        parse_run_result(r#"prefix noise {"summary":"ok","findings":[]} trailing"#),
        empty_findings("ok", None)
    );
}

#[test]
fn go_no_payload() {
    assert_eq!(
        parse_run_result("no json here at all"),
        markdown("no json here at all")
    );
}

#[test]
fn go_rejects_only_schema_example() {
    let reviewer = concat!(
        "```json\n",
        r#"{"summary":"1-3 sentence reviewer summary","findings":[{"file":"path/to/file","line":42,"severity":"critical|high|medium|low","title":"brief title","confidence":8}]}"#,
        "\n```",
    );
    failed(&parse_run_result(reviewer));
    let plan = concat!(
        "```json\n",
        r#"{"summary":"1-3 sentence overall assessment of the plan","rating":7,"findings":[{"file":"section or heading the issue is in (or the filename)","line":0,"severity":"critical|high|medium|low","category":"bug|gap|ambiguity|scope|verification","title":"one-line description of the issue","confidence":8}]}"#,
        "\n```",
    );
    failed(&parse_run_result(plan));
}

#[test]
fn go_accepts_clean_review_and_plan() {
    assert_eq!(
        parse_run_result(r#"prose {"summary": "No issues found.", "findings": []} prose"#),
        empty_findings("No issues found.", None)
    );
    assert_eq!(
        parse_run_result(r#"prose {"summary":"Airtight.","rating":9,"findings":[]} prose"#),
        empty_findings("Airtight.", Some(9))
    );
}

#[test]
fn go_drops_only_placeholder_findings() {
    let raw = concat!(
        r#"{"summary":"real","findings":["#,
        r#"{"file":"path/to/file","line":42,"severity":"critical|high|medium|low","title":"brief title","confidence":8},"#,
        r#"{"file":"real.go","line":7,"severity":"high","category":"bug","title":"real","body":"b","confidence":9}]}"#,
    );
    let r = parse_run_result(raw);
    assert_eq!(files(&findings(&r).2), ["real.go"]);
}

#[test]
fn go_nested_inside_invalid_region() {
    let raw = r#"wrapper { not valid json but balanced: {"summary":"nested real","findings":[{"file":"a.go","line":1,"severity":"high","confidence":8}]} }"#;
    assert_eq!(findings(&parse_run_result(raw)).0, "nested real");
}

#[test]
fn go_accepts_real_finding_discussing_enum() {
    let raw = r#"{"summary":"real review","findings":[{"file":"rival/internal/review/parse.go","line":84,"severity":"high","category":"bug","title":"placeholder check","body":"isPlaceholderFinding compares against critical|high|medium|low which is fine","confidence":9}]}"#;
    let r = parse_run_result(raw);
    let severities: Vec<&str> = findings(&r).2.iter().map(|f| f.severity.as_str()).collect();
    assert_eq!(severities, ["high"]);
}

#[test]
fn go_unbalanced_brace_before_answer() {
    let raw = r#"{"summary":"schema","findings":[{"file":"path/to/file","line":42}]}
exec: showing code
func New() {           // <- unbalanced brace, never closes as JSON
    x := "a } in a string"
codex
{"summary":"real","findings":[{"file":"real.go","line":1,"confidence":9}]}"#;
    let r = parse_run_result(raw);
    let (summary, _, fs) = findings(&r);
    assert_eq!(summary, "real");
    assert_eq!(files(&fs), ["real.go"]);
}

#[test]
fn go_keeps_dual_category_and_drops_antislop_echo() {
    let dual = r#"{"summary":"real","rating":6,"findings":[{"file":"a.go","line":3,"severity":"high","category":"bug|security","title":"real dual-category finding","body":"b","confidence":8}]}"#;
    let r = parse_run_result(dual);
    let titles: Vec<&str> = findings(&r).2.iter().map(|f| f.title.as_str()).collect();
    assert_eq!(titles, ["real dual-category finding"]);
    let echo = concat!(
        r#"{"summary":"lean enough","rating":9,"findings":["#,
        r#"{"file":"cmd/command_antislop.go","line":10,"severity":"high","category":"reuse|simplify|efficiency|altitude|compat|reinvention|slop|yagni","title":"echoed example","body":"copied","confidence":8},"#,
        r#"{"file":"cmd/command_antislop.go","line":20,"severity":"medium","category":"slop","title":"real finding","body":"a real cut","confidence":7}]}"#,
    );
    let r = parse_run_result(echo);
    let titles: Vec<&str> = findings(&r).2.iter().map(|f| f.title.as_str()).collect();
    assert_eq!(titles, ["real finding"]);
}

#[test]
fn go_plan_rejects_unrelated_json() {
    failed(&parse_run_result(r#"{"event":"done","ok":true}"#));
}

#[test]
fn go_real_captured_log() {
    let raw = fake_log("consilium_echoed_schema.log");
    let r = parse_run_result(&raw);
    let (summary, _, fs) = findings(&r);
    assert!(!fs.is_empty());
    assert!(!fs.iter().any(|f| f.file == "path/to/file"));
    assert_ne!(summary, "1-3 sentence reviewer summary");
}

// Fake fixture logs (dev_bundle.py --fixture)

#[test]
fn fake_plan_codex_log() {
    let raw = fake_log("plan-codex.log");
    assert!(
        raw.contains("path/to/file"),
        "the echoed schema placeholder is in the log"
    );
    let r = parse_run_result(&raw);
    let (_, rating, fs) = findings(&r);
    assert_eq!(rating, Some(6));
    assert_eq!(fs.len(), 6);
    assert!(!fs.iter().any(|f| is_placeholder_finding(f)));
    let severities: Vec<&str> = fs.iter().map(|f| f.severity.as_str()).collect();
    assert_eq!(
        severities,
        ["critical", "high", "high", "medium", "medium", "low"]
    );
}

#[test]
fn fake_review_markdown_log() {
    match parse_run_result(&fake_log("review-markdown.log")) {
        RunResult::Markdown { text } => assert!(text.starts_with("## Review"), "{text}"),
        other => panic!("want Markdown, got {other:?}"),
    }
}

#[test]
fn fake_broken_json_log() {
    let r = parse_run_result(&fake_log("broken-json.log"));
    let msg = failed(&r);
    assert!(msg.starts_with("JSON answer did not decode: "), "{msg}");
}

// json_objects

#[test]
fn json_objects_order_and_nesting() {
    assert_eq!(
        json_objects(r#"a {"x":{"y":1}} b { "z": "}{" } { broken"#),
        [r#"{"y":1}"#, r#"{"x":{"y":1}}"#, r#"{ "z": "}{" }"#]
    );
    assert!(json_objects("no braces").is_empty());
    assert_eq!(json_objects(r#"héllo {"k":"ü"} ✓"#), [r#"{"k":"ü"}"#]);
}

// Grouping. testMarkdownBlocks and testMarkdownWrappedListItemContinues test
// the Swift MarkdownBlocks module; Task 4.7's renderer ports them.

#[test]
fn severity_groups_buckets() {
    let fs = vec![
        finding("a", "high", 0),
        finding("b", "weird", 0),
        finding("c", "HIGH", 0),
        finding("d", "low", 0),
    ];
    let groups = severity_groups(fs);
    let names: Vec<&str> = groups.iter().map(|g| g.severity.as_str()).collect();
    assert_eq!(names, ["high", "low", "other"]);
    let first: Vec<&str> = groups[0].findings.iter().map(|f| f.file.as_str()).collect();
    assert_eq!(first, ["a", "c"]);
    assert_eq!(groups[0].findings[1].severity, "HIGH", "original case kept");
    assert!(severity_groups(Vec::new()).is_empty());
}

#[test]
fn unanswered_codex_transcript_ignores_prompt_clean_example() {
    let raw = r#"OpenAI Codex v0.153.4
--------
workdir: /Users/dev/src/acme-api
--------
user
Review the code. If the code is solid, return: {"summary": "No issues found.", "findings": []}
exec
/bin/zsh -lc 'git diff' in /Users/dev/src/acme-api"#;
    assert_eq!(
        parse_run_result(raw),
        RunResult::Failed {
            reason: "no answer in the log".into()
        }
    );
}

// Rust-only: decode details the Swift tests leave implicit.

#[test]
fn rating_is_converted_only_inside_range() {
    let r = parse_run_result(r#"{"summary":"s","rating":10,"findings":[]}"#);
    assert_eq!(r, empty_findings("s", Some(10)));
    // An out-of-range rating is not demoted to a review, even with a later
    // reviewer-shaped object absent.
    let r = parse_run_result(r#"{"summary":"s","rating":300,"findings":[]}"#);
    assert_eq!(
        failed(&r),
        "JSON answer did not decode: no summary/findings keys"
    );
    // A null rating is present but decodes as 0: rejected, not a review.
    let r = parse_run_result(r#"{"summary":"s","rating":null,"findings":[]}"#);
    assert!(matches!(r, RunResult::Failed { .. }), "{r:?}");
}

#[test]
fn decode_errors_name_the_path() {
    let cases = [
        (
            r#"{"summary":"s","findings":[{"file":"a.go","line":"42"}]}"#,
            "findings.0.line: Expected to decode Int but found a string instead.",
        ),
        (
            r#"{"summary":1,"findings":[]}"#,
            "summary: Expected to decode String but found number instead.",
        ),
        (
            r#"{"summary":"s","findings":"none"}"#,
            "findings: Expected to decode Array<Any> but found a string instead.",
        ),
        (
            r#"{"summary":"s","findings":[1]}"#,
            "findings.0: Expected to decode Dictionary<String, Any> but found number instead.",
        ),
        (
            r#"{"summary":"s","findings":[{"line":1.5}]}"#,
            "findings.0.line: Number 1.5 is not representable as Int.",
        ),
        (
            r#"{"summary":"s","findings":[{"confidence":true}]}"#,
            "findings.0.confidence: Expected to decode Int but found bool instead.",
        ),
        (
            r#"{"summary":"s","findings":[null]}"#,
            "findings.0: Expected to decode Dictionary<String, Any> but found null instead.",
        ),
    ];
    for (raw, want) in cases {
        let r = parse_run_result(raw);
        assert_eq!(
            failed(&r),
            format!("JSON answer did not decode: {want}"),
            "{raw}"
        );
    }
}

/// Foundation `JSONDecoder` Int results, measured by the controller
/// (`.superpowers/sdd/plan-v2.2/swift-integers.json` and a second boundary
/// probe).
#[test]
fn integer_tokens_follow_foundation() {
    let accepted: [(&str, i64); 13] = [
        ("1", 1),
        ("1.0", 1),
        ("1e0", 1),
        ("10e-1", 1),
        ("9223372036854775807", i64::MAX),
        ("-9223372036854775808", i64::MIN),
        ("-9223372036854775808.0", i64::MIN),
        // Out of i64, but the f64 fallback rounds to -2^63.
        ("-9223372036854775809", i64::MIN),
        ("-9223372036854775809.0", i64::MIN),
        ("9007199254740993.0", 9_007_199_254_740_992),
        ("9007199254740993e0", 9_007_199_254_740_992),
        ("1e-400", 0),
        ("null", 0),
    ];
    for (tok, want) in accepted {
        let raw = format!(r#"{{"summary":"s","findings":[{{"file":"a","line":{tok}}}]}}"#);
        let r = parse_run_result(&raw);
        assert_eq!(findings(&r).2[0].line, want, "line {tok}");
    }
    // Rejected with the field path: no truncation, no saturation.
    for tok in [
        "1.5",
        "true",
        r#""1""#,
        "9223372036854775808",
        "9223372036854775807.0",
        "9223372036854775808.0",
        "18446744073709551616",
    ] {
        let raw = format!(r#"{{"summary":"s","findings":[{{"line":{tok}}}]}}"#);
        let r = parse_run_result(&raw);
        let msg = failed(&r);
        assert!(
            msg.starts_with("JSON answer did not decode: findings.0.line: "),
            "line {tok}: {msg}"
        );
    }
    // 1e400 is not valid JSON for Foundation or serde_json: the object is no
    // candidate, and the whole-answer parse error is the reason.
    let r = parse_run_result(r#"{"summary":"s","findings":[{"line":1e400}]}"#);
    assert!(
        failed(&r).starts_with("JSON answer did not decode: "),
        "{r:?}"
    );
    // The rating takes the same path and converts to u8 only inside 1..=10.
    let r = parse_run_result(r#"{"summary":"s","rating":6.0,"findings":[]}"#);
    assert_eq!(r, empty_findings("s", Some(6)));
    let r = parse_run_result(r#"{"summary":"s","rating":262,"findings":[]}"#);
    assert!(matches!(r, RunResult::Failed { .. }), "{r:?}");
}

#[test]
fn final_answer_footer_and_hook_edges() {
    // Only the first footer is dropped; later "tokens used" text is kept.
    assert_eq!(
        final_answer("codex\na\ntokens used 5\nb\ntokens used 6"),
        "a\nb\ntokens used 6"
    );
    // A two-line footer needs a count on the next line.
    assert_eq!(
        final_answer("codex\ntokens used\nmany"),
        "tokens used\nmany"
    );
    // Hook lines: one ASCII-letter name, optionally " Completed".
    assert_eq!(
        final_answer("codex\nhook: Stop Failed\nhook: 9x\nhook: Stop\nok"),
        "hook: Stop Failed\nhook: 9x\nok"
    );
    // Odd line counts or unequal halves are not deduped.
    assert_eq!(final_answer("codex\na\nb\na"), "a\nb\na");
    assert_eq!(final_answer("codex\na\na"), "a");
}
