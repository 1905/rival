//! Go `viewport_test.go` and `detail_view_test.go` at the model level, the
//! stop flow, and the detail and preview golden frames. Every log read,
//! prompt load and stop runs through `testkit::drive`, the way the runtime's
//! workers will; processes are always the testkit fakes.

use std::sync::Arc;

use chrono::TimeDelta;
use rival_core::session::{NewSession, Session, sort_group_members};
use rival_core::sessionview::SessionEvent;

use super::*;
use crate::tui::jobs::JobEnv;
use crate::tui::testkit::{
    Harness, assert_golden, draw, drive, drive_with, fixed_now, frame_text, golden_frame,
    golden_path, group_fixture, harness, job_model, key, launched, open_detail,
    preview_fixture_logs, reads, rows, run, set_alive, set_signal_fails, signals, type_text,
};

fn sessions(list: Vec<Arc<Session>>) -> Msg {
    Msg::Sessions(SessionEvent { sessions: list })
}

fn resize(width: u16, height: u16) -> Msg {
    Msg::Resize { width, height }
}

fn keys(names: &[&str]) -> Vec<Msg> {
    names.iter().map(|k| key(k)).collect()
}

fn selected_id(m: &Model) -> String {
    m.list
        .selected()
        .and_then(DisplayItem::primary)
        .map(|s| s.id.clone())
        .unwrap_or_default()
}

/// The frame drawn on a terminal larger than the layout: nothing may land
/// outside `lay.width`×`lay.height`, the help bar holds the last row, and
/// every row fits the width.
fn assert_fits(m: &Model, label: &str) {
    let (w, h) = (m.lay.width, m.lay.height);
    let buf = draw(m, (w + 20) as u16, (h + 6) as u16);
    for y in 0..h + 6 {
        for x in 0..w + 20 {
            if x >= w || y >= h {
                assert_eq!(
                    buf[(x as u16, y as u16)].symbol(),
                    " ",
                    "{label}: cell {x},{y} is outside the {w}×{h} frame"
                );
            }
        }
    }
    let text = rows(&buf);
    let help = m.help_view()[0].to_string();
    assert_eq!(
        text[h - m.help_h][..]
            .trim_end()
            .chars()
            .take(w)
            .collect::<String>(),
        help.trim_end(),
        "{label}: the help bar is not on the last rows"
    );
}

/// The rows of the frame at its own size.
fn frame_rows(m: &Model) -> Vec<String> {
    frame_text(m).split('\n').map(str::to_string).collect()
}

/// Go `visible`: whether content line `line` is on screen.
fn visible(m: &Model, line: usize) -> bool {
    let off = m.detail.vp.y_offset();
    line >= off && line < off + m.detail.vp.height()
}

fn append(path: &str, line: &str) {
    use std::io::Write as _;
    let mut f = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    writeln!(f, "{line}").unwrap();
}

/// Go `newTestModel`: one running run with a long prompt and `log`.
fn width_test_run(h: &Harness, log: &str) -> Vec<Arc<Session>> {
    vec![Arc::new(Session {
        prompt: "review this repository carefully ".repeat(40),
        log_file: h.log("widthtest.log", log),
        ..run(
            "widthtest",
            "codex",
            rival_core::config::GPT56_SOL_MODEL,
            "review",
            "ultra",
            "running",
            fixed_now(),
            "",
        )
    })]
}

/// Go `nastyLog`: tabs, CJK and ANSI escapes.
const NASTY_LOG: &str = "\tfunc main() {\n\t\tfmt.Println(\"日本語のテキストはとても幅が広いですね、これは長い行です\")\n\x1b[31m\terror: something went terribly wrong in a very long line that must wrap\x1b[0m\n\t}\n";

// --- Go viewport_test.go --------------------------------------------------------

// Go: TestViewNeverExceedsWidth. Rust tab keys: 3 Prompt, 4 Info, 2 Raw.
#[test]
fn view_never_exceeds_width() {
    let h = harness();
    let body = format!(
        "{NASTY_LOG}{}",
        "padding line to make the log long enough to scroll\n".repeat(200)
    );
    for w in [60u16, 90, 120, 200] {
        let mut m = job_model(&h.env, width_test_run(&h, &body), w, 24);
        assert_fits(&m, "list mode");
        drive(&mut m, &h.env, [key("?")]);
        assert_fits(&m, "list mode, full help");
        drive(&mut m, &h.env, [key("enter")]);
        assert_eq!(m.mode, Mode::Detail, "width {w}");
        assert_fits(&m, "detail mode");
        let raw = frame_text(&m);
        assert!(
            raw.contains("padding line"),
            "width {w}: the tail is not on screen:\n{raw}"
        );
        for (k, want) in [
            ("3", "review this repository"),
            ("4", "widthtest"),
            ("2", "padding line"),
            ("/", "/ search"),
        ] {
            drive(&mut m, &h.env, [key(k)]);
            assert_fits(&m, &format!("width {w}, detail key {k}"));
            let text = frame_text(&m);
            assert!(
                text.contains(want),
                "width {w}, key {k}: no {want:?}:\n{text}"
            );
        }
    }
}

// Go: TestDetailFrameFitsTheTerminal.
#[test]
fn detail_frame_fits_the_terminal() {
    let h = harness();
    for w in [200u16, 120, 90, 60] {
        for ht in [50u16, 29, 16] {
            for fixture in [preview_fixture_logs(&h), group_fixture(&h)] {
                let mut m = open_detail(&h.env, fixture, w, ht);
                for step in ["", "?", "3", "4", "x"] {
                    if !step.is_empty() {
                        drive(&mut m, &h.env, [key(step)]);
                    }
                    let label = format!("{w}×{ht} after {step:?}");
                    assert_fits(&m, &label);
                    assert_eq!(
                        m.lay.header_h + m.content_height() + m.help_h,
                        usize::from(ht),
                        "{label}: rows do not add up"
                    );
                }
            }
        }
    }
}

