//! Key bindings, mode routing and the help bar.
//! Go: `internal/dashboard/keys.go` plus the bubbles `help` view.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::text::{Line, Span};

use super::detail_view::DetailTab;
use super::styles::Styles;
use super::text::{fit_line, width};

/// Which part of the UI owns the keyboard. Routing by mode, not by
/// key-match order, is what stops a list key ("j", "q") leaking into a text
/// input or a viewport.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mode {
    /// The list has focus.
    #[default]
    List,
    /// The list filter input has focus.
    Filter,
    /// The detail screen has focus.
    Detail,
    /// The detail search input has focus.
    Search,
    /// A y/n confirm bar is open.
    Confirm,
}

impl Mode {
    #[cfg(test)]
    pub const ALL: [Mode; 5] = [
        Mode::List,
        Mode::Filter,
        Mode::Detail,
        Mode::Search,
        Mode::Confirm,
    ];
}

/// One binding: the key names it answers to and its help text. Key names
/// follow bubbletea: "j", "G", "?", "enter", "esc", "tab", "shift+tab",
/// "ctrl+c", "pgdown", "space". See [`key_name`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    pub keys: &'static [&'static str],
    pub help_key: &'static str,
    pub help_desc: &'static str,
}

impl Binding {
    pub const fn new(
        keys: &'static [&'static str],
        help_key: &'static str,
        help_desc: &'static str,
    ) -> Binding {
        Binding {
            keys,
            help_key,
            help_desc,
        }
    }

    /// Whether `key` (a [`key_name`]) triggers this binding.
    pub fn matches(&self, key: &str) -> bool {
        self.keys.contains(&key)
    }

    /// A copy with a mode-specific description. Bindings are values, so the
    /// shared [`KeyMap`] never changes.
    pub const fn relabel(self, desc: &'static str) -> Binding {
        Binding {
            help_desc: desc,
            ..self
        }
    }
}

/// Every binding in the TUI. `filter` and `search` share "/", and
/// `next_page` (list), `next_match` (detail) and `no` (confirm) share "n":
/// only one mode is active at a time, so they never collide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyMap {
    pub up: Binding,
    pub down: Binding,
    pub top: Binding,
    pub bottom: Binding,
    pub next_page: Binding,
    pub prev_page: Binding,
    pub open: Binding,
    pub back: Binding,
    pub filter: Binding,
    pub next_tab: Binding,
    pub prev_tab: Binding,
    pub help: Binding,
    pub quit: Binding,
    pub follow: Binding,
    pub search: Binding,
    pub next_match: Binding,
    pub prev_match: Binding,
    pub next_member: Binding,
    pub prev_member: Binding,
    pub tab_result: Binding,
    pub tab_raw: Binding,
    pub tab_prompt: Binding,
    pub tab_info: Binding,
    pub open_log: Binding,
    pub stop: Binding,
    pub yes: Binding,
    pub no: Binding,
}

impl Default for KeyMap {
    fn default() -> Self {
        KeyMap {
            up: Binding::new(&["k", "up"], "↑/k", "up"),
            down: Binding::new(&["j", "down"], "↓/j", "down"),
            top: Binding::new(&["g", "home"], "g", "top"),
            bottom: Binding::new(&["G", "end"], "G", "bottom"),
            next_page: Binding::new(&["n", "pgdown"], "n/pgdn", "next page"),
            prev_page: Binding::new(&["p", "pgup"], "p/pgup", "prev page"),
            open: Binding::new(&["enter"], "enter", "open"),
            back: Binding::new(&["esc"], "esc", "back"),
            filter: Binding::new(&["/"], "/", "filter"),
            next_tab: Binding::new(&["tab"], "tab", "status"),
            prev_tab: Binding::new(&["shift+tab"], "shift+tab", "prev status"),
            help: Binding::new(&["?"], "?", "more"),
            quit: Binding::new(&["q", "ctrl+c"], "q", "quit"),
            follow: Binding::new(&["f"], "f", "follow"),
            search: Binding::new(&["/"], "/", "search"),
            next_match: Binding::new(&["n"], "n", "next match"),
            prev_match: Binding::new(&["N"], "N", "prev match"),
            next_member: Binding::new(&["]"], "]", "next member"),
            prev_member: Binding::new(&["["], "[", "prev member"),
            tab_result: Binding::new(&["1"], "1", "result"),
            tab_raw: Binding::new(&["2"], "2", "raw"),
            tab_prompt: Binding::new(&["3"], "3", "prompt"),
            tab_info: Binding::new(&["4"], "4", "info"),
            open_log: Binding::new(&["o"], "o", "open log"),
            stop: Binding::new(&["x"], "x", "stop"),
            yes: Binding::new(&["y"], "y", "yes"),
            no: Binding::new(&["n", "esc"], "n/esc", "no"),
        }
    }
}

/// Quits from any mode, including the text inputs where "q" is a character
/// rather than a command.
pub const FORCE_QUIT: Binding = Binding::new(&["ctrl+c"], "ctrl+c", "quit");

