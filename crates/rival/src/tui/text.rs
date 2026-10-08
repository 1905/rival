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

/// Cuts `s` to at most `w` cells. `tail` is
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

/// Truncates `s` to `w` cells with an ellipsis and pads it to
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

/// Cuts a styled line to `w` cells with an
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

/// Truncates a styled line to `w` cells. `tail` is added, unstyled, only
/// when the line was cut, and counts against `w`.
pub fn truncate_line(line: Line<'static>, w: usize, tail: &str) -> Line<'static> {
    if line_width(&line) <= w {
        return line;
    }
    let tail_w = width(tail);
    if tail_w > w {
        return Line::default();
    }
    let mut cut = fit_line(line, w - tail_w);
    if !tail.is_empty() {
        cut.spans.push(Span::raw(tail.to_string()));
    }
    cut
}

/// Hard-wraps plain text. Breaks every line at `limit` cells and keeps
/// leading spaces. A cluster wider than the room left moves to the next line
/// whole, even when that leaves an empty line.
pub fn hardwrap(s: &str, limit: usize) -> String {
    if limit == 0 {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + s.len() / limit);
    let mut cur = 0;
    for g in s.graphemes(true) {
        if g == "\n" {
            out.push('\n');
            cur = 0;
            continue;
        }
        let w = g.width();
        if cur + w > limit {
            out.push('\n');
            cur = 0;
        }
        out.push_str(g);
        cur += w;
    }
    out
}

/// Word-wraps plain text. Breaks at spaces and hyphens, never inside a
/// word; a word longer than `limit` stays long for [`hardwrap`] to break.
/// Pending spaces count in bytes, not cells.
pub fn wordwrap(s: &str, limit: usize) -> String {
    if limit == 0 {
        return s.to_string();
    }
    struct State {
        buf: String,
        word: String,
        space: String,
        cur: usize,
        word_len: usize,
    }
    impl State {
        fn add_space(&mut self) {
            self.cur += self.space.len();
            self.buf.push_str(&self.space);
            self.space.clear();
        }
        fn add_word(&mut self) {
            if self.word.is_empty() {
                return;
            }
            self.add_space();
            self.cur += self.word_len;
            self.buf.push_str(&self.word);
            self.word.clear();
            self.word_len = 0;
        }
        fn add_newline(&mut self) {
            self.buf.push('\n');
            self.cur = 0;
            self.space.clear();
        }
    }
    let mut st = State {
        buf: String::with_capacity(s.len()),
        word: String::new(),
        space: String::new(),
        cur: 0,
        word_len: 0,
    };
    for g in s.graphemes(true) {
        let first = g.chars().next().unwrap_or(' ');
        if g == "\n" {
            if st.word_len == 0 {
                if st.cur + st.space.len() > limit {
                    st.cur = 0;
                } else {
                    let space = std::mem::take(&mut st.space);
                    st.buf.push_str(&space);
                }
                st.space.clear();
            }
            st.add_word();
            st.add_newline();
        } else if first.is_whitespace() && first != '\u{a0}' && g.chars().count() == 1 {
            st.add_word();
            st.space.push_str(g);
        } else if g == "-" {
            st.add_space();
            st.add_word();
            st.buf.push('-');
            st.cur += 1;
        } else {
            st.word.push_str(g);
            st.word_len += g.width();
            if st.cur + st.space.len() + st.word_len > limit && st.word_len < limit {
                st.add_newline();
            }
        }
    }
    st.add_word();
    st.buf
}

/// Word-wraps `text` to `width` cells, then hard-breaks
/// anything still longer, so no line exceeds `width`. Width 0 returns the
/// text as one line.
pub fn wrap_cells(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    hardwrap(&wordwrap(text, width), width)
        .split('\n')
        .map(str::to_string)
        .collect()
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

    #[test]
    fn truncate_line_adds_the_tail_only_when_cut() {
        let red = Style::new().fg(Color::Red);
        let line = Line::from(vec![Span::styled("abc", red), Span::raw("def")]);
        assert_eq!(truncate_line(line.clone(), 6, "…"), line);
        let cut = truncate_line(line.clone(), 4, "…");
        assert_eq!(cut.to_string(), "abc…");
        assert_eq!(cut.spans[0].style, red);
        assert_eq!(truncate_line(line.clone(), 2, "").to_string(), "ab");
        assert_eq!(truncate_line(line, 0, "…").to_string(), "");
    }

    // x/ansi v0.11.7 TestHardwrap, the plain-text cases with
    // preserveSpace = true.
    #[test]
    fn hardwrap_ansi_upstream_cases() {
        let cases: &[(&str, &str, usize, &str)] = &[
            ("empty string", "", 0, ""),
            ("passthrough", "foobar\n ", 0, "foobar\n "),
            ("pass", "foo", 4, "foo"),
            ("simple", "foobarfoo", 4, "foob\narfo\no"),
            ("lf", "f\no\nobar", 3, "f\no\noba\nr"),
            ("lf_space", "foo bar\n  baz", 3, "foo\n ba\nr\n  b\naz"),
            ("begin_with_space", " foo", 4, " foo"),
            ("emoji", "foo🫧foobar", 4, "foo\n🫧fo\nobar"),
            ("column", "VERTICAL", 1, "V\nE\nR\nT\nI\nC\nA\nL"),
        ];
        for (name, input, limit, want) in cases {
            assert_eq!(hardwrap(input, *limit), *want, "{name}");
        }
        // A wide glyph that cannot fit moves whole, never split.
        assert_eq!(hardwrap("a日本", 2), "a\n日\n本");
    }

    // x/ansi v0.11.7 TestWordwrap, the plain-text cases (a hyphen is always
    // a breakpoint, so the "-" cases need no breakpoint argument).
    #[test]
    fn wordwrap_ansi_upstream_cases() {
        let cases: &[(&str, &str, usize, &str)] = &[
            ("empty string", "", 0, ""),
            ("passthrough", "foobar\n ", 0, "foobar\n "),
            ("pass", "foo", 3, "foo"),
            ("toolong", "foobarfoo", 4, "foobarfoo"),
            ("white space", "foo bar foo", 4, "foo\nbar\nfoo"),
            (
                "broken_at_spaces",
                "foo bars foobars",
                4,
                "foo\nbars\nfoobars",
            ),
            ("hyphen", "foo-foobar", 4, "foo-\nfoobar"),
            ("space_breakpoint", "foo --bar", 9, "foo --bar"),
            ("simple", "foo bars foobars", 4, "foo\nbars\nfoobars"),
            ("limit", "foo bar", 5, "foo\nbar"),
            ("remove white spaces", "foo    \nb   ar   ", 4, "foo\nb\nar"),
            (
                "white space trail width",
                "foo\nb\t a\n bar",
                4,
                "foo\nb\t a\n bar",
            ),
            ("explicit_line_break", "foo bar foo\n", 4, "foo\nbar\nfoo\n"),
            (
                "explicit_breaks",
                "\nfoo bar\n\n\nfoo\n",
                4,
                "\nfoo\nbar\n\n\nfoo\n",
            ),
            (
                "example",
                " This is a list: \n\n\t* foo\n\t* bar\n\n\n\t* foo  \nbar    ",
                6,
                " This\nis a\nlist: \n\n\t* foo\n\t* bar\n\n\n\t* foo\nbar",
            ),
        ];
        for (name, input, limit, want) in cases {
            assert_eq!(wordwrap(input, *limit), *want, "{name}");
        }
    }

    #[test]
    fn wrap_cells_never_exceeds_the_width() {
        assert_eq!(wrap_cells("hello wide world", 0), ["hello wide world"]);
        assert_eq!(wrap_cells("hello wide world", 10), ["hello wide", "world"]);
        // The long word stays whole for the word wrap, so "mn" goes to its
        // own line before the hard wrap breaks the word.
        assert_eq!(
            wrap_cells("abcdefghijkl mn", 5),
            ["abcde", "fghij", "kl", "mn"]
        );
        let text = format!("{} {}", "日本語".repeat(9), "word ".repeat(30));
        for w in 1..40 {
            for l in wrap_cells(&text, w) {
                assert!(width(&l) <= w.max(2), "{w}: {l:?}");
                if w >= 2 {
                    assert!(width(&l) <= w, "{w}: {l:?}");
                }
            }
        }
    }
}
