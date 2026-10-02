use std::sync::Arc;

use super::*;
use crate::tui::testkit::{harness, reads};
use crate::tui::text::width;

fn lines(path: &str, wrap: usize, last_n: usize) -> Result<LogLines, String> {
    read_log_lines(path, wrap, last_n, logfmt::read_tail)
}

// Go: TestSanitizeLog.
#[test]
fn sanitize_log_cases() {
    let cases: &[(&str, &str, &str)] = &[
        (
            "tab expands to four spaces",
            "if x {\n\treturn\n}",
            "if x {\n    return\n}",
        ),
        ("progress frames collapse to the last", "10%\r99%", "99%"),
        ("csi color stripped", "\x1b[31mred\x1b[0m", "red"),
        ("osc bel stripped", "\x1b]0;title\x07text", "text"),
        ("osc st stripped", "\x1b]8;;url\x1b\\link", "link"),
        ("backspace and nul dropped", "a\x08b\x00c", "abc"),
        ("newlines preserved", "one\ntwo\nthree", "one\ntwo\nthree"),
        ("plain text unchanged", "plain log line", "plain log line"),
        // CRLF: sanitize splits on "\n", so each line still ends in "\r".
        // The trailing-terminator trim must run before the progress-frame
        // rule or every line of a CRLF log collapses to empty.
        ("crlf lines survive", "alpha\r\nbeta\r\n", "alpha\nbeta\n"),
        (
            "trailing frame keeps its text",
            "working...\r",
            "working...",
        ),
        ("crlf plus progress frames", "10%\r99%\r\n", "99%\n"),
    ];
    for (name, raw, want) in cases {
        assert_eq!(sanitize_log(raw), *want, "{name}");
    }
}

// Go: TestWrapLogLinesDisplayWidth. Wrapping by rune count let tabs and wide
// runes push lines past the pane width. Every emitted line must fit.
#[test]
fn wrap_log_lines_display_width() {
    let h = harness();
    let raw = [
        "func main() {".to_string(),
        "\tif err := run(); err != nil {".into(),
        "\t\tlog.Fatal(err)".into(),
        "\t}".into(),
        "}".into(),
        "あ".repeat(50),
        format!("\x1b[31m{}\x1b[0m", "error detail ".repeat(20)),
    ]
    .join("\n");
    let path = h.log("session.log", &raw);
    for w in [20, 40, 80] {
        let got = lines(&path, w, 0).unwrap();
        assert!(!got.lines.is_empty(), "width {w}: no lines");
        for (i, l) in got.lines.iter().enumerate() {
            assert!(
                width(l) <= w,
                "width {w}: line {i} is {} cells: {l:?}",
                width(l)
            );
        }
    }
}

// Go: TestWrapLogLinesEmptyAndMissing.
#[test]
fn wrap_log_lines_empty_and_missing() {
    let h = harness();
    let empty = h.log("empty.log", "");
    assert_eq!(lines(&empty, 80, 0), Ok(LogLines::default()));
    let missing = h.home.path().join("missing.log");
    let missing = missing.to_str().unwrap();
    assert_eq!(
        lines(missing, 80, 0),
        Err(format!("open {missing}: no such file or directory")),
        "Go's *PathError text"
    );
}

// Go: TestWrapLogLinesKeepsRawModelID. The TUI shows raw model ids, so the
// log must too: no public renaming.
#[test]
fn wrap_log_lines_keeps_raw_model_id() {
    let h = harness();
    let path = h.log(
        "raw.log",
        "OpenAI Codex v1\nmodel: gpt-6-astra\n\x1b[31mred\x1b[0m\tend\n",
    );
    let got = lines(&path, 80, 0).unwrap().lines.join("\n");
    assert!(
        got.contains("model: gpt-6-astra") && got.contains("OpenAI Codex v1"),
        "{got:?}"
    );
    assert!(!got.contains('\x1b') && !got.contains('\t'), "{got:?}");
    assert!(got.contains("red    end"), "tab not expanded: {got:?}");
}

// Go: TestReadLogLinesLastNMatchesTheFullRead. The preview cuts raw lines
// before sanitizing. That must give the same tail as sanitizing the whole log
// first, even when the last lines sanitize to nothing or end in CRLF.
#[test]
fn last_n_matches_the_full_read() {
    let h = harness();
    let mut b = String::new();
    for i in 0..60 {
        b.push_str(&format!("\tline {i} 10%\r99%\r\n"));
    }
    b.push_str("\x1b[0m\n\x1b[0m\r\n\n");
    let path = h.log("cut.log", &b);
    let full = lines(&path, 0, 0).unwrap().lines;
    for n in [1, 5, 30, 200] {
        let got = lines(&path, 0, n).unwrap().lines;
        let want = &full[full.len().saturating_sub(n)..];
        assert_eq!(got, want, "last_n {n}");
    }
}

