//! The Result tab at the model level: the app's `RunDetailModelTests`
//! (default tab, follow, member transitions), finding focus and toggles,
//! the parse cache through real jobs, and the Result golden frames. Logs
//! live in the testkit's temp home; jobs run through `testkit::drive`.

use std::sync::Arc;

use chrono::TimeDelta;
use rival_core::session::Session;
use rival_core::sessionview::SessionEvent;

use super::*;
use crate::tui::result_view::{LIVE_NOTE, PARSE_FAILED, ResultTarget};
use crate::tui::testkit::{
    FINDINGS_JSON, Harness, assert_golden, draw, drive, fixed_now, frame_text, golden_frame,
    golden_path, harness, key, open_detail, reads, run,
};

fn sessions(list: Vec<Arc<Session>>) -> Msg {
    Msg::Sessions(SessionEvent { sessions: list })
}

/// A member of group "g0000000": queued `q` seconds after the base, with a
/// log of `log`.
fn member(
    h: &Harness,
    id: &str,
    mode: &str,
    model: &str,
    status: &str,
    q: i64,
    log: &str,
) -> Session {
    let at = fixed_now() - TimeDelta::minutes(1) + TimeDelta::seconds(q);
    Session {
        group_id: "g0000000".into(),
        queued_at: Some(at),
        log_file: h.log(&format!("{id}.log"), log),
        ..run(
            id,
            "codex",
            model,
            mode,
            "high",
            status,
            at,
            "/src/orbit-web",
        )
    }
}

/// A solo run started ten minutes ago.
fn solo(h: &Harness, id: &str, status: &str, log: &str) -> Session {
    Session {
        duration: "2m36s".into(),
        log_file: h.log(&format!("{id}.log"), log),
        ..run(
            id,
            "codex",
            "gpt-6-astra",
            "review",
            "xhigh",
            status,
            fixed_now() - TimeDelta::minutes(10),
            "/src/orbit-web",
        )
    }
}

fn arcs(list: &[&Session]) -> Vec<Arc<Session>> {
    list.iter().map(|s| Arc::new((*s).clone())).collect()
}

fn with_status(s: &Session, status: &str) -> Session {
    Session {
        status: status.into(),
        ..s.clone()
    }
}

fn long_log() -> String {
    "a line of output\n".repeat(200)
}

/// The app's fixture: two running reviewers.
fn pair(h: &Harness) -> (Session, Session) {
    (
        member(
            h,
            "a",
            "megareview",
            "gpt-6-astra",
            "running",
            0,
            &long_log(),
        ),
        member(
            h,
            "b",
            "megareview",
            "claude-opus-5-5",
            "running",
            1,
            &long_log(),
        ),
    )
}

fn member_id(m: &Model) -> String {
    m.detail.current(m.list.selected()).unwrap().id.clone()
}

// --- RivalKitTests/RunDetailModelTests ----------------------------------------------

// Swift: testOpeningARunResets.
#[test]
fn opening_a_run_resets() {
    let h = harness();
    let (a, b) = pair(&h);
    let x = solo(&h, "x", "completed", "done\n");
    let mut m = open_detail(&h.env, arcs(&[&a, &b, &x]), 100, 30);
    assert_eq!(member_id(&m), "a", "test premise: the group is on top");
    drive(&mut m, &h.env, [key("]"), key("4")]);
    // Swift `userScrolled(atBottom: false)`.
    m.detail.follow = false;
    drive(&mut m, &h.env, [key("esc"), key("j"), key("enter")]);
    assert_eq!(member_id(&m), "x");
    assert_eq!(m.detail.member_id, "x");
    assert_eq!(m.detail.tab, DetailTab::Result);
    assert!(m.detail.follow);
}

// Swift: testTabOrder.
#[test]
fn tab_order() {
    let labels: Vec<&str> = DetailTab::ALL.iter().map(|t| t.label()).collect();
    assert_eq!(labels, ["Result", "Raw", "Prompt", "Info"]);
}

// Swift: testOpenFinishedRunOnResult.
#[test]
fn open_finished_run_on_result() {
    let h = harness();
    let m = open_detail(&h.env, arcs(&[&solo(&h, "f", "failed", "x\n")]), 100, 30);
    assert_eq!(m.detail.tab, DetailTab::Result);
}

