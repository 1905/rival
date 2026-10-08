//! Go: `internal/review/parse_test.go` (reviewer cases) and
//! `TestParseReviewerOutputFailureScenario` from `prompt_test.go`, plus
//! Rust-only pins of Go's decoder rules.

use super::*;
use crate::review::format_review_result;

/// Mimics a Codex reviewer log: the CLI echoes the full input prompt (which
/// contains the JSON schema example from the output contract) before
/// streaming its real answer last.
const CODEX_STYLE_REVIEWER_LOG: &str = r#"OpenAI Codex
user
Review scope: rival/

## Output Format
Return JSON only.

```json
{
  "summary": "1-3 sentence reviewer summary",
  "findings": [
    {
      "file": "path/to/file",
      "line": 42,
      "severity": "critical|high|medium|low",
      "title": "brief title",
      "confidence": 8
    }
  ]
}
```

exec /bin/zsh -lc "nl -ba main.go"
codex
{"summary":"Found a real issue.","findings":[{"file":"rival/main.go","line":10,"severity":"high","category":"bug","title":"real bug","body":"real explanation","confidence":9}]}
tokens used 1234
"#;

/// Go: TestParseReviewerOutput_IgnoresEchoedSchemaExample.
#[test]
fn ignores_echoed_schema_example() {
    let out = parse_reviewer_output(CODEX_STYLE_REVIEWER_LOG).unwrap();
    assert_eq!(out.summary, "Found a real issue.");
    assert_eq!(out.findings.len(), 1);
    assert_eq!(out.findings[0].file, "rival/main.go");
}