// A log longer than the tail limit starts with the omitted marker, wrapped
// like the text and counted in marker_rows. The preview cut leaves it out.
#[test]
fn a_cut_log_starts_with_the_marker() {
    let h = harness();
    let body = "x".repeat(70) + "\n";
    let path = h.log("big.log", &body.repeat(5000));
    let got = lines(&path, 30, 0).unwrap();
    assert_eq!(got.marker_rows, 2, "57 cells wrap to 2 rows at 30");
    assert_eq!(got.lines[..2].concat(), OMITTED_MARKER);
    assert!(got.lines.iter().all(|l| width(l) <= 30));
    let tail = lines(&path, 30, 3).unwrap();
    assert_eq!(tail.marker_rows, 0);
    assert_eq!(
        tail.lines.len(),
        9,
        "3 source lines of 70 cells, 3 rows each"
    );
}

#[test]
fn nth_last_newline_cases() {
    assert_eq!(nth_last_newline("a\nb\nc", 1), Some(3));
    assert_eq!(nth_last_newline("a\nb\nc", 2), Some(1));
    assert_eq!(nth_last_newline("a\nb\nc", 3), None);
    assert_eq!(nth_last_newline("", 1), None);
}

// Go: TestCreateLogViewWritesTheRawLog.
#[test]
fn create_log_view_writes_the_raw_log() {
    let h = harness();
    let raw = "OpenAI Codex v1\n--------\nmodel: gpt-6-astra\n";
    let s = Session {
        cli: "codex".into(),
        model: "gpt-6-astra".into(),
        log_file: h.log("raw.log", raw),
        ..Session::default()
    };
    let view = create_log_view(&h.env.temp_dir, &s).unwrap();
    assert!(view.starts_with(&h.env.temp_dir));
    let name = view.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        name.starts_with("rival-log-") && name.ends_with(".txt"),
        "{name}"
    );
    assert_eq!(fs::read_to_string(&view).unwrap(), raw);
}

// Go: TestCreateGroupLogViewUsesRawIDsAndErrors.
#[test]
fn create_group_log_view_uses_raw_ids_and_errors() {
    let h = harness();
    let sessions = vec![
        Arc::new(Session {
            cli: "codex".into(),
            model: "gpt-5.5".into(),
            mode: "megareview".into(),
            effort: "high".into(),
            status: "failed".into(),
            error_msg: "codex gpt-5.5 exploded".into(),
            log_file: h.log("review.log", "review body gpt-5.5\n"),
            ..Session::default()
        }),
        Arc::new(Session {
            cli: "codex".into(),
            model: "gpt-6-astra".into(),
            mode: "consilium".into(),
            status: "completed".into(),
            log_file: h.log("judge.log", "judge body"),
            ..Session::default()
        }),
        Arc::new(Session {
            cli: "claude".into(),
            status: "completed".into(),
            log_file: "/nonexistent/x.log".into(),
            ..Session::default()
        }),
    ];
    let view = create_group_log_view(&h.env.temp_dir, &sessions).unwrap();
    let got = fs::read_to_string(view).unwrap();
    for want in [
        "=== gpt-5.5 REVIEW · EFFORT high (FAILED) ===",
        "Error: codex gpt-5.5 exploded",
        "review body gpt-5.5",
        "=== gpt-6-astra JUDGE ===",
        // A log without a trailing newline gets one before the gap.
        "judge body\n\n=== claude REVIEW ===",
        "(log unavailable: open /nonexistent/x.log: no such file or directory)",
    ] {
        assert!(got.contains(want), "group log lacks {want:?}:\n{got}");
    }
}

// --- the cache slot ------------------------------------------------------------

fn key(id: &str, path: &str, width: usize) -> LogKey {
    LogKey {
        session_id: id.into(),
        path: path.into(),
        width,
        last_n: 0,
    }
}

/// An unchanged file costs a stat, never a read; a grown one is read once.
#[test]
fn metadata_only_refresh_reuses_the_cached_lines() {
    let h = harness();
    let path = h.log("grow.log", "one\n");
    let read_tail = h.env.read_tail;
    let mut slot = LogSlot::default();
    let k = key("s1", &path, 40);

    let req = slot.request(LogPane::Detail, k.clone(), true).unwrap();
    assert_eq!(req.known, None);
    assert!(slot.accept(load_log(req, read_tail), Some(&k)));
    assert_eq!(reads(), 1);

    // Same key, no refresh: nothing to do.
    assert!(slot.request(LogPane::Detail, k.clone(), false).is_none());
    // A refresh stats the unchanged file and reads nothing.
    let req = slot.request(LogPane::Detail, k.clone(), true).unwrap();
    assert!(req.known.is_some());
    let res = load_log(req, read_tail);
    assert_eq!(res.outcome, LogOutcome::Unchanged);
    assert!(!slot.accept(res, Some(&k)));
    assert_eq!(reads(), 1, "an unchanged file is not re-read");
    assert_eq!(
        slot.entry().unwrap().result.as_ref().unwrap().lines,
        ["one"]
    );

    // The file grows: the next refresh reads it once.
    fs::write(&path, "one\ntwo and more\n").unwrap();
    let req = slot.request(LogPane::Detail, k.clone(), true).unwrap();
    assert!(slot.accept(load_log(req, read_tail), Some(&k)));
    assert_eq!(reads(), 2);
    assert_eq!(
        slot.entry().unwrap().result.as_ref().unwrap().lines,
        ["one", "two and more"]
    );
}

