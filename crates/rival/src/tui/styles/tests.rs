use std::collections::HashSet;

use super::*;

fn fg_colors(lines: &[Line<'_>]) -> HashSet<Color> {
    lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .filter_map(|s| s.style.fg)
        .collect()
}

#[test]
fn palette_matches_the_app_theme() {
    // Theme.swift values; the app and the TUI must not drift apart.
    assert_eq!(STYLES.text.fg, Some(Color::Rgb(0xB4, 0xC2, 0xB7)));
    assert_eq!(STYLES.dim.fg, Some(Color::Rgb(0x4F, 0x61, 0x54)));
    assert_eq!(STYLES.accent.fg, Some(Color::Rgb(0x6F, 0xC9, 0x85)));
    assert_eq!(STYLES.running.fg, Some(Color::Rgb(0xD6, 0xA3, 0x4E)));
    assert_eq!(STYLES.queued.fg, Some(Color::Rgb(0x6A, 0x7F, 0x70)));
    assert_eq!(STYLES.completed.fg, Some(Color::Rgb(0x7F, 0x94, 0x83)));
    assert_eq!(STYLES.failed.fg, Some(Color::Rgb(0xDB, 0x6B, 0x6B)));
    assert_eq!(STYLES.selected.bg, Some(Color::Rgb(0x16, 0x24, 0x1A)));
    assert_eq!(STYLES.selected.fg, Some(Color::Rgb(0xE2, 0xED, 0xE4)));
    assert!(STYLES.selected.add_modifier.contains(Modifier::BOLD));
    assert!(STYLES.running.add_modifier.contains(Modifier::BOLD));
    assert!(STYLES.heading.add_modifier.contains(Modifier::BOLD));
    assert_eq!(STYLES.heading.fg, STYLES.accent.fg);
}

#[test]
fn blend_hex_matches_swift() {
    assert_eq!(blend_hex(&LOGO_STOPS, 0.0), 0x8B6CD9);
    assert_eq!(blend_hex(&LOGO_STOPS, 0.5), 0x4FB8CC);
    assert_eq!(blend_hex(&LOGO_STOPS, 1.0), 0x6FC985);
    assert_eq!(blend_hex(&LOGO_STOPS, 7.0), 0x6FC985, "t clamps");
    // Halfway between violet and cyan: per channel round((a + b) / 2).
    assert_eq!(blend_hex(&LOGO_STOPS, 0.25), 0x6D92D3);
    assert_eq!(blend_hex(&[], 0.5), 0);
    assert_eq!(blend_hex(&[0x123456], 0.5), 0x123456);
}

#[test]
fn logo_applies_gradient_and_keeps_width() {
    let logo = logo_lines();
    assert_eq!(logo.len(), BANNER_LINES.len());
    for (i, line) in logo.iter().enumerate() {
        assert_eq!(line.width(), banner_width(), "logo line {i}");
        assert_eq!(line.to_string().trim_end(), BANNER_LINES[i]);
    }
    let colors = fg_colors(logo);
    assert!(
        colors.len() >= 3,
        "logo carries {} colours, want a gradient",
        colors.len()
    );
    assert!(
        std::ptr::eq(logo, logo_lines()),
        "the logo must be built once and cached"
    );
}

#[test]
fn render_header_wide() {
    let st = HeaderStats {
        running: 1,
        queued: 7,
        completed: 2684,
        failed: 296,
        total: 2988,
        version: "3.34.0".into(),
        loading: false,
    };
    let lines = render_header(200, false, &st, "⠋", &STYLES);
    assert_eq!(lines.len(), BANNER_LINES.len());
    for (i, line) in lines.iter().enumerate() {
        assert!(
            line.width() <= 200,
            "header line {i} is {} cells",
            line.width()
        );
    }
    let text: Vec<String> = lines.iter().map(ToString::to_string).collect();
    assert!(text[1].ends_with("⠋ 1 running  ◌ 7 queued"), "{text:?}");
    assert!(text[2].contains("✓ 2684  ✗ 296"), "{text:?}");
    assert!(text[3].contains("2988 sessions  3.34.0"), "{text:?}");
    // The stats block is right-aligned as a whole: its lines share a left
    // edge, and the widest ends at the terminal edge.
    let edge = |s: &str| s.find(['⠋', '✓', '2']).unwrap();
    assert_eq!(edge(&text[1]), edge(&text[2]), "{text:?}");
    assert_eq!(lines[1].width(), 200);
}

#[test]
fn render_header_compact_is_one_line() {
    let st = HeaderStats {
        running: 1,
        completed: 2684,
        failed: 296,
        total: 2981,
        version: "3.34.0".into(),
        ..HeaderStats::default()
    };
    let lines = render_header(80, true, &st, "⠋", &STYLES);
    assert_eq!(lines.len(), 1);
    assert!(lines[0].width() <= 80);
    assert_eq!(
        lines[0].to_string(),
        "rival  ⠋ 1 running  ◌ 0 queued  ✓ 2684  ✗ 296  2981 sessions  3.34.0"
    );
}

#[test]
fn render_header_never_exceeds_narrow_width() {
    let st = HeaderStats {
        running: 12,
        queued: 3,
        completed: 123_456,
        failed: 7890,
        total: 131_361,
        version: "3.34.0-dirty".into(),
        loading: false,
    };
    for compact in [false, true] {
        for w in [59, 40, 20, 1] {
            for (i, line) in render_header(w, compact, &st, "⠋", &STYLES)
                .iter()
                .enumerate()
            {
                assert!(
                    line.width() <= w,
                    "compact={compact} w={w}: line {i} is {} cells",
                    line.width()
                );
            }
        }
    }
    assert!(render_header(0, false, &st, "", &STYLES).is_empty());
}

#[test]
fn header_loading_shows_dots_and_idle_shows_a_dot() {
    let st = HeaderStats {
        loading: true,
        ..HeaderStats::default()
    };
    let line = render_header(120, true, &st, "", &STYLES)[0].to_string();
    assert!(
        line.contains("● … running  ◌ … queued  ✓ …  ✗ …  … sessions"),
        "{line}"
    );
}

#[test]
fn status_styles_colour_each_status() {
    let mut seen = HashSet::new();
    for status in ["running", "queued", "completed", "failed"] {
        let style = STYLES.status(status);
        assert_ne!(style, STYLES.text, "{status} is unstyled");
        assert!(seen.insert(style), "{status} looks like another status");
    }
    assert_eq!(STYLES.status("weird"), STYLES.text);
}

#[test]
fn gradient_bar_fills_by_percent() {
    let count = |line: &Line<'_>, c: char| line.to_string().chars().filter(|&x| x == c).count();
    let bar = gradient_bar(10, 0.41, &STYLES);
    assert_eq!(bar.width(), 10);
    assert_eq!((count(&bar, '█'), count(&bar, '░')), (4, 6));
    let full = gradient_bar(48, 1.0, &STYLES);
    assert_eq!(count(&full, '█'), 48);
    assert!(
        fg_colors(std::slice::from_ref(&full)).len() >= 3,
        "filled part is a gradient"
    );
    assert_eq!(count(&gradient_bar(5, 0.0, &STYLES), '░'), 5);
    assert_eq!(gradient_bar(0, 0.5, &STYLES).width(), 0);
}

#[test]
fn gradient_word_colours_each_char() {
    let spans = gradient_word("rival");
    assert_eq!(spans.len(), 5);
    assert_eq!(spans[0].style.fg, Some(rgb(LOGO_STOPS[0])));
    assert_eq!(spans[4].style.fg, Some(rgb(LOGO_STOPS[2])));
}
