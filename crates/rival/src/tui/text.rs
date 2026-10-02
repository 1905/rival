//! Terminal cell helpers. Every width here is display cells, never bytes or
//! chars: one CJK glyph is two cells, a combining mark none. Text is measured
//! and cut by grapheme cluster, the unit ratatui's buffer places, so a cut
//! never splits an emoji sequence, a flag or a base char from its marks.

use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// The display width of `s` in terminal cells: the sum of its grapheme
/// clusters' widths, as the buffer lays them out.
pub fn width(s: &str) -> usize {
    s.graphemes(true).map(UnicodeWidthStr::width).sum()
}

/// The display width of a styled line, measured like [`width`].
pub fn line_width(line: &Line<'_>) -> usize {
    line.spans.iter().map(|s| width(&s.content)).sum()
}

/// Go: `ansi.Truncate(s, w, tail)`. Cuts `s` to at most `w` cells. `tail` is
/// added only when `s` was cut, and it counts against `w`; a tail wider than
/// `w` leaves nothing.
pub fn truncate(s: &str, w: usize, tail: &str) -> String {
    if width(s) <= w {
        return s.to_string();
    }
    let tail_w = width(tail);
    if tail_w > w {
        return String::new();
    }
    let mut out = take_cells(s, w - tail_w).to_string();
    out.push_str(tail);
    out
}

/// Go: `fitCell`. Truncates `s` to `w` cells with an ellipsis and pads it to
/// exactly `w`.
pub fn fit_cell(s: &str, w: usize) -> String {
    if w == 0 {
        return String::new();
    }
    let mut out = truncate(s, w, "…");
    let pad = w.saturating_sub(width(&out));
    out.extend(std::iter::repeat_n(' ', pad));
    out
}

/// The longest prefix of `s` made of whole grapheme clusters that fits in
/// `w` cells. A wide cluster that would straddle the edge is dropped, never
/// split.
fn take_cells(s: &str, w: usize) -> &str {
    let mut used = 0;
    let mut end = 0;
    for (i, g) in s.grapheme_indices(true) {
        used += g.width();
        if used > w {
            break;
        }
        end = i + g.len();
    }
    &s[..end]
}

