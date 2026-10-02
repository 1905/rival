//! Markdown → styled terminal text for the result view.
//!
//! Swift: `RivalKit/MarkdownBlocks.swift` and `ResultPane.swift`. The Swift
//! app splits blocks with a small line parser; this port reads CommonMark
//! through pulldown-cmark, so a few inputs come out differently:
//!
//! - Inside a paragraph only "1." starts an ordered list: "text\n12. x" is one
//!   paragraph. Swift starts an item.
//! - Nesting follows the content column, not "leading spaces / 2".
//! - A new bullet character or number delimiter starts a new list, and a
//!   blank row separates sibling lists.
//! - A fence closes only on a fence at least as long as the opener; a line
//!   indented four spaces is code; `>` quotes and `---` rules are rendered.
//!   Swift shows all three as plain text.
//!
//! Like Swift, list numbers always show as "N." and HTML stays literal text.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd, TextMergeStream};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use unicode_segmentation::UnicodeSegmentation;

use super::logview::strip_controls;
use super::styles::Styles;
use super::text::width;

/// The bullet of every unordered item.
const BULLET: &str = "• ";
/// Sets code blocks off from prose.
const CODE_INDENT: &str = "  ";
/// Tab stops inside code blocks.
const TAB_STOP: usize = 4;
/// The bar in front of block-quote lines.
const QUOTE_BAR: &str = "│ ";

/// Renders `md` for a `width`-cell pane: headings bold accent, paragraphs and
/// list items word-wrapped by display cell with a hanging indent, fenced code
/// dim and never wrapped (the viewport clips it). Width 0 means no wrapping.
pub fn render(md: &str, width: u16, theme: &Styles) -> Text<'static> {
    let mut r = Renderer::new(usize::from(width), theme);
    // Merged text keeps an entity-decoded escape and its parameters in one
    // string, so `clean` removes the whole sequence.
    for event in TextMergeStream::new(Parser::new_ext(md, Options::empty())) {
        r.event(event);
    }
    r.finish()
}

/// Makes parsed text safe for the terminal. The result parser sanitizes the
/// log, but CommonMark decodes entities such as `&#27;` afterwards, so every
/// string is cleaned again before it becomes a span.
fn clean(s: &str) -> String {
    strip_controls(s)
}

/// A run of inline content waiting to be wrapped.
enum Frag {
    Text(String, Style),
    /// A hard line break, or a line end inside an HTML block.
    Break,
}

/// A list item or a block quote: the cells it puts in front of each line.
struct Container {
    /// Before the container's first line: the item marker.
    first: Vec<Span<'static>>,
    /// Before every later line: blanks as wide as the marker, or the bar.
    rest: Vec<Span<'static>>,
    started: bool,
    quote: bool,
}

struct Renderer<'t> {
    theme: &'t Styles,
    width: usize,
    lines: Vec<Line<'static>>,
    containers: Vec<Container>,
    /// The next number of each open list; `None` for a bullet list.
    lists: Vec<Option<u64>>,
    /// One blank row is owed before the next content line.
    need_gap: bool,
    /// Inline styles; the bottom entry is the body text.
    styles: Vec<Style>,
    inline: Vec<Frag>,
    /// The open links: destination and where their text starts in `inline`.
    links: Vec<(String, usize)>,
    /// The body of the open code block.
    code: Option<String>,
}

impl<'t> Renderer<'t> {
    fn new(width: usize, theme: &'t Styles) -> Self {
        Renderer {
            theme,
            width,
            lines: Vec::new(),
            containers: Vec::new(),
            lists: Vec::new(),
            need_gap: false,
            styles: vec![theme.text],
            inline: Vec::new(),
            links: Vec::new(),
            code: None,
        }
    }

    fn style(&self) -> Style {
        self.styles.last().copied().unwrap_or(self.theme.text)
    }

    fn push_style(&mut self, f: impl FnOnce(Style) -> Style) {
        let style = f(self.style());
        self.styles.push(style);
    }

    fn pop_style(&mut self) {
        if self.styles.len() > 1 {
            self.styles.pop();
        }
    }

    fn text(&mut self, s: &str, style: Style) {
        let s = clean(s);
        if !s.is_empty() {
            self.inline.push(Frag::Text(s, style));
        }
    }