/// A failed read is never cached as current: the next refresh reads again.
#[test]
fn a_failed_read_is_read_again() {
    let h = harness();
    let path = h
        .home
        .path()
        .join("late.log")
        .to_string_lossy()
        .into_owned();
    let mut slot = LogSlot::default();
    let k = key("s1", &path, 40);
    let req = slot.request(LogPane::Preview, k.clone(), true).unwrap();
    assert!(slot.accept(load_log(req, h.env.read_tail), Some(&k)));
    assert!(slot.entry().unwrap().result.is_err());
    fs::write(&path, "now here\n").unwrap();
    let req = slot.request(LogPane::Preview, k.clone(), true).unwrap();
    assert_eq!(req.known, None);
    assert!(slot.accept(load_log(req, h.env.read_tail), Some(&k)));
    assert_eq!(
        slot.entry().unwrap().result.as_ref().unwrap().lines,
        ["now here"]
    );
}

/// A request already in flight for the same key is not sent twice; a new
/// key is.
#[test]
fn one_read_in_flight_per_key() {
    let mut slot = LogSlot::default();
    let a = key("s1", "/a", 40);
    let first = slot.request(LogPane::Detail, a.clone(), true).unwrap();
    assert!(slot.request(LogPane::Detail, a.clone(), true).is_none());
    let b = key("s2", "/b", 40);
    let second = slot.request(LogPane::Detail, b.clone(), true).unwrap();
    assert!(second.seq > first.seq);
}

fn loaded(req: &LogRequest, text: &str) -> LogResult {
    LogResult {
        pane: req.pane,
        seq: req.seq,
        key: req.key.clone(),
        outcome: LogOutcome::Loaded {
            state: None,
            lines: LogLines {
                marker_rows: 0,
                lines: vec![text.into()],
            },
        },
    }
}

/// A late result for another member or an old width is dropped, and so is
/// an older result arriving after a newer one.
#[test]
fn late_results_are_dropped() {
    let mut slot = LogSlot::default();
    let old_member = key("s1", "/a", 40);
    let new_member = key("s2", "/b", 40);
    let r1 = slot
        .request(LogPane::Detail, old_member.clone(), true)
        .unwrap();
    let r2 = slot
        .request(LogPane::Detail, new_member.clone(), true)
        .unwrap();
    // s1's read lands after the user moved to s2.
    assert!(!slot.accept(loaded(&r1, "S1"), Some(&new_member)));
    assert!(slot.entry().is_none());
    assert!(slot.accept(loaded(&r2, "S2"), Some(&new_member)));

    // A resize: the read for the old width is dropped.
    let narrow = key("s2", "/b", 30);
    let wide = slot
        .request(LogPane::Detail, key("s2", "/b", 50), true)
        .unwrap();
    let r3 = slot.request(LogPane::Detail, narrow.clone(), true).unwrap();
    assert!(!slot.accept(loaded(&wide, "WIDE"), Some(&narrow)));
    assert!(slot.accept(loaded(&r3, "NARROW"), Some(&narrow)));

    // Same key twice: the older result arriving last is dropped.
    let x = key("s3", "/c", 40);
    let older = slot.request(LogPane::Detail, x.clone(), true).unwrap();
    slot.accept(loaded(&older, "first"), Some(&x));
    let newer = slot.request(LogPane::Detail, x.clone(), true).unwrap();
    assert!(slot.accept(loaded(&newer, "newer"), Some(&x)));
    assert!(!slot.accept(loaded(&older, "older"), Some(&x)));
    assert_eq!(
        slot.entry().unwrap().result.as_ref().unwrap().lines,
        ["newer"]
    );

    // After clear, a read still in flight is dropped on arrival.
    let pending = slot
        .request(LogPane::Detail, key("s4", "/d", 40), true)
        .unwrap();
    slot.clear();
    assert!(!slot.accept(loaded(&pending, "gone"), Some(&pending.key.clone())));
    assert!(slot.entry().is_none());
}
