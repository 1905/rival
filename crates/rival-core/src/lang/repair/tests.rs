use std::cell::Cell;

use super::*;

fn finding(title: &str, body: &str) -> ReviewerFinding {
    ReviewerFinding {
        file: "src/a.rs".to_string(),
        line: 12,
        severity: "high".to_string(),
        category: "bug".to_string(),
        title: title.to_string(),
        body: body.to_string(),
        failure_scenario: String::new(),
        suggestion: "Return an error.".to_string(),
        confidence: 8,
    }
}

fn review(summary: &str, findings: Vec<ReviewerFinding>) -> Review {
    Review {
        summary: summary.to_string(),
        rating: None,
        findings,
    }
}

/// A flagged review: "utilize" and "ensure" are not approved words.
fn flagged() -> Review {
    review(
        "We utilize the cache.",
        vec![finding(
            "Cache key is wrong",
            "The code in `load()` at src/a.rs:12 reads 3 entries. Ensure the key is unique.",
        )],
    )
}

fn repaired() -> Review {
    review(
        "We use the cache.",
        vec![finding(
            "Cache key is wrong",
            "The code in `load()` at src/a.rs:12 reads 3 entries. Make sure that the key is unique.",
        )],
    )
}

#[test]
fn review_text_joins_the_text_fields_and_skips_empty_ones() {
    let r = flagged();
    assert_eq!(
        review_text(&r),
        "We utilize the cache.\n\nCache key is wrong\n\nThe code in `load()` at src/a.rs:12 reads 3 entries. Ensure the key is unique.\n\nReturn an error."
    );
}

#[test]
fn a_flagged_review_needs_repair_and_a_clean_one_does_not() {
    assert!(needs_repair(&super::super::check(&review_text(&flagged()))));
    let clean = review("The change is correct.", vec![]);
    assert!(!needs_repair(&super::super::check(&review_text(&clean))));
    // "plan" is not in the dictionary: a technical noun alone starts no call.
    let unknown = super::super::check("The plan is correct.");
    assert!(!unknown.words.is_empty());
    assert!(!needs_repair(&unknown));
}

#[test]
fn the_prompt_has_the_rules_the_report_and_the_review_but_no_word_list() {
    let r = flagged();
    let report = super::super::check(&review_text(&r));
    let p = repair_prompt(&r.to_line(), &report);
    assert!(p.contains("ASD-STE100"));
    assert!(p.contains("strict mode"));
    assert!(p.contains("untrusted data"));
    assert!(p.contains("A hedge is a fact"));
    assert!(p.contains(&report.render()));
    assert!(p.contains(&r.to_line()));
    assert!(p.trim_end().ends_with("Do not use markdown fences."));
}

#[test]
fn the_guard_accepts_a_wording_change() {
    assert_eq!(guard(&flagged(), &repaired()), Some(repaired()));
}

#[test]
fn a_shape_change_reverts_the_whole_review() {
    let old = flagged();
    let changes: Vec<fn(&mut Review)> = vec![
        |r| r.findings.clear(),
        |r| r.findings.push(finding("x", "y")),
        |r| r.findings[0].file = "src/b.rs".to_string(),
        |r| r.findings[0].line = 13,
        |r| r.findings[0].severity = "low".to_string(),
        |r| r.findings[0].category = "tests".to_string(),
        |r| r.findings[0].confidence = 9,
        |r| r.rating = Some(7),
    ];
    for change in changes {
        let mut new = repaired();
        change(&mut new);
        assert_eq!(guard(&old, &new), None, "{new:?}");
    }
}

#[test]
fn the_finding_order_is_part_of_the_shape() {
    let mut a = finding("A", "a");
    a.line = 1;
    let mut b = finding("B", "b");
    b.line = 2;
    let old = review("s", vec![a.clone(), b.clone()]);
    let new = review("s", vec![b, a]);
    assert_eq!(guard(&old, &new), None);
}

#[test]
fn a_changed_fact_reverts_only_that_field() {
    let old = flagged();
    let cases: Vec<(&str, &str)> = vec![
        (
            "number",
            "The code in `load()` at src/a.rs:12 reads 4 entries. Make sure that the key is unique.",
        ),
        (
            "code span",
            "The code in `read()` at src/a.rs:12 reads 3 entries. Make sure that the key is unique.",
        ),
        (
            "file:line",
            "The code in `load()` at src/a.rs:13 reads 3 entries. Make sure that the key is unique.",
        ),
        (
            "path",
            "The code in `load()` at src/b.rs:12 reads 3 entries. Make sure that the key is unique.",
        ),
        (
            "removed fact",
            "The code reads entries. Make sure that the key is unique.",
        ),
    ];
    for (name, body) in cases {
        let mut new = repaired();
        new.findings[0].body = body.to_string();
        let got = guard(&old, &new).expect(name);
        assert_eq!(got.findings[0].body, old.findings[0].body, "{name}");
        assert_eq!(got.summary, "We use the cache.", "{name}");
    }
}

