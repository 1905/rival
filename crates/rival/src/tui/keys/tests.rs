use super::*;
use crate::tui::styles::STYLES;
use crate::tui::testkit::press;

/// Compares bindings by keys + help key only.
fn has_binding(list: &[Binding], want: Binding) -> bool {
    list.iter()
        .any(|b| b.keys == want.keys && b.help_key == want.help_key)
}

fn flatten(groups: Vec<Vec<Binding>>) -> Vec<Binding> {
    groups.into_iter().flatten().collect()
}

#[test]
fn default_keys_are_all_bound_and_documented() {
    for (name, b) in KeyMap::default().all() {
        assert!(!b.keys.is_empty(), "{name} has no keys");
        assert!(
            !b.help_key.is_empty() && !b.help_desc.is_empty(),
            "{name} has no help text: {b:?}"
        );
    }
}

#[test]
fn list_short_help() {
    let k = KeyMap::default();
    let short = k.help(Mode::List).short;
    for (name, b) in [
        ("up", k.up),
        ("down", k.down),
        ("open", k.open),
        ("filter", k.filter),
        ("next_tab", k.next_tab),
        ("help", k.help),
        ("quit", k.quit),
    ] {
        assert!(has_binding(&short, b), "list short help omits {name}");
    }
}

#[test]
fn detail_full_help() {
    let k = KeyMap::default();
    let full = flatten(k.help(Mode::Detail).full);
    for (name, b) in [
        ("follow", k.follow),
        ("search", k.search),
        ("next_member", k.next_member),
        ("open_log", k.open_log),
        ("stop", k.stop),
        ("tab_result", k.tab_result),
        ("tab_raw", k.tab_raw),
        ("tab_prompt", k.tab_prompt),
        ("tab_info", k.tab_info),
    ] {
        assert!(has_binding(&full, b), "detail full help omits {name}");
    }
}

#[test]
fn confirm_short_help_is_yes_no() {
    let k = KeyMap::default();
    let short = k.help(Mode::Confirm).short;
    assert!(
        short.len() == 2 && has_binding(&short[..1], k.yes) && has_binding(&short[1..], k.no),
        "confirm short help = {short:?}, want exactly Yes, No"
    );
}

#[test]
fn every_mode_has_help() {
    let k = KeyMap::default();
    for m in Mode::ALL {
        let h = k.help(m);
        assert!(
            !h.short.is_empty() && !h.full.is_empty(),
            "mode {m:?} has empty help"
        );
    }
}

#[test]
fn list_full_help_has_page_keys() {
    let k = KeyMap::default();
    let full = flatten(k.help(Mode::List).full);
    for (name, b) in [("next_page", k.next_page), ("prev_page", k.prev_page)] {
        assert!(has_binding(&full, b), "list full help omits {name}");
    }
}

#[test]
fn detail_tab_keys_map_to_the_shared_enum() {
    let k = KeyMap::default();
    assert_eq!(k.detail_tab("1"), Some(DetailTab::Result));
    assert_eq!(k.detail_tab("2"), Some(DetailTab::Raw));
    assert_eq!(k.detail_tab("3"), Some(DetailTab::Prompt));
    assert_eq!(k.detail_tab("4"), Some(DetailTab::Info));
    assert_eq!(k.detail_tab("5"), None);
}

#[test]
fn key_names_follow_bubbletea() {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    let cases = [
        (press("q"), "q"),
        (press("G"), "G"),
        (press("?"), "?"),
        (press("enter"), "enter"),
        (press("esc"), "esc"),
        (press("tab"), "tab"),
        (press("shift+tab"), "shift+tab"),
        (press("ctrl+c"), "ctrl+c"),
        (press("pgdown"), "pgdown"),
        (press("pgup"), "pgup"),
        (press("space"), "space"),
        (
            KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT),
            "shift+tab",
        ),
        (
            KeyEvent::new(
                KeyCode::Char('C'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
            "ctrl+c",
        ),
        (
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT),
            "alt+backspace",
        ),
    ];
    for (ev, want) in cases {
        assert_eq!(key_name(&ev).as_deref(), Some(want), "{ev:?}");
    }
    let mut release = press("q");
    release.kind = KeyEventKind::Release;
    assert_eq!(key_name(&release), None, "a release is not a press");
}

