use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Text};
use unicode_segmentation::UnicodeSegmentation;

use super::render;
use crate::tui::styles::STYLES as T;
use crate::tui::text::{line_width, width};

/// The plain text of every line.
fn plain(text: &Text<'_>) -> Vec<String> {
    text.lines.iter().map(Line::to_string).collect()
}

/// Each span of a line as (content, style).
fn spans(line: &Line<'_>) -> Vec<(String, Style)> {
    line.spans
        .iter()
        .map(|s| (s.content.to_string(), s.style))
        .collect()
}

fn s(content: &str, style: Style) -> (String, Style) {
    (content.to_string(), style)
}

fn bold(style: Style) -> Style {
    style.add_modifier(Modifier::BOLD)
}

fn italic(style: Style) -> Style {
    style.add_modifier(Modifier::ITALIC)
}

const ZWJ_EMOJI: &str = "👩\u{200D}💻";
const FLAG: &str = "🇺🇦";
const E_ACUTE: &str = "e\u{301}";

#[test]
fn empty_input_has_no_lines() {
    assert!(render("", 40, &T).lines.is_empty());
    assert!(render("\n\n  \n", 40, &T).lines.is_empty());
}

#[test]
fn headings_are_bold_accent() {
    let out = render("# Title\n\n### Sub *part*", 40, &T);
    assert_eq!(plain(&out), ["Title", "", "Sub part"]);
    assert_eq!(spans(&out.lines[0]), [s("Title", T.heading)]);
    assert_eq!(
        spans(&out.lines[2]),
        [s("Sub ", T.heading), s("part", italic(T.heading))]
    );
    assert_eq!(T.heading, bold(T.accent));
}

#[test]
fn paragraphs_wrap_at_spaces_and_keep_one_gap() {
    let out = render(
        "The quick brown fox jumps over the lazy dog\n\n\n\nsoft\nbreak joins",
        16,
        &T,
    );
    assert_eq!(
        plain(&out),
        [
            "The quick brown",
            "fox jumps over",
            "the lazy dog",
            "",
            "soft break joins"
        ]
    );
    assert_eq!(spans(&out.lines[0]), [s("The quick brown", T.text)]);
    assert!(out.lines[3].spans.is_empty(), "the gap row is empty");
}

#[test]
fn hard_break_starts_a_new_line() {
    let out = render("one  \ntwo\\\nthree", 40, &T);
    assert_eq!(plain(&out), ["one", "two", "three"]);
}

#[test]
fn bullet_items_hang_under_their_text() {
    let out = render("- alpha beta gamma delta\n* star item", 12, &T);
    assert_eq!(
        plain(&out),
        ["• alpha beta", "  gamma", "  delta", "", "• star item"],
        "a new bullet character starts a new list"
    );
    assert_eq!(
        spans(&out.lines[0]),
        [s("• ", T.dim), s("alpha beta", T.text)]
    );
    assert_eq!(
        spans(&out.lines[1]),
        [s("  ", Style::new()), s("gamma", T.text)]
    );
}

#[test]
fn ordered_items_keep_numbers_and_join_continuations() {
    let md = "1. first item text\n   continues here\nlazy line\n2. second";
    let out = render(md, 20, &T);
    assert_eq!(
        plain(&out),
        [
            "1. first item text",
            "   continues here",
            "   lazy line",
            "2. second",
        ]
    );
    assert_eq!(spans(&out.lines[3])[0], s("2. ", T.dim));

    // The list starts at its first number; a wider number gets a wider
    // hanging indent.
    let out = render("9. nine words to\n10. ten words to wrap", 12, &T);
    assert_eq!(
        plain(&out),
        [
            "9. nine",
            "   words to",
            "10. ten",
            "    words to",
            "    wrap"
        ]
    );
}

#[test]
fn nested_lists_indent_under_the_parent_text() {
    let out = render("- outer\n  - inner text\n    1. deep\n- next", 40, &T);
    assert_eq!(
        plain(&out),
        ["• outer", "  • inner text", "    1. deep", "• next"]
    );
}

#[test]
fn list_paragraphs_inside_an_item_keep_a_gap() {
    let out = render("- first para\n\n  second para\n- next", 40, &T);
    assert_eq!(plain(&out), ["• first para", "", "  second para", "• next"]);
}

#[test]
fn empty_item_shows_its_marker() {
    let out = render("-\n- b", 40, &T);
    assert_eq!(plain(&out), ["•", "• b"]);
}

