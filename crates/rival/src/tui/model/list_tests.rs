//! List model and pagination tests at the model level, and the list's
//! golden frames.

use std::sync::Arc;

use chrono::TimeDelta;
use rival_core::session::Session;
use rival_core::sessionview::{LoadProgress, SessionEvent};

use super::*;
use crate::tui::session_list::{Row, StatusTab};
use crate::tui::testkit::{
    assert_golden, draw, fixed_now, frame_text, golden_frame, golden_path, key, list_fixture,
    list_model, loading_model, many_runs, preview_fixture, rows, run, send, type_text,
};

fn sessions(list: Vec<Arc<Session>>) -> Msg {
    Msg::Sessions(SessionEvent { sessions: list })
}

fn selected_id(m: &Model) -> String {
    m.list
        .selected()
        .and_then(DisplayItem::primary)
        .map(|s| s.id.clone())
        .unwrap_or_default()
}

/// How many runs (not headers) pass the tab and the filter.
fn run_rows(m: &Model) -> usize {
    m.list.rows.iter().filter(|r| r.is_run()).count()
}

fn tab_bar_text(m: &Model) -> String {
    m.list.tab_bar(120, false, &m.styles).to_string()
}

/// (0-based page, selected id).
fn at(m: &Model) -> (usize, String) {
    (m.list.page(), selected_id(m))
}

// --- list model ----------------------------------------------------------------

#[test]
fn list_keys_move_the_cursor() {
    let mut m = list_model(list_fixture(), 120, 40);
    assert_eq!(selected_id(&m), "a0000000-run");
    send(&mut m, [key("j"), key("j")]);
    assert_eq!(
        selected_id(&m),
        "c0000000-fail",
        "the failed run across the OLDER header"
    );
    m.update(key("G"));
    assert_eq!(selected_id(&m), "d0000000-old");
    m.update(key("g"));
    assert_eq!(selected_id(&m), "a0000000-run");
}

/// A queued run starting sorts above the selected one; the cursor follows
/// the run, not the index.
#[test]
fn list_selection_survives_insert_above() {
    let list = list_fixture();
    let mut m = list_model(list.clone(), 120, 40);
    m.update(key("j"));
    assert_eq!(selected_id(&m), "b0000000-done");
    let fresh = Arc::new(run(
        "e0000000-new",
        "codex",
        "gpt-5.5",
        "",
        "",
        "running",
        fixed_now(),
        "",
    ));
    let mut with_fresh = vec![fresh];
    with_fresh.extend(list);
    m.update(sessions(with_fresh));
    assert_eq!(selected_id(&m), "b0000000-done");
}

#[test]
fn filter_narrows_clears_and_keeps() {
    let mut m = list_model(list_fixture(), 120, 40);
    m.update(key("/"));
    assert!(m.mode == Mode::Filter && m.list.filter.focused());
    send(&mut m, type_text("orbit"));
    assert_eq!(run_rows(&m), 2, "filter orbit");

    // esc clears and blurs.
    m.update(key("esc"));
    assert_eq!(m.mode, Mode::List);
    assert!(!m.list.filter.focused());
    assert_eq!(m.list.filter.value(), "");
    assert_eq!(run_rows(&m), 4);

    // enter keeps the filter and blurs.
    m.update(key("/"));
    send(&mut m, type_text("mathquest"));
    m.update(key("enter"));
    assert_eq!(m.mode, Mode::List);
    assert!(!m.list.filter.focused());
    assert_eq!(m.list.filter.value(), "mathquest");
    assert_eq!(selected_id(&m), "d0000000-old", "the only match");
    // esc in the list clears the kept filter.
    m.update(key("esc"));
    assert_eq!(m.list.filter.value(), "");
    assert_eq!(run_rows(&m), 4);
}

#[test]
fn q_inside_filter_types_q() {
    let mut m = list_model(list_fixture(), 120, 40);
    m.update(key("/"));
    // The refilter may ask for the new selection's preview log; it must not
    // quit.
    let cmds = m.update(key("q"));
    assert!(
        cmds.iter().all(|c| matches!(c, Cmd::Job(_))),
        "q inside the filter quit: {cmds:?}"
    );
    assert!(!m.quitting());
    assert_eq!(m.list.filter.value(), "q");
    // ctrl+c still quits from inside the input.
    assert_eq!(m.update(key("ctrl+c")), vec![Cmd::Quit]);
    assert!(m.quitting());
}