    fn event(&mut self, event: Event<'_>) {
        if let Some(code) = &mut self.code {
            match event {
                Event::Text(t) => code.push_str(&t),
                Event::End(TagEnd::CodeBlock) => self.end_code(),
                _ => {}
            }
            return;
        }
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) | Event::InlineHtml(t) => self.text(&t, self.style()),
            Event::Code(t) => self.text(&t, self.style().patch(self.theme.inline_code)),
            Event::Html(t) => {
                // An HTML block arrives line by line; keep its line breaks.
                for (i, part) in t.split('\n').enumerate() {
                    if i > 0 {
                        self.inline.push(Frag::Break);
                    }
                    self.text(part, self.theme.text);
                }
            }
            Event::SoftBreak => self.text(" ", self.style()),
            Event::HardBreak => self.inline.push(Frag::Break),
            Event::Rule => {
                self.flush_inline();
                let n = match self.avail() {
                    0 => 3,
                    n => n,
                };
                self.emit(vec![Span::styled("─".repeat(n), self.theme.dim)]);
                self.need_gap = true;
            }
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph | Tag::HtmlBlock => self.flush_inline(),
            Tag::Heading { .. } => {
                self.flush_inline();
                self.styles.push(self.theme.heading);
            }
            Tag::BlockQuote(_) => {
                self.flush_inline();
                self.flush_gap();
                let bar = vec![Span::styled(QUOTE_BAR, self.theme.dim)];
                self.containers.push(Container {
                    first: bar.clone(),
                    rest: bar,
                    started: false,
                    quote: true,
                });
            }
            Tag::CodeBlock(_) => {
                self.flush_inline();
                self.code = Some(String::new());
            }
            Tag::List(first) => {
                self.flush_inline();
                if self.containers.last().is_some_and(|c| !c.quote) {
                    // A nested list hugs its parent item.
                    self.need_gap = false;
                } else {
                    self.flush_gap();
                }
                self.lists.push(first);
            }
            Tag::Item => {
                self.flush_inline();
                self.need_gap = false;
                let marker = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        let m = format!("{n}. ");
                        *n += 1;
                        m
                    }
                    _ => BULLET.to_string(),
                };
                let pad = " ".repeat(width(&marker));
                self.containers.push(Container {
                    first: vec![Span::styled(marker, self.theme.dim)],
                    rest: vec![Span::raw(pad)],
                    started: false,
                    quote: false,
                });
            }
            Tag::Emphasis => self.push_style(|s| s.add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.push_style(|s| s.add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => self.push_style(|s| s.add_modifier(Modifier::CROSSED_OUT)),
            Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                self.links.push((clean(&dest_url), self.inline.len()));
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::HtmlBlock => {
                self.flush_inline();
                self.need_gap = true;
            }
            TagEnd::Heading(_) => {
                self.flush_inline();
                self.pop_style();
                self.need_gap = true;
            }
            TagEnd::BlockQuote(_) => {
                self.flush_inline();
                self.containers.pop();
                self.need_gap = true;
            }
            TagEnd::List(_) => {
                self.flush_inline();
                self.lists.pop();
                self.need_gap = true;
            }
            TagEnd::Item => {
                self.flush_inline();
                if self.containers.last().is_some_and(|c| !c.started) {
                    // An empty item still shows its marker.
                    self.emit(Vec::new());
                }
                self.containers.pop();
                self.need_gap = false;
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => self.pop_style(),
            TagEnd::Link | TagEnd::Image => {
                let Some((url, from)) = self.links.pop() else {
                    return;
                };
                let label: String = self.inline[from.min(self.inline.len())..]
                    .iter()
                    .filter_map(|f| match f {
                        Frag::Text(s, _) => Some(s.as_str()),
                        Frag::Break => None,
                    })
                    .collect();
                // An autolink's text is its URL already.
                if !url.is_empty() && label != url {
                    self.text(&format!(" ({url})"), self.theme.dim);
                }
            }
            _ => {}
        }
    }

    fn end_code(&mut self) {
        let body = self.code.take().unwrap_or_default();
        let body = body.strip_suffix('\n').unwrap_or(&body);
        if !body.is_empty() {
            for line in body.split('\n') {
                let line = expand_tabs(&clean(line));
                if line.is_empty() {
                    self.emit(Vec::new());
                } else {
                    let cell = format!("{CODE_INDENT}{line}");
                    self.emit(vec![Span::styled(cell, self.theme.code_block)]);
                }
            }
        }
        self.need_gap = true;
    }

    /// The cells left for content after the container prefixes; 0 means no
    /// limit. A prefix as wide as the pane still leaves one cell.
    fn avail(&self) -> usize {
        if self.width == 0 {
            return 0;
        }
        let prefix: usize = self
            .containers
            .iter()
            .flat_map(|c| &c.rest)
            .map(|s| width(&s.content))
            .sum();
        self.width.saturating_sub(prefix).max(1)
    }

    fn flush_inline(&mut self) {
        if self.inline.is_empty() {
            return;
        }
        let frags = std::mem::take(&mut self.inline);
        for line in wrap(&frags, self.avail()) {
            self.emit(line);
        }
    }

    /// Writes the owed blank row, if any. Inside a quote the row keeps the
    /// bar, so the quote reads as one block.
    fn flush_gap(&mut self) {
        if self.need_gap && !self.lines.is_empty() {
            let mut spans: Vec<Span<'static>> = Vec::new();
            if self.containers.iter().any(|c| c.quote) {
                spans = self
                    .containers
                    .iter()
                    .flat_map(|c| c.rest.clone())
                    .collect();
                trim_end(&mut spans);
            }
            self.lines.push(Line::from(spans));
        }
        self.need_gap = false;
    }

    /// Adds one content line behind the container prefixes. The first line of
    /// each container gets its marker; later lines get the hanging indent.
    fn emit(&mut self, content: Vec<Span<'static>>) {
        self.flush_gap();
        let mut spans = Vec::with_capacity(self.containers.len() + content.len());
        for c in &mut self.containers {
            if c.started {
                spans.extend(c.rest.iter().cloned());
            } else {
                spans.extend(c.first.iter().cloned());
                c.started = true;
            }
        }
        if content.is_empty() {
            trim_end(&mut spans);
        }
        spans.extend(content);
        self.lines.push(Line::from(spans));
    }

    fn finish(mut self) -> Text<'static> {
        self.flush_inline();
        Text::from(self.lines)
    }
}

