use std::sync::Arc;

use ratatui::style::Color;
use rival_core::session::Session;
use rival_core::sessionview::{LoadProgress, SessionEvent};

use super::*;
use crate::tui::detail_view::DetailTab;
use crate::tui::session_list::PAGE_SIZE;
use crate::tui::testkit::{
    draw, frame_text, key, list_fixture, list_model, loading_model, rows, send, type_text,
};

fn progress(done: usize, total: usize) -> Msg {
    Msg::Progress(LoadProgress { done, total })
}

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

/// The frame fills exactly the terminal: header, tab bar, list body and
/// help bar add up to the height, and the help bar sits on the last rows.
fn assert_frame_exact(m: &Model, label: &str) {
    let h = m.lay.height;
    let body = if m.in_detail() {
        m.content_height()
    } else {
        1 + m.list_body_height()
    };
    assert_eq!(
        m.lay.header_h + body + m.help_h,
        h,
        "{label}: rows do not add up"
    );
    let text = frame_text(m);
    let rows: Vec<&str> = text.split('\n').collect();
    assert_eq!(rows.len(), h, "{label}");
    let first_help = m.help_view()[0].to_string();
    assert_eq!(
        rows[h - m.help_h].trim_end(),
        first_help.trim_end(),
        "{label}: help bar not on the last rows:\n{text}"
    );
}

// --- Go loader_test.go -------------------------------------------------------

#[test]
fn loader_shows_before_first_snapshot() {
    for h in [16, 24, 40] {
        let m = loading_model(100, h);
        let v = frame_text(&m);
        assert!(
            v.contains("reading sessions"),
            "height {h}: no loader:\n{v}"
        );
        assert!(
            !v.contains("No sessions yet"),
            "height {h}: empty state while loading:\n{v}"
        );
        for zero in ["0 running", "0 queued", "✓ 0", "✗ 0", "0 sessions", "ALL 0"] {
            assert!(
                !v.contains(zero),
                "height {h}: {zero:?} shown while loading:\n{v}"
            );
        }
        for dots in ["… running", "… queued", "✓ …", "… sessions", "ALL …"] {
            assert!(
                v.contains(dots),
                "height {h}: want {dots:?} while loading:\n{v}"
            );
        }
    }
}

#[test]
fn load_progress_updates_the_bar() {
    let mut m = loading_model(100, 30);
    let before = frame_text(&m);
    m.update(progress(1240, 2999));
    let v = frame_text(&m);
    assert!(
        v.contains("reading sessions 1240/2999"),
        "progress text missing:\n{v}"
    );
    assert!(
        v.contains('█') && v.contains('░'),
        "a 41% bar should be part filled, part empty:\n{v}"
    );
    assert_ne!(v, before, "frame did not change after a progress message");
}

#[test]
fn first_session_event_removes_the_loader() {
    let mut m = loading_model(100, 30);
    send(&mut m, [progress(100, 4), sessions(list_fixture())]);
    let v = frame_text(&m);
    assert!(
        !v.contains("reading sessions") && !v.contains('…'),
        "loader survived the first snapshot:\n{v}"
    );
    assert!(
        v.contains("4 sessions"),
        "header counts missing after load:\n{v}"
    );
    // A late progress message must not bring the loader back.
    m.update(progress(1, 4));
    assert!(
        !frame_text(&m).contains("reading sessions"),
        "late progress revived the loader"
    );
}

#[test]
fn empty_snapshot_ends_loading_at_once() {
    let mut m = loading_model(100, 30);
    m.update(sessions(Vec::new()));
    let v = frame_text(&m);
    assert!(
        !v.contains("reading sessions") && v.contains("No sessions yet"),
        "want the empty state after an empty snapshot:\n{v}"
    );
    assert!(
        v.contains("0 sessions"),
        "want real zero counts after load:\n{v}"
    );
}

