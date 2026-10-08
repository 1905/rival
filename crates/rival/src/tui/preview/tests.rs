use chrono::{NaiveDate, TimeDelta, TimeZone};

use super::*;
use crate::tui::jobs::JobEnv;
use crate::tui::logview::load_log;
use crate::tui::styles::STYLES;
use crate::tui::testkit::{Harness, fixed_now, fixed_zone, harness, reads};
use crate::tui::text::{line_width, width};

fn ctx() -> Ctx<'static> {
    Ctx {
        now: fixed_now(),
        zone: fixed_zone,
        styles: &STYLES,
    }
}

/// Go `previewPane.refresh`: asks for the tail, runs the read on this thread
/// and takes the result.
fn refresh(p: &mut PreviewPane, env: &JobEnv, item: &DisplayItem, w: usize, h: usize, force: bool) {
    let ctx = ctx();
    if let Some(req) = p.request(Some(item), w, h, &ctx, force) {
        let res = load_log(req, env.read_tail);
        p.accept(res, Some(item), w, h, &ctx);
    }
}

/// Go `previewLines`: the pane's rows, checked to be exactly `h` rows of
/// exactly `w` cells.
fn view(p: &PreviewPane, item: &DisplayItem, w: u16, h: u16) -> Vec<String> {
    let area = Rect::new(0, 0, w, h);
    let mut buf = Buffer::empty(area);
    p.render(Some(item), area, &mut buf, &ctx());
    let rows = crate::tui::testkit::rows(&buf);
    assert_eq!(rows.len(), usize::from(h));
    for (i, r) in rows.iter().enumerate() {
        assert_eq!(width(r), usize::from(w), "row {i} {r:?}");
    }
    rows
}

fn solo(s: Session) -> DisplayItem {
    DisplayItem {
        sessions: vec![Arc::new(s)],
    }
}

fn single_run(h: &Harness) -> DisplayItem {
    let mut log = String::new();
    for i in 0..100 {
        log.push_str("log line ");
        log.push_str(&"x".repeat(i % 7));
        log.push('\n');
    }
    log.push_str("VERDICT: 6/10\n");
    let start = fixed_now() - TimeDelta::minutes(20);
    solo(Session {
        id: "a1b2c3d4-single".into(),
        cli: "codex".into(),
        model: "gpt-6-astra".into(),
        mode: "review".into(),
        effort: "xhigh".into(),
        status: "completed".into(),
        duration: "1m31s".into(),
        start_time: Some(start),
        pid: 81233,
        work_dir: "/src/orbit-web".into(),
        review_scope: "plans/2026-09-26-service-identity".into(),
        log_file: h.log("single.log", &log),
        ..Session::default()
    })
}

// Go: TestPreviewSingleRun.
#[test]
fn preview_single_run() {
    let h = harness();
    let item = single_run(&h);
    let mut p = PreviewPane::default();
    refresh(&mut p, &h.env, &item, 60, 20, false);
    let rows = view(&p, &item, 60, 20);
    let text = rows.join("\n");
    for want in [
        "orbit-web · review · codex",
        "gpt-6-astra · xhigh · 1m31s",
        "started 11:40 · pid 81233",
        "─ scope ─",
        "plans/2026-09-26-service-identity",
        "─ output (tail) ─",
        "VERDICT: 6/10",
    ] {
        assert!(text.contains(want), "preview lacks {want:?}:\n{text}");
    }
    assert!(text.find("VERDICT").unwrap() > text.find("output (tail)").unwrap());
    // The pane shows the END of the log: the last line sits on the last row.
    assert!(
        rows.last().unwrap().starts_with("VERDICT: 6/10"),
        "{rows:?}"
    );
    // Line 99 carries 99 % 7 = 1 x.
    assert_eq!(rows[rows.len() - 2].trim_end(), "log line x", "{rows:?}");
}

// Go: TestPreviewGroupListsEveryMember.
#[test]
fn preview_group_lists_every_member() {
    let h = harness();
    let now = fixed_now();
    let m = |id: &str, cli: &str, model: &str, mode: &str, status: &str, body: &str| {
        Arc::new(Session {
            id: id.into(),
            group_id: "g-1".into(),
            cli: cli.into(),
            model: model.into(),
            mode: mode.into(),
            status: status.into(),
            start_time: Some(now),
            work_dir: "/src/mathquest".into(),
            log_file: h.log(&format!("{id}.log"), body),
            ..Session::default()
        })
    };
    let item = DisplayItem {
        sessions: vec![
            m(
                "m1",
                "codex",
                "gpt-5.5",
                "review",
                "completed",
                "reviewer one output\n",
            ),
            m(
                "m2",
                "opencode",
                "gemini-3.1",
                "review",
                "failed",
                "reviewer two output\n",
            ),
            m(
                "m3",
                "claude",
                "claude-opus-5-5",
                "consilium",
                "completed",
                "JUDGE VERDICT\n",
            ),
        ],
    };
    let mut p = PreviewPane::default();
    refresh(&mut p, &h.env, &item, 70, 24, false);
    let rows = view(&p, &item, 70, 24);
    for want in [
        "✓ gpt-5.5  completed",
        "✗ gemini-3.1  failed",
        "✓ claude-opus-5-5  completed · judge",
    ] {
        assert!(
            rows.iter().any(|r| r.starts_with(want)),
            "no member line {want:?}:\n{}",
            rows.join("\n")
        );
    }
    let text = rows.join("\n");
    assert!(text.contains("mathquest · mega · 3 models"), "{text}");
    assert!(
        text.contains("JUDGE VERDICT"),
        "group preview does not tail the judge:\n{text}"
    );
    assert!(
        !text.contains("reviewer one output"),
        "tails a reviewer:\n{text}"
    );
}

