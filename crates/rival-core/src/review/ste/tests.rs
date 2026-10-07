use super::*;
use std::collections::BTreeMap;

fn words(text: &str) -> Vec<String> {
    check_text(text).into_iter().map(|h| h.word).collect()
}

#[test]
fn data_parses_and_has_the_expected_size() {
    assert!(DICT.banned.len() > 1000);
    assert!(DICT.approved.len() >= 876);
}

#[test]
fn flags_not_approved_words_with_replacements() {
    let hits = check_text("Ensure the lock is held. Verify the result.");
    let ensure = hits.iter().find(|h| h.word == "ensure").unwrap();
    assert_eq!(ensure.use_instead, ["MAKE SURE (v)"]);
    assert!(words("Verify the result").contains(&"verify".to_string()));
}

#[test]
fn counts_repeats_and_sorts_most_frequent_first() {
    let hits = check_text("ensure a. ensure b. ensure c. verify d.");
    assert_eq!(hits[0].word, "ensure");
    assert_eq!(hits[0].count, 3);
    assert_eq!(hits[1].word, "verify");
}

#[test]
fn approved_words_pass() {
    assert!(words("Remove the cover. Make sure that the valve is closed.").is_empty());
}

#[test]
fn a_word_on_both_lists_is_not_flagged() {
    // "able" is on both lists in the source data. Approved wins.
    assert!(!words("able").contains(&"able".to_string()));
}

#[test]
fn skips_code_identifiers_and_paths() {
    let text = "The `ensure` function in src/verify/ensure.rs and ensure_ready() and \
                Foo::verify and obj.verify() and verifyAll.\n```\nensure verify\n```\n";
    assert_eq!(words(text), Vec::<String>::new());
}

#[test]
fn matches_phrases_before_single_words() {
    let hits = check_text("According to the log, take off the cover.");
    let found: Vec<_> = hits.iter().map(|h| h.word.as_str()).collect();
    assert!(found.contains(&"according to"), "{found:?}");
}

#[test]
fn finding_checks_every_free_text_field() {
    let f = ReviewerFinding {
        title: "Ensure x".into(),
        body: "Utilize y".into(),
        failure_scenario: "Verify z".into(),
        suggestion: "Ensure w".into(),
        ..Default::default()
    };
    let hits = check_finding(&f);
    assert_eq!(hits.iter().find(|h| h.word == "ensure").unwrap().count, 2);
    assert!(hits.iter().any(|h| h.word == "utilize"));
    assert!(hits.iter().any(|h| h.word == "verify"));
}

#[test]
fn contractions_do_not_join_words() {
    assert!(
        words("It doesn't leak").is_empty() || !words("It doesn't leak").contains(&"doesn".into())
    );
}

/// Not a regression test. Run by hand to see which words fire on real
/// reviews: `STE_LOGS=~/.rival/sessions cargo test -p rival-core measure -- --ignored --nocapture`.
#[test]
#[ignore = "reads local session logs"]
fn measure_session_logs() {
    let Ok(dir) = std::env::var("STE_LOGS") else {
        return;
    };
    let (mut logs, mut findings, mut words_total) = (0, 0, 0usize);
    let mut counts: BTreeMap<String, (usize, Vec<String>)> = BTreeMap::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "log") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(out) = crate::review::parse_reviewer_log(&raw) else {
            continue;
        };
        logs += 1;
        for f in &out.findings {
            findings += 1;
            words_total += f.body.split_whitespace().count();
            for h in check_finding(f) {
                let e = counts.entry(h.word).or_insert((0, h.use_instead));
                e.0 += h.count;
            }
        }
    }
    let mut rows: Vec<_> = counts.into_iter().collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.1.0));
    println!("logs={logs} findings={findings} body_words={words_total}");
    for (w, (n, alt)) in rows.iter().take(60) {
        println!("{n:5}  {w:<18} -> {}", alt.join(" / "));
    }
}

fn review(body: &str) -> ReviewerOutput {
    ReviewerOutput {
        summary: "One bug.".into(),
        findings: vec![ReviewerFinding {
            file: "a.rs".into(),
            line: 3,
            severity: "high".into(),
            category: "bug".into(),
            title: "Lock not held".into(),
            body: body.into(),
            failure_scenario: "Two threads write the data".into(),
            suggestion: "Hold the lock".into(),
            confidence: 8,
        }],
    }
}

#[test]
fn output_hits_merge_across_fields() {
    let mut out = review("Ensure the lock. Verify it.");
    out.summary = "Ensure x".into();
    let hits = check_output(&out);
    assert_eq!(hits.iter().find(|h| h.word == "ensure").unwrap().count, 2);
    assert_eq!(total(&hits), 3, "{hits:?}");
}

#[test]
fn rewrite_prompt_carries_json_words_and_rules() {
    let out = review("Ensure the lock is held.");
    let p = rewrite_prompt(&out, &check_output(&out));
    assert!(p.contains("ensure -> MAKE SURE (v)"), "{p}");
    assert!(p.contains("## Writing rules"));
    assert!(p.contains("\"file\": \"a.rs\""));
    assert!(
        !p.contains("No issues found"),
        "must not echo the clean example"
    );
}

#[test]
fn accepts_a_cleaner_rewrite_only() {
    let old = review("Ensure the lock is held. Verify that it is.");
    let good = review("Make sure that the lock is held. Check that it is.");
    assert!(accept_rewrite(&old, &good));
    // No improvement.
    assert!(!accept_rewrite(&old, &old));
    // A changed non-text field.
    let mut moved = good.clone();
    moved.findings[0].line = 4;
    assert!(!accept_rewrite(&old, &moved));
    let mut sev = good.clone();
    sev.findings[0].severity = "low".into();
    assert!(!accept_rewrite(&old, &sev));
    // A dropped finding.
    let mut dropped = good.clone();
    dropped.findings.clear();
    assert!(!accept_rewrite(&old, &dropped));
    // A body cut to almost nothing.
    let mut short = good.clone();
    short.findings[0].body = "Lock.".into();
    assert!(!accept_rewrite(&old, &short));
    // A field emptied.
    let mut empty = good;
    empty.findings[0].suggestion.clear();
    assert!(!accept_rewrite(&old, &empty));
}