#[test]
fn help_lines_fit_the_width() {
    let k = KeyMap::default();
    for (m, findings) in Mode::ALL.into_iter().flat_map(|m| [(m, false), (m, true)]) {
        for show_all in [false, true] {
            for w in [0, 1, 5, 20, 59, 60, 90, 200] {
                let lines = help_lines(&k.help_for(m, findings), show_all, w, &STYLES);
                assert!(!lines.is_empty(), "{m:?} w={w}: no help line");
                for (i, l) in lines.iter().enumerate() {
                    assert!(
                        l.width() <= w,
                        "{m:?} all={show_all} w={w}: line {i} is {} cells",
                        l.width()
                    );
                }
            }
        }
    }
}

#[test]
fn short_help_is_one_line_cut_with_an_ellipsis() {
    let k = KeyMap::default();
    let line = &help_lines(&k.help(Mode::List), false, 200, &STYLES)[0];
    assert_eq!(
        line.to_string(),
        "↑/k up · ↓/j down · n/p page · enter open · / filter · tab status · ? more · q quit"
    );
    // Too narrow for every entry: the rest becomes " …".
    let line = &help_lines(&k.help(Mode::List), false, 24, &STYLES)[0];
    assert_eq!(line.to_string(), "↑/k up · ↓/j down …");
}

#[test]
fn full_help_is_one_column_per_group() {
    let k = KeyMap::default();
    let lines = help_lines(&k.help(Mode::List), true, 200, &STYLES);
    assert_eq!(lines.len(), 4, "the tallest list group has 4 bindings");
    let text: Vec<String> = lines.iter().map(ToString::to_string).collect();
    assert!(text[0].starts_with("↑/k up    "), "{text:?}");
    assert!(text[0].contains("n/pgdn next page"), "{text:?}");
    // Every column starts at the same cell on each row.
    let col = text[0].find("n/pgdn").unwrap();
    assert_eq!(text[1].find("p/pgup"), Some(col), "{text:?}");
    let detail = help_lines(&k.help(Mode::Detail), true, 200, &STYLES);
    assert_eq!(detail.len(), 6);
    let filter = help_lines(&k.help(Mode::Filter), true, 200, &STYLES);
    assert_eq!(filter.len(), 2, "simple help expands to one 2-row column");
}

/// A Result tab with findings advertises its focus and toggle keys in place
/// of follow; the expanded help keeps its height. Elsewhere nothing changes.
#[test]
fn result_help_lists_the_finding_keys() {
    let k = KeyMap::default();
    let help = k.help_for(Mode::Detail, true);
    let line = &help_lines(&help, false, 200, &STYLES)[0];
    assert_eq!(
        line.to_string(),
        "1-4 tab · j/k finding · enter open · [/] member · / search · o open log · x stop · esc back · ? more"
    );
    assert!(!has_binding(&help.short, k.follow));
    let full = flatten(help.full.clone());
    assert!(has_binding(&full, k.toggle), "full help omits toggle");
    assert_eq!(
        help_lines(&help, true, 200, &STYLES).len(),
        help_lines(&k.help(Mode::Detail), true, 200, &STYLES).len(),
    );
    for m in Mode::ALL {
        assert_eq!(k.help_for(m, false), k.help(m), "{m:?}");
        if m != Mode::Detail {
            assert_eq!(k.help_for(m, true), k.help(m), "{m:?}");
        }
    }
    assert!(k.toggle.matches("enter") && k.toggle.matches("space"));
}