/// Cuts a styled line to at most `w` cells, keeping every span's style. No
/// tail is added.
pub fn fit_line(line: Line<'static>, w: usize) -> Line<'static> {
    if line_width(&line) <= w {
        return line;
    }
    let style = line.style;
    let alignment = line.alignment;
    let mut left = w;
    let mut spans = Vec::with_capacity(line.spans.len());
    for span in line.spans {
        if left == 0 {
            break;
        }
        let sw = width(&span.content);
        if sw <= left {
            left -= sw;
            spans.push(span);
            continue;
        }
        let cut = take_cells(&span.content, left).to_string();
        left = 0;
        if !cut.is_empty() {
            spans.push(Span::styled(cut, span.style));
        }
    }
    let mut out = Line::from(spans).style(style);
    out.alignment = alignment;
    out
}

/// Cuts or pads a styled line to exactly `w` cells. Padding is unstyled
/// spaces after the last span, so it inherits only the line style.
pub fn pad_line(line: Line<'static>, w: usize) -> Line<'static> {
    let mut line = fit_line(line, w);
    let pad = w.saturating_sub(line_width(&line));
    if pad > 0 {
        line.spans.push(Span::raw(" ".repeat(pad)));
    }
    line
}

/// Go: `fitCell` on a styled string. Cuts a styled line to `w` cells with an
/// ellipsis and pads it to exactly `w`, keeping every span's style.
pub fn fit_cell_line(line: Line<'static>, w: usize) -> Line<'static> {
    if w == 0 {
        return Line::default();
    }
    if line_width(&line) <= w {
        return pad_line(line, w);
    }
    let mut cut = fit_line(line, w - 1);
    cut.spans.push(Span::raw("…"));
    pad_line(cut, w)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::{Color, Style};

    const ZWJ_EMOJI: &str = "👩\u{200D}💻";
    const FLAG: &str = "🇺🇦";
    const E_ACUTE: &str = "e\u{301}";

    /// Whether `cut` (minus `tail`) is whole leading clusters of `s`.
    fn is_cluster_prefix(s: &str, cut: &str, tail: &str) -> bool {
        let body = cut.strip_suffix(tail).unwrap_or(cut);
        let src: Vec<&str> = s.graphemes(true).collect();
        let got: Vec<&str> = body.graphemes(true).collect();
        got.len() <= src.len() && src[..got.len()] == got[..]
    }

    #[test]
    fn truncate_counts_cells_not_bytes() {
        assert_eq!(truncate("abc", 5, "…"), "abc");
        assert_eq!(truncate("abcdef", 4, "…"), "abc…");
        assert_eq!(truncate("日本語", 4, ""), "日本");
        // A wide glyph never straddles the edge.
        assert_eq!(truncate("日本語", 3, ""), "日");
        assert_eq!(truncate("日本語", 3, "…"), "日…");
        assert_eq!(truncate("abc", 0, "…"), "");
        assert_eq!(truncate("✓ done", 3, ""), "✓ d");
    }

    #[test]
    fn truncate_keeps_grapheme_clusters_whole() {
        assert_eq!(width(ZWJ_EMOJI), 2);
        assert_eq!(width(FLAG), 2);
        assert_eq!(width(E_ACUTE), 1);

        let s = format!("{ZWJ_EMOJI}abcd");
        assert_eq!(truncate(&s, 3, "…"), format!("{ZWJ_EMOJI}…"));
        // Too narrow for the emoji: drop it whole, never leave a dangling
        // joiner.
        assert_eq!(truncate(&s, 2, "…"), "…");
        assert_eq!(truncate(&s, 1, ""), "");

        let flags = format!("{FLAG}{FLAG}{FLAG}");
        assert_eq!(truncate(&flags, 5, "…"), format!("{FLAG}{FLAG}…"));
        assert_eq!(truncate(&flags, 3, ""), FLAG);

        let marks = format!("{E_ACUTE}{E_ACUTE}{E_ACUTE}");
        assert_eq!(truncate(&marks, 2, "…"), format!("{E_ACUTE}…"));
        assert_eq!(truncate(&marks, 2, ""), format!("{E_ACUTE}{E_ACUTE}"));

        for s in [s.as_str(), flags.as_str(), marks.as_str(), "日本語"] {
            for w in 0..=8 {
                for tail in ["", "…"] {
                    let cut = truncate(s, w, tail);
                    assert!(width(&cut) <= w, "{s:?} w={w}: {cut:?}");
                    assert!(is_cluster_prefix(s, &cut, tail), "{s:?} w={w}: {cut:?}");
                }
            }
        }
    }

    #[test]
    fn fit_cell_pads_to_exact_width() {
        assert_eq!(fit_cell("ab", 4), "ab  ");
        assert_eq!(fit_cell("abcdef", 4), "abc…");
        assert_eq!(fit_cell("日本語", 5), "日本…");
        assert_eq!(width(&fit_cell("日本語", 4)), 4);
        assert_eq!(fit_cell("x", 0), "");
        assert_eq!(fit_cell(&format!("a{ZWJ_EMOJI}b"), 3), "a… ");
        assert_eq!(width(&fit_cell(&format!("{FLAG}{E_ACUTE}"), 4)), 4);
    }

    #[test]
    fn fit_line_keeps_styles_and_cuts_inside_a_span() {
        let red = Style::new().fg(Color::Red);
        let line = Line::from(vec![Span::styled("ab", red), Span::raw("日本語")]);
        let cut = fit_line(line, 5);
        assert_eq!(line_width(&cut), 4, "the wide glyph at the edge is dropped");
        assert_eq!(cut.spans[0].style, red);
        assert_eq!(cut.spans[1].content, "日");
        let padded = pad_line(cut, 6);
        assert_eq!(line_width(&padded), 6);
    }

    #[test]
    fn fit_line_keeps_clusters_whole() {
        let red = Style::new().fg(Color::Red);
        let line = Line::from(vec![
            Span::styled(format!("x{E_ACUTE}"), red),
            Span::raw(format!("{ZWJ_EMOJI}{FLAG}")),
        ]);
        assert_eq!(line_width(&line), 6);
        let cut = fit_line(line.clone(), 5);
        assert_eq!(cut.spans[0].content, format!("x{E_ACUTE}"));
        assert_eq!(cut.spans[0].style, red);
        assert_eq!(cut.spans[1].content, ZWJ_EMOJI);
        let cut = fit_line(line.clone(), 3);
        assert_eq!(cut.spans.len(), 1, "the emoji would straddle the edge");
        let cut = fit_line(line, 1);
        assert_eq!(
            cut.spans[0].content, "x",
            "the mark stays with its base or goes"
        );
        assert_eq!(line_width(&pad_line(cut, 4)), 4);
    }

    #[test]
    fn fit_cell_line_cuts_with_an_ellipsis_and_pads() {
        let red = Style::new().fg(Color::Red);
        let line = Line::from(vec![Span::styled("ab", red), Span::raw("日本語")]);
        let cut = fit_cell_line(line.clone(), 5);
        assert_eq!(cut.to_string(), "ab日…");
        assert_eq!(cut.spans[0].style, red);
        let cut = fit_cell_line(line.clone(), 6);
        assert_eq!(
            cut.to_string(),
            "ab日… ",
            "the wide glyph at the edge is dropped"
        );
        assert_eq!(line_width(&cut), 6);
        assert_eq!(fit_cell_line(line.clone(), 9).to_string(), "ab日本語 ");
        assert_eq!(fit_cell_line(line, 0).to_string(), "");
    }
}