// Swift: testOpenLiveRunOnRawWithFollow.
#[test]
fn open_live_run_on_raw_with_follow() {
    for status in ["running", "queued"] {
        let h = harness();
        let m = open_detail(&h.env, arcs(&[&solo(&h, "l", status, "x\n")]), 100, 30);
        assert_eq!(m.detail.tab, DetailTab::Raw, "{status}");
        assert!(m.detail.follow, "{status}");
    }
}

// Swift: testLiveRunFinishingOnRawWithFollowMovesToResult.
#[test]
fn live_run_finishing_on_raw_with_follow_moves_to_result() {
    let h = harness();
    let live = solo(&h, "l", "running", FINDINGS_JSON);
    let done = with_status(&live, "completed");
    let mut m = open_detail(&h.env, arcs(&[&live]), 100, 30);
    drive(&mut m, &h.env, [sessions(arcs(&[&live]))]);
    assert_eq!(
        m.detail.tab,
        DetailTab::Raw,
        "a refresh while live changes nothing"
    );
    drive(&mut m, &h.env, [sessions(arcs(&[&done]))]);
    assert_eq!(m.detail.tab, DetailTab::Result);
    assert!(
        frame_text(&m).contains("CRITICAL"),
        "the parse ran on the switch"
    );
    assert_eq!(m.detail.vp.y_offset(), 0, "Result opens at its top");
    drive(&mut m, &h.env, [key("2")]);
    drive(&mut m, &h.env, [sessions(arcs(&[&done]))]);
    assert_eq!(
        m.detail.tab,
        DetailTab::Raw,
        "only the live→finished edge switches"
    );
}

// Swift: testLiveRunFinishingScrolledUpStays.
#[test]
fn live_run_finishing_scrolled_up_stays() {
    let h = harness();
    let live = solo(&h, "l", "running", &long_log());
    let mut m = open_detail(&h.env, arcs(&[&live]), 100, 30);
    drive(&mut m, &h.env, [key("k")]);
    assert!(!m.detail.follow, "test premise: scrolled up");
    drive(
        &mut m,
        &h.env,
        [sessions(arcs(&[&with_status(&live, "completed")]))],
    );
    assert_eq!(m.detail.tab, DetailTab::Raw);
}

// Swift: testLiveRunFinishingOnOtherTabStays.
#[test]
fn live_run_finishing_on_other_tab_stays() {
    let h = harness();
    let live = solo(&h, "l", "running", "x\n");
    let mut m = open_detail(&h.env, arcs(&[&live]), 100, 30);
    drive(&mut m, &h.env, [key("3")]);
    drive(
        &mut m,
        &h.env,
        [sessions(arcs(&[&with_status(&live, "completed")]))],
    );
    assert_eq!(m.detail.tab, DetailTab::Prompt);
}

// Swift: testMemberSwitchIsNotAFinish. The user leaves a live member for a
// finished one: no switch. The picked member itself finishing does switch.
#[test]
fn member_switch_is_not_a_finish() {
    let h = harness();
    let (a, _) = pair(&h);
    let done = member(
        &h,
        "b",
        "megareview",
        "claude-opus-5-5",
        "completed",
        1,
        "x\n",
    );
    let mut m = open_detail(&h.env, arcs(&[&a, &done]), 100, 30);
    assert_eq!(m.detail.tab, DetailTab::Raw);
    drive(&mut m, &h.env, [key("]")]);
    assert_eq!(member_id(&m), "b");
    assert_eq!(m.detail.tab, DetailTab::Raw);
    drive(&mut m, &h.env, [key("[")]);
    drive(
        &mut m,
        &h.env,
        [sessions(arcs(&[&with_status(&a, "completed"), &done]))],
    );
    assert_eq!(member_id(&m), "a");
    assert_eq!(m.detail.tab, DetailTab::Result);
}

// Swift: testSelectMemberTracksWithoutSync. Picking a live member tracks
// it, so its own finish switches to Result on the next refresh.
#[test]
fn select_member_tracks_without_sync() {
    let h = harness();
    let (_, b) = pair(&h);
    let done = member(&h, "a", "megareview", "gpt-6-astra", "completed", 0, "x\n");
    let mut m = open_detail(&h.env, arcs(&[&done, &b]), 100, 30);
    assert_eq!(m.detail.tab, DetailTab::Result);
    drive(&mut m, &h.env, [key("2"), key("]")]);
    assert_eq!(member_id(&m), "b");
    drive(
        &mut m,
        &h.env,
        [sessions(arcs(&[&done, &with_status(&b, "completed")]))],
    );
    assert_eq!(m.detail.tab, DetailTab::Result);
}

