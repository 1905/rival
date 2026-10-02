use super::*;
use crate::tui::session_list::layout_columns;

#[test]
fn compute_layout_wide_shows_preview() {
    let l = compute_layout(200, 50);
    assert!(l.show_preview, "200 cols: preview hidden");
    assert_eq!(l.list_w + l.preview_w + 1, 200);
    assert_eq!(l.list_w, 110, "55% of 200");
    assert!(!l.compact, "50 rows: header collapsed");
}

#[test]
fn compute_layout_preview_threshold() {
    let l = compute_layout(119, 50);
    assert!(
        !l.show_preview && l.list_w == 119 && l.preview_w == 0,
        "119 cols: {l:?}, want no preview and a full-width list"
    );
    let l = compute_layout(120, 50);
    assert!(l.show_preview, "120 cols: preview hidden");
    assert!(
        l.list_w >= MIN_WIDTH,
        "120 cols: list_w {} below the minimum",
        l.list_w
    );
    assert_eq!(l.list_w + l.preview_w + 1, 120);
}

#[test]
fn compute_layout_compact() {
    let l = compute_layout(120, 29);
    assert!(
        l.compact && l.header_h == 1,
        "120×29: {l:?}, want compact with header_h 1"
    );
}

#[test]
fn compute_layout_too_small() {
    for (w, h) in [(59, 30), (80, 15)] {
        assert!(compute_layout(w, h).too_small, "{w}×{h}: not too_small");
    }
    assert!(
        !compute_layout(60, 16).too_small,
        "60×16 is the minimum and must fit"
    );
}

#[test]
fn compute_layout_rows_add_up() {
    for w in [60, 90, 119, 120, 200] {
        for h in [16, 24, 29, 30, 50] {
            let l = compute_layout(w, h);
            if l.too_small {
                continue;
            }
            assert_eq!(l.header_h + 1 + l.body_h + 1, h, "{w}×{h}: {l:?}");
        }
    }
}

/// At 120-145 cols a bare 55% split left the list under its fixed columns
/// and silently dropped EFF and PROJECT; the split must keep them.
#[test]
fn split_list_keeps_every_fixed_column() {
    for w in [120, 130, 145, 200] {
        let l = compute_layout(w, 40);
        assert!(l.show_preview, "width {w}: want preview");
        let c = layout_columns(l.list_w - 2);
        assert!(
            c.effort > 0 && c.project > 0,
            "width {w}: list inner {} drops columns: {c:?}",
            l.list_w - 2
        );
        assert!(
            l.preview_w >= 40,
            "width {w}: preview only {} cols",
            l.preview_w
        );
    }
}

#[test]
fn degenerate_sizes_never_underflow() {
    for (w, h) in [(0, 0), (1, 1), (0, 50), (200, 0), (120, 3)] {
        let l = compute_layout(w, h);
        assert!(l.too_small, "{w}×{h}");
        assert_eq!(l.body_h, h.saturating_sub(l.header_h + 2), "{w}×{h}");
    }
}
