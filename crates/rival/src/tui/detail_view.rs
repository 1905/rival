//! The full-screen view of one run. Go: `internal/dashboard/detail_view.go`.
//!
//! Task 4.2 holds the shared tab enum, the pane state the model routes keys
//! to (tab, member, search input, confirm, notice) and its chrome. Task 4.4
//! adds the viewport, log/prompt/info content, search matching, members and
//! stop; Task 4.8 adds the Result view.

use std::sync::Arc;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use rival_core::session::Session;

use super::input::TextInput;
use super::keys::Mode;
use super::model::DisplayItem;
use super::styles::{Styles, gradient_word};
use super::text::pad_line;

/// The detail tabs, in key order 1-4. Every detail-screen module uses this
/// one enum.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum DetailTab {
    /// The parsed review (Task 4.8).
    Result,
    /// The raw log (Go: Output).
    #[default]
    Raw,
    Prompt,
    Info,
}

impl DetailTab {
    pub const ALL: [DetailTab; 4] = [
        DetailTab::Result,
        DetailTab::Raw,
        DetailTab::Prompt,
        DetailTab::Info,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DetailTab::Result => "Result",
            DetailTab::Raw => "Raw",
            DetailTab::Prompt => "Prompt",
            DetailTab::Info => "Info",
        }
    }

    /// The tab `delta` steps away, wrapping.
    pub fn cycle(self, delta: isize) -> DetailTab {
        let n = Self::ALL.len() as isize;
        let i = Self::ALL.iter().position(|&t| t == self).unwrap_or(0) as isize;
        Self::ALL[(i + delta).rem_euclid(n) as usize]
    }
}

/// The rows the detail screen draws around its content: breadcrumb, tab
/// bar, separator, separator, status line. The help bar is counted by the
/// caller, which owns its height.
pub const DETAIL_CHROME_H: usize = 5;

/// An open "stop N running sessions? y/n" bar. `targets` are the sessions
/// that were live when x was pressed; y re-checks them against the current
/// snapshot before it sends anything.
#[derive(Debug, Clone, PartialEq)]
pub struct KillConfirm {
    pub targets: Vec<Arc<Session>>,
}

/// The detail screen state.
#[derive(Debug, Clone)]
pub struct DetailPane {
    pub tab: DetailTab,
    /// The member the tabs show. An id, not an index: a group that gains or
    /// loses a session reorders its slice.
    pub member_id: String,
    /// Whether the Raw tab sticks to the log's tail.
    pub follow: bool,
    pub search: TextInput,
    /// The applied search query.
    pub query: String,
    pub confirm: Option<KillConfirm>,
    /// A one-shot status message ("nothing running"). The next key clears it.
    pub notice: String,
    /// The content viewport size in cells, set by [`DetailPane::resize`].
    pub width: usize,
    pub height: usize,
}

impl Default for DetailPane {
    fn default() -> Self {
        DetailPane {
            tab: DetailTab::default(),
            member_id: String::new(),
            follow: false,
            search: TextInput::new("/ ", "search"),
            query: String::new(),
            confirm: None,
            notice: String::new(),
            width: 0,
            height: 0,
        }
    }
}

impl DetailPane {
    /// Resets the pane for `item`: Raw tab, first member, follow on, no
    /// search. Task 4.8 picks the per-run default tab here.
    pub fn open(&mut self, item: Option<&DisplayItem>) {
        self.tab = DetailTab::Raw;
        self.member_id = item
            .and_then(DisplayItem::primary)
            .map(|s| s.id.clone())
            .unwrap_or_default();
        self.follow = true;
        self.clear_search();
        self.confirm = None;
        self.notice.clear();
    }

    /// Drops everything that belongs to one run.
    pub fn close(&mut self) {
        self.confirm = None;
        self.notice.clear();
        self.clear_search();
    }

    pub fn clear_search(&mut self) {
        self.query.clear();
        self.search.reset();
        self.search.blur();
    }