// Swift: testMemberIsAnchoredByIDAcrossReorders. A refresh re-sorts the
// group and adds the judge; the picked member, the tab and the paused
// follow stay.
#[test]
fn member_is_anchored_by_id_across_reorders() {
    let h = harness();
    let (a, b) = pair(&h);
    let mut m = open_detail(&h.env, arcs(&[&a, &b]), 100, 30);
    drive(&mut m, &h.env, [key("]"), key("3")]);
    m.detail.follow = false;
    let first_b = Session {
        queued_at: Some(fixed_now() - TimeDelta::minutes(5)),
        ..b.clone()
    };
    let judge = member(&h, "j", "consilium", "gpt-5.5", "queued", 2, "");
    drive(&mut m, &h.env, [sessions(arcs(&[&a, &first_b, &judge]))]);
    let order: Vec<&str> = m
        .list
        .selected()
        .unwrap()
        .sessions
        .iter()
        .map(|s| s.id.as_str())
        .collect();
    assert_eq!(
        order,
        ["b", "a", "j"],
        "test premise: the refresh re-sorted"
    );
    assert_eq!(member_id(&m), "b");
    assert_eq!(m.detail.tab, DetailTab::Prompt, "a refresh keeps the tab");
    assert!(!m.detail.follow, "a refresh keeps follow paused");
}

// Swift: testVanishedMemberFallsBackToFirst.
#[test]
fn vanished_member_falls_back_to_first() {
    let h = harness();
    let (a, b) = pair(&h);
    let mut m = open_detail(&h.env, arcs(&[&a, &b]), 100, 30);
    drive(&mut m, &h.env, [key("]")]);
    let shrunk = DisplayItem {
        sessions: arcs(&[&a]),
    };
    assert_eq!(
        m.detail.current(Some(&shrunk)).unwrap().id,
        "a",
        "safe before sync"
    );
    drive(&mut m, &h.env, [sessions(arcs(&[&a]))]);
    assert_eq!(m.detail.member_id, "a");
}

// Swift: testMemberBeforeSyncIsFirst.
#[test]
fn member_before_sync_is_first() {
    let h = harness();
    let (a, b) = pair(&h);
    let pane = crate::tui::detail_view::DetailPane::default();
    let item = DisplayItem {
        sessions: arcs(&[&a, &b]),
    };
    assert_eq!(pane.current(Some(&item)).unwrap().id, "a");
    let empty = DisplayItem {
        sessions: Vec::new(),
    };
    assert!(pane.current(Some(&empty)).is_none());
}

// Swift: testFollow. The "re-selecting the same member changes nothing"
// step is `reselecting_the_same_member_keeps_follow`.
#[test]
fn follow() {
    let h = harness();
    let (a, b) = pair(&h);
    let mut m = open_detail(&h.env, arcs(&[&a, &b]), 100, 30);
    assert!(m.detail.follow);
    drive(&mut m, &h.env, [key("k")]);
    assert!(!m.detail.follow);
    drive(&mut m, &h.env, [key("j")]);
    assert!(m.detail.follow, "back at the tail resumes");
    m.detail.follow = false;
    drive(&mut m, &h.env, [key("]")]);
    assert!(m.detail.follow, "a new member starts at its tail");
    drive(&mut m, &h.env, [key("k")]);
    assert!(!m.detail.follow);
    drive(&mut m, &h.env, [key("f")]);
    assert!(m.detail.follow);
}

// Swift: testFollow, its last steps. In the TUI "]" and "[" on a run of one
// re-pick the member shown: follow and the scroll stay (the old TUI reset
// both).
#[test]
fn reselecting_the_same_member_keeps_follow() {
    let h = harness();
    let live = solo(&h, "l", "running", &long_log());
    let mut m = open_detail(&h.env, arcs(&[&live]), 100, 30);
    drive(&mut m, &h.env, [key("k"), key("k")]);
    assert!(!m.detail.follow);
    let at = m.detail.vp.y_offset();
    for k in ["]", "["] {
        let cmds = m.update(key(k));
        assert!(cmds.is_empty(), "{k}: {cmds:?}");
        assert!(
            !m.detail.follow,
            "{k} re-picking the same member resumed follow"
        );
        assert_eq!(m.detail.vp.y_offset(), at, "{k}");
    }
    drive(&mut m, &h.env, [key("f")]);
    assert!(m.detail.follow);
}