// Go: TestEnterEscKeepsTheCursor.
#[test]
fn enter_esc_keeps_the_cursor() {
    let h = harness();
    let mut m = job_model(&h.env, preview_fixture_logs(&h), 120, 40);
    drive(&mut m, &h.env, [key("j")]);
    let want = selected_id(&m);
    drive(&mut m, &h.env, [key("enter")]);
    assert_eq!(m.mode, Mode::Detail);
    drive(&mut m, &h.env, [key("esc")]);
    assert_eq!((m.mode, selected_id(&m)), (Mode::List, want));
}

// Go: TestDetailScrollKeysReachTheViewport.
#[test]
fn detail_scroll_keys_reach_the_viewport() {
    let h = harness();
    let mut m = open_detail(
        &h.env,
        width_test_run(&h, &"scrollable line\n".repeat(300)),
        80,
        24,
    );
    let before = m.detail.vp.y_offset();
    let cursor = m.list.cursor;
    drive(&mut m, &h.env, [key("k")]);
    assert!(
        m.detail.vp.y_offset() < before,
        "k did not scroll the log up"
    );
    assert_eq!(
        m.list.cursor, cursor,
        "k moved the list cursor in detail mode"
    );
    // g jumps to the top of the log, not the top of the list.
    drive(&mut m, &h.env, [key("g")]);
    assert!(m.detail.vp.at_top());
    assert!(!m.detail.follow, "g on a long log leaves the tail");
    assert!(frame_rows(&m)[m.lay.header_h + 3].starts_with("scrollable line"));
}

// Go: TestDetailSelectionSurvivesReorder. A queued run starting while a
// detail view is open re-sorts the list; the view follows the session, not
// the index.
#[test]
fn detail_selection_survives_reorder() {
    let h = harness();
    let now = fixed_now();
    let watched = Arc::new(Session {
        pid: 4242,
        log_file: h.log("watched.log", &"watched output\n".repeat(50)),
        ..run(
            "11111111-1111-1111-1111-111111111111",
            "codex",
            rival_core::config::GPT56_SOL_MODEL,
            "review",
            "",
            "running",
            now - TimeDelta::minutes(1),
            "",
        )
    });
    let mut other = Session {
        pid: 9999,
        log_file: h.log("other.log", &"other output\n".repeat(50)),
        ..run(
            "22222222-2222-2222-2222-222222222222",
            "claude",
            rival_core::config::CLAUDE_MODEL,
            "review",
            "",
            "queued",
            now - TimeDelta::minutes(2),
            "",
        )
    };
    let mut m = open_detail(
        &h.env,
        vec![watched.clone(), Arc::new(other.clone())],
        80,
        24,
    );
    assert_eq!(selected_id(&m), watched.id);
    other.status = "running".into();
    other.start_time = now;
    drive(
        &mut m,
        &h.env,
        [sessions(vec![Arc::new(other), watched.clone()])],
    );
    assert_eq!(
        m.mode,
        Mode::Detail,
        "reorder dropped the user out of the detail view"
    );
    assert_eq!(selected_id(&m), watched.id);
    assert_eq!(
        m.list.selected().unwrap().primary().unwrap().pid,
        4242,
        "x would target another pid"
    );
    assert!(frame_text(&m).contains("watched output"));
}

// Go: TestDetailExitsWhenSelectionDisappears.
#[test]
fn detail_exits_when_selection_disappears() {
    let h = harness();
    let now = fixed_now();
    let gone = Arc::new(Session {
        log_file: h.log("gone.log", "some output\n"),
        ..run(
            "33333333-3333",
            "codex",
            "gpt-5.5",
            "review",
            "",
            "running",
            now,
            "",
        )
    });
    let survivor = Arc::new(Session {
        log_file: h.log("survivor.log", "other output\n"),
        ..run(
            "44444444-4444",
            "codex",
            "gpt-5.5",
            "review",
            "",
            "running",
            now - TimeDelta::minutes(1),
            "",
        )
    });
    let mut m = open_detail(&h.env, vec![gone, survivor.clone()], 80, 24);
    drive(&mut m, &h.env, [sessions(vec![survivor])]);
    assert_eq!(
        m.mode,
        Mode::List,
        "a vanished run left the user in someone else's log"
    );
}

// Go: TestWideFrameShowsListAndPreviewSideBySide.
#[test]
fn wide_frame_shows_list_and_preview_side_by_side() {
    let h = harness();
    let m = job_model(&h.env, preview_fixture_logs(&h), 200, 50);
    assert_fits(&m, "wide list + preview");
    let rows = frame_rows(&m);
    assert_eq!(rows.len(), 50);
    let body_top = m.lay.header_h + 1;
    let top: Vec<char> = rows[body_top].chars().collect();
    assert_eq!(top.len(), 200);
    assert_eq!((top[0], top[m.lay.list_w - 1]), ('╭', '╮'));
    assert_eq!(
        top[m.lay.list_w + 1],
        '╭',
        "the preview starts after the 1-col gap"
    );
    assert_eq!(top[199], '╮');
    assert!(
        rows[body_top + 3].contains("gpt-6-astra"),
        "{}",
        rows[body_top + 3]
    );
    // The preview text starts after the border and its 1-col pad.
    let col = m.lay.list_w + 3;
    let preview: Vec<String> = rows[body_top + 1..]
        .iter()
        .map(|r| r.chars().skip(col).collect())
        .collect();
    assert!(
        preview[0].starts_with("orbit-web · review · codex"),
        "{preview:?}"
    );
    assert!(
        preview[1].starts_with("gpt-6-astra · xhigh · 1m0s"),
        "{preview:?}"
    );
    assert!(
        preview[2].starts_with("started 11:59 · pid 101"),
        "{preview:?}"
    );
    assert!(preview[3].starts_with("─ output (tail) ─"), "{preview:?}");
    assert!(preview[4].starts_with("LIVE-RUN-OUTPUT"), "{preview:?}");
}