// Go: TestPreviewGroupWithoutJudgeTailsLastMember.
#[test]
fn preview_group_without_judge_tails_last_member() {
    let h = harness();
    let m = |id: &str, body: &str| {
        Arc::new(Session {
            id: id.into(),
            group_id: "g-2".into(),
            cli: "codex".into(),
            mode: "plan".into(),
            status: "completed".into(),
            log_file: h.log(&format!("{id}.log"), body),
            ..Session::default()
        })
    };
    let item = DisplayItem {
        sessions: vec![m("p1", "first plan\n"), m("p2", "second plan\n")],
    };
    let mut p = PreviewPane::default();
    refresh(&mut p, &h.env, &item, 60, 20, false);
    let text = view(&p, &item, 60, 20).join("\n");
    assert!(
        text.contains("second plan") && !text.contains("first plan"),
        "{text}"
    );
}

// Go: TestPreviewMissingLog.
#[test]
fn preview_missing_log() {
    let h = harness();
    let item = solo(Session {
        id: "missing".into(),
        cli: "codex".into(),
        model: "gpt-5.5".into(),
        status: "completed".into(),
        log_file: h
            .home
            .path()
            .join("nope.log")
            .to_string_lossy()
            .into_owned(),
        ..Session::default()
    });
    let mut p = PreviewPane::default();
    refresh(&mut p, &h.env, &item, 60, 16, false);
    let text = view(&p, &item, 60, 16).join("\n");
    assert!(text.contains("(log unavailable: open "), "{text}");
    assert!(text.contains('…'), "the error is cut to the width: {text}");
}

// Go: TestPreviewLinesFitEveryWidth.
#[test]
fn preview_lines_fit_every_width() {
    let h = harness();
    let nasty = "\tfunc main() {\n\t\tfmt.Println(\"日本語のテキストはとても幅が広いですね、これは長い行です\")\n\x1b[31m\terror: something went terribly wrong in a very long line that must wrap\x1b[0m\n\t}\n";
    let item = solo(Session {
        id: "wide".into(),
        cli: "codex".into(),
        model: "claude-opus-4-6[1m]".into(),
        mode: "review".into(),
        effort: "ultra".into(),
        status: "running".into(),
        start_time: Some(fixed_now()),
        pid: 1,
        work_dir: "/src/日本語のプロジェクト名前はとても長い".into(),
        review_scope: "scope/path/that/is/long ".repeat(20),
        log_file: h.log("wide.log", &format!("{nasty}{}", "padding\n".repeat(50))),
        ..Session::default()
    });
    for w in [10u16, 30, 53, 89] {
        for ht in [1u16, 5, 30] {
            let mut p = PreviewPane::default();
            refresh(
                &mut p,
                &h.env,
                &item,
                usize::from(w),
                usize::from(ht),
                false,
            );
            view(&p, &item, w, ht);
        }
    }
}

// Go: TestPreviewRefreshSkipsRereadForFinishedRun.
#[test]
fn preview_refresh_skips_reread_for_unchanged_log() {
    let h = harness();
    let path = h.log("done.log", "finished\n");
    let mut s = Session {
        id: "done".into(),
        cli: "codex".into(),
        model: "gpt-5.5".into(),
        status: "completed".into(),
        log_file: path.clone(),
        ..Session::default()
    };
    let item = solo(s.clone());
    let mut p = PreviewPane::default();
    refresh(&mut p, &h.env, &item, 60, 20, true);
    refresh(&mut p, &h.env, &item, 60, 20, true);
    assert_eq!(reads(), 1, "an unchanged log is read once");
    // A resize has to re-wrap, so it re-reads.
    refresh(&mut p, &h.env, &item, 70, 20, true);
    assert_eq!(reads(), 2);
    // A running run re-reads once its log grows, and not while it is idle.
    s.status = "running".into();
    let item = solo(s);
    refresh(&mut p, &h.env, &item, 70, 20, true);
    assert_eq!(reads(), 2);
    std::fs::write(&path, "finished\nmore output\n").unwrap();
    refresh(&mut p, &h.env, &item, 70, 20, true);
    assert_eq!(reads(), 3);
    assert!(view(&p, &item, 70, 20).join("\n").contains("more output"));
}

