//! The run list: status tabs, filter input, rows and the cursor.
//! Go: `internal/dashboard/session_list.go`.
//!
//! Task 4.2 holds only what the model, layout and loader need: column
//! widths, status tabs and counts, the filter input, the cursor and a plain
//! row view. Task 4.3 ports the rest of `session_list.go` behind the same
//! methods: text filtering, day sections, anchoring by item key, paging and
//! the full row and footer rendering.

use chrono::{DateTime, FixedOffset};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use rival_core::gostd;
use rival_core::sessionview;

use super::input::TextInput;
use super::model::{DisplayItem, item_key};
use super::styles::{HeaderStats, Styles};
use super::text::{fit_cell, line_width, pad_line, width};

/// How many runs one page holds. Section headers do not count.
pub const PAGE_SIZE: usize = 50;

/// Narrow-pane thresholds: EFF drops first, then PROJECT. They are the fixed
/// column sums (leading space + cells + gaps), so a column only drops when it
/// cannot fit at all.
pub const EFFORT_MIN_WIDTH: usize = 68; // 1 + (3+1)+(8+1)+(20+1)+(7+1)+(7+1)+(16+1)
pub const PROJECT_MIN_WIDTH: usize = 60; // same minus the EFF column
pub const LIST_FIXED_WIDTH: usize = EFFORT_MIN_WIDTH;

/// The list column widths in cells. A zero width drops the column.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Columns {
    pub status: usize,
    pub kind: usize,
    pub model: usize,
    pub effort: usize,
    pub time: usize,
    pub project: usize,
}

impl Columns {
    fn widths(&self) -> [usize; 6] {
        [
            self.status,
            self.kind,
            self.model,
            self.effort,
            self.time,
            self.project,
        ]
    }
}

/// Go: `layoutColumns`. Sizes the columns for a pane width. A row is a
/// leading space then the cells separated by single spaces; PROJECT takes
/// whatever is left.
pub fn layout_columns(width: usize) -> Columns {
    let mut c = Columns {
        status: 3,
        kind: 8,
        model: 20,
        time: 7,
        ..Columns::default()
    };
    if width >= EFFORT_MIN_WIDTH {
        c.effort = 7;
    }
    if width >= PROJECT_MIN_WIDTH {
        c.project = 16;
    }
    // Leading space, plus a gap after every cell (one too many: the row is
    // used-1 wide).
    let used = 1 + c
        .widths()
        .iter()
        .filter(|&&w| w > 0)
        .map(|w| w + 1)
        .sum::<usize>();
    if c.project > 0 {
        c.project += width.saturating_sub(used - 1);
    }
    c
}

/// Go: `joinCells`. Lays cells out per the column widths, dropping
/// zero-width columns.
pub fn join_cells(c: &Columns, cells: [&str; 6]) -> String {
    let mut out = String::from(" ");
    let mut first = true;
    for (cell, w) in cells.iter().zip(c.widths()) {
        if w == 0 {
            continue;
        }
        if !first {
            out.push(' ');
        }
        first = false;
        out.push_str(&fit_cell(cell, w));
    }
    out
}

/// The status filter above the list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StatusTab {
    #[default]
    All,
    Running,
    Failed,
    Done,
}

impl StatusTab {
    pub const ALL: [StatusTab; 4] = [
        StatusTab::All,
        StatusTab::Running,
        StatusTab::Failed,
        StatusTab::Done,
    ];

    pub fn label(self) -> &'static str {
        match self {
            StatusTab::All => "ALL",
            StatusTab::Running => "RUNNING",
            StatusTab::Failed => "FAILED",
            StatusTab::Done => "DONE",
        }
    }

    /// Whether a row with this status belongs on the tab. Queued runs sit
    /// under RUNNING: both are "not finished yet" from the user's side.
    pub fn accepts(self, status: &str) -> bool {
        match self {
            StatusTab::All => true,
            StatusTab::Running => is_live(status),
            StatusTab::Failed => status == "failed",
            StatusTab::Done => status == "completed",
        }
    }

    /// The tab `delta` steps away, wrapping.
    pub fn cycle(self, delta: isize) -> StatusTab {
        let n = Self::ALL.len() as isize;
        let i = Self::ALL.iter().position(|&t| t == self).unwrap_or(0) as isize;
        Self::ALL[(i + delta).rem_euclid(n) as usize]
    }
}

/// Whether a status means "not finished yet": running, or queued and
/// waiting for a slot.
pub fn is_live(status: &str) -> bool {
    status == "running" || status == "queued"
}

/// The row status: a solo run's own status, a group's reduced status.
pub fn item_status(item: &DisplayItem) -> &str {
    if item.is_group() {
        return sessionview::status(&item.sessions);
    }
    item.primary().map_or("", |s| s.status.as_str())
}

/// Each status gets its own shape, so status reads without colour. Running
/// rows show the live spinner frame.
pub fn status_glyph<'a>(status: &str, spin: &'a str) -> &'a str {
    match status {
        "running" => spin,
        "queued" => "◌",
        "completed" => "✓",
        "failed" => "✗",
        _ => "·",
    }
}