/// Arrow keys still move the list cursor while the filter has focus; j/k are
/// letters there.
pub const ARROW_UP: Binding = Binding::new(&["up"], "↑", "up");
pub const ARROW_DOWN: Binding = Binding::new(&["down"], "↓", "down");

/// "1-4": the detail tab keys as one help entry.
const DETAIL_TABS: Binding = Binding::new(&["1", "2", "3", "4"], "1-4", "tab");
/// "[/]": the member keys as one help entry.
const MEMBERS: Binding = Binding::new(&["[", "]"], "[/]", "member");
/// "n/p": the page keys as one help entry.
const PAGES: Binding = Binding::new(&["n", "p"], "n/p", "page");

impl KeyMap {
    /// Every binding with its field name, for tests.
    #[cfg(test)]
    pub fn all(&self) -> [(&'static str, Binding); 27] {
        [
            ("up", self.up),
            ("down", self.down),
            ("top", self.top),
            ("bottom", self.bottom),
            ("next_page", self.next_page),
            ("prev_page", self.prev_page),
            ("open", self.open),
            ("back", self.back),
            ("filter", self.filter),
            ("next_tab", self.next_tab),
            ("prev_tab", self.prev_tab),
            ("help", self.help),
            ("quit", self.quit),
            ("follow", self.follow),
            ("search", self.search),
            ("next_match", self.next_match),
            ("prev_match", self.prev_match),
            ("next_member", self.next_member),
            ("prev_member", self.prev_member),
            ("tab_result", self.tab_result),
            ("tab_raw", self.tab_raw),
            ("tab_prompt", self.tab_prompt),
            ("tab_info", self.tab_info),
            ("open_log", self.open_log),
            ("stop", self.stop),
            ("yes", self.yes),
            ("no", self.no),
        ]
    }

    /// The detail tab a key jumps to, if any.
    pub fn detail_tab(&self, key: &str) -> Option<DetailTab> {
        [
            (self.tab_result, DetailTab::Result),
            (self.tab_raw, DetailTab::Raw),
            (self.tab_prompt, DetailTab::Prompt),
            (self.tab_info, DetailTab::Info),
        ]
        .into_iter()
        .find(|(b, _)| b.matches(key))
        .map(|(_, tab)| tab)
    }

    /// Go: `keyMap.help`. The bindings to advertise in `mode`.
    pub fn help(&self, mode: Mode) -> ModeHelp {
        match mode {
            Mode::Filter => ModeHelp::simple(vec![
                self.open.relabel("keep filter"),
                self.back.relabel("clear"),
            ]),
            Mode::Detail => ModeHelp {
                short: vec![
                    DETAIL_TABS,
                    MEMBERS,
                    self.follow,
                    self.search,
                    self.open_log,
                    self.stop,
                    self.back,
                    self.help,
                ],
                full: vec![
                    vec![self.up, self.down, self.top, self.bottom, self.follow],
                    vec![
                        self.tab_result,
                        self.tab_raw,
                        self.tab_prompt,
                        self.tab_info,
                        self.next_member,
                        self.prev_member,
                    ],
                    vec![self.search, self.next_match, self.prev_match],
                    vec![
                        self.open_log,
                        self.stop,
                        self.back,
                        self.next_tab.relabel("next tab"),
                        self.help,
                        self.quit,
                    ],
                ],
            },
            Mode::Search => ModeHelp::simple(vec![
                self.open.relabel("search"),
                self.back.relabel("clear"),
            ]),
            Mode::Confirm => ModeHelp::simple(vec![self.yes, self.no]),
            Mode::List => ModeHelp {
                short: vec![
                    self.up,
                    self.down,
                    PAGES,
                    self.open,
                    self.filter,
                    self.next_tab,
                    self.help,
                    self.quit,
                ],
                full: vec![
                    vec![self.up, self.down, self.top, self.bottom],
                    vec![self.next_page, self.prev_page],
                    vec![self.open, self.filter, self.back],
                    vec![self.next_tab, self.prev_tab],
                    vec![self.help, self.quit],
                ],
            },
        }
    }
}

/// The help for one mode: one short row, or columns when expanded with "?".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeHelp {
    pub short: Vec<Binding>,
    pub full: Vec<Vec<Binding>>,
}

impl ModeHelp {
    /// A one-group help: the same bindings short and expanded.
    fn simple(bindings: Vec<Binding>) -> ModeHelp {
        ModeHelp {
            full: vec![bindings.clone()],
            short: bindings,
        }
    }
}

const SHORT_SEPARATOR: &str = " · ";
const FULL_SEPARATOR: &str = "    ";
const ELLIPSIS: &str = "…";