#[test]
fn fenced_code_is_dim_unwrapped_and_keeps_whitespace() {
    let md = "intro\n\n```rust\nlet  x =\tfoo(); // a long line that must not wrap\n\n    indented\n```\nafter";
    let out = render(md, 10, &T);
    assert_eq!(
        plain(&out),
        [
            "intro",
            "",
            "  let  x =    foo(); // a long line that must not wrap",
            "",
            "      indented",
            "",
            "after",
        ]
    );
    assert_eq!(
        spans(&out.lines[2]),
        [s(
            "  let  x =    foo(); // a long line that must not wrap",
            T.code_block
        )]
    );
    assert_eq!(spans(&out.lines[4]), [s("      indented", T.code_block)]);
    assert!(out.lines[3].spans.is_empty());
    assert_eq!(T.code_block, T.dim);
}

#[test]
fn fenced_code_inside_an_item_keeps_the_indent() {
    let out = render("- run:\n\n  ```\n  make test\n  ```", 40, &T);
    assert_eq!(plain(&out), ["• run:", "", "    make test"]);
}

#[test]
fn inline_code_is_accent() {
    let out = render("use `x()`, then `a  b`", 40, &T);
    assert_eq!(
        spans(&out.lines[0]),
        [
            s("use ", T.text),
            s("x()", T.inline_code),
            s(", then ", T.text),
            s("a  b", T.inline_code),
        ],
        "a code span keeps its inner spaces"
    );
    assert_eq!(T.inline_code, T.accent);
}

#[test]
fn bold_italic_and_code_nest() {
    let out = render("a **bold *both* `code`** _it_", 40, &T);
    let accent_bold = Style::new()
        .fg(T.accent.fg.unwrap())
        .add_modifier(Modifier::BOLD);
    assert_eq!(
        spans(&out.lines[0]),
        [
            s("a ", T.text),
            s("bold ", bold(T.text)),
            s("both", italic(bold(T.text))),
            s(" ", bold(T.text)),
            s("code", accent_bold),
            s(" ", T.text),
            s("it", italic(T.text)),
        ]
    );
}

#[test]
fn links_show_text_then_url() {
    let out = render(
        "see [the docs](https://x.dev/a) and <https://y.dev>",
        80,
        &T,
    );
    assert_eq!(
        plain(&out),
        ["see the docs (https://x.dev/a) and https://y.dev"]
    );
    assert_eq!(
        spans(&out.lines[0]),
        [
            s("see the docs", T.text),
            s(" (https://x.dev/a)", T.dim),
            s(" and https://y.dev", T.text),
        ]
    );
}

#[test]
fn link_url_wraps_like_a_word() {
    let out = render("read [docs](https://example.com/x)", 12, &T);
    assert_eq!(plain(&out), ["read docs", "(https://exa", "mple.com/x)"]);
}

#[test]
fn html_is_literal_text() {
    let md = "<div>\n*hi* <script>x</script>\n</div>\n\ntext <b>bold</b> &amp; end";
    let out = render(md, 80, &T);
    assert_eq!(
        plain(&out),
        [
            "<div>",
            "*hi* <script>x</script>",
            "</div>",
            "",
            "text <b>bold</b> & end",
        ]
    );
    assert_eq!(spans(&out.lines[4]), [s("text <b>bold</b> & end", T.text)]);
}

/// Whether any span of `text` holds a control char.
fn has_control(text: &Text<'_>) -> bool {
    text.lines
        .iter()
        .flat_map(|l| &l.spans)
        .any(|s| s.content.chars().any(char::is_control))
}

#[test]
fn decoded_entities_never_reach_the_terminal() {
    // CommonMark decodes numeric entities after the result parser's
    // sanitize: an ESC, BEL or C1 CSI here must not survive.
    let cases: &[(&str, &[&str])] = &[
        ("a &#27;[31mred&#27;[0m b &#7;bell", &["a red b bell"]),
        (
            "&#27;]8;;http://evil&#7;click&#27;]8;;&#7; here",
            &["click here"],
        ),
        ("x &#155;[2J y", &["x [2J y"]),
        ("# T&#27;[1mitle&#8;", &["Title"]),
        ("[t](http://x/&#27;[31m)", &["t (http://x/)"]),
        // Literal escapes, not entities, are cut the same way.
        ("raw \u{1b}[2Jtext\u{7}", &["raw text"]),
        ("`\u{1b}[31mcode`", &["code"]),
        // A code span does not decode entities: the text shows as typed.
        ("`&#27;`", &["&#27;"]),
        ("<div>\u{1b}[31m\n</div>", &["<div>", "</div>"]),
        ("```\n\tx\u{1b}[31my\u{7}\n```", &["      xy"]),
    ];
    for (md, want) in cases {
        let out = render(md, 40, &T);
        assert_eq!(plain(&out), *want, "{md:?}");
        assert!(!has_control(&out), "{md:?}");
    }
}