// Go: TestNarrowFrameHasNoPreview.
#[test]
fn narrow_frame_has_no_preview() {
    let h = harness();
    let m = job_model(&h.env, preview_fixture_logs(&h), 100, 40);
    assert_fits(&m, "narrow");
    let text = frame_text(&m);
    assert!(
        !text.contains("output (tail)") && !text.contains('╭'),
        "{text}"
    );
    assert_eq!(frame_rows(&m).len(), 40);
    assert_eq!(reads(), 0, "no preview, no read");
}

// Go: TestTooSmallFrameIsTheNoticeOnly.
#[test]
fn too_small_frame_is_the_notice_only() {
    let h = harness();
    let m = job_model(&h.env, preview_fixture_logs(&h), 50, 20);
    let rows = frame_rows(&m);
    assert_eq!(rows[0].trim_end(), "terminal too small (need 60×16)");
    assert!(rows[1..].iter().all(|r| r.trim().is_empty()));
    assert_eq!(reads(), 0);
}

// Go: TestMovingTheCursorRefreshesThePreview.
#[test]
fn moving_the_cursor_refreshes_the_preview() {
    let h = harness();
    let mut m = job_model(&h.env, preview_fixture_logs(&h), 200, 50);
    assert!(frame_text(&m).contains("LIVE-RUN-OUTPUT"));
    drive(&mut m, &h.env, [key("j")]);
    let text = frame_text(&m);
    assert!(
        text.contains("DONE-RUN-OUTPUT") && !text.contains("LIVE-RUN-OUTPUT"),
        "{text}"
    );
    drive(&mut m, &h.env, [key("G")]);
    assert!(frame_text(&m).contains("FAIL-RUN-OUTPUT"));
}

// Go: TestTickRereadsThePreviewOnlyWhenTheLogChanged.
#[test]
fn tick_rereads_the_preview_only_when_the_log_changed() {
    let h = harness();
    let list = preview_fixture_logs(&h);
    let mut m = job_model(&h.env, list.clone(), 200, 50);
    // The selected run is running and its log is idle: the tick is free.
    let before = reads();
    drive(&mut m, &h.env, [Msg::Tick]);
    assert_eq!(reads(), before, "tick on an idle running run");
    // Its log grows: the next tick re-reads it once.
    append(&list[0].log_file, "LIVE-MORE");
    drive(&mut m, &h.env, [Msg::Tick]);
    assert_eq!(reads(), before + 1);
    assert!(frame_text(&m).contains("LIVE-MORE"));
    // A finished run selected: ticks (kept alive by the running run) are
    // free once its log is read.
    drive(&mut m, &h.env, [key("j")]);
    let before = reads();
    drive(&mut m, &h.env, [Msg::Tick, Msg::Tick]);
    assert_eq!(reads(), before, "ticks on a finished run re-read its log");
    // Drawing never reads.
    draw(&m, 200, 50);
    assert_eq!(reads(), before);
}

// Go: TestSessionEventRefreshesAFinishedPreview.
#[test]
fn session_event_refreshes_a_finished_preview() {
    let h = harness();
    let list = preview_fixture_logs(&h);
    let mut m = job_model(&h.env, list.clone(), 200, 50);
    drive(&mut m, &h.env, [key("j")]);
    std::fs::write(&list[1].log_file, "REWRITTEN\n").unwrap();
    drive(&mut m, &h.env, [sessions(list)]);
    assert!(frame_text(&m).contains("REWRITTEN"));
}

// --- Go detail_view_test.go -------------------------------------------------------

// Go: TestDetailOpenDefaults.
#[test]
fn detail_open_defaults() {
    let h = harness();
    let m = open_detail(&h.env, group_fixture(&h), 120, 40);
    let item = m.list.selected();
    assert_eq!(m.detail.tab, DetailTab::Raw);
    assert_eq!(m.detail.member_index(item.unwrap()), 0);
    assert!(m.detail.follow);
}

// Go: TestDetailTabKeys, with Rust's numbers and the Result tab.
#[test]
fn detail_tab_keys() {
    let h = harness();
    let mut m = open_detail(&h.env, preview_fixture_logs(&h), 120, 40);
    for (k, want) in [
        ("3", DetailTab::Prompt),
        ("4", DetailTab::Info),
        ("2", DetailTab::Raw),
        ("tab", DetailTab::Prompt),
        ("tab", DetailTab::Info),
        ("tab", DetailTab::Result),
        ("tab", DetailTab::Raw),
        ("shift+tab", DetailTab::Result),
        ("4", DetailTab::Info),
    ] {
        drive(&mut m, &h.env, [key(k)]);
        assert_eq!(m.detail.tab, want, "after {k}");
    }
    // The tab bar marks Info active.
    let y = (m.lay.header_h + 1) as u16;
    let bar = &frame_rows(&m)[usize::from(y)];
    let x = bar.find("4 Info").unwrap() as u16;
    let buf = draw(&m, 120, 40);
    assert_eq!(buf[(x, y)].modifier, STYLES.active_tab.add_modifier);
    assert_ne!(
        buf[(1, y)].modifier,
        STYLES.active_tab.add_modifier,
        "Result is not active"
    );
}

// Go: TestDetailMembersFollowSortGroupMembersAndWrap.
#[test]
fn detail_members_follow_sort_group_members_and_wrap() {
    let h = harness();
    let list = group_fixture(&h);
    let mut m = open_detail(&h.env, list.clone(), 160, 40);
    let mut want = list;
    sort_group_members(&mut want);
    assert_eq!(
        want.last().unwrap().mode,
        "consilium",
        "test premise: the judge sorts last"
    );
    let got: Vec<&str> = m
        .list
        .selected()
        .unwrap()
        .sessions
        .iter()
        .map(|s| s.id.as_str())
        .collect();
    let want_ids: Vec<&str> = want.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(got, want_ids);

    let bar = frame_rows(&m)[m.lay.header_h + 1].clone();
    assert!(bar.ends_with("[ gpt-5.5 ] gemini-3.1 judge "), "{bar:?}");
    assert!(
        !bar.contains("gpt-6-astra"),
        "the judge shows as judge: {bar:?}"
    );

    let member = |m: &Model| m.detail.member_index(m.list.selected().unwrap());
    assert!(frame_text(&m).contains("GPT55-OUTPUT"));
    drive(&mut m, &h.env, [key("]")]);
    assert_eq!(member(&m), 1);
    assert!(frame_text(&m).contains("GEMINI-OUTPUT"));
    drive(&mut m, &h.env, keys(&["]", "]"]));
    assert_eq!(member(&m), 0, "] wraps");
    drive(&mut m, &h.env, [key("[")]);
    assert_eq!(member(&m), 2, "[ wraps to the judge");
    assert!(frame_text(&m).contains("JUDGE-OUTPUT"));
}