#[test]
fn loader_frame_is_exact() {
    for w in [60, 90, 120, 200] {
        for h in [16, 24, 29, 30, 50] {
            let mut m = loading_model(w, h);
            assert_frame_exact(&m, &format!("{w}×{h} loader, no progress"));
            m.update(progress(2999, 2999));
            assert_frame_exact(&m, &format!("{w}×{h} loader, full bar"));
            m.update(key("?"));
            assert_frame_exact(&m, &format!("{w}×{h} loader, full help"));
        }
    }
}

// TestWatchSessionsClosesProgressOnEmptyDir is ported with the watcher in
// rival_core::sessionview::watcher (Task 4.1); the runtime that drains the
// progress channel comes with Task 4.5.

// --- model behaviour --------------------------------------------------------

#[test]
fn loaded_frame_is_exact_in_every_mode() {
    for w in [60, 90, 120, 200] {
        for h in [16, 24, 29, 30, 50] {
            let mut m = list_model(list_fixture(), w, h);
            assert_frame_exact(&m, &format!("{w}×{h} list"));
            m.update(key("?"));
            assert_frame_exact(&m, &format!("{w}×{h} list, full help"));
            m.update(key("/"));
            assert_frame_exact(&m, &format!("{w}×{h} filter, full help"));
            send(&mut m, [key("esc"), key("?"), key("enter")]);
            assert_eq!(m.mode, Mode::Detail);
            assert_frame_exact(&m, &format!("{w}×{h} detail"));
            m.update(key("?"));
            assert_frame_exact(&m, &format!("{w}×{h} detail, full help"));
            m.update(key("/"));
            assert_frame_exact(&m, &format!("{w}×{h} search, full help"));
        }
    }
}

#[test]
fn list_keys_do_not_leak_into_the_filter() {
    let mut m = list_model(list_fixture(), 100, 30);
    m.update(key("/"));
    assert_eq!(m.mode, Mode::Filter);
    let cmds: Vec<Cmd> = type_text("qjnG?")
        .into_iter()
        .flat_map(|k| m.update(k))
        .collect();
    assert!(cmds.is_empty(), "filter text produced commands: {cmds:?}");
    assert!(!m.quitting(), "q quit from the filter");
    assert!(!m.show_all_help, "? toggled help from the filter");
    assert_eq!(m.mode, Mode::Filter);
    assert_eq!(m.list.filter.value(), "qjnG?");
    // enter keeps the filter and leaves the input.
    m.update(key("enter"));
    assert_eq!(m.mode, Mode::List);
    assert_eq!(m.list.filter.value(), "qjnG?");
    // esc in the list clears a kept filter.
    m.update(key("esc"));
    assert_eq!(m.list.filter.value(), "");
}

#[test]
fn filter_esc_clears_and_arrows_move() {
    let mut m = list_model(list_fixture(), 100, 30);
    send(&mut m, [key("/"), key("x"), key("esc")]);
    assert_eq!(m.mode, Mode::List);
    assert_eq!(m.list.filter.value(), "");
    assert!(!m.list.filter.focused());
    send(&mut m, [key("/"), key("down")]);
    assert_eq!(
        selected_id(&m),
        "b0000000-done",
        "down moves the cursor in the filter"
    );
    m.update(key("up"));
    assert_eq!(selected_id(&m), "a0000000-run");
}

#[test]
fn paste_lands_in_the_focused_input_only() {
    let mut m = list_model(list_fixture(), 100, 30);
    m.update(Msg::Paste("q".into()));
    assert_eq!(
        m.list.filter.value(),
        "",
        "a paste outside an input is dropped"
    );
    send(&mut m, [key("/"), Msg::Paste("ledger\nq".into())]);
    assert_eq!(m.list.filter.value(), "ledger q");
    assert!(!m.quitting());
}