#[test]
fn quotes_and_rules() {
    let out = render("> quoted line\n>\n> more\n\n---\n\nafter", 10, &T);
    assert_eq!(
        plain(&out),
        [
            "│ quoted",
            "│ line",
            "│",
            "│ more",
            "",
            "──────────",
            "",
            "after"
        ]
    );
    assert_eq!(spans(&out.lines[0])[0], s("│ ", T.dim));
}

#[test]
fn cjk_wraps_between_wide_glyphs() {
    let out = render("日本語のテキスト", 6, &T);
    assert_eq!(plain(&out), ["日本語", "のテキ", "スト"]);
    let out = render("abc日本", 4, &T);
    assert_eq!(plain(&out), ["abc", "日本"]);
    // An odd width leaves a cell free rather than splitting a glyph.
    let out = render("日本語", 5, &T);
    assert_eq!(plain(&out), ["日本", "語"]);
}

#[test]
fn clusters_are_never_split() {
    let md = format!("a {ZWJ_EMOJI}{ZWJ_EMOJI} {FLAG}{FLAG} {E_ACUTE}{E_ACUTE}{E_ACUTE}");
    let out = render(&md, 3, &T);
    let want = [
        "a".to_string(),
        ZWJ_EMOJI.to_string(),
        ZWJ_EMOJI.to_string(),
        FLAG.to_string(),
        FLAG.to_string(),
        E_ACUTE.repeat(3),
    ];
    assert_eq!(plain(&out), want);
    for line in &out.lines {
        assert!(line_width(line) <= 3, "{line:?}");
    }
}

#[test]
fn narrow_widths_break_long_words() {
    assert_eq!(plain(&render("abc de", 1, &T)), ["a", "b", "c", "d", "e"]);
    // The prefix fills the pane: content still gets one cell per line.
    assert_eq!(plain(&render("- abc", 2, &T)), ["• a", "  b", "  c"]);
    // Width 0 does not wrap.
    assert_eq!(
        plain(&render("a long line of words", 0, &T)),
        ["a long line of words"]
    );
}

#[test]
fn widths_zero_and_one_stay_bounded() {
    // A glyph wider than the room sits alone on its line, whole.
    assert_eq!(plain(&render("日本", 1, &T)), ["日", "本"]);
    let md = format!("{ZWJ_EMOJI}{FLAG}x{E_ACUTE}");
    assert_eq!(plain(&render(&md, 1, &T)), [ZWJ_EMOJI, FLAG, "x", E_ACUTE]);

    // A nested prefix wider than the pane: one content cluster per line.
    assert_eq!(plain(&render("- - - ab", 2, &T)), ["• • • a", "      b"]);
    assert_eq!(plain(&render("- - - ab cd", 0, &T)), ["• • • ab cd"]);

    // Every line takes at least one cluster: never more lines than clusters,
    // and never an empty content line.
    let words = format!("{} {}", "x".repeat(200), "日本語".repeat(20));
    let md = format!("- - - - {words}");
    let clusters = words.graphemes(true).filter(|g| *g != " ").count();
    for w in [0u16, 1, 2, 5] {
        let out = render(&md, w, &T);
        assert!(out.lines.len() <= clusters, "w={w}: {}", out.lines.len());
        for line in &out.lines {
            assert!(line.to_string().trim_start().chars().count() > 0, "w={w}");
        }
    }
    assert_eq!(render(&md, 1, &T).lines.len(), clusters);
}

#[test]
fn wrapped_lines_fit_the_width() {
    let md = format!(
        "# {}\n\n{} {}\n\n- {}\n  1. {}",
        "heading ".repeat(6),
        "日本語".repeat(5),
        "word ".repeat(20),
        "item text ".repeat(8),
        "nested words ".repeat(6),
    );
    for w in 6..60 {
        for line in &render(&md, w, &T).lines {
            assert!(line_width(line) <= usize::from(w), "w={w}: {line:?}");
        }
    }
}