// Go: TestDetailPromptTab. The list holds summaries; the Prompt tab loads
// the full stored prompt, and falls back to the preview when it cannot.
#[test]
fn detail_prompt_tab() {
    let h = harness();
    let stored = Session::new_queued(
        h.paths(),
        NewSession {
            cli: "codex",
            mode: "review",
            model: "gpt-6-astra",
            effort: "high",
            workdir: "/src/proj",
            prompt: &format!("FULL-PROMPT {}THE-END", "word ".repeat(60)),
            review_scope: "",
            group_id: "",
        },
    )
    .unwrap();
    let summary = Session {
        prompt: String::new(),
        pid: 0,
        start_time: fixed_now() - TimeDelta::minutes(1),
        ..stored
    };
    let missing = Session {
        prompt_preview: "PREVIEW-ONLY".into(),
        ..run(
            "missing0-0000",
            "codex",
            "gpt-5.5",
            "",
            "",
            "completed",
            fixed_now() - TimeDelta::hours(1),
            "",
        )
    };
    let mut m = open_detail(&h.env, vec![Arc::new(summary), Arc::new(missing)], 100, 40);
    drive(&mut m, &h.env, [key("3")]);
    let got = m.detail.vp.content_text();
    assert!(
        got.contains("FULL-PROMPT") && got.contains("THE-END"),
        "{got}"
    );
    assert!(m.detail.lines.iter().all(|l| line_width(l) <= 100));
    assert!(frame_text(&m).contains("FULL-PROMPT"));

    drive(&mut m, &h.env, keys(&["esc", "j", "enter", "3"]));
    let got = m.detail.vp.content_text();
    assert!(
        got.contains("PREVIEW-ONLY") && got.contains("(full prompt unavailable)"),
        "{got}"
    );
}

/// A prompt load that lands after the run was closed is dropped.
#[test]
fn a_late_prompt_load_is_dropped() {
    let h = harness();
    let s = Session {
        prompt_preview: "PREVIEW".into(),
        log_file: h.log("p.log", "x\n"),
        ..run(
            "late0000",
            "codex",
            "gpt-5.5",
            "",
            "",
            "completed",
            fixed_now(),
            "",
        )
    };
    let mut m = job_model(&h.env, vec![Arc::new(s)], 100, 30);
    let cmds = m.update(key("enter"));
    let prompts = cmds
        .into_iter()
        .find_map(|c| match c {
            Cmd::Job(job @ Job::Prompts(_)) => Some(job),
            _ => None,
        })
        .expect("enter asks for the prompts");
    drive(&mut m, &h.env, [key("3")]);
    assert!(
        m.detail
            .vp
            .content_text()
            .contains("(loading full prompt…)")
    );
    drive(&mut m, &h.env, [key("esc")]);
    let res = h.env.run(prompts).into_msg().unwrap();
    drive(&mut m, &h.env, [res]);
    assert_eq!(m.detail.prompts_pending, None);
    assert!(m.detail.prompts.is_empty());
}

// Go: TestDetailFollowPausesAndResumes.
#[test]
fn detail_follow_pauses_and_resumes() {
    let h = harness();
    let s = Arc::new(Session {
        log_file: h.log("follow.log", &"initial line\n".repeat(200)),
        ..run(
            "follow00-0000",
            "codex",
            "gpt-6-astra",
            "review",
            "",
            "running",
            fixed_now(),
            "",
        )
    });
    let path = s.log_file.clone();
    let mut m = open_detail(&h.env, vec![s.clone()], 80, 30);
    assert!(
        m.detail.vp.at_bottom() && m.detail.follow,
        "opens at the tail, following"
    );
    assert!(frame_text(&m).contains("follow ●"));

    append(&path, "MARKER-ONE");
    drive(&mut m, &h.env, [Msg::Tick]);
    assert!(m.detail.vp.content_text().contains("MARKER-ONE"));
    assert!(
        m.detail.vp.at_bottom(),
        "a tick while following keeps the tail in view"
    );

    drive(&mut m, &h.env, [key("k")]);
    assert!(
        !m.detail.follow && !m.detail.vp.at_bottom(),
        "scrolling up pauses follow"
    );
    assert!(frame_text(&m).contains("follow ○"));
    let offset = m.detail.vp.y_offset();
    append(&path, "MARKER-TWO");
    drive(&mut m, &h.env, [Msg::Tick]);
    assert_eq!(
        m.detail.vp.y_offset(),
        offset,
        "a tick moved a reader who scrolled up"
    );
    assert!(!m.detail.vp.at_bottom());
    assert!(
        m.detail.vp.content_text().contains("MARKER-TWO"),
        "the tick re-read while paused"
    );

    drive(&mut m, &h.env, [key("G")]);
    assert!(
        m.detail.follow && m.detail.vp.at_bottom(),
        "G resumes follow"
    );
    drive(&mut m, &h.env, keys(&["k", "f"]));
    assert!(
        m.detail.follow && m.detail.vp.at_bottom(),
        "f resumes follow"
    );

    // Shrinking while following keeps the tail, and so does the help bar
    // growing.
    drive(&mut m, &h.env, [resize(80, 18)]);
    assert!(
        m.detail.vp.at_bottom(),
        "resize while following lost the tail"
    );
    drive(&mut m, &h.env, [key("?")]);
    append(&path, "MARKER-THREE");
    drive(&mut m, &h.env, [sessions(vec![s])]);
    assert!(m.detail.vp.at_bottom());
    let rows = frame_rows(&m);
    let last_content = m.lay.header_h + 3 + m.detail.vp.height() - 1;
    assert_eq!(
        rows[last_content].trim_end(),
        "MARKER-THREE",
        "the tail is on the last viewport row"
    );
}