/// A member switch on the Result tab turns follow on too, so switching
/// back to Raw lands on the tail.
#[test]
fn a_member_switch_on_result_turns_follow_on() {
    let h = harness();
    let a = member(
        &h,
        "a",
        "megareview",
        "gpt-6-astra",
        "completed",
        0,
        FINDINGS_JSON,
    );
    let b = member(
        &h,
        "b",
        "megareview",
        "gemini-3.1",
        "completed",
        1,
        "plain answer\n",
    );
    let mut m = open_detail(&h.env, arcs(&[&a, &b]), 100, 30);
    m.detail.follow = false;
    drive(&mut m, &h.env, [key("]")]);
    assert!(m.detail.follow);
    assert_eq!(m.detail.tab, DetailTab::Result);
}

// --- focus and toggles ------------------------------------------------------------

fn findings_run(h: &Harness) -> Session {
    solo(h, "f0000000-find", "completed", FINDINGS_JSON)
}

/// The focused finding's bar, as text.
fn focused_row(m: &Model) -> String {
    let rows = &m.detail.findings.rows[m.detail.findings.focus];
    m.detail
        .vp
        .content_text()
        .split('\n')
        .nth(rows.start)
        .unwrap()
        .to_string()
}

#[test]
fn j_k_move_the_finding_focus_and_enter_space_toggle() {
    let h = harness();
    let mut m = open_detail(&h.env, arcs(&[&findings_run(&h)]), 100, 30);
    let cursor = m.list.cursor;
    assert_eq!(m.detail.tab, DetailTab::Result);
    assert_eq!(m.detail.findings.rows.len(), 3);
    assert!(focused_row(&m).starts_with("▌ src/auth/fingerprint.rs:42"));
    for (k, want) in [
        ("j", 1),
        ("down", 2),
        ("j", 2),
        ("k", 1),
        ("up", 0),
        ("k", 0),
    ] {
        drive(&mut m, &h.env, [key(k)]);
        assert_eq!(m.detail.findings.focus, want, "after {k}");
    }
    assert!(focused_row(&m).starts_with("▌ "));
    drive(&mut m, &h.env, [key("enter")]);
    let text = frame_text(&m);
    assert!(text.contains("▾ failure scenario"), "{text}");
    assert!(text.contains("signed out."), "{text}");
    drive(&mut m, &h.env, [key("space")]);
    assert!(frame_text(&m).contains("▸ failure scenario"));
    // The second finding has nothing to open.
    drive(&mut m, &h.env, [key("j"), key("enter")]);
    assert!(m.detail.findings.expanded.is_empty());
    // The list cursor never moved.
    assert_eq!(m.list.cursor, cursor);
    // The help names the Result keys.
    let help = m.help_view()[0].to_string();
    assert!(
        help.starts_with("1-4 tab · j/k finding · enter open · [/] member"),
        "{help}"
    );
    drive(&mut m, &h.env, [key("2")]);
    let help = m.help_view()[0].to_string();
    assert!(
        help.contains("f follow") && !help.contains("finding"),
        "{help}"
    );
}

/// On a short screen the focused finding's first row scrolls into view,
/// opened or not, even when the finding is taller than the view.
#[test]
fn the_focused_finding_stays_in_view() {
    let h = harness();
    let mut m = open_detail(&h.env, arcs(&[&findings_run(&h)]), 70, 16);
    let in_view = |m: &Model| {
        let rows = m.detail.findings.rows[m.detail.findings.focus].clone();
        let (top, h) = (m.detail.vp.y_offset(), m.detail.vp.height());
        rows.start >= top && rows.start < top + h
    };
    for k in ["enter", "j", "j", "enter", "k", "k"] {
        drive(&mut m, &h.env, [key(k)]);
        assert!(in_view(&m), "after {k}: focus {}", m.detail.findings.focus);
        let text = frame_text(&m);
        assert!(
            text.contains("▌ "),
            "after {k}: the focus bar is off screen\n{text}"
        );
    }
    // The page keys still scroll.
    let at = m.detail.vp.y_offset();
    drive(&mut m, &h.env, [key("pgdown")]);
    assert!(m.detail.vp.y_offset() > at);
}