#[test]
fn status_tabs_cycle_and_show_counts() {
    let mut m = list_model(list_fixture(), 120, 40);
    let bar = tab_bar_text(&m);
    for want in ["ALL 4", "RUNNING 1", "FAILED 1", "DONE 2"] {
        assert!(bar.contains(want), "{bar:?} omits {want:?}");
    }
    m.update(key("tab"));
    assert_eq!((m.list.tab, run_rows(&m)), (StatusTab::Running, 1));
    m.update(key("tab"));
    assert_eq!(m.list.tab, StatusTab::Failed);
    assert_eq!(selected_id(&m), "c0000000-fail");
    send(
        &mut m,
        [key("shift+tab"), key("shift+tab"), key("shift+tab")],
    );
    assert_eq!(m.list.tab, StatusTab::Done, "wraps");

    // Counts follow the filter.
    m.update(key("/"));
    send(&mut m, type_text("orbit"));
    let bar = tab_bar_text(&m);
    for want in ["ALL 2", "RUNNING 1", "FAILED 1", "DONE 0"] {
        assert!(bar.contains(want), "filtered {bar:?} omits {want:?}");
    }
}

#[test]
fn empty_filter_result_explains_itself() {
    let mut m = list_model(list_fixture(), 120, 40);
    m.update(key("/"));
    send(&mut m, type_text("zzz"));
    let v = frame_text(&m);
    assert!(v.contains(r#"no runs match "zzz" · esc clears"#), "{v}");
    assert!(m.list.selected().is_none());
    // enter on an empty list must not open a detail view.
    send(&mut m, [key("enter"), key("enter")]);
    assert_eq!(m.mode, Mode::List);
    // An empty tab explains itself too.
    let mut m = list_model(list_fixture(), 120, 40);
    let done: Vec<_> = list_fixture()
        .into_iter()
        .filter(|s| s.status == "completed")
        .collect();
    m.update(sessions(done));
    m.update(key("tab"));
    assert!(frame_text(&m).contains(" no running runs"));
}

#[test]
fn help_toggle() {
    let mut m = list_model(list_fixture(), 120, 40);
    let short = m.help_view();
    m.update(key("?"));
    assert!(m.show_all_help);
    let full = m.help_view();
    assert!(full.len() > 1 && full != short, "full help did not expand");
    m.update(key("?"));
    assert!(!m.show_all_help);
}

#[test]
fn spinner_ticks_only_while_running() {
    let mut m = list_model(list_fixture(), 120, 40);
    assert!(m.spinning, "a running session did not start the spinner");
    let before = m.spin_frame();
    assert_eq!(m.update(Msg::SpinTick), vec![Cmd::Spin]);
    assert_ne!(m.spin_frame(), before, "spinner did not advance");

    // Everything finishes: the next tick stops the chain.
    let done: Vec<_> = list_fixture()
        .into_iter()
        .map(|s| {
            let mut s = (*s).clone();
            if s.status == "running" {
                s.status = "completed".into();
            }
            Arc::new(s)
        })
        .collect();
    m.update(sessions(done));
    assert!(m.update(Msg::SpinTick).is_empty());
    assert!(!m.spinning);

    // A new running run restarts it.
    m.update(sessions(list_fixture()));
    assert!(m.spinning);
}

#[test]
fn tiny_terminal_shows_notice() {
    let m = list_model(list_fixture(), 50, 20);
    let frame = rows(&draw(&m, 50, 20));
    assert_eq!(frame[0].trim_end(), "terminal too small (need 60×16)");
    assert!(frame[1..].iter().all(|r| r.trim().is_empty()));
}

#[test]
fn compact_header_leaves_the_tab_bar_on_line_one() {
    let m = list_model(list_fixture(), 120, 29);
    let frame = frame_text(&m);
    let lines: Vec<&str> = frame.lines().collect();
    assert_eq!(lines.len(), 29);
    assert!(lines[1].contains("ALL 4"), "{:?}", lines[1]);
}

/// A bracketed paste reaches the filter as a paste, not keys; it filters
/// the rows like typing does.
#[test]
fn paste_into_filter_refilters() {
    let mut m = list_model(preview_fixture(), 120, 40);
    send(&mut m, [key("/"), Msg::Paste("mathquest".into())]);
    assert_eq!(run_rows(&m), 1);
    for r in &m.list.rows {
        if let Row::Run(i) = *r {
            assert_eq!(
                m.list.items[i].primary().unwrap().work_dir,
                "/src/mathquest"
            );
        }
    }
}

/// Only one 1s refresh chain may exist: repeated events must not start more.
#[test]
fn single_refresh_tick_chain() {
    let mut m = list_model(preview_fixture(), 120, 40);
    assert!(m.ticking, "a running row did not start the refresh tick");
    assert!(m.update(sessions(preview_fixture())).is_empty());
    assert!(m.ticking, "ticking flag lost");
    let done = preview_fixture()[1..].to_vec();
    send(&mut m, [sessions(done), Msg::Tick]);
    assert!(!m.ticking, "tick chain did not end once nothing runs");
}

/// The prompt is noise in the list (every review starts with the same
/// boilerplate), so no row or header shows it.
#[test]
fn list_shows_no_prompt() {
    let list: Vec<_> = preview_fixture()
        .into_iter()
        .map(|s| {
            Arc::new(Session {
                prompt_preview: "BOILERPLATE-PROMPT".into(),
                ..(*s).clone()
            })
        })
        .collect();
    let frame = frame_text(&list_model(list, 200, 40));
    assert!(
        !frame.contains("BOILERPLATE-PROMPT") && !frame.contains("PROMPT"),
        "{frame}"
    );
}

// --- pagination ----------------------------------------------------------------

fn many_model(n: usize, width: u16, height: u16) -> Model {
    list_model(many_runs(n, fixed_now()), width, height)
}

#[test]
fn page_keys_stop_at_the_ends() {
    let mut m = many_model(120, 120, 40);
    m.update(key("p"));
    assert_eq!(at(&m), (0, "r000".into()), "p on page 1");
    m.update(key("n"));
    assert_eq!(at(&m), (1, "r050".into()));
    send(&mut m, [key("n"), key("n"), key("n")]);
    assert_eq!(at(&m), (2, "r100".into()), "n past the end");
    m.update(key("pgup"));
    assert_eq!(at(&m), (1, "r050".into()));
    m.update(key("pgdown"));
    assert_eq!(m.list.page(), 2);
    send(&mut m, [key("p"), key("p"), key("p")]);
    assert_eq!(at(&m), (0, "r000".into()), "p past the start");
}

#[test]
fn jk_cross_page_edges_and_g_jumps_across_pages() {
    let mut m = many_model(120, 120, 40);
    send(&mut m, [key("n"), key("k")]);
    assert_eq!(at(&m), (0, "r049".into()), "k from the top of page 2");
    m.update(key("j"));
    assert_eq!(at(&m), (1, "r050".into()), "j from the bottom of page 1");
    m.update(key("G"));
    assert_eq!(at(&m), (2, "r119".into()));
    m.update(key("j"));
    assert_eq!(selected_id(&m), "r119", "j past the last run");
    m.update(key("g"));
    assert_eq!(at(&m), (0, "r000".into()));
    // The selected run is always on screen, whatever the page.
    m.update(key("G"));
    assert!(frame_text(&m).contains("page 3/3"));
    m.update(key("end"));
    m.update(key("home"));
    assert_eq!(at(&m), (0, "r000".into()));
}

#[test]
fn tab_and_filter_changes_reset_to_page_one() {
    let mut m = many_model(200, 120, 40);
    send(&mut m, [key("n"), key("n"), key("j")]);
    assert_eq!(m.list.page(), 2);
    m.update(key("tab")); // RUNNING: empty
    m.update(key("tab")); // FAILED: 67 runs, 2 pages
    assert_eq!(m.list.tab, StatusTab::Failed);
    assert_eq!(at(&m), (0, "r000".into()));
    m.update(key("n"));
    assert_eq!(m.list.page(), 1, "n on FAILED");
    send(&mut m, [key("shift+tab"), key("shift+tab")]);
    assert_eq!(m.list.tab, StatusTab::All);
    assert_eq!(at(&m), (0, "r000".into()));

    // A filter change resets too, and n/p typed into the filter stay text.
    send(&mut m, [key("n"), key("n"), key("/")]);
    send(&mut m, type_text("np"));
    assert_eq!(m.mode, Mode::Filter);
    assert_eq!(m.list.filter.value(), "np");
    m.update(key("esc"));
    assert_eq!(at(&m), (0, "r000".into()), "filter change");
    send(&mut m, [key("n"), key("/")]);
    send(&mut m, type_text("r1"));
    assert_eq!(at(&m), (0, "r100".into()), "typing a filter");
}

#[test]
fn refresh_keeps_the_selected_run_and_its_page() {
    let now = fixed_now();
    let list = many_runs(120, now);
    let mut m = list_model(list.clone(), 120, 40);
    m.update(key("n"));
    send(&mut m, (0..25).map(|_| key("j")));
    assert_eq!(at(&m), (1, "r075".into()));

    // 30 new runs above push r075 to ordinal 105: page 3.
    let mut fresh: Vec<Arc<Session>> = (0..30)
        .map(|i| {
            Arc::new(run(
                &format!("n{i:03}"),
                "codex",
                "",
                "",
                "",
                "completed",
                now + TimeDelta::milliseconds(i + 1),
                "",
            ))
        })
        .collect();
    fresh.extend(list.iter().cloned());
    m.update(sessions(fresh));
    assert_eq!(at(&m), (2, "r075".into()), "after insert above");
    assert!(frame_text(&m).contains("page 3/3"));

    // The run vanishes and the list shrinks to 2 pages: the page clamps.
    m.update(sessions(list[..60].to_vec()));
    assert!(m.list.selected().is_some());
    assert_eq!(m.list.page(), 1, "the last page");
}

/// The footer sits on the last line of the list pane, in the narrow and the
/// split layout.
#[test]
fn footer_is_the_last_line_of_the_list_pane() {
    for width in [90, 200] {
        let mut m = many_model(120, width, 40);
        m.update(key("n"));
        let lines = m.list.view_lines(
            m.list_inner_width(),
            m.list_inner_height(),
            "",
            fixed_now(),
            &m.styles,
        );
        let last = lines.last().unwrap().to_string();
        assert!(last.contains("page 2/3"), "width {width}: {last:?}");
    }
}

/// Detail mode keeps n for search matches: it never turns the list page.
#[test]
fn detail_n_does_not_turn_the_page() {
    let mut m = many_model(120, 120, 40);
    send(&mut m, [key("enter"), key("n")]);
    assert_eq!(m.mode, Mode::Detail);
    assert_eq!(at(&m), (0, "r000".into()));
}

/// The paginated frame fills the terminal exactly on every page, with the
/// help short or expanded, even when the pane is shorter than a page.
#[test]
fn paginated_frame_fits_the_terminal() {
    for width in [60u16, 90, 120, 200] {
        for height in [16u16, 30, 50] {
            let mut m = many_model(120, width, height);
            let steps = [
                None,
                Some("n"),
                Some("j"),
                Some("G"),
                Some("k"),
                Some("?"),
                Some("p"),
                Some("g"),
            ];
            for step in steps {
                if let Some(k) = step {
                    m.update(key(k));
                }
                let label = format!("{width}×{height} after {step:?}");
                let h = usize::from(height);
                let body_end = m.lay.header_h + 1 + m.list_body_height();
                assert_eq!(body_end + m.help_h, h, "{label}: rows do not add up");
                let frame = rows(&draw(&m, width, height));
                assert_eq!(frame.len(), h, "{label}");
                assert_eq!(
                    frame[body_end].trim_end(),
                    m.help_view()[0].to_string().trim_end(),
                    "{label}: help bar not right below the list"
                );
                // The list pane is exactly its width on every line.
                let lines = m.list.view_lines(
                    m.list_inner_width(),
                    m.list_inner_height(),
                    m.spin_frame(),
                    fixed_now(),
                    &m.styles,
                );
                assert_eq!(lines.len(), m.list_inner_height(), "{label}");
                for line in &lines {
                    assert_eq!(
                        crate::tui::text::line_width(line),
                        m.list_inner_width(),
                        "{label}: {line}"
                    );
                }
                let mut footer_line = body_end - 1;
                if m.lay.show_preview {
                    footer_line -= 1; // the bottom border is below it
                }
                let page = format!("page {}/3", m.list.page() + 1);
                assert!(
                    frame[footer_line].contains(&page),
                    "{label}: the list pane's last line is not the footer: {:?}",
                    frame[footer_line]
                );
                // The cursor is inside the scrolled window of its page.
                let (_, cur) = m.list.page_rows();
                let off = m.list.offset;
                assert!(
                    cur >= off && cur < off + m.list_rows(),
                    "{label}: cursor {cur} outside the window {off}+{}",
                    m.list_rows()
                );
            }
        }
    }
}

// --- golden frames -----------------------------------------------------------

/// Golden fixture: every row kind on page 1 (running, queued, a group, the
/// docker suffix, each task kind), then 60 plain runs for page 2.
fn golden_fixture() -> Vec<Arc<Session>> {
    let now = fixed_now();
    let ago = |m| now - TimeDelta::minutes(m);
    let mut list = vec![
        Arc::new(run(
            "g0000001-live",
            "codex",
            "gpt-6-astra",
            "review",
            "xhigh",
            "running",
            ago(3) - TimeDelta::seconds(7),
            "/src/orbit-web",
        )),
        Arc::new(Session {
            queue_position: 2,
            queued_at: Some(ago(1)),
            start_time: None,
            ..run(
                "g0000002-wait",
                "opencode",
                rival_core::config::KIMI_MODEL,
                "review",
                "high",
                "queued",
                ago(1),
                "/src/ledger",
            )
        }),
    ];
    for (id, model, effort, status) in [
        (
            "g0000003-m1",
            rival_core::config::GPT56_SOL_MODEL,
            "high",
            "completed",
        ),
        (
            "g0000004-m2",
            rival_core::config::CLAUDE_MODEL,
            "max",
            "failed",
        ),
        (
            "g0000005-m3",
            rival_core::config::GPT56_SOL_MODEL,
            "high",
            "completed",
        ),
    ] {
        list.push(Arc::new(Session {
            group_id: "mega-1".into(),
            end_time: Some(ago(5)),
            ..run(
                id,
                "codex",
                model,
                "megareview",
                effort,
                status,
                ago(12),
                "/src/mathquest",
            )
        }));
    }
    for (id, cli, model, mode, status, duration, start) in [
        (
            "g0000006-dk",
            "claude",
            rival_core::config::CLAUDE_MODEL,
            "docker",
            "completed",
            "6m24s",
            ago(20),
        ),
        (
            "g0000007-sec",
            "codex",
            rival_core::config::GPT56_SOL_MODEL,
            "security",
            "failed",
            "41s",
            ago(30),
        ),
        (
            "g0000008-plan",
            "claude",
            rival_core::config::CLAUDE_MODEL,
            "plan",
            "completed",
            "12m3s",
            ago(45),
        ),
        (
            "g0000009-slop",
            "grok",
            rival_core::config::GROK_MODEL,
            "plan",
            "completed",
            "2m0s",
            ago(50),
        ),
    ] {
        list.push(Arc::new(Session {
            duration: duration.into(),
            ..run(id, cli, model, mode, "medium", status, start, "/src/rival")
        }));
    }
    list.extend(many_runs(60, now - TimeDelta::hours(1)));
    list
}

/// The frames under `src/tui/testdata`, built from fixed sessions, a fixed
/// clock and a fixed version.
fn golden_cases() -> Vec<(&'static str, String)> {
    let mut cases = Vec::new();
    let mut frame = |name, m: &Model| {
        let (w, h) = (m.lay.width as u16, m.lay.height as u16);
        cases.push((name, golden_frame(&draw(m, w, h))));
    };

    let mut loading = loading_model(100, 24);
    loading.update(Msg::Progress(LoadProgress {
        done: 1240,
        total: 2999,
    }));
    frame("list_loading", &loading);

    frame("list_empty", &list_model(Vec::new(), 100, 24));

    let mut list = list_model(golden_fixture(), 100, 24);
    frame("list_page1", &list);
    list.update(key("n"));
    frame("list_page2", &list);

    let mut filter = list_model(golden_fixture(), 100, 24);
    filter.update(key("/"));
    send(&mut filter, type_text("orbit"));
    frame("list_filter", &filter);

    let mut narrow = list_model(golden_fixture(), 60, 16);
    narrow.update(key("j"));
    frame("list_min_width", &narrow);
    cases
}

#[test]
fn list_frames_match_golden() {
    for (name, frame) in golden_cases() {
        assert_golden(name, &frame);
    }
}

#[test]
#[ignore = "rewrites the committed golden frames"]
fn regenerate_list_golden_frames() {
    for (name, frame) in golden_cases() {
        let path = golden_path(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, frame).unwrap();
    }
}