// Go: TestDetailBreadcrumb.
#[test]
fn detail_breadcrumb() {
    let h = harness();
    let mut m = open_detail(&h.env, preview_fixture_logs(&h), 120, 40);
    let crumb = frame_rows(&m)[m.lay.header_h].clone();
    assert!(
        crumb.starts_with(" rival › orbit-web › review a0000000"),
        "{crumb:?}"
    );
    assert!(crumb.ends_with("⠋ running 1m0s   follow ● "), "{crumb:?}");
    drive(&mut m, &h.env, keys(&["esc", "j", "enter"]));
    let crumb = frame_rows(&m)[m.lay.header_h].clone();
    assert!(crumb.contains("ledger › plan b0000000"), "{crumb:?}");
    assert!(crumb.contains("✓ completed 2m36s"), "{crumb:?}");
}

// Go: TestDetailTooSmallTerminal.
#[test]
fn detail_too_small_terminal() {
    let h = harness();
    let mut m = open_detail(&h.env, preview_fixture_logs(&h), 80, 30);
    drive(&mut m, &h.env, [resize(80, 10)]);
    assert_eq!(
        frame_rows(&m)[0].trim_end(),
        "terminal too small (need 60×16)"
    );
    assert!(m.detail.vp.height() >= 1);
}

/// Go `searchFixture`: 120 lines, "Fingerprint" on lines 10, 50 and 90.
fn search_fixture(h: &Harness) -> Vec<Arc<Session>> {
    let mut b = String::new();
    for i in 0..120 {
        if matches!(i, 10 | 50 | 90) {
            b.push_str(&format!("line {i} has a Fingerprint in it\n"));
        } else {
            b.push_str(&format!("line {i} plain\n"));
        }
    }
    vec![Arc::new(Session {
        duration: "1m".into(),
        log_file: h.log("search.log", &b),
        ..run(
            "search00-0000",
            "codex",
            "gpt-6-astra",
            "review",
            "",
            "completed",
            fixed_now(),
            "",
        )
    })]
}

/// Opens `list`'s first run on the Raw tab. A finished run opens on
/// Result; Go's search cases run on its Output tab, which is Raw here.
fn open_raw(h: &Harness, list: Vec<Arc<Session>>, width: u16, height: u16) -> Model {
    let mut m = open_detail(&h.env, list, width, height);
    drive(&mut m, &h.env, [key("2")]);
    assert_eq!(m.detail.tab, DetailTab::Raw);
    m
}

/// The cells painted as search hits, as text.
fn painted(m: &Model) -> String {
    let buf = draw(m, m.lay.width as u16, m.lay.height as u16);
    let mut out = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            let cell = &buf[(x, y)];
            if cell.bg == STYLES.matched.bg.unwrap() {
                out.push_str(cell.symbol());
            }
        }
    }
    out
}

// Go: TestDetailSearch.
#[test]
fn detail_search() {
    let h = harness();
    let mut m = open_raw(&h, search_fixture(&h), 100, 30);
    drive(&mut m, &h.env, [key("/")]);
    assert!(m.mode == Mode::Search && m.detail.search.focused());
    drive(&mut m, &h.env, type_text("fingerprint"));
    drive(&mut m, &h.env, [key("enter")]);
    assert_eq!(
        (m.mode, m.detail.query.as_str()),
        (Mode::Detail, "fingerprint")
    );
    assert_eq!(m.detail.matches, [10, 50, 90]);
    assert!(
        visible(&m, 10),
        "first match not visible at {}",
        m.detail.vp.y_offset()
    );
    assert!(!m.detail.follow, "jumping to a match left follow on");
    assert!(frame_text(&m).contains("1/3 matches"));
    // A third of the way down the viewport.
    let vp_top = m.lay.header_h + 3;
    let row = vp_top + m.detail.vp.height() / 3;
    assert!(frame_rows(&m)[row].starts_with("line 10 has a Fingerprint"));

    drive(&mut m, &h.env, [key("n")]);
    assert!(visible(&m, 50) && frame_text(&m).contains("2/3 matches"));
    drive(&mut m, &h.env, keys(&["n", "n"]));
    assert!(
        visible(&m, 10) && frame_text(&m).contains("1/3 matches"),
        "n wraps"
    );
    drive(&mut m, &h.env, [key("N")]);
    assert!(
        visible(&m, 90) && frame_text(&m).contains("3/3 matches"),
        "N wraps back"
    );

    // The hit is painted with the match style; the text is unchanged.
    assert_eq!(painted(&m), "Fingerprint");
    assert!(frame_text(&m).contains("line 90 has a Fingerprint in it"));

    drive(&mut m, &h.env, [key("esc")]);
    assert!(m.detail.query.is_empty() && m.detail.matches.is_empty());
    assert_eq!(m.mode, Mode::Detail);
    assert_eq!(painted(&m), "", "highlight survived esc");
    drive(&mut m, &h.env, [key("esc")]);
    assert_eq!(m.mode, Mode::List, "second esc returns to the list");
}

// Go: TestDetailSearchNoMatches.
#[test]
fn detail_search_no_matches() {
    let h = harness();
    let mut m = open_raw(&h, search_fixture(&h), 100, 30);
    drive(&mut m, &h.env, [key("/")]);
    drive(&mut m, &h.env, type_text("zzz"));
    drive(&mut m, &h.env, [key("enter")]);
    assert!(frame_text(&m).contains("/zzz  no matches · esc clear"));
}