/// Before its first read lands the pane says so instead of showing nothing,
/// and a read for the previous selection is never shown under a new one.
#[test]
fn a_late_tail_for_another_run_is_dropped() {
    let h = harness();
    let a = solo(Session {
        id: "aaaa".into(),
        status: "completed".into(),
        log_file: h.log("a.log", "A-OUTPUT\n"),
        ..Session::default()
    });
    let b = solo(Session {
        id: "bbbb".into(),
        status: "completed".into(),
        log_file: h.log("b.log", "B-OUTPUT\n"),
        ..Session::default()
    });
    let ctx = ctx();
    let mut p = PreviewPane::default();
    let req_a = p.request(Some(&a), 60, 20, &ctx, false).unwrap();
    // The cursor moves to b before a's read lands.
    let req_b = p.request(Some(&b), 60, 20, &ctx, false).unwrap();
    assert!(view(&p, &b, 60, 20).join("\n").contains(LOADING_LOG));
    assert!(!p.accept(load_log(req_a, h.env.read_tail), Some(&b), 60, 20, &ctx));
    assert!(!view(&p, &b, 60, 20).join("\n").contains("A-OUTPUT"));
    assert!(p.accept(load_log(req_b, h.env.read_tail), Some(&b), 60, 20, &ctx));
    assert!(view(&p, &b, 60, 20).join("\n").contains("B-OUTPUT"));
}

/// The tail is cut to at most 200 source lines, and to the rows left under
/// the meta block.
#[test]
fn tail_key_caps_the_lines() {
    let item = solo(Session {
        id: "x".into(),
        log_file: "/x.log".into(),
        ..Session::default()
    });
    let ctx = ctx();
    // Solo meta is 3 lines plus the heading.
    let key = PreviewPane::tail_key(Some(&item), 40, 10, &ctx).unwrap();
    assert_eq!((key.width, key.last_n), (40, 6));
    let key = PreviewPane::tail_key(Some(&item), 40, 500, &ctx).unwrap();
    assert_eq!(key.last_n, PREVIEW_TAIL_LINES);
    assert_eq!(
        PreviewPane::tail_key(Some(&item), 40, 4, &ctx),
        None,
        "no row left"
    );
    assert_eq!(PreviewPane::tail_key(None, 40, 10, &ctx), None);
}

#[test]
fn started_line_dates_other_days_in_the_zone() {
    let now = fixed_now();
    assert_eq!(started_line(None, 0, now, fixed_zone), "started -");
    assert_eq!(
        started_line(Some(now - TimeDelta::minutes(20)), 81233, now, fixed_zone),
        "started 11:40 · pid 81233"
    );
    assert_eq!(
        started_line(Some(now - TimeDelta::days(2)), 0, now, fixed_zone),
        "started Oct 01 12:00"
    );
    // A UTC start time shows in the local zone.
    let utc = chrono::Utc
        .from_utc_datetime(
            &NaiveDate::from_ymd_opt(2026, 10, 3)
                .unwrap()
                .and_hms_opt(8, 5, 0)
                .unwrap(),
        )
        .fixed_offset();
    assert_eq!(started_line(Some(utc), 0, now, fixed_zone), "started 10:05");
}

/// The zone is looked up per instant, so a start before a DST switch shows
/// its own offset, not today's.
#[test]
fn started_line_uses_the_offset_of_its_own_instant() {
    // A fake zone: +01:00 before 2026-10-01 00:00 UTC, +02:00 after.
    fn switching(utc: chrono::NaiveDateTime) -> FixedOffset {
        let switch = NaiveDate::from_ymd_opt(2026, 10, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        FixedOffset::east_opt(if utc < switch { 3600 } else { 7200 }).unwrap()
    }
    let now = fixed_now();
    let before = chrono::Utc
        .from_utc_datetime(
            &NaiveDate::from_ymd_opt(2026, 9, 20)
                .unwrap()
                .and_hms_opt(9, 0, 0)
                .unwrap(),
        )
        .fixed_offset();
    assert_eq!(
        started_line(Some(before), 0, now, switching),
        "started Sep 20 10:00"
    );
}

#[test]
fn join_meta_skips_empty_parts() {
    let ctx = ctx();
    let line = join_meta(
        vec![
            Span::raw("a"),
            Span::raw(""),
            Span::styled("b", STYLES.value),
        ],
        &ctx,
    );
    assert_eq!(line.to_string(), "a · b");
    assert_eq!(line.spans[1].style, STYLES.dim);
    assert_eq!(line_width(&join_meta(Vec::new(), &ctx)), 0);
    assert_eq!(one_line("  a \n\t b  "), "a b");
}