/// Go: TestParseReviewerOutput_BareObjectWithProse.
#[test]
fn bare_object_with_prose() {
    let out =
        parse_reviewer_output(r#"prefix noise {"summary":"ok","findings":[]} trailing"#).unwrap();
    assert_eq!(out.summary, "ok");
    assert!(out.findings.is_empty());
}

/// Go: TestParseReviewerOutput_NoPayload.
#[test]
fn no_payload() {
    let err = parse_reviewer_output("no json here at all").unwrap_err();
    assert_eq!(err.to_string(), "no reviewer JSON payload found in output");
}

/// Go: TestParseReviewerOutput_RejectsUnrelatedJSON. A tool/telemetry event
/// must not be accepted as an empty successful parse.
#[test]
fn rejects_unrelated_json() {
    assert!(parse_reviewer_output(r#"{"event":"done","ok":true}"#).is_err());
}

/// Go: TestParseReviewerOutput_RejectsOnlySchemaExample.
#[test]
fn rejects_only_schema_example() {
    let schema_only = concat!(
        "```json\n",
        r#"{"summary":"1-3 sentence reviewer summary","findings":[{"file":"path/to/file","line":42,"severity":"critical|high|medium|low","title":"brief title","confidence":8}]}"#,
        "\n```"
    );
    assert!(parse_reviewer_output(schema_only).is_err());
}

/// Go: TestParseReviewerOutput_AcceptsCleanReview.
#[test]
fn accepts_clean_review() {
    let out =
        parse_reviewer_output(r#"prose {"summary": "No issues found.", "findings": []} prose"#)
            .unwrap();
    assert_eq!(out.summary, "No issues found.");
    assert!(out.findings.is_empty());
}

/// Go: TestParseReviewerOutput_DropsOnlyPlaceholderFindings.
#[test]
fn drops_only_placeholder_findings() {
    let raw = concat!(
        r#"{"summary":"real","findings":["#,
        r#"{"file":"path/to/file","line":42,"severity":"critical|high|medium|low","title":"brief title","confidence":8},"#,
        r#"{"file":"real.go","line":7,"severity":"high","category":"bug","title":"real","body":"b","confidence":9}]}"#
    );
    let out = parse_reviewer_output(raw).unwrap();
    assert_eq!(out.findings.len(), 1);
    assert_eq!(out.findings[0].file, "real.go");
}

/// Go: TestParseReviewerOutput_NestedInsideInvalidRegion.
#[test]
fn nested_inside_invalid_region() {
    let raw = r#"wrapper { not valid json but balanced: {"summary":"nested real","findings":[{"file":"a.go","line":1,"severity":"high","confidence":8}]} }"#;
    assert_eq!(parse_reviewer_output(raw).unwrap().summary, "nested real");
}

/// Go: TestParseReviewerOutput_AcceptsRealFindingDiscussingEnum.
#[test]
fn accepts_real_finding_discussing_enum() {
    let raw = r#"{"summary":"real review","findings":[{"file":"rival/internal/review/parse.go","line":84,"severity":"high","category":"bug","title":"placeholder check","body":"isPlaceholderFinding compares against critical|high|medium|low which is fine","confidence":9}]}"#;
    let out = parse_reviewer_output(raw).unwrap();
    assert_eq!(out.summary, "real review");
    assert_eq!(out.findings.len(), 1);
    assert_eq!(out.findings[0].severity, "high");
}

/// Go: TestExtractJSON_UnbalancedBraceBeforeAnswer.
#[test]
fn unbalanced_brace_before_answer() {
    let raw = r#"{"summary":"schema","findings":[{"file":"path/to/file","line":42}]}
exec: showing code
func New() {           // <- unbalanced brace, never closes as JSON
    x := "a } in a string"
codex
{"summary":"real","findings":[{"file":"real.go","line":1,"confidence":9}]}"#;
    let out = parse_reviewer_output(raw).unwrap();
    assert_eq!(out.summary, "real");
    assert_eq!(out.findings.len(), 1);
    assert_eq!(out.findings[0].file, "real.go");
}

/// Go: TestParseReviewerOutput_RealCapturedLog. A real Codex log: the CLI
/// echoed the prompt and cat'd source files (braces, JSON fixtures), then
/// emitted the real answer last.
#[test]
fn real_captured_log() {
    let data = include_str!("../testdata/consilium_echoed_schema.log");
    let out = parse_reviewer_output(data).unwrap();
    assert!(!out.findings.is_empty(), "got 0 findings");
    for f in &out.findings {
        assert_ne!(
            f.file, "path/to/file",
            "parsed the schema placeholder: {f:?}"
        );
    }
    assert!(!is_example_summary(&out.summary), "{:?}", out.summary);
}

/// Go: TestToolOutputJSONIsNotTheReview. Review-shaped JSON printed by a
/// tool must not become the review when the final codex answer is prose.
#[test]
fn tool_output_json_is_not_the_review() {
    let raw = concat!(
        "user\nprompt…\nexec cat saved-review.json\n",
        r#"{"summary": "Saved: nothing wrong.", "findings": []}"#,
        "\ncodex\nI could not finish the review.\ntokens used\n1234\n"
    );
    assert!(
        parse_reviewer_output(final_answer(raw)).is_err(),
        "tool-output JSON accepted as the review"
    );
    assert!(parse_reviewer_log(raw).is_err());
    // Without final_answer the tool's JSON would win (the Go gap this guards).
    assert_eq!(
        parse_reviewer_output(raw).unwrap().summary,
        "Saved: nothing wrong."
    );
    let got = format_review_result(None, raw, "codex", "gpt-6-astra", "src/", "/tmp/x.log");
    assert!(got.contains("UNPARSED OUTPUT"), "want UNPARSED:\n{got}");
    // Without the codex header the whole log is the answer.
    assert_eq!(final_answer("plain log"), "plain log");
    // The last header wins.
    assert_eq!(final_answer("codex\nfirst\ncodex\nsecond").trim(), "second");
}

/// Rust-only: the header must be a whole line. A CR before the newline, a
/// prefix or a suffix is no header; the text after the match starts with
/// its newline.
#[test]
fn final_answer_header_is_an_exact_line() {
    assert_eq!(final_answer("a\ncodex\nb"), "\nb");
    assert_eq!(final_answer("codex"), "");
    assert_eq!(final_answer("x\r\ncodex\r\nanswer"), "x\r\ncodex\r\nanswer");
    assert_eq!(final_answer("a\n codex\nb"), "a\n codex\nb");
    assert_eq!(final_answer("a\ncodex:\nb"), "a\ncodex:\nb");
    assert_eq!(final_answer("a\nCodex\nb"), "a\nCodex\nb");
    assert_eq!(final_answer("codex\r\nb"), "codex\r\nb");
}

/// Known Go gap 1: a log with no codex header (claude, opencode) is
/// scanned whole, so tool-printed JSON there can still win.
#[test]
fn known_gap_no_header_scans_whole_log() {
    let raw = concat!(
        "tool: cat saved.json\n",
        r#"{"summary": "Saved: nothing wrong.", "findings": []}"#,
        "\nI could not finish the review.\n"
    );
    assert_eq!(final_answer(raw), raw);
    assert_eq!(
        parse_reviewer_log(raw).unwrap().summary,
        "Saved: nothing wrong."
    );
}

/// Known Go gap 2: a final answer that prints the same payload twice is not
/// deduplicated; the last copy wins and nothing flags the repeat.
#[test]
fn known_gap_duplicate_answer_not_deduped() {
    let answer = r#"{"summary":"One bug.","findings":[{"file":"a.go","line":1,"severity":"high","title":"t","confidence":9}]}"#;
    let raw = format!("codex\n{answer}\n{answer}\n");
    let out = parse_reviewer_log(&raw).unwrap();
    assert_eq!(out.findings.len(), 1);
    assert_eq!(json_objects(final_answer(&raw)).len(), 4);
}

/// Go: TestParseReviewerOutputFailureScenario (prompt_test.go).
#[test]
fn failure_scenario_round_trip() {
    let raw = r#"{"summary":"one bug","findings":[{"file":"a.go","line":3,"severity":"high","category":"bug","title":"t","body":"b","failure_scenario":"empty list → index panic","suggestion":"s","confidence":8}]}"#;
    let out = parse_reviewer_output(raw).unwrap();
    assert_eq!(out.findings[0].failure_scenario, "empty list → index panic");
    let enc = serde_json::to_string(&out).unwrap();
    let back = parse_reviewer_output(&enc).unwrap();
    assert_eq!(back.findings[0], out.findings[0]);

    let old = r#"{"summary":"one bug","findings":[{"file":"a.go","line":3,"severity":"high","category":"bug","title":"t","body":"b","suggestion":"s","confidence":8}]}"#;
    let out = parse_reviewer_output(old).unwrap();
    assert_eq!(out.findings.len(), 1);
    assert_eq!(out.findings[0].failure_scenario, "");
    let enc = serde_json::to_string(&out.findings[0]).unwrap();
    assert!(
        !enc.contains("failure_scenario"),
        "an empty failure_scenario is not omitted"
    );
}

// ---- Rust-only: payload decoding rules ----

/// `has_json_key` and the field decoding both match keys exactly.
#[test]
fn keys_match_exactly() {
    assert!(parse_reviewer_output(r#"{"Summary":"x","findings":[]}"#).is_err());
    let out = parse_reviewer_output(
        r#"{"summary":"a","SUMMARY":"b","findings":[{"file":"f.go","Line":2}],"FINDINGS":[]}"#,
    )
    .unwrap();
    assert_eq!(out.summary, "a");
    assert_eq!(out.findings.len(), 1);
    assert_eq!(
        (out.findings[0].file.as_str(), out.findings[0].line),
        ("f.go", 0)
    );
    // An escaped key is the same key.
    assert!(parse_reviewer_output(r#"{"summ\u0061ry":"x","findings":[]}"#).is_ok());
}

/// `null` reads as the default; unknown fields are ignored, even a number
/// no integer type can hold.
#[test]
fn nulls_and_unknown_fields() {
    let out = parse_reviewer_output(
        r#"{"summary":"s","findings":null,"extra":1e999999,"nested":{"a":[1,2]}}"#,
    )
    .unwrap();
    assert_eq!(out.summary, "s");
    assert!(out.findings.is_empty());
    let out = parse_reviewer_output(
        r#"{"summary":null,"findings":[null,{"file":null,"line":null,"title":"t"}]}"#,
    )
    .unwrap();
    assert_eq!(out.summary, "");
    assert_eq!(out.findings.len(), 2);
    assert_eq!(out.findings[0], ReviewerFinding::default());
    assert_eq!(out.findings[1].title, "t");
}

/// A type mismatch or a duplicate key rejects the candidate with
/// serde_json's text; an earlier valid payload is then used.
#[test]
fn type_mismatch_and_duplicate_key_texts() {
    let cases = [
        (
            r#"{"summary":1,"findings":[]}"#,
            "invalid type: integer `1`, expected a string at line 1 column 12",
        ),
        (
            r#"{"summary":"s","findings":"x"}"#,
            "invalid type: string \"x\", expected a sequence at line 1 column 29",
        ),
        (
            r#"{"summary":"s","findings":{}}"#,
            "invalid type: map, expected a sequence at line 1 column 26",
        ),
        (
            r#"{"summary":"s","findings":[7]}"#,
            "invalid type: integer `7`, expected a JSON object at line 1 column 28",
        ),
        (
            r#"{"summary":"s","findings":[[]]}"#,
            "invalid type: sequence, expected a JSON object at line 1 column 27",
        ),
        (
            r#"{"summary":"s","findings":[{"line":"3"}]}"#,
            "invalid type: string \"3\", expected i64 at line 1 column 38",
        ),
        (
            r#"{"summary":"s","findings":[{"confidence":1.5}]}"#,
            "invalid type: floating point `1.5`, expected i64 at line 1 column 44",
        ),
        (
            r#"{"summary":"s","findings":[{"line":1e999999}]}"#,
            "number out of range at line 1 column 43",
        ),
        (
            r#"{"summary":"s","findings":[{"title":true}]}"#,
            "invalid type: boolean `true`, expected a string at line 1 column 40",
        ),
        // The first error in document order wins.
        (
            r#"{"findings":[{"body":[]}],"summary":{}}"#,
            "invalid type: sequence, expected a string at line 1 column 21",
        ),
        (
            r#"{"summary":"s","findings":[{"file":"a"}],"findings":[{"title":"t2"}]}"#,
            "duplicate field `findings` at line 1 column 51",
        ),
    ];
    for (raw, want) in cases {
        let err = parse_reviewer_output(raw).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("no valid reviewer JSON payload (last decode error: {want})"),
            "{raw}"
        );
    }
    // The scan goes on to an earlier valid payload.
    let raw = r#"{"summary":"good","findings":[]} {"summary":2,"findings":[]}"#;
    assert_eq!(parse_reviewer_output(raw).unwrap().summary, "good");
}

/// `json_objects` reports every valid object in closing order, nested ones
/// included, and only valid ones.
#[test]
fn json_objects_closing_order() {
    let s = r#"x {"a":{"b":1}} {bad} {"s":"}{"} {"#;
    assert_eq!(
        json_objects(s),
        [r#"{"b":1}"#, r#"{"a":{"b":1}}"#, r#"{"s":"}{"}"#]
    );
    // A quote outside any object cannot desync the scan.
    assert_eq!(json_objects(r#"say "hi {"k":1}"#), [r#"{"k":1}"#]);
    // A huge number is valid JSON.
    assert_eq!(json_objects(r#"{"n":1e999999}"#).len(), 1);
}

/// A codex run that exits without an answer: the log is the banner and
/// tool output, and a tool printed a saved review. That review must not
/// pass as this run's result.
#[test]
fn codex_transcript_without_answer_header_has_no_answer() {
    let saved = r#"{"summary":"Old review.","findings":[{"file":"a.rs","line":1,"severity":"high","category":"bug","title":"t","body":"b","confidence":9}]}"#;
    for raw in [
        format!("OpenAI Codex v0.50\n--------\nexec\ncat review.json\n{saved}\n"),
        format!("exec\ncat review.json\n{saved}\n"),
    ] {
        assert_eq!(final_answer(&raw), "", "{raw}");
        assert!(parse_reviewer_log(&raw).is_err(), "{raw}");
    }
    // With the header, the answer after it still parses.
    let answered = format!("OpenAI Codex v0.50\nexec\ncat x\n{saved}\ncodex\n{saved}\n");
    assert_eq!(
        parse_reviewer_log(&answered).unwrap().summary,
        "Old review."
    );
    // A non-codex log is still returned whole.
    assert_eq!(final_answer(saved), saved);
}