// Go: TestSearchInputTypesQ.
#[test]
fn search_input_types_q() {
    let h = harness();
    let mut m = open_raw(&h, search_fixture(&h), 100, 30);
    drive(&mut m, &h.env, keys(&["/", "q", "n"]));
    assert!(!m.quitting(), "q in the search input quit");
    assert_eq!(m.detail.search.value(), "qn");
    drive(&mut m, &h.env, [key("esc")]);
    assert!(m.mode == Mode::Detail && m.detail.search.value().is_empty());
}

// Go: TestSearchNearTailKeepsFollowOff.
#[test]
fn search_near_tail_keeps_follow_off() {
    let h = harness();
    let s = Arc::new(Session {
        pid: 101,
        log_file: h.log("tail.log", &format!("{}NEEDLE\n", "filler\n".repeat(200))),
        ..run(
            "tail0000-0000",
            "codex",
            "gpt-5.5",
            "review",
            "",
            "running",
            fixed_now(),
            "/src/p",
        )
    });
    let mut m = open_detail(&h.env, vec![s], 120, 40);
    drive(&mut m, &h.env, [key("/")]);
    drive(&mut m, &h.env, type_text("needle"));
    drive(&mut m, &h.env, [key("enter")]);
    assert_eq!(m.detail.matches.len(), 1);
    assert!(m.detail.vp.at_bottom(), "the match clamps to the bottom");
    assert!(!m.detail.follow, "a search near the tail turned follow on");
}

// Go: TestDetailMemberSurvivesMembershipChange.
#[test]
fn detail_member_survives_membership_change() {
    let h = harness();
    let list = group_fixture(&h);
    let mut m = open_detail(&h.env, list.clone(), 120, 40);
    drive(&mut m, &h.env, [key("]")]);
    let want = m.detail.current(m.list.selected()).unwrap().id.clone();
    let rest: Vec<_> = list
        .into_iter()
        .filter(|s| s.id != "gpt55000-0000")
        .collect();
    drive(&mut m, &h.env, [sessions(rest)]);
    assert_eq!(m.detail.current(m.list.selected()).unwrap().id, want);
    assert!(frame_text(&m).contains("GEMINI-OUTPUT"));
}

// Go: TestDetailViewportResizesOnModeChange.
#[test]
fn detail_viewport_resizes_on_mode_change() {
    let h = harness();
    let list = preview_fixture_logs(&h);
    let mut m = open_detail(&h.env, list.clone(), 120, 40);
    drive(&mut m, &h.env, [key("?")]);
    let expanded = m.detail.vp.height();
    drive(&mut m, &h.env, [key("/")]);
    drive(&mut m, &h.env, [sessions(list)]);
    drive(&mut m, &h.env, [key("esc")]);
    assert_eq!(m.detail.vp.height(), expanded);
    assert_fits(&m, "after leaving search");
}

/// A log read for the member the user just left, or for the width before a
/// resize, is dropped when it lands late.
#[test]
fn late_detail_reads_are_dropped() {
    let h = harness();
    let mut m = open_detail(&h.env, group_fixture(&h), 120, 40);
    let log_job = |cmds: Vec<Cmd>| {
        cmds.into_iter()
            .find_map(|c| match c {
                Cmd::Job(job @ Job::Log(_)) => Some(job),
                _ => None,
            })
            .expect("a log read")
    };
    // ] asks for gemini's log; the user moves on to the judge before it lands.
    let gemini = log_job(m.update(key("]")));
    drive(&mut m, &h.env, [key("]")]);
    let late = h.env.run(gemini).into_msg().unwrap();
    drive(&mut m, &h.env, [late]);
    let text = m.detail.vp.content_text();
    assert!(
        text.contains("JUDGE-OUTPUT") && !text.contains("GEMINI-OUTPUT"),
        "{text}"
    );

    // A resize: the read for the old width lands after the one for the new.
    let old = log_job(m.update(resize(100, 40)));
    drive(&mut m, &h.env, [resize(90, 40)]);
    let width_now = m.detail.vp.width();
    let late = h.env.run(old).into_msg().unwrap();
    drive(&mut m, &h.env, [late]);
    assert_eq!(m.detail.log.entry().unwrap().key.width, width_now);
}

// --- stop -------------------------------------------------------------------------

/// Go `runningGroup`: two running members stored in the temp home with fake
/// PIDs and real recorded start times. Only the testkit fake ever sees the
/// PIDs.
fn running_group(h: &Harness) -> Vec<Arc<Session>> {
    (0..2)
        .map(|i| {
            let mut s = Session::new_queued(
                h.paths(),
                NewSession {
                    cli: "codex",
                    mode: "megareview",
                    model: ["gpt-5.5", "gemini-3.1"][i],
                    effort: "high",
                    workdir: "/src/proj",
                    prompt: "prompt",
                    review_scope: "",
                    group_id: "killgroup",
                },
            )
            .unwrap();
            s.mark_running(h.paths()).unwrap();
            s.pid = 990001 + i as i64;
            s.pid_start = 77;
            s.save(h.paths()).unwrap();
            Arc::new(s)
        })
        .collect()
}

fn open_kill_model(h: &Harness, list: Vec<Arc<Session>>) -> Model {
    let m = open_detail(&h.env, list, 120, 40);
    set_alive(true); // fake pids stand in for live runs
    m
}

fn stored(env: &JobEnv, s: &Session) -> Session {
    Session::load(&env.paths, &s.id).unwrap()
}

// Go: TestStopAsksBeforeSignalling.
#[test]
fn stop_asks_before_signalling() {
    let h = harness();
    let mut m = open_kill_model(&h, running_group(&h));
    drive(&mut m, &h.env, [key("x")]);
    assert_eq!(m.mode, Mode::Confirm);
    let rows = frame_rows(&m);
    let status_row = m.lay.header_h + m.content_height() - 1;
    assert_eq!(rows[status_row].trim_end(), " stop 2 running sessions? y/n");
    assert!(signals().is_empty(), "x alone signalled");
    assert_eq!(m.help_view()[0].to_string().trim_end(), "y yes · n/esc no");
}