// Swift: ResultParserTests.testMarkdownBlocks, same input and the same
// blocks in the same order. CommonMark differences: the "*" bullet nests by
// its column, and "12." and "3)" each open a new list (a new delimiter is a
// new list), so a blank row separates them. As in Swift, "3)" shows as "3.".
// The fenced block's blank last line is code and stays.
#[test]
fn swift_markdown_blocks() {
    let md = "# Title\nIntro line one\nintro line two\n\n- dash item\n  * nested star\n12. numbered\n3) paren\n\n**bold** start is a paragraph\n#nospace is a paragraph\n```swift\nlet x = 1\n\n```\n```\nunclosed";
    let out = render(md, 80, &T);
    assert_eq!(
        plain(&out),
        [
            "Title",
            "",
            "Intro line one intro line two",
            "",
            "• dash item",
            "  • nested star",
            "",
            "12. numbered",
            "",
            "3. paren",
            "",
            "bold start is a paragraph #nospace is a paragraph",
            "",
            "  let x = 1",
            "",
            "",
            "  unclosed",
        ]
    );
    assert_eq!(
        spans(&out.lines[11]),
        [
            s("bold", bold(T.text)),
            s(" start is a paragraph #nospace is a paragraph", T.text)
        ]
    );

    // Inside a paragraph only "1." starts a list; Swift made "12." an item.
    let out = render("text\n12. not an item\n\ntext\n1. item\n2. next", 80, &T);
    assert_eq!(
        plain(&out),
        ["text 12. not an item", "", "text", "", "1. item", "2. next"]
    );
}

// Swift: ResultParserTests.testMarkdownWrappedListItemContinues.
#[test]
fn swift_wrapped_list_item_continues() {
    let md = "- a.go:1 - first line\n  wraps here.\nstill the item\n\nnew paragraph";
    assert_eq!(
        plain(&render(md, 80, &T)),
        [
            "• a.go:1 - first line wraps here. still the item",
            "",
            "new paragraph",
        ]
    );
    assert_eq!(
        plain(&render(md, 20, &T)),
        [
            "• a.go:1 - first",
            "  line wraps here.",
            "  still the item",
            "",
            "new paragraph",
        ]
    );
}

#[test]
fn review_markdown_fake_log() {
    let md = include_str!("../../../../../testdata/logs/review-markdown.log");
    let out = render(md, 80, &T);
    let code =
        r#"  const fmt = new Intl.NumberFormat(locale, { style: "currency", currency: "EUR" });"#;
    assert_eq!(
        plain(&out),
        [
            "Review: web/src/checkout/ — cart totals refactor",
            "",
            "Three real problems, one nit. The refactor is otherwise clean and the new",
            "useCartTotals hook is easier to test than the old reducer.",
            "",
            "High",
            "",
            "• web/src/checkout/useCartTotals.ts:42 — discounts are applied after tax. VAT is",
            "  charged on the pre-discount amount: a 100.00 cart with a 10% coupon shows VAT",
            "  20.00 instead of 18.00. Apply the discount to the net amount first.",
            "• web/src/checkout/api.ts:118 — submitOrder retries on any 5xx with no",
            "  idempotency key. A slow 504 from the gateway followed by a retry creates two",
            "  orders.",
            "",
            "Medium",
            "",
            "• web/src/checkout/CartSummary.tsx:77 — the currency is hard-coded to EUR in",
            "  Intl.NumberFormat. Accounts billed in GBP see the wrong symbol.",
            "",
            "  // CartSummary.tsx:77",
            code,
            "  // should be: currency: cart.currency",
            "",
            "Low",
            "",
            "• web/src/checkout/useCartTotals.test.ts:15 — the fixture cart has one line",
            "  item, so the rounding bug above never shows. Add a three-item cart with a",
            "  percentage coupon.",
            "",
            "Verdict: fix the two High items before merge. The rest can follow.",
        ]
    );

    // The heading's code span is accent already, so the line is one span.
    assert_eq!(
        spans(&out.lines[0]),
        [s(
            "Review: web/src/checkout/ — cart totals refactor",
            T.heading
        )]
    );
    let accent_bold = bold(T.inline_code);
    assert_eq!(
        spans(&out.lines[10]),
        [
            s("• ", T.dim),
            s("web/src/checkout/api.ts:118", accent_bold),
            s(" — ", T.text),
            s("submitOrder", T.inline_code),
            s(" retries on any 5xx with no", T.text),
        ]
    );
    assert_eq!(
        spans(&out.lines[17]),
        [
            s("  ", Style::new()),
            s("Intl.NumberFormat", T.inline_code),
            s(". Accounts billed in GBP see the wrong symbol.", T.text),
        ],
        "a word split across styles stays one word"
    );
    assert_eq!(spans(&out.lines[20]), [s(code, T.code_block)]);
    assert!(width(code) > 80, "code is never wrapped");
    assert_eq!(
        spans(&out.lines[29]),
        [
            s("Verdict:", bold(T.text)),
            s(
                " fix the two High items before merge. The rest can follow.",
                T.text
            ),
        ]
    );
}
