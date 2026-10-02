//! The parse job, the keyed cache and the Result lines. Logs live in the
//! testkit's temp home; no test reads `~/.rival`.

use std::cell::Cell;
use std::io;

use super::*;
use crate::tui::model::Ctx;
use crate::tui::styles::STYLES;
use crate::tui::testkit::{FINDINGS_JSON, Harness, fixed_now, fixed_zone, harness, reads};
use rival_core::result::parse_run_result;

fn ctx() -> Ctx<'static> {
    Ctx {
        now: fixed_now(),
        zone: fixed_zone,
        styles: &STYLES,
    }
}

fn finished(h: &Harness, id: &str, log: &str) -> Session {
    Session {
        id: id.into(),
        cli: "codex".into(),
        model: "gpt-6-astra".into(),
        effort: "xhigh".into(),
        mode: "review".into(),
        status: "completed".into(),
        duration: "2m36s".into(),
        log_file: h.log(&format!("{id}.log"), log),
        ..Session::default()
    }
}

/// Requests, runs and accepts one parse of `s` for the pane showing `s`.
fn round_trip(h: &Harness, slot: &mut ResultSlot, s: &Session) -> bool {
    let target = ResultTarget::of(s);
    let req = slot.request(target.clone()).expect("a request");
    let res = load_result(req, h.env.read_tail);
    slot.accept(res, Some(&target))
}