#[test]
fn ctrl_c_quits_from_every_mode() {
    let to_mode: [(Mode, &[&str]); 4] = [
        (Mode::List, &[]),
        (Mode::Filter, &["/"]),
        (Mode::Detail, &["enter"]),
        (Mode::Search, &["enter", "/"]),
    ];
    for (mode, keys) in to_mode {
        let mut m = list_model(list_fixture(), 100, 30);
        send(&mut m, keys.iter().map(|k| key(k)));
        assert_eq!(m.mode, mode);
        assert_eq!(m.update(key("ctrl+c")), vec![Cmd::Quit], "{mode:?}");
        assert!(m.quitting(), "{mode:?}");
        assert_eq!(
            frame_text(&m).trim(),
            "",
            "{mode:?}: a quitting model draws nothing"
        );
    }
    let mut m = list_model(list_fixture(), 100, 30);
    send(&mut m, [key("enter")]);
    m.mode = Mode::Confirm;
    assert_eq!(m.update(key("ctrl+c")), vec![Cmd::Quit], "confirm");
}

#[test]
fn q_quits_from_list_and_detail_only() {
    let mut m = list_model(list_fixture(), 100, 30);
    assert_eq!(m.update(key("q")), vec![Cmd::Quit]);
    let mut m = list_model(list_fixture(), 100, 30);
    send(&mut m, [key("enter")]);
    assert_eq!(m.update(key("q")), vec![Cmd::Quit]);
    let mut m = list_model(list_fixture(), 100, 30);
    send(&mut m, [key("enter"), key("/")]);
    assert!(m.update(key("q")).is_empty());
    assert_eq!(m.detail.search.value(), "q", "q is text in the search");
}

#[test]
fn search_keys_are_text_and_esc_peels_one_layer() {
    let mut m = list_model(list_fixture(), 100, 30);
    send(&mut m, [key("enter"), key("/")]);
    assert_eq!(m.mode, Mode::Search);
    send(&mut m, type_text("n1]x"));
    assert_eq!(m.detail.search.value(), "n1]x");
    assert_eq!(
        m.detail.tab,
        DetailTab::Raw,
        "1 in the search must not switch tabs"
    );
    m.update(key("enter"));
    assert_eq!(m.mode, Mode::Detail);
    assert_eq!(m.detail.query, "n1]x");
    // "/" again starts from the applied query.
    m.update(key("/"));
    assert_eq!(m.detail.search.value(), "n1]x");
    m.update(key("esc"));
    assert_eq!((m.mode, m.detail.query.as_str()), (Mode::Detail, ""));
    // esc in the detail: a query first, then the screen.
    send(&mut m, [key("/"), key("a"), key("enter")]);
    m.update(key("esc"));
    assert_eq!((m.mode, m.detail.query.as_str()), (Mode::Detail, ""));
    m.update(key("esc"));
    assert_eq!(m.mode, Mode::List);
}

#[test]
fn detail_tabs_switch_by_number_and_cycle() {
    let mut m = list_model(list_fixture(), 100, 30);
    m.update(key("enter"));
    assert_eq!(m.detail.tab, DetailTab::Raw);
    for (k, tab) in [
        ("1", DetailTab::Result),
        ("4", DetailTab::Info),
        ("3", DetailTab::Prompt),
        ("2", DetailTab::Raw),
    ] {
        m.update(key(k));
        assert_eq!(m.detail.tab, tab, "key {k}");
    }
    m.update(key("tab"));
    assert_eq!(m.detail.tab, DetailTab::Prompt);
    send(&mut m, [key("tab"), key("tab")]);
    assert_eq!(m.detail.tab, DetailTab::Result, "tab wraps");
    m.update(key("shift+tab"));
    assert_eq!(m.detail.tab, DetailTab::Info, "shift+tab wraps back");
    // n and the page keys belong to the list, not the detail screen.
    let cursor = selected_id(&m);
    send(&mut m, [key("n"), key("p"), key("G")]);
    assert_eq!(selected_id(&m), cursor);
    assert_eq!(m.mode, Mode::Detail);
}