#[test]
fn a_changed_fence_or_path_without_a_line_reverts_the_field() {
    let mut old = review(
        "s",
        vec![finding(
            "t",
            "Run:\n```\ncargo test -p a\n```\nin crates/a.",
        )],
    );
    old.findings[0].suggestion = "Edit config.yaml.".to_string();
    let mut new = old.clone();
    new.findings[0].body = "Run:\n```\ncargo test -p b\n```\nin crates/a.".to_string();
    new.findings[0].suggestion = "Edit settings.yaml.".to_string();
    let got = guard(&old, &new).unwrap();
    assert_eq!(got, old);
}

#[test]
fn an_emptied_or_filled_field_keeps_its_original_text() {
    let old = flagged();
    let mut new = repaired();
    new.findings[0].suggestion = String::new();
    new.findings[0].failure_scenario = "A new scenario.".to_string();
    let got = guard(&old, &new).unwrap();
    assert_eq!(got.findings[0].suggestion, "Return an error.");
    assert_eq!(got.findings[0].failure_scenario, "");
    assert_eq!(got.summary, "We use the cache.");
}

fn codex_log(answer: &str) -> String {
    format!(
        "OpenAI Codex v1\n--------\nuser\nprompt\ncodex\n{answer}\ntokens used\n1,234\n{answer}\n"
    )
}

#[test]
fn repair_returns_the_repaired_line() {
    let raw = codex_log(&flagged().to_line());
    let calls = Cell::new(0);
    let got = repair(&raw, PayloadKind::Any, |prompt| {
        calls.set(calls.get() + 1);
        assert!(prompt.contains(&flagged().to_line()));
        Ok(repaired().to_line())
    });
    assert_eq!(calls.get(), 1);
    assert_eq!(got, Some(repaired().to_line()));
}

#[test]
fn a_clean_review_makes_no_call() {
    let raw = codex_log(&review("The change is correct.", vec![]).to_line());
    let got = repair(&raw, PayloadKind::Any, |_| -> anyhow::Result<String> {
        panic!("no call for a clean review")
    });
    assert_eq!(got, None);
}

#[test]
fn a_log_with_no_review_makes_no_call() {
    let got = repair(
        "plain prose, no JSON",
        PayloadKind::Any,
        |_| -> anyhow::Result<String> { panic!("no call without a review") },
    );
    assert_eq!(got, None);
}

#[test]
fn a_failed_call_a_bad_reply_or_a_shape_change_keeps_the_review() {
    let raw = codex_log(&flagged().to_line());
    assert_eq!(
        repair(&raw, PayloadKind::Any, |_| Err(anyhow::anyhow!("timeout"))),
        None
    );
    assert_eq!(
        repair(&raw, PayloadKind::Any, |_| Ok("not json".to_string())),
        None
    );
    let mut moved = repaired();
    moved.findings[0].line = 99;
    assert_eq!(
        repair(&raw, PayloadKind::Any, |_| Ok(moved.to_line())),
        None
    );
}

#[test]
fn a_reply_with_no_change_after_the_guard_appends_nothing() {
    let raw = codex_log(&flagged().to_line());
    assert_eq!(
        repair(&raw, PayloadKind::Any, |_| Ok(flagged().to_line())),
        None
    );
}

#[test]
fn a_plan_review_keeps_its_rating() {
    let mut old = flagged();
    old.rating = Some(6);
    let mut new = repaired();
    new.rating = Some(6);
    let raw = codex_log(&old.to_line());
    let got = repair(&raw, PayloadKind::Plan, |_| Ok(new.to_line())).unwrap();
    assert!(got.contains("\"rating\":6"));
    let p = result::find_payload(&got, PayloadKind::Plan).unwrap();
    assert_eq!(p.summary, "We use the cache.");
}

#[test]
fn the_appended_line_is_what_every_reader_shows() {
    let raw = codex_log(&flagged().to_line());
    let line = repair(&raw, PayloadKind::Any, |_| Ok(repaired().to_line())).unwrap();
    let log = format!("{raw}\n{line}\n");
    match result::parse_run_result(&log) {
        result::RunResult::Findings { summary, .. } => assert_eq!(summary, "We use the cache."),
        other => panic!("{other:?}"),
    }
    let p = result::log_payload(&log, PayloadKind::Any).unwrap();
    assert!(p.findings[0].body.contains("Make sure that"));
}

#[test]
fn a_dropped_minus_sign_or_a_changed_quoted_error_reverts_the_field() {
    let mut old = flagged();
    old.findings[0].body = "Ensure the call returns -1 with \"permission denied\".".to_string();
    let cases = [
        "Make sure that the call returns 1 with \"permission denied\".",
        "Make sure that the call returns -1 with \"access denied\".",
        "Make sure that the call returns -1 with \u{201c}access denied\u{201d}.",
    ];
    for body in cases {
        let mut new = repaired();
        new.findings[0].body = body.to_string();
        let got = guard(&old, &new).unwrap();
        assert_eq!(got.findings[0].body, old.findings[0].body, "{body}");
        assert_eq!(got.summary, "We use the cache.");
    }
    let mut new = repaired();
    new.findings[0].body =
        "Make sure that the call returns -1 with \"permission denied\".".to_string();
    assert_eq!(
        guard(&old, &new).unwrap().findings[0].body,
        new.findings[0].body
    );
}