/// Where the header and the first finding fit, focusing the first finding
/// scrolls back to the top.
#[test]
fn the_first_finding_brings_the_header_back() {
    let h = harness();
    let mut m = open_detail(&h.env, arcs(&[&findings_run(&h)]), 70, 22);
    drive(&mut m, &h.env, [key("j"), key("j")]);
    assert!(
        m.detail.vp.y_offset() > 0,
        "test premise: the last finding scrolled"
    );
    drive(&mut m, &h.env, [key("k"), key("k")]);
    assert_eq!(m.detail.vp.y_offset(), 0);
}

/// Without findings j/k scroll as on the other tabs, and space pages.
#[test]
fn a_markdown_result_scrolls_with_the_pager_keys() {
    let h = harness();
    let body: String = (0..80).map(|i| format!("Paragraph {i}.\n\n")).collect();
    let mut m = open_detail(
        &h.env,
        arcs(&[&solo(&h, "md", "completed", &body)]),
        100,
        24,
    );
    assert!(m.detail.findings.rows.is_empty());
    drive(&mut m, &h.env, [key("j")]);
    assert_eq!(m.detail.vp.y_offset(), 1);
    drive(&mut m, &h.env, [key("space")]);
    assert_eq!(m.detail.vp.y_offset(), 1 + m.detail.vp.height());
    let help = m.help_view()[0].to_string();
    assert!(!help.contains("finding"), "{help}");
}

// --- the parse through jobs --------------------------------------------------------

/// Each finished member is parsed once. Coming back to it, or reopening the
/// run, only stats its log.
#[test]
fn a_parse_is_reused_across_members_and_reopening() {
    let h = harness();
    let a = member(
        &h,
        "a",
        "megareview",
        "gpt-6-astra",
        "completed",
        0,
        FINDINGS_JSON,
    );
    let b = member(
        &h,
        "b",
        "megareview",
        "gemini-3.1",
        "completed",
        1,
        "plain answer\n",
    );
    let mut m = open_detail(&h.env, arcs(&[&a, &b]), 100, 30);
    assert_eq!(reads(), 1);
    assert!(frame_text(&m).contains("CRITICAL"));
    drive(&mut m, &h.env, [key("]")]);
    assert_eq!(reads(), 2);
    assert!(frame_text(&m).contains("plain answer"));
    drive(
        &mut m,
        &h.env,
        [key("["), key("]"), key("esc"), key("enter")],
    );
    assert_eq!(reads(), 2, "an unchanged log was parsed again");
    assert!(frame_text(&m).contains("CRITICAL"));
    // A refresh with the log rewritten parses it again.
    std::fs::write(&a.log_file, "rewritten answer\n").unwrap();
    drive(&mut m, &h.env, [sessions(arcs(&[&a, &b]))]);
    assert_eq!(reads(), 3);
    assert!(frame_text(&m).contains("rewritten answer"));
    // Drawing never reads.
    draw(&m, 100, 30);
    assert_eq!(reads(), 3);
}

/// The parse of a member the user already left lands late: it is dropped,
/// and the member on screen keeps its own parse.
#[test]
fn a_late_parse_for_a_member_left_behind_is_dropped() {
    let h = harness();
    let a = member(
        &h,
        "a",
        "megareview",
        "gpt-6-astra",
        "completed",
        0,
        "answer A\n",
    );
    let b = member(
        &h,
        "b",
        "megareview",
        "gemini-3.1",
        "completed",
        1,
        "answer B\n",
    );
    let c = member(
        &h,
        "c",
        "megareview",
        "claude-opus-5-5",
        "completed",
        2,
        "answer C\n",
    );
    let mut m = open_detail(&h.env, arcs(&[&a, &b, &c]), 100, 30);
    let job = m
        .update(key("]"))
        .into_iter()
        .find_map(|cmd| match cmd {
            Cmd::Job(job @ Job::Result(_)) => Some(job),
            _ => None,
        })
        .expect("] asks for b's parse");
    assert!(frame_text(&m).contains("(reading result…)"));
    drive(&mut m, &h.env, [key("]")]);
    assert_eq!(member_id(&m), "c");
    let late = h.env.run(job).into_msg().unwrap();
    drive(&mut m, &h.env, [late]);
    let text = frame_text(&m);
    assert!(
        text.contains("answer C") && !text.contains("answer B"),
        "{text}"
    );
    assert!(m.detail.result.entry_of(&ResultTarget::of(&b)).is_none());
}