/// The run list state.
#[derive(Debug, Clone)]
pub struct ListPane {
    /// The full, unfiltered set the rows derive from.
    pub items: Vec<DisplayItem>,
    /// Indexes into `items` of the runs that pass the tab, in order.
    rows: Vec<usize>,
    /// Indexes `rows`.
    pub cursor: usize,
    /// The first visible row of the window.
    pub offset: usize,
    pub tab: StatusTab,
    pub filter: TextInput,
    /// Filter matches per tab, for the tab bar.
    pub counts: [usize; 4],
    /// The header's per-status session counts.
    pub stats: HeaderStats,
    /// Whether anything runs or waits.
    pub any_live: bool,
}

impl Default for ListPane {
    fn default() -> Self {
        ListPane {
            items: Vec::new(),
            rows: Vec::new(),
            cursor: 0,
            offset: 0,
            tab: StatusTab::All,
            filter: TextInput::new("/ ", "filter"),
            counts: [0; 4],
            stats: HeaderStats::default(),
            any_live: false,
        }
    }
}

impl ListPane {
    /// Replaces the data and keeps the cursor on the same run.
    pub fn set_items(&mut self, items: Vec<DisplayItem>, now: DateTime<FixedOffset>) {
        let anchor = self.selected_key();
        self.items = items;
        self.count_statuses();
        self.rebuild(now, &anchor);
    }

    /// Recounts sessions (not rows) per status. Call it whenever a session's
    /// status changes in place.
    pub fn count_statuses(&mut self) {
        let mut st = HeaderStats::default();
        for item in &self.items {
            st.total += item.sessions.len();
            for s in &item.sessions {
                match s.status.as_str() {
                    "running" => st.running += 1,
                    "queued" => st.queued += 1,
                    "completed" => st.completed += 1,
                    "failed" => st.failed += 1,
                    _ => {}
                }
            }
        }
        self.any_live = st.running + st.queued > 0;
        self.stats = st;
    }

    /// Re-derives the rows and tab counts, then puts the cursor back on
    /// `anchor`. Reports whether the anchor survived; when it did not, the
    /// cursor clamps.
    pub fn rebuild(&mut self, _now: DateTime<FixedOffset>, anchor: &str) -> bool {
        self.counts = [0; 4];
        self.rows.clear();
        for (i, item) in self.items.iter().enumerate() {
            let status = item_status(item);
            for (t, count) in StatusTab::ALL.iter().zip(self.counts.iter_mut()) {
                if t.accepts(status) {
                    *count += 1;
                }
            }
            if self.tab.accepts(status) {
                self.rows.push(i);
            }
        }
        if !anchor.is_empty()
            && let Some(pos) = self
                .rows
                .iter()
                .position(|&i| item_key(&self.items[i]) == anchor)
        {
            self.cursor = pos;
            return true;
        }
        self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
        false
    }

    /// Moves the cursor `delta` runs; it stops at the ends.
    pub fn move_by(&mut self, delta: isize) {
        let last = self.rows.len().saturating_sub(1) as isize;
        self.cursor = (self.cursor as isize + delta).clamp(0, last.max(0)) as usize;
    }

    /// Puts the cursor on the first run.
    pub fn top(&mut self) {
        self.cursor = 0;
        self.offset = 0;
    }

    /// Puts the cursor on the last run.
    pub fn bottom(&mut self) {
        self.cursor = self.rows.len().saturating_sub(1);
    }

    /// The number of pages, at least 1.
    pub fn page_count(&self) -> usize {
        self.rows.len().div_ceil(PAGE_SIZE).max(1)
    }

    /// The 0-based page that holds the cursor.
    pub fn page(&self) -> usize {
        self.cursor / PAGE_SIZE
    }

    /// Moves `delta` pages to the first run of the new page. It stops at the
    /// first and the last page.
    pub fn turn_page(&mut self, delta: isize) {
        let last = self.page_count() as isize - 1;
        let p = (self.page() as isize + delta).clamp(0, last) as usize;
        if p == self.page() || self.rows.is_empty() {
            return;
        }
        self.cursor = p * PAGE_SIZE;
        self.offset = 0;
    }

    /// Switches the status tab and goes back to the first run.
    pub fn cycle_tab(&mut self, delta: isize, now: DateTime<FixedOffset>) {
        self.tab = self.tab.cycle(delta);
        self.refilter(now);
    }

    /// Rebuilds after the tab or the filter text changed and resets to the
    /// first run.
    pub fn refilter(&mut self, now: DateTime<FixedOffset>) {
        self.rebuild(now, "");
        self.top();
    }

    /// The run under the cursor.
    pub fn selected(&self) -> Option<&DisplayItem> {
        self.rows.get(self.cursor).map(|&i| &self.items[i])
    }

    /// `item_key` of the selected run, or "" when nothing is selected.
    pub fn selected_key(&self) -> String {
        self.selected().map(item_key).unwrap_or_default()
    }