fn text(lines: &[Line<'_>]) -> Vec<String> {
    lines.iter().map(ToString::to_string).collect()
}

fn findings_entry(s: &Session) -> ResultEntry {
    ResultEntry {
        target: ResultTarget::of(s),
        state: None,
        result: Ok(Arc::new(parse_run_result(FINDINGS_JSON))),
    }
}

// --- the keyed cache -------------------------------------------------------------

/// An unchanged log is stat-ed, never read or parsed again, and the cached
/// parse stays the same value.
#[test]
fn unchanged_key_is_reused_without_a_read() {
    let h = harness();
    let s = finished(&h, "same", FINDINGS_JSON);
    let mut slot = ResultSlot::default();
    assert!(round_trip(&h, &mut slot, &s));
    assert_eq!(reads(), 1);
    let first = slot.entry_of(&ResultTarget::of(&s)).unwrap().clone();
    assert!(first.state.is_some(), "the parse carries its file state");

    let req = slot.request(ResultTarget::of(&s)).unwrap();
    assert_eq!(req.known, first.state, "the request names the cached state");
    let res = load_result(req, h.env.read_tail);
    assert_eq!(res.outcome, ResultOutcome::Unchanged);
    assert!(!slot.accept(res, Some(&ResultTarget::of(&s))));
    assert_eq!(reads(), 1, "an unchanged log was read again");
    let again = slot.entry_of(&ResultTarget::of(&s)).unwrap();
    let (Ok(a), Ok(b)) = (&first.result, &again.result) else {
        panic!("not parsed");
    };
    assert!(Arc::ptr_eq(a, b), "the cached parse was replaced");
}

/// A new size or a new mtime is a new key: the log is read and parsed again.
#[test]
fn a_changed_key_invalidates_the_parse() {
    let h = harness();
    let s = finished(&h, "grows", "first answer\n");
    let mut slot = ResultSlot::default();
    round_trip(&h, &mut slot, &s);
    let target = ResultTarget::of(&s);
    let shown = |slot: &ResultSlot| match &slot.entry_of(&target).unwrap().result {
        Ok(r) => (*r.as_ref()).clone(),
        Err(e) => panic!("{e}"),
    };
    assert_eq!(
        shown(&slot),
        RunResult::Markdown {
            text: "first answer".into()
        }
    );

    // A new size.
    std::fs::write(&s.log_file, "second answer, longer\n").unwrap();
    assert!(round_trip(&h, &mut slot, &s));
    assert_eq!(
        shown(&slot),
        RunResult::Markdown {
            text: "second answer, longer".into()
        }
    );
    assert_eq!(reads(), 2);

    // Same size, new mtime.
    std::fs::write(&s.log_file, "SECOND answer, longer\n").unwrap();
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
    std::fs::File::options()
        .write(true)
        .open(&s.log_file)
        .unwrap()
        .set_modified(later)
        .unwrap();
    assert!(round_trip(&h, &mut slot, &s));
    assert_eq!(
        shown(&slot),
        RunResult::Markdown {
            text: "SECOND answer, longer".into()
        }
    );
    assert_eq!(reads(), 3);
}

/// The user moves to another member while the first parse is on a worker:
/// that parse is dropped when it lands, and the new member's is taken.
#[test]
fn a_selection_change_while_parsing_drops_the_stale_parse() {
    let h = harness();
    let a = finished(&h, "aaaa", "answer A\n");
    let b = finished(&h, "bbbb", "answer B\n");
    let mut slot = ResultSlot::default();
    let req_a = slot.request(ResultTarget::of(&a)).unwrap();
    let req_b = slot.request(ResultTarget::of(&b)).unwrap();
    assert!(slot.in_flight(&ResultTarget::of(&b)));
    let res_a = load_result(req_a, h.env.read_tail);
    assert!(!slot.accept(res_a, Some(&ResultTarget::of(&b))));
    assert!(slot.entry_of(&ResultTarget::of(&a)).is_none());
    assert!(
        slot.in_flight(&ResultTarget::of(&b)),
        "the stale parse cleared the wanted one's wait"
    );
    let res_b = load_result(req_b, h.env.read_tail);
    assert!(slot.accept(res_b, Some(&ResultTarget::of(&b))));
    assert!(slot.entry_of(&ResultTarget::of(&b)).is_some());
}

/// A parse delivered late never replaces a newer revision of the same log.
#[test]
fn a_late_parse_never_replaces_a_newer_revision() {
    let h = harness();
    let s = finished(&h, "rev", "revision one\n");
    let target = ResultTarget::of(&s);
    let mut slot = ResultSlot::default();
    let old = load_result(slot.request(target.clone()).unwrap(), h.env.read_tail);
    std::fs::write(&s.log_file, "revision two, newer\n").unwrap();
    // The old parse was dropped (the pane closed); the reopened pane asks
    // again and gets the new revision first.
    slot.cancel();
    let new = load_result(slot.request(target.clone()).unwrap(), h.env.read_tail);
    assert!(slot.accept(new, Some(&target)));
    assert!(
        !slot.accept(old, Some(&target)),
        "the old revision came back"
    );
    let Ok(r) = &slot.entry_of(&target).unwrap().result else {
        panic!("not parsed");
    };
    assert_eq!(
        **r,
        RunResult::Markdown {
            text: "revision two, newer".into()
        }
    );
}

/// Controller finding 4: A(1) → B(2) → A(3), and A's first reply lands
/// before its newest. Only the newest request's reply counts; the older one
/// neither shows nor ends the wait for the newest.
#[test]
fn a_superseded_reply_never_becomes_current() {
    let h = harness();
    let a = finished(&h, "aaaa", "answer A\n");
    let b = finished(&h, "bbbb", "answer B\n");
    let (ta, tb) = (ResultTarget::of(&a), ResultTarget::of(&b));
    let mut slot = ResultSlot::default();
    let a1 = load_result(slot.request(ta.clone()).unwrap(), h.env.read_tail);
    let b2 = load_result(slot.request(tb.clone()).unwrap(), h.env.read_tail);
    let a3 = load_result(slot.request(ta.clone()).unwrap(), h.env.read_tail);
    assert_eq!((a1.seq, b2.seq, a3.seq), (1, 2, 3));
    assert!(
        !slot.accept(a1, Some(&ta)),
        "a superseded reply became current"
    );
    assert!(slot.entry_of(&ta).is_none());
    assert!(slot.in_flight(&ta), "the old reply ended the newest wait");
    assert!(!slot.accept(b2, Some(&ta)));
    assert!(slot.in_flight(&ta));
    assert!(slot.accept(a3, Some(&ta)), "the newest reply was dropped");
    assert!(!slot.in_flight(&ta));
    assert!(slot.entry_of(&ta).is_some());
}

/// An `Unchanged` reply that lands after its request was cancelled, or
/// after its entry was evicted by newer requests, is dropped: it never
/// vouches for an entry it did not see, and the next request reads again.
#[test]
fn a_late_unchanged_after_cancel_or_eviction_is_dropped() {
    let h = harness();
    let a = finished(&h, "aaaa", "answer A\n");
    let ta = ResultTarget::of(&a);
    let mut slot = ResultSlot::default();
    round_trip(&h, &mut slot, &a);

    // Cancelled (the pane closed) while the stat was on a worker.
    let late = load_result(slot.request(ta.clone()).unwrap(), h.env.read_tail);
    assert_eq!(late.outcome, ResultOutcome::Unchanged);
    slot.cancel();
    assert!(!slot.accept(late, Some(&ta)));
    assert!(slot.entry_of(&ta).is_some(), "the cached parse stays");

    // Superseded and evicted while the stat was on a worker.
    let late = load_result(slot.request(ta.clone()).unwrap(), h.env.read_tail);
    assert_eq!(late.outcome, ResultOutcome::Unchanged);
    for i in 0..RESULT_CACHE_CAP {
        round_trip(&h, &mut slot, &finished(&h, &format!("o{i}"), "other\n"));
    }
    assert!(slot.entry_of(&ta).is_none(), "test premise: A was evicted");
    assert!(!slot.accept(late, Some(&ta)));
    assert!(slot.entry_of(&ta).is_none());
    let before = reads();
    let req = slot.request(ta.clone()).unwrap();
    assert_eq!(req.known, None, "an evicted parse is read again");
    assert!(slot.accept(load_result(req, h.env.read_tail), Some(&ta)));
    assert_eq!(reads(), before + 1);
}

/// One request per target at a time, and the cache keeps at most
/// `RESULT_CACHE_CAP` parses, the least recently used going first.
#[test]
fn requests_dedupe_and_the_cache_is_bounded() {
    let h = harness();
    let mut slot = ResultSlot::default();
    let sessions: Vec<Session> = (0..RESULT_CACHE_CAP + 2)
        .map(|i| finished(&h, &format!("s{i:02}"), &format!("answer {i}\n")))
        .collect();
    let t0 = ResultTarget::of(&sessions[0]);
    assert!(slot.request(t0.clone()).is_some());
    assert!(slot.request(t0.clone()).is_none(), "a twin request");
    slot.cancel();
    for s in &sessions {
        round_trip(&h, &mut slot, s);
        // Keep the first one in use.
        round_trip(&h, &mut slot, &sessions[0]);
    }
    assert_eq!(slot.len(), RESULT_CACHE_CAP);
    assert!(slot.entry_of(&t0).is_some(), "the entry in use was evicted");
    assert!(slot.entry_of(&ResultTarget::of(&sessions[1])).is_none());
    assert!(
        slot.entry_of(&ResultTarget::of(sessions.last().unwrap()))
            .is_some()
    );
}

/// A log that cannot be read shows the read error and is read again next
/// time: a failure is never cached as current.
#[test]
fn a_missing_log_fails_and_is_read_again() {
    let h = harness();
    let mut s = finished(&h, "gone", "x\n");
    s.log_file = h
        .home
        .path()
        .join("nope.log")
        .to_string_lossy()
        .into_owned();
    let mut slot = ResultSlot::default();
    assert!(round_trip(&h, &mut slot, &s));
    let entry = slot.entry_of(&ResultTarget::of(&s)).unwrap();
    assert_eq!(
        entry.result,
        Err(format!("open {}: no such file or directory", s.log_file))
    );
    let req = slot.request(ResultTarget::of(&s)).unwrap();
    assert_eq!(req.known, None);
}

thread_local! {
    static ASKED: Cell<i64> = const { Cell::new(0) };
}

fn recording_tail(path: &Path, max: i64) -> io::Result<(Vec<u8>, bool)> {
    ASKED.set(max);
    logfmt::read_tail(path, max)
}

/// Only the last 256 KB are read and parsed: an answer earlier in a long
/// log is not seen.
#[test]
fn the_parse_reads_the_256k_tail_only() {
    let h = harness();
    let filler = "tool output line that pads the log past the tail\n".repeat(6000);
    let log = format!("{FINDINGS_JSON}\n{filler}The final answer is prose.\n");
    assert!(log.len() as i64 > logfmt::MAX_TAIL_BYTES);
    let s = finished(&h, "long", &log);
    let req = ResultSlot::default().request(ResultTarget::of(&s)).unwrap();
    let res = load_result(req, recording_tail);
    assert_eq!(ASKED.get(), logfmt::MAX_TAIL_BYTES);
    let ResultOutcome::Parsed { result, .. } = res.outcome else {
        panic!("not parsed: {:?}", res.outcome);
    };
    let RunResult::Markdown { text } = result.as_ref() else {
        panic!("the head's JSON was parsed: {result:?}");
    };
    assert!(text.ends_with("The final answer is prose."), "{text}");
}

// --- the lines -----------------------------------------------------------------------

#[test]
fn notes_for_live_missing_and_unread_members() {
    let h = harness();
    let mut focus = FindingFocus::default();
    let live = Session {
        status: "running".into(),
        ..finished(&h, "live", "x\n")
    };
    assert_eq!(
        text(&result_lines(&live, None, &mut focus, 80, &ctx())),
        [LIVE_NOTE]
    );
    let queued = Session {
        status: "queued".into(),
        ..live.clone()
    };
    assert_eq!(
        text(&result_lines(&queued, None, &mut focus, 80, &ctx())),
        [LIVE_NOTE]
    );
    let no_log = Session {
        log_file: String::new(),
        ..finished(&h, "nolog", "x\n")
    };
    assert_eq!(
        text(&result_lines(&no_log, None, &mut focus, 80, &ctx())),
        [NO_LOG, "", RAW_HINT]
    );
    let s = finished(&h, "unread", "x\n");
    assert_eq!(
        text(&result_lines(&s, None, &mut focus, 80, &ctx())),
        [READING]
    );
    let failed_read = ResultEntry {
        target: ResultTarget::of(&s),
        state: None,
        result: Err("open /x: permission denied".into()),
    };
    assert_eq!(
        text(&result_lines(
            &s,
            Some(&failed_read),
            &mut focus,
            80,
            &ctx()
        )),
        [
            "(log unavailable: open /x: permission denied)",
            "",
            RAW_HINT
        ]
    );
}

/// Swift `ResultNote` with a title: the failure, its reason, the session's
/// error and the Raw hint.
#[test]
fn a_parse_failure_shows_reason_error_and_hint() {
    let h = harness();
    let s = Session {
        status: "failed".into(),
        error_msg: "codex exited 1: rate limited".into(),
        ..finished(&h, "bad", "x\n")
    };
    let entry = ResultEntry {
        target: ResultTarget::of(&s),
        state: None,
        result: Ok(Arc::new(RunResult::Failed {
            reason: "JSON answer did not decode: findings: Expected to decode Array<Any> but found a string instead.".into(),
        })),
    };
    let lines = result_lines(&s, Some(&entry), &mut FindingFocus::default(), 60, &ctx());
    assert_eq!(
        text(&lines),
        [
            PARSE_FAILED,
            "",
            "JSON answer did not decode: findings: Expected to decode",
            "Array<Any> but found a string instead.",
            "",
            "error:",
            "codex exited 1: rate limited",
            "",
            RAW_HINT,
        ]
    );
    assert_eq!(lines[0].spans[0].style, STYLES.running);
}

/// The header box: meta and rating on one row, counts, summary wrapped
/// inside the frame. Then a rule per severity and the findings, collapsed.
#[test]
fn findings_layout() {
    let h = harness();
    let s = finished(&h, "lay", "x\n");
    let entry = findings_entry(&s);
    let mut focus = FindingFocus::default();
    let lines = result_lines(&s, Some(&entry), &mut focus, 72, &ctx());
    let got = text(&lines);
    let want = [
        "╭──────────────────────────────────────────────────────────────────────╮",
        "│ gpt-6-astra · xhigh · review · 2m36s                     rating 7/10 │",
        "│ ● 1 critical  ● 1 high  ● 1 medium                                   │",
        "│                                                                      │",
        "│ Two real problems in the fingerprint re-key and one nit. The         │",
        "│ migration path is otherwise sound.                                   │",
        "╰──────────────────────────────────────────────────────────────────────╯",
        "",
        "CRITICAL ───────────────────────────────────────────────────────────────",
        "▌ src/auth/fingerprint.rs:42 · bug · conf 90",
        "▌ Re-key drops sessions created during the migration window",
        "▌ The loop reads the old key once and writes the new one after every",
        "▌ batch, so sessions created mid-run keep the old fingerprint.",
        "▌ ▸ failure scenario",
        "▌ ▸ suggestion",
        "",
        "HIGH ───────────────────────────────────────────────────────────────────",
        "│ src/auth/store.rs:118 · concurrency · conf 75",
        "│ Unbounded retry on lock contention",
        "│ The retry has no cap and no backoff.",
        "",
        "MEDIUM ─────────────────────────────────────────────────────────────────",
        "│ — · tests · conf 60",
        "│ No test covers the empty store",
        "│ ▸ suggestion",
    ];
    assert_eq!(got, want);
    assert_eq!(focus.rows, [9..15, 17..20, 22..25]);
    // The rating is amber, the meta accent, a critical rule red.
    let rating = lines[1].spans.iter().find(|s| s.content.contains("rating"));
    assert_eq!(rating.unwrap().style, STYLES.running);
    assert_eq!(lines[1].spans[1].style, STYLES.accent);
    assert_eq!(lines[8].spans[0].style.fg, STYLES.failed.fg);
}

/// Opened, the focused finding shows its failure scenario and suggestion
/// wrapped under their labels; the bar marks the focus.
#[test]
fn an_expanded_finding_shows_its_details() {
    let h = harness();
    let s = finished(&h, "exp", "x\n");
    let entry = findings_entry(&s);
    let mut focus = FindingFocus::default();
    result_lines(&s, Some(&entry), &mut focus, 60, &ctx());
    focus.expanded.insert(0);
    focus.focus = 1;
    let lines = text(&result_lines(&s, Some(&entry), &mut focus, 60, &ctx()));
    let first: Vec<&str> = lines[focus.rows[0].clone()]
        .iter()
        .map(String::as_str)
        .collect();
    assert!(first.iter().all(|l| l.starts_with("│ ")), "{first:#?}");
    assert!(first.contains(&"│ ▾ failure scenario"), "{first:#?}");
    assert!(
        first.contains(&"│   A user logs in while the re-key runs; their next request"),
        "{first:#?}"
    );
    assert!(first.contains(&"│ ▾ suggestion"), "{first:#?}");
    assert!(lines[focus.rows[1].start].starts_with("▌ src/auth/store.rs:118"));
    // The focus survives a re-render of the same parse and resets for a
    // new one.
    let again = findings_entry(&s);
    result_lines(&s, Some(&again), &mut focus, 60, &ctx());
    assert_eq!((focus.focus, focus.expanded.len()), (0, 0));
}

/// Long meta wraps instead of being cut, and the rating moves to its own
/// row; every row fits.
#[test]
fn narrow_headers_wrap_and_fit() {
    let h = harness();
    let s = Session {
        model: "claude-opus-5-5-with-a-very-long-deployment-name".into(),
        ..finished(&h, "narrow", "x\n")
    };
    let entry = findings_entry(&s);
    for w in [20, 40, 60, 61, 80, 120] {
        let lines = result_lines(&s, Some(&entry), &mut FindingFocus::default(), w, &ctx());
        for l in &lines {
            assert!(line_width(l) <= w, "w={w}: {l}");
        }
    }
    let lines = text(&result_lines(
        &s,
        Some(&entry),
        &mut FindingFocus::default(),
        60,
        &ctx(),
    ));
    assert_eq!(
        lines[1],
        "│ claude-opus-5-5-with-a-very-long-deployment-name · xhigh │"
    );
    assert_eq!(
        lines[2],
        "│ · review · 2m36s                                         │"
    );
    assert_eq!(
        lines[3],
        "│                                              rating 7/10 │"
    );
}

/// Swift `FindingCard.location` and `.meta`.
#[test]
fn finding_location_and_meta() {
    let f = |file: &str, line, severity: &str, category: &str, confidence| Finding {
        file: file.into(),
        line,
        severity: severity.into(),
        category: category.into(),
        confidence,
        ..Finding::default()
    };
    assert_eq!(location(&f("a.rs", 3, "", "", 0)), "a.rs:3");
    assert_eq!(location(&f("a.rs", 0, "", "", 0)), "a.rs");
    assert_eq!(location(&f("", 7, "", "", 0)), "line 7");
    assert_eq!(location(&f("", 0, "", "", 0)), "—");
    assert_eq!(
        finding_meta(&f("", 0, "high", "bug", 80)),
        ["bug", "conf 80"]
    );
    assert_eq!(finding_meta(&f("", 0, "blocker", "", 0)), ["blocker"]);
    assert!(finding_meta(&f("", 0, "", "", 0)).is_empty());
}

#[test]
fn no_findings_and_markdown_headers() {
    let h = harness();
    let s = finished(&h, "md", "x\n");
    let clean = ResultEntry {
        target: ResultTarget::of(&s),
        state: None,
        result: Ok(Arc::new(parse_run_result(
            r#"{"summary": "No issues found.", "findings": []}"#,
        ))),
    };
    let lines = text(&result_lines(
        &s,
        Some(&clean),
        &mut FindingFocus::default(),
        50,
        &ctx(),
    ));
    assert_eq!(
        lines[2],
        "│ No findings.                                   │"
    );
    assert_eq!(
        lines[4],
        "│ No issues found.                               │"
    );

    let md = ResultEntry {
        target: ResultTarget::of(&s),
        state: None,
        result: Ok(Arc::new(RunResult::Markdown {
            text: "# Verdict\n\nShip it.".into(),
        })),
    };
    let lines = text(&result_lines(
        &s,
        Some(&md),
        &mut FindingFocus::default(),
        50,
        &ctx(),
    ));
    assert_eq!(
        lines,
        [
            "╭────────────────────────────────────────────────╮",
            "│ gpt-6-astra · xhigh · review · 2m36s           │",
            "╰────────────────────────────────────────────────╯",
            "",
            "Verdict",
            "",
            "Ship it.",
        ]
    );
}

/// Provider text can carry terminal controls: ANSI colours, OSC links and
/// titles, C1 CSI. None of them reaches a span on any Result surface; the
/// stored session and the parse keep them.
#[test]
fn terminal_controls_never_reach_a_span() {
    let h = harness();
    let esc = |s: &str| s.replace("ESC", "\\u001b").replace("C1", "\\u009b");
    let json = esc(
        r#"{"summary": "ESC[31mred summaryESC[0m ESC]0;pwnedESC\\ done C131m", "rating": 4, "findings": [
{"file": "ESC]8;;http://evil/ESC\\src/x.rsESC]8;;ESC\\", "line": 1, "severity": "high", "category": "bugESC[2J", "title": "ESC[1mtitleESC[0m C1", "body": "bodyESC]0;titleESC\\ text", "failure_scenario": "fsESC[5m blink", "suggestion": "sgC1 31m", "confidence": 9}]}"#,
    );
    let parsed = parse_run_result(&json);
    let RunResult::Findings { summary, .. } = &parsed else {
        panic!("{parsed:?}");
    };
    assert!(summary.contains('\x1b'), "the parse keeps the raw text");
    let s = Session {
        model: "gpt\x1b[31m-6".into(),
        effort: "x\u{9b}31mhigh".into(),
        mode: "rev\x07iew".into(),
        status: "failed".into(),
        error_msg: "boom \x1b]0;title\x07 \x1b[31mred".into(),
        ..finished(&h, "ctl", "x\n")
    };
    let entry = ResultEntry {
        target: ResultTarget::of(&s),
        state: None,
        result: Ok(Arc::new(parsed)),
    };
    let mut focus = FindingFocus::default();
    result_lines(&s, Some(&entry), &mut focus, 80, &ctx());
    focus.expanded.insert(0);
    let mut all = result_lines(&s, Some(&entry), &mut focus, 80, &ctx());
    let failed = ResultEntry {
        result: Ok(Arc::new(RunResult::Failed {
            reason: "bad \x1b[31mreason\u{9b}2J".into(),
        })),
        ..entry.clone()
    };
    all.extend(result_lines(&s, Some(&failed), &mut focus, 80, &ctx()));
    let unread = ResultEntry {
        result: Err("open /x\x1b[2J: denied".into()),
        ..entry
    };
    all.extend(result_lines(&s, Some(&unread), &mut focus, 80, &ctx()));
    for line in &all {
        for span in &line.spans {
            assert!(
                !span.content.chars().any(char::is_control),
                "control char in {:?}",
                span.content
            );
        }
    }
    let joined = text(&all).join("\n");
    for want in [
        "red summary",
        "src/x.rs",
        "title",
        "body text",
        "fs blink",
        "gpt-6",
        "review",
        "boom",
        "bad reason",
    ] {
        assert!(joined.contains(want), "{want:?} missing:\n{joined}");
    }
}