#[test]
fn members_cycle_inside_a_group() {
    let group = |id: &str| {
        Arc::new(Session {
            id: id.into(),
            group_id: "g1".into(),
            status: "completed".into(),
            ..Session::default()
        })
    };
    let mut m = list_model(vec![group("m1"), group("m2"), group("m3")], 100, 30);
    m.update(key("enter"));
    let first = m.detail.member_id.clone();
    m.update(key("]"));
    assert_ne!(m.detail.member_id, first);
    m.update(key("["));
    assert_eq!(m.detail.member_id, first);
    m.update(key("["));
    let last = m
        .list
        .selected()
        .unwrap()
        .sessions
        .last()
        .unwrap()
        .id
        .clone();
    assert_eq!(m.detail.member_id, last, "[ wraps to the last member");
}

#[test]
fn confirm_closes_on_any_key() {
    let mut m = list_model(list_fixture(), 100, 30);
    m.update(key("enter"));
    for k in ["n", "y", "q", "esc"] {
        m.mode = Mode::Confirm;
        m.detail.confirm = Some(crate::tui::detail_view::KillConfirm {
            targets: list_fixture(),
        });
        assert!(m.update(key(k)).is_empty(), "{k}");
        assert_eq!(m.mode, Mode::Detail, "{k}");
        assert!(m.detail.confirm.is_none(), "{k}");
        assert!(!m.quitting(), "{k}: q in the confirm must not quit");
    }
}

#[test]
fn help_toggle_borrows_rows_from_the_list() {
    let mut m = list_model(list_fixture(), 100, 30);
    let short = m.list_body_height();
    assert_eq!(m.help_h, 1);
    m.update(key("?"));
    assert_eq!(m.help_h, 4);
    assert_eq!(m.list_body_height(), short - 3);
    m.update(key("enter"));
    assert_eq!(m.help_h, 6, "the detail full help has 6 rows");
    m.update(key("esc"));
    m.update(key("?"));
    assert_eq!(m.help_h, 1);
}

#[test]
fn snapshot_keeps_the_selection_and_closes_a_vanished_run() {
    let mut m = list_model(list_fixture(), 100, 30);
    send(&mut m, [key("j"), key("enter")]);
    assert_eq!(selected_id(&m), "b0000000-done");
    // A reordered snapshot keeps the detail open on the same run.
    let mut reordered = list_fixture();
    reordered.reverse();
    m.update(sessions(reordered));
    assert_eq!(
        (m.mode, selected_id(&m).as_str()),
        (Mode::Detail, "b0000000-done")
    );
    // The run vanished: back to the list.
    let rest: Vec<_> = list_fixture()
        .into_iter()
        .filter(|s| s.id != "b0000000-done")
        .collect();
    m.update(sessions(rest));
    assert_eq!(m.mode, Mode::List);
}

#[test]
fn live_timers_start_once_and_stop_when_idle() {
    let mut m = loading_model(100, 30);
    assert_eq!(m.init(), vec![Cmd::Spin]);
    // The loader keeps the spinner alive before the first snapshot.
    assert_eq!(m.update(Msg::SpinTick), vec![Cmd::Spin]);
    // A running row starts the refresh tick; the spinner is already going.
    assert_eq!(m.update(sessions(list_fixture())), vec![Cmd::Tick]);
    assert!(
        m.update(sessions(list_fixture())).is_empty(),
        "no second chain"
    );
    assert_eq!(m.update(Msg::Tick), vec![Cmd::Tick]);
    // Nothing live: both chains end on their next tick.
    let done: Vec<_> = list_fixture()
        .into_iter()
        .filter(|s| s.status != "running")
        .collect();
    m.update(sessions(done));
    assert!(m.update(Msg::Tick).is_empty());
    assert!(m.update(Msg::SpinTick).is_empty());
    assert!(!m.ticking && !m.spinning);
    assert_eq!(m.spin_frame(), "", "no spinner when nothing runs");
    // A new running row restarts both.
    assert_eq!(
        m.update(sessions(list_fixture())),
        vec![Cmd::Tick, Cmd::Spin]
    );
}