// Go: TestStopCancelSendsNothing.
#[test]
fn stop_cancel_sends_nothing() {
    for k in ["n", "esc", "j", "q"] {
        let h = harness();
        let mut m = open_kill_model(&h, running_group(&h));
        drive(&mut m, &h.env, keys(&["x", k]));
        assert!(signals().is_empty(), "{k} after x signalled");
        assert!(m.mode == Mode::Detail && m.detail.confirm.is_none(), "{k}");
        assert!(!m.quitting(), "{k}");
        assert!(
            !frame_text(&m).contains("y/n"),
            "{k} left the bar on screen"
        );
    }
}

// Go: TestStopYesSignalsAndFails. The signal leaves as a job: the update
// itself never signals.
#[test]
fn stop_yes_signals_and_fails() {
    let h = harness();
    let list = running_group(&h);
    let mut m = open_kill_model(&h, list.clone());
    drive(&mut m, &h.env, [key("x")]);
    let cmds = m.update(key("y"));
    assert!(signals().is_empty(), "update signalled directly");
    let [Cmd::Job(job @ Job::Stop(_))] = &cmds[..] else {
        panic!("y must hand the stop to a job: {cmds:?}");
    };
    let res = h.env.run(job.clone()).into_msg().unwrap();
    drive(&mut m, &h.env, [res]);
    assert_eq!(signals(), [990001, 990002]);
    for s in &list {
        let st = stored(&h.env, s);
        assert_eq!(
            (
                st.status.as_str(),
                st.exit_code,
                st.error_msg.as_str(),
                st.prompt.as_str()
            ),
            ("failed", Some(137), "killed by user", "prompt"),
            "{}",
            s.model
        );
    }
    assert_eq!(m.mode, Mode::Detail);
    // The rows on screen copy what was stored, and the header counts them.
    assert_eq!(m.list.stats.failed, 2);
    assert!(
        m.list
            .selected()
            .unwrap()
            .sessions
            .iter()
            .all(|s| s.status == "failed")
    );
    assert!(frame_rows(&m)[m.lay.header_h].contains("✗ failed"));
}

// Go: TestStopYesOnDeadProcessFailsWithExit1.
#[test]
fn stop_yes_on_dead_process_fails_with_exit_1() {
    let h = harness();
    let list = running_group(&h)[..1].to_vec();
    let mut m = open_kill_model(&h, list.clone());
    set_signal_fails(true);
    drive(&mut m, &h.env, keys(&["x", "y"]));
    let st = stored(&h.env, &list[0]);
    assert_eq!(
        (st.exit_code, st.error_msg.as_str()),
        (Some(1), "killed (process already dead)")
    );
}

// Go: TestStopOnFinishedRunSaysNothingRunning.
#[test]
fn stop_on_finished_run_says_nothing_running() {
    let h = harness();
    let done = Arc::new(Session {
        duration: "1m".into(),
        pid: 990009,
        pid_start: 77,
        log_file: h.log("done.log", "done\n"),
        ..run(
            "done0000-0000",
            "codex",
            "gpt-5.5",
            "",
            "",
            "completed",
            fixed_now(),
            "",
        )
    });
    let mut m = open_kill_model(&h, vec![done]);
    drive(&mut m, &h.env, [key("x")]);
    assert!(m.mode == Mode::Detail && m.detail.confirm.is_none());
    assert!(frame_text(&m).contains(" nothing running"));
    assert!(signals().is_empty());
    drive(&mut m, &h.env, [key("j")]);
    assert!(
        !frame_text(&m).contains("nothing running"),
        "the notice clears on the next key"
    );
}

// Go: TestStopYesSkipsARunThatFinishedMeanwhile.
#[test]
fn stop_yes_skips_a_run_that_finished_meanwhile() {
    let h = harness();
    let list = running_group(&h);
    let mut m = open_kill_model(&h, list.clone());
    drive(&mut m, &h.env, [key("x")]);
    let done = Arc::new(Session {
        status: "completed".into(),
        ..(*list[0]).clone()
    });
    drive(&mut m, &h.env, [sessions(vec![done, list[1].clone()])]);
    drive(&mut m, &h.env, [key("y")]);
    assert_eq!(signals(), [990002], "only the still-running run");
    assert_eq!(
        stored(&h.env, &list[0]).status,
        "running",
        "the finished one is not rewritten"
    );
}

// Go: TestStopNeverSignalsAReusedPID. The process dies and its PID is reused
// while the bar is open: the job's identity check refuses the signal.
#[test]
fn stop_never_signals_a_reused_pid() {
    let h = harness();
    let list = running_group(&h)[..1].to_vec();
    let mut m = open_kill_model(&h, list.clone());
    drive(&mut m, &h.env, [key("x")]);
    assert_eq!(m.mode, Mode::Confirm);
    set_alive(false);
    drive(&mut m, &h.env, [key("y")]);
    assert!(signals().is_empty(), "signalled a reused pid");
    let st = stored(&h.env, &list[0]);
    assert_eq!((st.status.as_str(), st.exit_code), ("failed", Some(1)));
}

// Go: TestStopOnDeadRunningRecordSaysNothingRunning.
#[test]
fn stop_on_dead_running_record_says_nothing_running() {
    let h = harness();
    let mut m = open_kill_model(&h, running_group(&h)[..1].to_vec());
    set_alive(false);
    drive(&mut m, &h.env, [key("x")]);
    assert!(m.mode == Mode::Detail && m.detail.confirm.is_none());
    assert!(frame_text(&m).contains("nothing running"));
    assert!(signals().is_empty());
}

// Go: TestStopRefusesARunWithoutRecordedStart.
#[test]
fn stop_refuses_a_run_without_recorded_start() {
    let h = harness();
    let list = running_group(&h)[..1].to_vec();
    let mut s = (*list[0]).clone();
    s.pid_start = 0;
    s.save(h.paths()).unwrap();
    let mut m = open_kill_model(&h, vec![Arc::new(s)]);
    drive(&mut m, &h.env, [key("x")]);
    assert!(m.mode == Mode::Detail && m.detail.confirm.is_none());
    assert!(frame_text(&m).contains("cannot verify process identity"));
    assert!(signals().is_empty());
}