    /// Scrolls so the cursor is inside a window of `visible` rows of the
    /// current page.
    pub fn clamp_offset(&mut self, visible: usize) {
        let visible = visible.max(1);
        let page_start = self.page() * PAGE_SIZE;
        let page_len = self.rows.len().saturating_sub(page_start).min(PAGE_SIZE);
        let cursor = self.cursor - page_start.min(self.cursor);
        if cursor < self.offset {
            self.offset = cursor;
        }
        if cursor >= self.offset + visible {
            self.offset = cursor + 1 - visible;
        }
        self.offset = self.offset.min(page_len.saturating_sub(visible));
    }

    /// Explains an empty body.
    pub fn empty_message(&self) -> String {
        if !self.filter.is_empty() {
            return format!(
                "no runs match {} · esc clears",
                gostd::quote(&self.filter.value())
            );
        }
        if self.tab != StatusTab::All {
            return format!("no {} runs", self.tab.label().to_lowercase());
        }
        "No sessions yet. Run rival to get started.".to_string()
    }

    /// Go: `tabBar`. The status tabs with their counts on the left and the
    /// filter on the right, as exactly `w` cells.
    pub fn tab_bar(&self, w: usize, loading: bool, styles: &Styles) -> Line<'static> {
        if w == 0 {
            return Line::default();
        }
        let mut left: Vec<Span<'static>> = Vec::new();
        for (t, count) in StatusTab::ALL.iter().zip(self.counts) {
            let count = if loading {
                "…".to_string()
            } else {
                count.to_string()
            };
            let style = if *t == self.tab {
                styles.active_tab
            } else {
                styles.inactive_tab
            };
            left.push(Span::raw(" "));
            left.push(Span::styled(format!("{} {count}", t.label()), style));
            left.push(Span::raw("  "));
        }
        let right: Line<'static> = if self.filter.focused() {
            self.filter.line(styles)
        } else if !self.filter.is_empty() {
            Line::from(vec![
                Span::styled("/ ", styles.dim),
                Span::styled(self.filter.value(), styles.text),
            ])
        } else {
            Line::from(Span::styled("/ filter", styles.dim))
        };
        let lw: usize = left.iter().map(|s| width(&s.content)).sum();
        let rw = line_width(&right);
        // Both fit with the trailing space after the filter.
        if lw + rw < w {
            let mut spans = left;
            spans.push(Span::raw(" ".repeat(w - lw - rw - 1)));
            spans.extend(right.spans);
            spans.push(Span::raw(" "));
            return Line::from(spans);
        }
        if self.filter.focused() || !self.filter.is_empty() {
            // Too narrow for both: the filter the user is typing wins.
            let mut spans = vec![Span::raw(" ")];
            spans.extend(right.spans);
            return pad_line(Line::from(spans), w);
        }
        pad_line(Line::from(left), w)
    }

    /// Draws the column titles, the visible rows and the footer into `area`.
    /// Task 4.3 replaces the row and footer rendering.
    pub fn render(&self, area: Rect, buf: &mut Buffer, spin: &str, styles: &Styles) {
        let (w, h) = (usize::from(area.width), usize::from(area.height));
        if w == 0 || h == 0 {
            return;
        }
        let c = layout_columns(w);
        let titles = ["ST", "KIND", "MODEL", "EFF", "TIME", "PROJECT"];
        let mut lines = vec![Line::styled(
            fit_cell(&join_cells(&c, titles), w),
            styles.column_title,
        )];
        // The footer takes the last line whenever there is a line for it.
        let body_end = if h >= 2 { h - 1 } else { h };
        if self.rows.is_empty() {
            if lines.len() < body_end {
                lines.push(Line::styled(
                    fit_cell(&format!(" {}", self.empty_message()), w),
                    styles.dim,
                ));
            }
        } else {
            let page_start = self.page() * PAGE_SIZE;
            let page_end = (page_start + PAGE_SIZE).min(self.rows.len());
            for pos in page_start + self.offset..page_end {
                if lines.len() >= body_end {
                    break;
                }
                let item = &self.items[self.rows[pos]];
                let status = item_status(item);
                let model = item
                    .primary()
                    .map_or("", |s| if s.model.is_empty() { &s.cli } else { &s.model });
                let cells = [status_glyph(status, spin), "", model, "", "", ""];
                let text = fit_cell(&join_cells(&c, cells), w);
                let style = if pos == self.cursor {
                    styles.selected
                } else {
                    styles.text
                };
                lines.push(Line::styled(text, style));
            }
        }
        while lines.len() < body_end {
            lines.push(Line::raw(" ".repeat(w)));
        }
        if lines.len() < h {
            let runs = match self.rows.len() {
                1 => "1 run".to_string(),
                n => format!("{n} runs"),
            };
            lines.push(Line::styled(fit_cell(&format!(" {runs}"), w), styles.dim));
        }
        for (y, line) in (area.y..).zip(lines) {
            buf.set_line(area.x, y, &line, area.width);
        }
    }
}