#[test]
fn error_replaces_the_frame() {
    let mut m = loading_model(100, 30);
    m.update(Msg::Error("mkdir /x: permission denied".into()));
    assert!(m.loaded);
    assert!(frame_text(&m).starts_with("Error: mkdir /x: permission denied"));
}

#[test]
fn page_keys_move_fifty_runs() {
    let many: Vec<_> = (0..120)
        .map(|i| {
            Arc::new(Session {
                id: format!("s{i:03}"),
                status: "completed".into(),
                ..Session::default()
            })
        })
        .collect();
    // No start time: every run sits under one OLDER header at row 0, so the
    // run with ordinal i is row i + 1.
    let mut m = list_model(many, 100, 30);
    assert_eq!((m.list.cursor, selected_id(&m).as_str()), (1, "s000"));
    m.update(key("n"));
    assert_eq!(
        (m.list.page(), selected_id(&m)),
        (1, format!("s{PAGE_SIZE:03}"))
    );
    assert_eq!(m.list.cursor, PAGE_SIZE + 1);
    send(&mut m, [key("n"), key("n")]);
    assert_eq!(
        (m.list.page(), selected_id(&m).as_str()),
        (2, "s100"),
        "stops at the last page"
    );
    m.update(key("p"));
    assert_eq!(selected_id(&m), "s050");
    m.update(key("G"));
    assert_eq!((m.list.cursor, selected_id(&m).as_str()), (120, "s119"));
    m.update(key("g"));
    assert_eq!((m.list.cursor, selected_id(&m).as_str()), (1, "s000"));
}

#[test]
fn status_tabs_cycle_and_count() {
    let mut m = list_model(list_fixture(), 100, 30);
    assert_eq!(m.list.counts, [4, 1, 1, 2]);
    m.update(key("tab"));
    assert!(frame_text(&m).contains("RUNNING 1"));
    assert_eq!(selected_id(&m), "a0000000-run");
    send(&mut m, [key("shift+tab"), key("shift+tab")]);
    assert_eq!(selected_id(&m), "b0000000-done", "DONE tab");
}

// --- rendering ---------------------------------------------------------------

#[test]
fn tiny_and_mismatched_terminals_never_panic() {
    let mut m = list_model(list_fixture(), 100, 30);
    for (w, h) in [
        (1, 1),
        (2, 1),
        (10, 3),
        (59, 30),
        (80, 15),
        (60, 16),
        (300, 80),
    ] {
        m.update(Msg::Resize {
            width: w,
            height: h,
        });
        let buf = draw(&m, w, h);
        let text = rows(&buf).join("\n");
        if w < 60 || h < 16 {
            let notice = "terminal too small (need 60×16)";
            assert!(
                notice.starts_with(rows(&buf)[0].trim_end()),
                "{w}×{h}: {text}"
            );
        }
        // A stale layout (the terminal changed before the resize arrived)
        // is clipped, not written out of bounds.
        draw(&m, 5, 2);
        draw(&m, w.saturating_add(40), h.saturating_add(10));
    }
    let fresh = Model::new("dev");
    assert_eq!(rows(&draw(&fresh, 20, 2))[0].trim_end(), "Initializing...");
    // Every mode at the minimum size.
    let mut m = list_model(list_fixture(), 60, 16);
    for k in ["?", "/", "esc", "enter", "?", "/"] {
        m.update(key(k));
        draw(&m, 60, 16);
        draw(&m, 1, 1);
    }
}