/// "o" hands a copy of the raw log to the viewer through a job; the runtime's
/// log-view owner keeps it. A group opens every member's log in one copy.
#[test]
fn o_opens_the_log_through_a_job() {
    let h = harness();
    let mut views = crate::tui::jobs::LogViews::default();
    let mut m = open_detail(&h.env, preview_fixture_logs(&h), 120, 40);
    drive_with(&mut m, &h.env, Some(&mut views), [key("o")]);
    let opened = launched();
    assert_eq!(opened.len(), 1);
    assert_eq!(
        std::fs::read_to_string(&opened[0]).unwrap(),
        "LIVE-RUN-OUTPUT\n"
    );
    assert_eq!(views.len(), 1);

    let mut m = open_detail(&h.env, group_fixture(&h), 120, 40);
    drive_with(&mut m, &h.env, Some(&mut views), [key("o")]);
    let text = std::fs::read_to_string(&launched()[1]).unwrap();
    assert!(
        text.contains("=== gpt-5.5 REVIEW ===\nGPT55-OUTPUT"),
        "{text}"
    );
    assert!(
        text.contains("=== gpt-6-astra JUDGE ===\nJUDGE-OUTPUT"),
        "{text}"
    );
    // Finding 10: fresh copies outlive the TUI; the viewer may not have
    // read them yet. They sit in the harness's temp dir.
    views.close();
    assert!(launched().iter().all(|p| p.exists()));
}

/// A stop result that lands after the user left the run still updates its
/// rows, and leaves the screen where the user is.
#[test]
fn a_stop_result_after_leaving_updates_the_rows_only() {
    let h = harness();
    let list = running_group(&h);
    let mut m = open_kill_model(&h, list.clone());
    drive(&mut m, &h.env, [key("x")]);
    let cmds = m.update(key("y"));
    let Some(Cmd::Job(job)) = cmds.into_iter().next() else {
        panic!("no stop job");
    };
    drive(&mut m, &h.env, [key("esc")]);
    let res = h.env.run(job).into_msg().unwrap();
    drive(&mut m, &h.env, [res]);
    assert_eq!(m.mode, Mode::List);
    assert_eq!(m.list.stats.failed, 2);
}

// --- golden frames ------------------------------------------------------------------

/// The fixed fixtures behind the detail and preview goldens. Paths that the
/// frames show are fixed strings; only the Raw tab reads real temp files.
fn golden_cases() -> Vec<(&'static str, String)> {
    let h = harness();
    let mut cases = Vec::new();
    let mut frame = |name, m: &Model| {
        let (w, ht) = (m.lay.width as u16, m.lay.height as u16);
        cases.push((name, golden_frame(&draw(m, w, ht))));
    };

    // Raw tab of a group: member bar, follow, the judge's error.
    let m = open_detail(&h.env, group_fixture(&h), 100, 24);
    frame("detail_raw_group", &m);

    // Search hits on the Raw tab.
    let mut m = open_raw(&h, search_fixture(&h), 100, 24);
    drive(&mut m, &h.env, [key("/")]);
    drive(&mut m, &h.env, type_text("fingerprint"));
    drive(&mut m, &h.env, keys(&["enter", "n"]));
    frame("detail_search", &m);

    // Prompt and Info of a failed solo run.
    let now = fixed_now();
    let failed = Arc::new(Session {
        group_id: String::new(),
        prompt: format!(
            "Review the fingerprint re-key.\n\n\tCheck {} and report.",
            "every caller ".repeat(12)
        ),
        exit_code: Some(1),
        end_time: Some(now - TimeDelta::minutes(5)),
        duration: "1m30s".into(),
        queued_at: Some(now - TimeDelta::minutes(7)),
        review_scope: "plans/2026-09-26-service-identity".into(),
        account: "work".into(),
        pid: 81233,
        output_bytes: 2048,
        output_lines: 17,
        error_msg: "codex exited 1: rate limited".into(),
        log_file: "/var/log/rival/golden-info.log".into(),
        ..run(
            "abcdef01-2345-6789",
            "codex",
            "gpt-6-astra",
            "review",
            "xhigh",
            "failed",
            now - TimeDelta::minutes(6) - TimeDelta::seconds(30),
            "/src/orbit-web",
        )
    });
    let mut m = open_detail(&h.env, vec![failed], 100, 30);
    drive(&mut m, &h.env, [key("3")]);
    frame("detail_prompt", &m);
    drive(&mut m, &h.env, [key("4")]);
    frame("detail_info", &m);

    // The stop confirm over a running pair.
    let pair: Vec<Arc<Session>> = ["gpt-5.5", "gemini-3.1"]
        .iter()
        .enumerate()
        .map(|(i, model)| {
            Arc::new(Session {
                group_id: "stopgrp0-0000".into(),
                queued_at: Some(now - TimeDelta::minutes(3)),
                pid: 990001 + i as i64,
                pid_start: 77,
                log_file: h.log(&format!("stop{i}.log"), &format!("{model} reviewing\n")),
                ..run(
                    &format!("stop000{i}-0000"),
                    "codex",
                    model,
                    "megareview",
                    "high",
                    "running",
                    now - TimeDelta::minutes(2),
                    "/src/ledger",
                )
            })
        })
        .collect();
    let mut m = open_kill_model(&h, pair);
    drive(&mut m, &h.env, [key("x")]);
    frame("detail_confirm", &m);

    // The split view with the preview at its narrowest.
    let m = job_model(&h.env, preview_fixture_logs(&h), 130, 24);
    frame("list_preview", &m);
    assert!(signals().is_empty(), "goldens never signal");
    cases
}

#[test]
fn detail_frames_match_golden() {
    for (name, frame) in golden_cases() {
        assert_golden(name, &frame);
    }
}

#[test]
#[ignore = "rewrites the committed golden frames"]
fn regenerate_detail_golden_frames() {
    for (name, frame) in golden_cases() {
        let path = golden_path(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, frame).unwrap();
    }
}