/// Drops trailing blanks from a prefix-only line.
fn trim_end(spans: &mut Vec<Span<'static>>) {
    while let Some(last) = spans.last_mut() {
        let kept = last.content.trim_end().len();
        if kept > 0 {
            last.content.to_mut().truncate(kept);
            return;
        }
        spans.pop();
    }
}

/// Replaces tabs with spaces up to the next [`TAB_STOP`] column: the buffer
/// gives a tab no width.
fn expand_tabs(line: &str) -> String {
    if !line.contains('\t') {
        return line.to_string();
    }
    let mut out = String::with_capacity(line.len() + TAB_STOP);
    let mut col = 0;
    for g in line.graphemes(true) {
        if g == "\t" {
            let n = TAB_STOP - col % TAB_STOP;
            out.extend(std::iter::repeat_n(' ', n));
            col += n;
        } else {
            out.push_str(g);
            col += width(g);
        }
    }
    out
}

/// Appends `text` to the last span when it has the same style.
fn push_span(spans: &mut Vec<Span<'static>>, text: &str, style: Style) {
    if let Some(last) = spans.last_mut()
        && last.style == style
    {
        last.content.to_mut().push_str(text);
        return;
    }
    spans.push(Span::styled(text.to_string(), style));
}

fn is_space(g: &str) -> bool {
    g.chars().all(|c| c.is_whitespace() && c != '\u{a0}')
}

/// Word-wraps styled inline content to `avail` cells (0: no limit).
///
/// Breaks go at spaces and around wide (CJK, emoji) clusters, which need no
/// space to break. A word longer than a line starts a new line and breaks
/// between grapheme clusters. Spaces at a wrap point are dropped; the rest
/// keep their run length.
fn wrap(frags: &[Frag], avail: usize) -> Vec<Vec<Span<'static>>> {
    let mut w = Wrapper {
        avail,
        ..Wrapper::default()
    };
    for frag in frags {
        match frag {
            Frag::Text(s, style) => {
                for g in s.graphemes(true) {
                    w.grapheme(g, *style);
                }
            }
            Frag::Break => w.hard_break(),
        }
    }
    w.finish()
}

#[derive(Default)]
struct Wrapper {
    avail: usize,
    lines: Vec<Vec<Span<'static>>>,
    cur: Vec<Span<'static>>,
    cur_w: usize,
    /// Spaces seen since the last word, written only before a next word on
    /// the same line.
    space: Vec<Span<'static>>,
    space_w: usize,
    word: Vec<Span<'static>>,
    word_w: usize,
}

impl Wrapper {
    fn grapheme(&mut self, g: &str, style: Style) {
        if is_space(g) {
            self.end_word();
            push_span(&mut self.space, " ", style);
            self.space_w += 1;
            return;
        }
        let gw = width(g);
        if gw > 1 {
            self.end_word();
        }
        push_span(&mut self.word, g, style);
        self.word_w += gw;
        if gw > 1 {
            self.end_word();
        }
    }

    fn end_word(&mut self) {
        if self.word.is_empty() {
            return;
        }
        let word = std::mem::take(&mut self.word);
        let ww = std::mem::take(&mut self.word_w);
        let limited = self.avail > 0;
        if self.cur_w > 0 && limited && self.cur_w + self.space_w + ww > self.avail {
            self.newline();
        }
        let space = std::mem::take(&mut self.space);
        if self.cur_w > 0 {
            for s in space {
                push_span(&mut self.cur, &s.content, s.style);
            }
            self.cur_w += self.space_w;
        }
        self.space_w = 0;
        if limited && ww > self.avail {
            for span in word {
                for g in span.content.graphemes(true) {
                    let gw = width(g);
                    if self.cur_w > 0 && self.cur_w + gw > self.avail {
                        self.newline();
                    }
                    push_span(&mut self.cur, g, span.style);
                    self.cur_w += gw;
                }
            }
        } else {
            for span in word {
                push_span(&mut self.cur, &span.content, span.style);
            }
            self.cur_w += ww;
        }
    }

    fn newline(&mut self) {
        self.lines.push(std::mem::take(&mut self.cur));
        self.cur_w = 0;
        self.space.clear();
        self.space_w = 0;
    }

    fn hard_break(&mut self) {
        self.end_word();
        self.newline();
    }

    fn finish(mut self) -> Vec<Vec<Span<'static>>> {
        self.end_word();
        if !self.cur.is_empty() {
            self.lines.push(self.cur);
        }
        self.lines
    }
}

#[cfg(test)]
mod tests;