/// A live member is never parsed; its Result tab says so. Its finish
/// parses it.
#[test]
fn a_live_member_is_parsed_only_once_it_finishes() {
    let h = harness();
    let live = solo(&h, "l", "running", FINDINGS_JSON);
    let mut m = open_detail(&h.env, arcs(&[&live]), 100, 30);
    let cmds = m.update(key("1"));
    assert!(
        !cmds.iter().any(|c| matches!(c, Cmd::Job(Job::Result(_)))),
        "{cmds:?}"
    );
    assert_eq!(m.detail.vp.content_text(), LIVE_NOTE);
    assert_eq!(reads(), 1, "only the Raw read");
    drive(
        &mut m,
        &h.env,
        [sessions(arcs(&[&with_status(&live, "completed")]))],
    );
    assert_eq!(reads(), 2);
    assert!(frame_text(&m).contains("rating 7/10"));
}

/// A result message after the detail screen closed is dropped.
#[test]
fn a_parse_after_closing_is_dropped() {
    let h = harness();
    let s = findings_run(&h);
    let mut m = job_model_with(&h, &s);
    let job = m
        .update(key("enter"))
        .into_iter()
        .find_map(|cmd| match cmd {
            Cmd::Job(job @ Job::Result(_)) => Some(job),
            _ => None,
        })
        .expect("enter asks for the parse");
    drive(&mut m, &h.env, [key("esc")]);
    let late = h.env.run(job).into_msg().unwrap();
    drive(&mut m, &h.env, [late]);
    assert!(m.detail.result.entry_of(&ResultTarget::of(&s)).is_none());
}

fn job_model_with(h: &Harness, s: &Session) -> Model {
    crate::tui::testkit::job_model(&h.env, arcs(&[s]), 100, 30)
}

// --- golden frames ------------------------------------------------------------------

/// The Result goldens at a useful and a narrow size. Paths never show on
/// this tab, so the temp logs leave no trace in the frames.
fn result_golden_cases() -> Vec<(String, String)> {
    let h = harness();
    let mut cases = Vec::new();
    let markdown = "## Verdict\n\nThe re-key is **safe to ship** once the lock is held for the whole run. The `migrate` step already retries.\n\n- keep the batch size at 500\n- log every skipped session\n  - with its id\n\n```rust\nlet key = store.read_key()?;\nfor batch in sessions.chunks(500) { rekey(batch, &key)?; }\n```\n\nSee [the runbook](https://example.com/runbook) and <b>this</b> note.\n";
    let failed = Session {
        error_msg: "codex exited 1: rate limited".into(),
        ..solo(
            &h,
            "p0000000-bad",
            "failed",
            "{\"summary\": 3, \"findings\": []}\n",
        )
    };
    for (w, ht, suffix) in [(100u16, 28u16, ""), (60, 20, "_narrow")] {
        let mut frame = |name: &str, m: &Model| {
            cases.push((format!("{name}{suffix}"), golden_frame(&draw(m, w, ht))));
        };
        let mut m = open_detail(&h.env, arcs(&[&findings_run(&h)]), w, ht);
        frame("result_findings", &m);
        drive(&mut m, &h.env, [key("enter")]);
        frame("result_expanded", &m);
        let m = open_detail(
            &h.env,
            arcs(&[&solo(&h, "m0000000-md", "completed", markdown)]),
            w,
            ht,
        );
        frame("result_markdown", &m);
        let m = open_detail(&h.env, arcs(&[&failed]), w, ht);
        assert!(frame_text(&m).contains(PARSE_FAILED));
        frame("result_parse_failed", &m);
        let mut m = open_detail(
            &h.env,
            arcs(&[&solo(&h, "l0000000-live", "running", "working\n")]),
            w,
            ht,
        );
        drive(&mut m, &h.env, [key("1")]);
        frame("result_live", &m);
    }
    cases
}

#[test]
fn result_frames_match_golden() {
    for (name, frame) in result_golden_cases() {
        assert_golden(&name, &frame);
    }
}

#[test]
#[ignore = "rewrites the committed golden frames"]
fn regenerate_result_golden_frames() {
    for (name, frame) in result_golden_cases() {
        let path = golden_path(&name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, frame).unwrap();
    }
}