#[test]
fn wide_terminal_splits_list_and_preview() {
    let m = list_model(list_fixture(), 160, 40);
    let buf = draw(&m, 160, 40);
    let rows = rows(&buf);
    let body_top = m.lay.header_h + 1;
    let list_w = m.lay.list_w;
    let top: Vec<char> = rows[body_top].chars().collect();
    assert_eq!(top[0], '╭', "list box:\n{}", rows.join("\n"));
    assert_eq!(top[list_w - 1], '╮');
    assert_eq!(top[list_w], ' ', "1-col gap");
    assert_eq!(top[list_w + 1], '╭', "preview box");
    assert_eq!(top[159], '╮');
    // The list border is the accent, the preview border dim.
    let accent = STYLES.accent.fg;
    let dim = STYLES.dim.fg;
    assert_eq!(buf[(0, body_top as u16)].fg, accent.unwrap_or(Color::Reset));
    assert_eq!(
        buf[((list_w + 1) as u16, body_top as u16)].fg,
        dim.unwrap_or(Color::Reset)
    );
    // Below the preview width the list takes the whole body.
    let m = list_model(list_fixture(), 119, 40);
    let rows = super::super::testkit::rows(&draw(&m, 119, 40));
    assert!(!rows[m.lay.header_h + 1].contains('╭'));
}

#[test]
fn compact_header_below_thirty_rows() {
    let tall = list_model(list_fixture(), 100, 30);
    let short = list_model(list_fixture(), 100, 29);
    assert_eq!(tall.lay.header_h, 5);
    assert_eq!(short.lay.header_h, 1);
    let first = frame_text(&short);
    let first = first.lines().next().unwrap();
    assert!(
        first.starts_with("rival  ● 1 running") || first.starts_with("rival  ⠋ 1 running"),
        "{first}"
    );
    let tall_text = frame_text(&tall);
    assert!(
        tall_text
            .lines()
            .next()
            .unwrap()
            .contains("_             __")
    );
}

#[test]
fn selected_row_uses_the_selection_bar() {
    let m = list_model(list_fixture(), 100, 30);
    let buf = draw(&m, 100, 30);
    // Header, tab bar, column titles, the TODAY heading, then the first run
    // (the cursor).
    let y = (m.lay.header_h + 3) as u16;
    assert!(rows(&buf)[usize::from(y) - 1].starts_with(" TODAY"));
    assert_ne!(buf[(0, y - 1)].bg, STYLES.selected.bg.unwrap());
    for x in [0u16, 50, 99] {
        assert_eq!(buf[(x, y)].bg, STYLES.selected.bg.unwrap(), "cell {x}");
    }
    assert_ne!(buf[(0, y + 1)].bg, STYLES.selected.bg.unwrap());
}

#[test]
fn tab_bar_reserves_the_filter_on_the_right() {
    let mut m = list_model(list_fixture(), 100, 30);
    let row = |m: &Model| {
        let text = frame_text(m);
        text.lines().nth(m.lay.header_h).unwrap().to_string()
    };
    let bar = row(&m);
    assert!(
        bar.starts_with(" ALL 4   RUNNING 1   FAILED 1   DONE 2"),
        "{bar:?}"
    );
    assert!(bar.ends_with("/ filter "), "{bar:?}");
    send(&mut m, [key("/")]);
    send(&mut m, type_text("ledger"));
    assert!(row(&m).contains("/ ledger"), "{}", row(&m));
    // Too narrow for both: the typed filter wins.
    m.update(Msg::Resize {
        width: 60,
        height: 30,
    });
    send(&mut m, type_text(" long enough to push the tabs out"));
    let bar = row(&m);
    assert!(bar.starts_with(" / "), "{bar:?}");
    assert_eq!(crate::tui::text::width(&bar), 60);
}

#[test]
fn detail_frame_shows_tabs_and_search() {
    let mut m = list_model(list_fixture(), 100, 30);
    m.update(key("enter"));
    let v = frame_text(&m);
    assert!(v.contains(" 1 Result  2 Raw  3 Prompt  4 Info"), "{v}");
    assert!(v.contains(" rival › orbit-web › review a0000000"), "{v}");
    send(&mut m, [key("/"), key("q")]);
    let v = frame_text(&m);
    assert!(
        v.contains("/ q"),
        "the search input shows what was typed:\n{v}"
    );
    assert!(v.contains("enter search · esc clear"), "search help:\n{v}");
}
