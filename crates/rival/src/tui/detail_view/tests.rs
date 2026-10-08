use chrono::{DateTime, TimeDelta};

use super::*;
use crate::tui::styles::STYLES;
use crate::tui::testkit::{fixed_now, fixed_zone};
use crate::tui::text::width;

fn ctx() -> Ctx<'static> {
    Ctx {
        now: fixed_now(),
        zone: fixed_zone,
        styles: &STYLES,
    }
}

fn texts(lines: &[Line<'static>]) -> String {
    lines
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn find_matches_ignores_case_and_styling() {
    let red = Style::new().fg(ratatui::style::Color::Red);
    let lines = vec![
        Line::raw("alpha"),
        Line::raw("Fingerprint here"),
        Line::raw("none"),
        Line::from(vec![Span::styled("FINGER", red), Span::raw("print")]),
        Line::raw("fingerprint again"),
    ];
    assert_eq!(find_matches(&lines, "fingerPRINT"), [1, 3, 4]);
    assert!(find_matches(&lines, "").is_empty(), "empty query matched");
    // Lowercasing is per char: İ matches i.
    assert_eq!(find_matches(&[Line::raw("İstanbul")], "istanbul"), [0]);
}

// Every search hit gets the match style; the line keeps its text and width,
// and the rest keeps its style.
#[test]
fn highlight_line_paints_hits_only() {
    let red = Style::new().fg(ratatui::style::Color::Red);
    let line = Line::from(vec![
        Span::styled("a Finger", red),
        Span::raw("print, fingerprint 日本"),
    ]);
    let hl = highlight_line(&line, "fingerprint", STYLES.matched);
    assert_eq!(hl.to_string(), line.to_string());
    assert_eq!(line_width(&hl), line_width(&line));
    let painted: String = hl
        .spans
        .iter()
        .filter(|s| s.style == STYLES.matched)
        .map(|s| s.content.as_ref())
        .collect();
    assert_eq!(painted, "Fingerprintfingerprint");
    assert_eq!(hl.spans[0].content, "a ");
    assert_eq!(
        hl.spans[0].style, red,
        "text before the hit keeps its style"
    );
    assert_eq!(highlight_line(&line, "", STYLES.matched), line);
    // Non-overlapping, left to right: "aaa" holds one "aa".
    let hl = highlight_line(&Line::raw("aaa"), "aa", STYLES.matched);
    assert_eq!(hl.spans[0].content, "aa");
    assert_eq!(hl.spans[1].content, "a");
}

#[test]
fn info_lines_list_every_field() {
    let start = DateTime::parse_from_rfc3339("2026-09-26T11:40:00+02:00").unwrap();
    let long_err = format!("provider exploded: {}TAIL_OF_ERROR", "detail ".repeat(40));
    let s = Session {
        id: "abcdef01-2345".into(),
        group_id: "group123".into(),
        cli: "codex".into(),
        model: "gpt-6-astra".into(),
        effort: "xhigh".into(),
        mode: "review".into(),
        status: "failed".into(),
        exit_code: Some(1),
        start_time: Some(start),
        end_time: Some(start + TimeDelta::seconds(90)),
        duration: "1m30s".into(),
        queued_at: Some(start - TimeDelta::minutes(1)),
        work_dir: "/src/orbit-web".into(),
        review_scope: "plans/x.md".into(),
        account: "work".into(),
        pid: 81233,
        output_bytes: 2048,
        output_lines: 17,
        log_file: "/tmp/x.log".into(),
        error_msg: long_err,
        ..Session::default()
    };
    let lines = info_lines(&s, 80, &ctx());
    let got = texts(&lines);
    for want in [
        "id          abcdef01-2345",
        "group id    group123",
        "cli         codex",
        "model       gpt-6-astra",
        "effort      xhigh",
        "mode        review",
        "status      failed",
        "exit        1",
        "started     2026-09-26 11:40:00",
        "ended       2026-09-26 11:41:30",
        "duration    1m30s",
        "queued at   2026-09-26 11:39:00",
        "workdir     /src/orbit-web",
        "scope       plans/x.md",
        "account     work",
        "pid         81233",
        "output      2048 bytes, 17 lines",
        "log         /tmp/x.log",
        "error:",
        "TAIL_OF_ERROR",
    ] {
        assert!(got.contains(want), "info lacks {want:?}:\n{got}");
    }
    for l in &lines {
        assert!(line_width(l) <= 80, "{l}");
    }
    // Empty fields read "-", and a long value wraps under its label.
    let bare = Session {
        id: "x".into(),
        work_dir: format!("/{}", "deep/".repeat(30)),
        ..Session::default()
    };
    let lines = info_lines(&bare, 40, &ctx());
    let got = texts(&lines);
    assert!(got.contains("group id    -"), "{got}");
    assert!(
        got.contains("started     -"),
        "no start time is empty: {got}"
    );
    let workdir = lines
        .iter()
        .position(|l| l.to_string().starts_with("workdir"))
        .unwrap();
    // A path has no break point, so the hard wrap cuts it at the width.
    assert_eq!(
        lines[workdir].to_string(),
        "workdir     /deep/deep/deep/deep/deep/de"
    );
    assert_eq!(
        lines[workdir + 1].to_string(),
        "            ep/deep/deep/deep/deep/deep/"
    );
    assert_eq!(lines[0].spans[0].style, STYLES.dim, "labels are dim");
    assert_eq!(lines[0].spans[2].style, STYLES.value, "the id is a value");
}

#[test]
fn prompt_lines_fall_back_to_the_preview() {
    let s = Session {
        id: "p1".into(),
        prompt_preview: "PREVIEW-ONLY".into(),
        ..Session::default()
    };
    let mut prompts = HashMap::new();
    let got = texts(&prompt_lines(&s, &prompts, false, 40, &STYLES));
    assert_eq!(got, "PREVIEW-ONLY\n(full prompt unavailable)");
    let got = texts(&prompt_lines(&s, &prompts, true, 40, &STYLES));
    assert_eq!(got, "PREVIEW-ONLY\n(loading full prompt…)");
    let empty = Session {
        id: "p2".into(),
        ..Session::default()
    };
    assert_eq!(
        texts(&prompt_lines(&empty, &prompts, false, 40, &STYLES)),
        "(full prompt unavailable)"
    );
    prompts.insert("p1".into(), format!("FULL\t{}THE-END", "word ".repeat(30)));
    let lines = prompt_lines(&s, &prompts, false, 40, &STYLES);
    let got = texts(&lines);
    assert!(
        got.starts_with("FULL    word") && got.ends_with("THE-END"),
        "{got}"
    );
    assert!(lines.iter().all(|l| line_width(l) <= 40));
}

#[test]
fn output_lines_show_the_log_state_and_the_error() {
    let s = Session {
        id: "o1".into(),
        log_file: "/x.log".into(),
        status: "failed".into(),
        error_msg: "boom\tbad".into(),
        ..Session::default()
    };
    let slot = LogSlot::default();
    assert_eq!(
        texts(&output_lines(&s, 40, &slot, &STYLES)),
        format!("{LOADING_LOG}\n\nerror:\nboom    bad")
    );
}

#[test]
fn join_ends_cuts_the_left_first() {
    let l = Line::raw("left side text");
    let r = Line::raw("RIGHT");
    assert_eq!(
        join_ends(l.clone(), r.clone(), 25).to_string(),
        "left side text      RIGHT"
    );
    assert_eq!(
        join_ends(l.clone(), r.clone(), 12).to_string(),
        "left s…RIGHT"
    );
    assert_eq!(join_ends(l, r, 4).to_string(), "RIGH");
}

#[test]
fn member_label_names_the_judge() {
    let judge = Session {
        mode: "consilium".into(),
        model: "gpt-6-astra".into(),
        ..Session::default()
    };
    assert_eq!(member_label(&judge), "judge");
    let reviewer = Session {
        cli: "codex".into(),
        ..Session::default()
    };
    assert_eq!(member_label(&reviewer), "codex", "no model: the CLI");
}

#[test]
fn vp_height_keeps_one_row() {
    assert_eq!(vp_height(20), 15);
    assert_eq!(vp_height(5), 1);
    assert_eq!(vp_height(0), 1);
}

#[test]
fn status_line_priorities() {
    let mut d = DetailPane::default();
    d.vp.set_width(40);
    d.vp.set_height(5);
    d.lines = (0..12).map(|i| Line::raw(format!("l{i}"))).collect();
    d.apply_content(&STYLES);
    d.vp.set_y_offset(2);
    assert_eq!(d.status_line(40, &STYLES).to_string(), " lines 3-7 of 12");
    d.notice = "nothing running".into();
    assert_eq!(d.status_line(40, &STYLES).to_string(), " nothing running");
    d.run_search("l1", &STYLES);
    assert_eq!(
        d.status_line(60, &STYLES).to_string(),
        " /l1  1/3 matches · n next · N prev · esc clear"
    );
    d.search.focus();
    assert!(d.status_line(40, &STYLES).to_string().starts_with(" / "));
    d.confirm = Some(KillConfirm {
        targets: vec![Arc::new(Session::default())],
    });
    assert_eq!(
        d.status_line(40, &STYLES).to_string(),
        " stop 1 running session? y/n"
    );
    assert_eq!(width(&d.status_line(10, &STYLES).to_string()), 10);
}