    /// Applies a search query. Task 4.4 adds the matching and scrolling.
    pub fn run_search(&mut self, query: &str) {
        self.query = query.to_string();
    }

    /// The position of `member_id` in `item`, or 0 when it is gone.
    pub fn member_index(&self, item: &DisplayItem) -> usize {
        item.sessions
            .iter()
            .position(|s| s.id == self.member_id)
            .unwrap_or(0)
    }

    /// The member the tabs show, or `None` for an empty item.
    pub fn current<'a>(&self, item: Option<&'a DisplayItem>) -> Option<&'a Arc<Session>> {
        let item = item?;
        item.sessions.get(self.member_index(item))
    }

    /// Moves to the next (+1) or previous (-1) member, wrapping.
    pub fn cycle_member(&mut self, item: Option<&DisplayItem>, delta: isize) {
        let Some(item) = item.filter(|i| i.sessions.len() >= 2) else {
            return;
        };
        let n = item.sessions.len() as isize;
        let i = (self.member_index(item) as isize + delta).rem_euclid(n) as usize;
        self.member_id = item.sessions[i].id.clone();
    }

    /// Fits the content viewport to a `width`×`height` pane.
    pub fn resize(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height.saturating_sub(DETAIL_CHROME_H).max(1);
    }

    /// Re-targets the current tab at `item`. `reset` puts the scroll back
    /// to the tab's start: Raw jumps to the tail and follows it. Task 4.4
    /// rebuilds the tab content here.
    pub fn reload(&mut self, item: Option<&DisplayItem>, reset: bool) {
        let Some(s) = self.current(item) else {
            return;
        };
        // A vanished member fell back to the first; stay there if it returns.
        self.member_id = s.id.clone();
        if self.tab == DetailTab::Raw && reset {
            self.follow = true;
        }
    }

    /// Draws the detail screen into `area`: breadcrumb, tab bar, separator,
    /// content, separator, status line.
    pub fn render(
        &self,
        item: Option<&DisplayItem>,
        mode: Mode,
        area: Rect,
        buf: &mut Buffer,
        styles: &Styles,
    ) {
        let (w, h) = (usize::from(area.width), usize::from(area.height));
        if w == 0 || h == 0 {
            return;
        }
        let mut crumb = gradient_word("rival");
        crumb.push(Span::styled(" › ", styles.dim));
        if let Some(s) = item.and_then(DisplayItem::primary) {
            let model = if s.model.is_empty() { &s.cli } else { &s.model };
            crumb.push(Span::styled(model.clone(), styles.value));
        }
        let mut tabs = Vec::new();
        for (i, tab) in DetailTab::ALL.iter().enumerate() {
            let style = if *tab == self.tab {
                styles.active_tab
            } else {
                styles.inactive_tab
            };
            tabs.push(Span::raw(" "));
            tabs.push(Span::styled(format!("{} {}", i + 1, tab.label()), style));
            tabs.push(Span::raw(" "));
        }
        let rule = Line::styled("─".repeat(w), styles.dim);
        let status = match mode {
            Mode::Search => {
                let mut line = self.search.line(styles);
                line.spans.insert(0, Span::raw(" "));
                line
            }
            Mode::Confirm => Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    format!(
                        "stop {} running sessions? y/n",
                        self.confirm.as_ref().map_or(0, |c| c.targets.len())
                    ),
                    styles.running,
                ),
            ]),
            _ if !self.notice.is_empty() => Line::styled(format!(" {}", self.notice), styles.dim),
            _ if !self.query.is_empty() => Line::styled(format!(" /{}", self.query), styles.dim),
            _ => Line::default(),
        };
        let mut lines = vec![Line::from(crumb), Line::from(tabs), rule.clone()];
        let content_h = h.saturating_sub(DETAIL_CHROME_H).max(1);
        lines.extend((0..content_h).map(|_| Line::default()));
        lines.push(rule);
        lines.push(status);
        for (y, line) in (area.y..).zip(lines.into_iter().take(h)) {
            buf.set_line(area.x, y, &pad_line(line, w), area.width);
        }
    }
}