/// The help bar for `help`: one row, or with `show_all` its columns side by
/// side. An entry or column that does not fit is replaced by " …" (when that
/// fits). Every line is at most `w` cells and there is always at least one.
pub fn help_lines(
    help: &ModeHelp,
    show_all: bool,
    w: usize,
    styles: &Styles,
) -> Vec<Line<'static>> {
    let lines = if show_all {
        full_help(&help.full, w, styles)
    } else {
        vec![short_help(&help.short, w, styles)]
    };
    let mut lines: Vec<Line<'static>> = lines.into_iter().map(|l| fit_line(l, w)).collect();
    if lines.is_empty() {
        lines.push(Line::default());
    }
    lines
}

/// The " …" that replaces entries past the width, if it fits after `used`
/// cells.
fn ellipsis(used: usize, w: usize, styles: &Styles) -> Option<Span<'static>> {
    let tail = format!(" {ELLIPSIS}");
    (used + width(&tail) <= w).then(|| Span::styled(tail, styles.dim))
}

fn short_help(bindings: &[Binding], w: usize, styles: &Styles) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut used = 0;
    for b in bindings {
        let sep = if used > 0 { SHORT_SEPARATOR } else { "" };
        let entry_w = width(sep) + width(b.help_key) + 1 + width(b.help_desc);
        if used + entry_w > w {
            spans.extend(ellipsis(used, w, styles));
            break;
        }
        used += entry_w;
        if !sep.is_empty() {
            spans.push(Span::styled(sep, styles.dim));
        }
        spans.push(Span::styled(b.help_key, styles.text));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(b.help_desc, styles.dim));
    }
    Line::from(spans)
}

/// The expanded help: one column per group, keys left-aligned in a column of
/// their own, then the descriptions.
fn full_help(groups: &[Vec<Binding>], w: usize, styles: &Styles) -> Vec<Line<'static>> {
    let mut rows: Vec<Vec<Span<'static>>> = Vec::new();
    let mut used = 0;
    for group in groups.iter().filter(|g| !g.is_empty()) {
        let sep = if used > 0 { FULL_SEPARATOR } else { "" };
        let key_w = group.iter().map(|b| width(b.help_key)).max().unwrap_or(0);
        let desc_w = group.iter().map(|b| width(b.help_desc)).max().unwrap_or(0);
        let col_w = width(sep) + key_w + 1 + desc_w;
        if used + col_w > w {
            if let Some(tail) = ellipsis(used, w, styles) {
                rows.resize_with(rows.len().max(1), Vec::new);
                rows[0].push(tail);
            }
            break;
        }
        rows.resize_with(rows.len().max(group.len()), Vec::new);
        for (row, b) in rows.iter_mut().zip(group) {
            // Earlier columns end ragged; pad this row up to the column's
            // left edge first.
            let row_w: usize = row.iter().map(|s| width(&s.content)).sum();
            if row_w < used {
                row.push(Span::raw(" ".repeat(used - row_w)));
            }
            let key_pad = key_w - width(b.help_key);
            row.push(Span::raw(sep));
            row.push(Span::styled(b.help_key, styles.text));
            row.push(Span::raw(" ".repeat(key_pad + 1)));
            row.push(Span::styled(b.help_desc, styles.dim));
        }
        used += col_w;
    }
    rows.into_iter().map(Line::from).collect()
}

/// The bubbletea-style name of a key press, or `None` for a release or a key
/// the TUI never binds. Plain printable chars are themselves ("q", "G", "?");
/// a space is "space".
pub fn key_name(ev: &KeyEvent) -> Option<String> {
    if ev.kind == KeyEventKind::Release {
        return None;
    }
    let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
    let alt = ev.modifiers.contains(KeyModifiers::ALT);
    let shift = ev.modifiers.contains(KeyModifiers::SHIFT);
    let prefix = |name: &str| {
        let mut out = String::new();
        if ctrl {
            out.push_str("ctrl+");
        }
        if alt {
            out.push_str("alt+");
        }
        out.push_str(name);
        out
    };
    let name = match ev.code {
        KeyCode::Char(' ') => prefix("space"),
        KeyCode::Char(c) if ctrl || alt => prefix(&c.to_lowercase().to_string()),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => prefix("enter"),
        KeyCode::Esc => prefix("esc"),
        KeyCode::Tab if shift => "shift+tab".to_string(),
        KeyCode::Tab => prefix("tab"),
        KeyCode::BackTab => "shift+tab".to_string(),
        KeyCode::Backspace => prefix("backspace"),
        KeyCode::Delete => prefix("delete"),
        KeyCode::Up => prefix("up"),
        KeyCode::Down => prefix("down"),
        KeyCode::Left => prefix("left"),
        KeyCode::Right => prefix("right"),
        KeyCode::Home => prefix("home"),
        KeyCode::End => prefix("end"),
        KeyCode::PageUp => prefix("pgup"),
        KeyCode::PageDown => prefix("pgdown"),
        KeyCode::F(n) => prefix(&format!("f{n}")),
        _ => return None,
    };
    Some(name)
}

#[cfg(test)]
mod tests;
