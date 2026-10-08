//! Reviewer output parsing tests, the failure-scenario round trip, and pins
//! of the decoder rules.

use super::*;
use crate::result::{final_answer, find_payload, json_objects, log_payload};
use crate::review::ReviewerFinding;
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

#[test]
fn ignores_echoed_schema_example() {
    let out = parse_reviewer_log(CODEX_STYLE_REVIEWER_LOG).unwrap();
    assert_eq!(out.summary, "Found a real issue.");
    assert_eq!(out.findings.len(), 1);
    assert_eq!(out.findings[0].file, "rival/main.go");
}

#[test]
fn bare_object_with_prose() {
    let out =
        parse_reviewer_log(r#"prefix noise {"summary":"ok","findings":[]} trailing"#).unwrap();
    assert_eq!(out.summary, "ok");
    assert!(out.findings.is_empty());
}

#[test]
fn no_payload() {
    let err = parse_reviewer_log("no json here at all").unwrap_err();
    assert_eq!(err.to_string(), "no reviewer JSON payload found in output");
}

/// A tool/telemetry event
/// must not be accepted as an empty successful parse.
#[test]
fn rejects_unrelated_json() {
    assert!(parse_reviewer_log(r#"{"event":"done","ok":true}"#).is_err());
}

#[test]
fn rejects_only_schema_example() {
    let schema_only = concat!(
        "```json\n",
        r#"{"summary":"1-3 sentence reviewer summary","findings":[{"file":"path/to/file","line":42,"severity":"critical|high|medium|low","title":"brief title","confidence":8}]}"#,
        "\n```"
    );
    assert!(parse_reviewer_log(schema_only).is_err());
}

#[test]
fn accepts_clean_review() {
    let out = parse_reviewer_log(r#"prose {"summary": "No issues found.", "findings": []} prose"#)
        .unwrap();
    assert_eq!(out.summary, "No issues found.");
    assert!(out.findings.is_empty());
}

#[test]
fn drops_only_placeholder_findings() {
    let raw = concat!(
        r#"{"summary":"real","findings":["#,
        r#"{"file":"path/to/file","line":42,"severity":"critical|high|medium|low","title":"brief title","confidence":8},"#,
        r#"{"file":"real.go","line":7,"severity":"high","category":"bug","title":"real","body":"b","confidence":9}]}"#
    );
    let out = parse_reviewer_log(raw).unwrap();
    assert_eq!(out.findings.len(), 1);
    assert_eq!(out.findings[0].file, "real.go");
}

#[test]
fn nested_inside_invalid_region() {
    let raw = r#"wrapper { not valid json but balanced: {"summary":"nested real","findings":[{"file":"a.go","line":1,"severity":"high","confidence":8}]} }"#;
    assert_eq!(parse_reviewer_log(raw).unwrap().summary, "nested real");
}

#[test]
fn accepts_real_finding_discussing_enum() {
    let raw = r#"{"summary":"real review","findings":[{"file":"rival/internal/review/parse.go","line":84,"severity":"high","category":"bug","title":"placeholder check","body":"isPlaceholderFinding compares against critical|high|medium|low which is fine","confidence":9}]}"#;
    let out = parse_reviewer_log(raw).unwrap();
    assert_eq!(out.summary, "real review");
    assert_eq!(out.findings.len(), 1);
    assert_eq!(out.findings[0].severity, "high");
}

#[test]
fn unbalanced_brace_before_answer() {
    let raw = r#"{"summary":"schema","findings":[{"file":"path/to/file","line":42}]}
exec: showing code
func New() {           // <- unbalanced brace, never closes as JSON
    x := "a } in a string"
codex
{"summary":"real","findings":[{"file":"real.go","line":1,"confidence":9}]}"#;
    let out = parse_reviewer_log(raw).unwrap();
    assert_eq!(out.summary, "real");
    assert_eq!(out.findings.len(), 1);
    assert_eq!(out.findings[0].file, "real.go");
}

/// A real Codex log: the CLI
/// echoed the prompt and cat'd source files (braces, JSON fixtures), then
/// emitted the real answer last.
#[test]
fn real_captured_log() {
    let data = include_str!("../testdata/consilium_echoed_schema.log");
    let out = parse_reviewer_log(data).unwrap();
    assert!(!out.findings.is_empty(), "got 0 findings");
    for f in &out.findings {
        assert_ne!(
            f.file, "path/to/file",
            "parsed the schema placeholder: {f:?}"
        );
    }
    assert_ne!(out.summary.trim(), "1-3 sentence reviewer summary");
}

/// Review-shaped JSON printed by a
/// tool must not become the review when the final codex answer is prose.
#[test]
fn tool_output_json_is_not_the_review() {
    let raw = concat!(
        "user\nprompt…\nexec cat saved-review.json\n",
        r#"{"summary": "Saved: nothing wrong.", "findings": []}"#,
        "\ncodex\nI could not finish the review.\ntokens used\n1234\n"
    );
    assert!(
        parse_reviewer_log(raw).is_err(),
        "tool-output JSON accepted as the review"
    );
    // Without final_answer the tool's JSON would win (the gap this guards).
    assert_eq!(
        find_payload(raw, PayloadKind::Any).unwrap().summary,
        "Saved: nothing wrong."
    );
    let got = format_review_result(None, raw, "codex", "gpt-6-astra", "src/", "/tmp/x.log");
    assert!(got.contains("UNPARSED OUTPUT"), "want UNPARSED:\n{got}");
    // Without the codex header the whole log is the answer.
    assert_eq!(final_answer("plain log"), "plain log");
    // The last header wins.
    assert_eq!(final_answer("codex\nfirst\ncodex\nsecond"), "second");
}

/// Rust-only: the header must be a whole line. A CR before the newline, a
/// prefix or a suffix is no header. Blank lines around the answer are
/// dropped.
#[test]
fn final_answer_header_is_an_exact_line() {
    assert_eq!(final_answer("a\ncodex\nb"), "b");
    assert_eq!(final_answer("codex"), "");
    assert_eq!(final_answer("x\r\ncodex\r\nanswer"), "x\r\ncodex\r\nanswer");
    assert_eq!(final_answer("a\n codex\nb"), "a\n codex\nb");
    assert_eq!(final_answer("a\ncodex:\nb"), "a\ncodex:\nb");
    assert_eq!(final_answer("a\nCodex\nb"), "a\nCodex\nb");
    assert_eq!(final_answer("codex\r\nb"), "codex\r\nb");
}

/// Known gap 1: a log with no codex header (claude, opencode) is
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

/// A final answer that prints the same payload twice is one answer: the
/// shared answer finder keeps one copy.
#[test]
fn duplicate_answer_is_deduped() {
    let answer = r#"{"summary":"One bug.","findings":[{"file":"a.go","line":1,"severity":"high","title":"t","confidence":9}]}"#;
    let raw = format!("codex\n{answer}\n{answer}\n");
    let out = parse_reviewer_log(&raw).unwrap();
    assert_eq!(out.findings.len(), 1);
    assert_eq!(final_answer(&raw), answer);
    assert_eq!(json_objects(&final_answer(&raw)).len(), 2);
}

/// `failure_scenario` survives a JSON round trip; an old payload without it
/// still parses.
#[test]
fn failure_scenario_round_trip() {
    let raw = r#"{"summary":"one bug","findings":[{"file":"a.go","line":3,"severity":"high","category":"bug","title":"t","body":"b","failure_scenario":"empty list → index panic","suggestion":"s","confidence":8}]}"#;
    let out = parse_reviewer_log(raw).unwrap();
    assert_eq!(out.findings[0].failure_scenario, "empty list → index panic");
    let enc = serde_json::to_string(&out).unwrap();
    let back = parse_reviewer_log(&enc).unwrap();
    assert_eq!(back.findings[0], out.findings[0]);

    let old = r#"{"summary":"one bug","findings":[{"file":"a.go","line":3,"severity":"high","category":"bug","title":"t","body":"b","suggestion":"s","confidence":8}]}"#;
    let out = parse_reviewer_log(old).unwrap();
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
    assert!(parse_reviewer_log(r#"{"Summary":"x","findings":[]}"#).is_err());
    let out = parse_reviewer_log(
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
    assert!(parse_reviewer_log(r#"{"summ\u0061ry":"x","findings":[]}"#).is_ok());
}

/// `null` reads as the default and unknown fields are ignored. A `null`
/// finding is a decode error (Foundation `JSONDecoder` rules; the old CLI
/// decoder read it as an empty finding). A number out of f64 range anywhere
/// makes the object invalid JSON for the scan (the old CLI decoder ignored
/// it in an unknown field).
#[test]
fn nulls_and_unknown_fields() {
    let out =
        parse_reviewer_log(r#"{"summary":"s","findings":null,"nested":{"a":[1,2]}}"#).unwrap();
    assert_eq!(out.summary, "s");
    assert!(out.findings.is_empty());
    let out = parse_reviewer_log(
        r#"{"summary":null,"findings":[{"file":null,"line":null,"title":"t"}]}"#,
    )
    .unwrap();
    assert_eq!(out.summary, "");
    assert_eq!(out.findings.len(), 1);
    assert_eq!(out.findings[0].title, "t");
    assert_eq!(out.findings[0].file, "");
    let err = parse_reviewer_log(r#"{"summary":"s","findings":[null]}"#).unwrap_err();
    assert_eq!(
        err.to_string(),
        "no valid reviewer JSON payload (last decode error: findings.0: Expected to decode Dictionary<String, Any> but found null instead.)"
    );
    let err =
        parse_reviewer_log(r#"{"summary":"s","findings":null,"extra":1e999999}"#).unwrap_err();
    assert_eq!(err.to_string(), "no reviewer JSON payload found in output");
}

/// A type mismatch rejects the candidate with the shared decoder's text
/// (the field path, then Foundation's wording); an earlier valid payload is
/// then used. A duplicate key is no error: the last value wins.
#[test]
fn type_mismatch_and_duplicate_key_texts() {
    let cases = [
        (
            r#"{"summary":1,"findings":[]}"#,
            "summary: Expected to decode String but found number instead.",
        ),
        (
            r#"{"summary":"s","findings":"x"}"#,
            "findings: Expected to decode Array<Any> but found a string instead.",
        ),
        (
            r#"{"summary":"s","findings":{}}"#,
            "findings: Expected to decode Array<Any> but found a dictionary instead.",
        ),
        (
            r#"{"summary":"s","findings":[7]}"#,
            "findings.0: Expected to decode Dictionary<String, Any> but found number instead.",
        ),
        (
            r#"{"summary":"s","findings":[[]]}"#,
            "findings.0: Expected to decode Dictionary<String, Any> but found an array instead.",
        ),
        (
            r#"{"summary":"s","findings":[{"line":"3"}]}"#,
            "findings.0.line: Expected to decode Int but found a string instead.",
        ),
        (
            r#"{"summary":"s","findings":[{"confidence":1.5}]}"#,
            "findings.0.confidence: Number 1.5 is not representable as Int.",
        ),
        (
            r#"{"summary":"s","findings":[{"title":true}]}"#,
            "findings.0.title: Expected to decode String but found bool instead.",
        ),
        // The summary decodes first, whatever the document order.
        (
            r#"{"findings":[{"body":[]}],"summary":{}}"#,
            "summary: Expected to decode String but found a dictionary instead.",
        ),
    ];
    for (raw, want) in cases {
        let err = parse_reviewer_log(raw).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("no valid reviewer JSON payload (last decode error: {want})"),
            "{raw}"
        );
    }
    // An integral number token is an Int.
    let out = parse_reviewer_log(r#"{"summary":"s","findings":[{"line":1.0,"confidence":9e0}]}"#)
        .unwrap();
    assert_eq!((out.findings[0].line, out.findings[0].confidence), (1, 9));
    // A duplicate key: the last value wins.
    let out = parse_reviewer_log(
        r#"{"summary":"s","findings":[{"file":"a"}],"findings":[{"title":"t2"}]}"#,
    )
    .unwrap();
    assert_eq!(out.findings.len(), 1);
    assert_eq!(out.findings[0].title, "t2");
    // The scan goes on to an earlier valid payload.
    let raw = r#"{"summary":"good","findings":[]} {"summary":2,"findings":[]}"#;
    assert_eq!(parse_reviewer_log(raw).unwrap().summary, "good");
}

/// A rating key makes an object a plan payload, for the CLI as for the TUI:
/// a review run shows a valid plan payload (without its rating), prefers it
/// over a later review payload, and rejects one with a rating outside
/// 1..=10 or the plan schema example.
#[test]
fn rated_payloads_follow_the_shared_rules() {
    let out = parse_reviewer_log(
        r#"{"summary":"plan","rating":5,"findings":[]} {"summary":"review","findings":[]}"#,
    )
    .unwrap();
    assert_eq!(out.summary, "plan");
    for rating in ["0", "11", "-3", "300", "null"] {
        let raw = format!(r#"{{"summary":"s","rating":{rating},"findings":[]}}"#);
        assert!(parse_reviewer_log(&raw).is_err(), "{raw}");
    }
    let err = parse_reviewer_log(r#"{"summary":"s","rating":"7","findings":[]}"#).unwrap_err();
    assert_eq!(
        err.to_string(),
        "no valid reviewer JSON payload (last decode error: rating: Expected to decode Int but found a string instead.)"
    );
    let example =
        r#"{"summary":"1-3 sentence overall assessment of the plan","rating":7,"findings":[]}"#;
    assert!(parse_reviewer_log(example).is_err());
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
    // A number out of f64 range is no object for the shared scan (as for
    // Foundation's JSONSerialization).
    assert!(json_objects(r#"{"n":1e999999}"#).is_empty());
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
        assert_eq!(
            log_payload(&raw, PayloadKind::Any),
            Err(PayloadError::NoAnswer)
        );
        let err = parse_reviewer_log(&raw).unwrap_err();
        assert_eq!(err.to_string(), "no answer in the log", "{raw}");
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

/// A codex log tail with no banner and no bare `exec` line: the answer
/// header still bounds the answer, so review JSON a tool printed above it is
/// ignored, and a doubled answer with the footer inside is one answer. With
/// no header the log is not known as codex, and the whole text is the answer
/// (the CLI and the TUI agree on both).
#[test]
fn codex_log_without_banner_or_exec_line() {
    let tool = r#"{"summary":"Saved: nothing wrong.","findings":[]}"#;
    let answer = r#"{"summary":"One bug.","findings":[{"file":"a.rs","line":3,"severity":"high","category":"bug","title":"t","body":"b","confidence":9}]}"#;
    let raw = format!(
        "user\nreview src/\n/bin/zsh -lc 'cat saved.json' in /repo\n{tool}\ncodex\n{answer}\ntokens used\n1,234\n{answer}\n"
    );
    assert!(!crate::result::is_codex_transcript(&raw));
    assert_eq!(final_answer(&raw), answer);
    let out = parse_reviewer_log(&raw).unwrap();
    assert_eq!((out.summary.as_str(), out.findings.len()), ("One bug.", 1));
    assert_cli_matches_tui(&raw);

    let headless = format!("user\nreview src/\n{tool}\nI could not finish the review.\n");
    assert_eq!(final_answer(&headless), headless);
    assert_eq!(
        parse_reviewer_log(&headless).unwrap().summary,
        "Saved: nothing wrong."
    );
    assert_cli_matches_tui(&headless);
}

/// The CLI review parse and the TUI result agree: a payload for one is the
/// same payload for the other, and no payload for one is none for the other.
fn assert_cli_matches_tui(raw: &str) {
    use crate::result::{RunResult, parse_run_result};
    match (parse_reviewer_log(raw), parse_run_result(raw)) {
        (
            Ok(cli),
            RunResult::Findings {
                summary, groups, ..
            },
        ) => {
            assert_eq!(cli.summary, summary);
            let mut tui: Vec<_> = groups
                .into_iter()
                .flat_map(|g| g.findings)
                .map(ReviewerFinding::from)
                .collect();
            let mut cli = cli.findings;
            let key = |f: &ReviewerFinding| (f.file.clone(), f.line, f.title.clone());
            tui.sort_by_key(key);
            cli.sort_by_key(key);
            assert_eq!(cli, tui);
        }
        (Err(_), RunResult::Markdown { .. } | RunResult::Failed { .. }) => {}
        (cli, tui) => panic!("CLI {cli:?} vs TUI {tui:?} for {raw:?}"),
    }
}

/// Every fixture log reads the same in the CLI and the TUI.
#[test]
fn fixture_logs_match_the_tui() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/logs");
    let mut n = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let raw = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        assert_cli_matches_tui(&raw);
        n += 1;
    }
    assert!(n >= 4, "{n} fixture logs");
    assert_cli_matches_tui(CODEX_STYLE_REVIEWER_LOG);
    assert_cli_matches_tui(include_str!("../testdata/consilium_echoed_schema.log"));
}
