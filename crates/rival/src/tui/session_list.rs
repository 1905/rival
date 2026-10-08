//! The run list: status tabs, filter input, day sections, pages and the
//! cursor. Go: `internal/dashboard/session_list.go`.
//!
//! The grouping rules (status, kind, effort, elapsed) live in
//! `rival_core::sessionview`; only presentation stays here.

use std::borrow::Cow;

use chrono::{DateTime, Days, FixedOffset, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use rival_core::session::{self, Session};
use rival_core::sessionview;

use super::input::TextInput;
use super::model::DisplayItem;
use super::styles::{HeaderStats, Styles};
use super::text::{fit_cell, fit_cell_line, line_width, pad_line, width};

// --- derivations ------------------------------------------------------------

/// The EFF cell: the shared effort, or "mixed".
pub fn group_effort(item: &DisplayItem) -> String {
    sessionview::effort(&item.sessions)
}

/// The group's reduced status.
pub fn group_status(item: &DisplayItem) -> &'static str {
    sessionview::status(&item.sessions)
}

/// The wall-clock span of the whole group.
pub fn group_elapsed(item: &DisplayItem, now: DateTime<FixedOffset>) -> String {
    sessionview::elapsed_at(&item.sessions, now)
}

/// A finished run's recorded duration, a running run's
/// age, or how long a queued run has waited in line.
pub fn format_elapsed(s: &Session, now: DateTime<FixedOffset>) -> String {
    if !s.duration.is_empty() {
        return s.duration.clone();
    }
    if s.status == "running" {
        // An unset start saturates, as the distant unset time of older
        // releases did.
        let nanos = s
            .start_time
            .map_or(i64::MAX, |t| session::sub_nanos(now, t));
        return session::duration_text(nanos);
    }
    if s.status == "queued"
        && let Some(queued_at) = s.queued_at
    {
        return session::duration_text(session::sub_nanos(now, queued_at));
    }
    "-".to_string()
}

/// The model id the TUI shows: the stored id verbatim, so "gpt-6-astra" and
/// "claude-fable-5" stay readable instead of collapsing to a public label or
/// "retired-model". The CLI stands in when no model was recorded.
pub fn model_name(s: &Session) -> &str {
    if s.model.is_empty() { &s.cli } else { &s.model }
}

/// The MODEL cell: the first member's model plus "+N" for the other
/// distinct models. The judge usually reuses a reviewer's model, so counting
/// distinct ids keeps "+N" honest.
pub fn group_model_name(item: &DisplayItem) -> String {
    let Some(first) = item.primary() else {
        return String::new();
    };
    let name = model_name(first);
    let mut seen = vec![name];
    for s in &item.sessions[1..] {
        let other = model_name(s);
        if !seen.contains(&other) {
            seen.push(other);
        }
    }
    match seen.len() - 1 {
        0 => name.to_string(),
        n => format!("{name} +{n}"),
    }
}

/// The KIND cell: what the run does, never how it was transported. A group
/// classifies through `sessionview`; a solo run maps its own mode.
pub fn kind_label(item: &DisplayItem) -> String {
    if item.is_group() {
        return short_kind(sessionview::kind(&item.sessions)).to_string();
    }
    let Some(s) = item.primary() else {
        return String::new();
    };
    let mut kind = short_kind(&s.mode).to_string();
    // "fable" is read-compat for sessions written before 3.34.
    if s.mode == "docker" && (s.cli == "claude" || s.cli == "fable") {
        kind.push_str("/dk");
    }
    kind
}

/// Maps a session mode or `sessionview` kind to its column label. Transport
/// modes (native, docker) and unset modes are plain reviews.
pub fn short_kind(mode: &str) -> &str {
    match mode {
        "megareview" | "consilium" => "mega",
        session::MODE_SECURITY => "sec",
        session::MODE_PLAN => "plan",
        "raw" => "raw",
        "" | "review" | "native" | "docker" => "review",
        _ => mode,
    }
}

/// The last element of the work dir: the part that tells runs apart, which
/// a truncated absolute path cuts off.
pub fn project_name(workdir: &str) -> String {
    if workdir.is_empty() {
        return "-".to_string();
    }
    #[cfg(windows)]
    return windows_base_name(workdir);
    #[cfg(not(windows))]
    return base_name(workdir).to_string();
}

/// The last element on Windows ([`std::path::Path::file_name`]). Both
/// separators count and a drive or UNC volume is dropped: `C:\work\app` and
/// `\\host\share\app` give `app`. A path without a last element, such as a
/// drive root, is shown whole. Session data written on Windows may use either
/// separator.
#[cfg(windows)]
fn windows_base_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned())
}

/// Go: `filepath.Base` on Unix: only `/` separates, so a backslash is an
/// ordinary name byte.
#[cfg_attr(windows, allow(dead_code))]
fn base_name(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/";
    }
    match trimmed.rfind('/') {
        Some(i) => &trimmed[i + 1..],
        None => trimmed,
    }
}

// --- day sections -----------------------------------------------------------

/// Section labels, in display order.
pub const SECTION_ORDER: [&str; 4] = ["TODAY", "YESTERDAY", "THIS WEEK", "OLDER"];

/// The local time zone as a lookup: the UTC offset in force at a UTC
/// instant. Go: `Location.lookup`. Tests inject a fixed or a switching zone.
pub type Zone = fn(NaiveDateTime) -> FixedOffset;

/// The system time zone. Go: `time.Local`.
pub fn local_zone(utc: NaiveDateTime) -> FixedOffset {
    Local.offset_from_utc_datetime(&utc)
}

/// It guesses the offset at the
/// wall time read as UTC, then uses the offset in force at the guessed
/// instant. Each midnight gets its own offset, so a DST switch between two
/// boundaries moves neither. A midnight the switch skips or repeats
/// resolves as in Go.
fn midnight(zone: Zone, day: NaiveDate) -> DateTime<FixedOffset> {
    let wall = day.and_time(NaiveTime::MIN);
    let at = |offset: FixedOffset| wall.checked_sub_offset(offset).unwrap_or(wall);
    let utc = at(zone(at(zone(wall))));
    zone(utc).from_utc_datetime(&utc)
}

/// The section boundaries for one "now": the starts of today, yesterday and
/// THIS WEEK (the five days before yesterday). Day starts are local
/// midnights in `zone`.
#[derive(Debug, Clone, Copy)]
struct DayBounds {
    today: DateTime<FixedOffset>,
    yesterday: DateTime<FixedOffset>,
    week: DateTime<FixedOffset>,
}

impl DayBounds {
    fn new(now: DateTime<FixedOffset>, zone: Zone) -> DayBounds {
        let utc = now.naive_utc();
        let date = now.with_timezone(&zone(utc)).date_naive();
        let start = |days: u64| {
            let day = date
                .checked_sub_days(Days::new(days))
                .unwrap_or(NaiveDate::MIN);
            midnight(zone, day)
        };
        DayBounds {
            today: start(0),
            yesterday: start(1),
            week: start(6),
        }
    }

    /// `t`'s index in [`SECTION_ORDER`]. No time sorts with the oldest.
    fn section_index(&self, t: Option<DateTime<FixedOffset>>) -> usize {
        let Some(t) = t else {
            return 3;
        };
        if t >= self.today {
            0
        } else if t >= self.yesterday {
            1
        } else if t >= self.week {
            2
        } else {
            3
        }
    }
}

/// Buckets `t` relative to `now` by calendar day in `zone`.
pub fn section_for(
    t: Option<DateTime<FixedOffset>>,
    now: DateTime<FixedOffset>,
    zone: Zone,
) -> &'static str {
    SECTION_ORDER[DayBounds::new(now, zone).section_index(t)]
}

/// When a run appeared: its start, or its queue time when it has not
/// started yet.
pub fn item_time(item: &DisplayItem) -> Option<DateTime<FixedOffset>> {
    let s = item.primary()?;
    s.start_time.or(s.queued_at)
}

// --- status -----------------------------------------------------------------

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
        return group_status(item);
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

// --- text filter ------------------------------------------------------------

/// Splits a filter into lowercase, AND-ed terms.
pub fn filter_terms(filter: &str) -> Vec<String> {
    filter
        .to_lowercase()
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// Whether every term (already lowercase) occurs in the item's searchable
/// text. `hay` caches the lowercase haystack for the item's lifetime, so a
/// keystroke never rebuilds it.
pub fn matches_filter(item: &DisplayItem, hay: &mut Option<String>, terms: &[String]) -> bool {
    if terms.is_empty() {
        return true;
    }
    let hay = hay.get_or_insert_with(|| filter_haystack(item).to_lowercase());
    terms.iter().all(|t| hay.contains(t.as_str()))
}

/// The searchable fields: status, kind, then per member its status, model,
/// effort, project, prompt preview, review scope and short id. A newline
/// separates them; no term can hold one (terms split on whitespace), so a
/// match never spans two fields.
pub fn filter_haystack(item: &DisplayItem) -> String {
    let mut b = String::with_capacity(256);
    b.push_str(item_status(item));
    b.push('\n');
    b.push_str(&kind_label(item));
    for s in &item.sessions {
        let preview = s.prompt_preview.as_str();
        let project = project_name(&s.work_dir);
        let id = short_id(&s.id);
        for f in [
            s.status.as_str(),
            model_name(s),
            s.effort.as_str(),
            project.as_str(),
            preview,
            s.review_scope.as_str(),
            &*id,
        ] {
            b.push('\n');
            b.push_str(f);
        }
    }
    b
}

/// Go: `id[:8]`, the first 8 bytes. A cut inside a multi-byte char keeps
/// its stray bytes as U+FFFD.
pub fn short_id(id: &str) -> Cow<'_, str> {
    if id.len() > 8 {
        String::from_utf8_lossy(&id.as_bytes()[..8])
    } else {
        Cow::Borrowed(id)
    }
}

// --- rows -------------------------------------------------------------------

/// One line of the list body: a non-selectable day heading, or a run (an
/// index into the pane's items).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Section(&'static str),
    Run(usize),
}

impl Row {
    pub fn run(self) -> Option<usize> {
        match self {
            Row::Run(i) => Some(i),
            Row::Section(_) => None,
        }
    }

    pub fn is_run(&self) -> bool {
        matches!(self, Row::Run(_))
    }
}

/// Applies the status tab and the text filter, then
/// groups what is left under section headers. Empty sections are omitted.
/// Items keep their relative order inside a section, so the watcher's
/// newest-first sort holds. It also returns the per-tab counts of filter
/// matches from the same pass. `hays` is the haystack cache, one slot per
/// item.
pub fn rows_and_counts(
    items: &[DisplayItem],
    hays: &mut [Option<String>],
    tab: StatusTab,
    terms: &[String],
    now: DateTime<FixedOffset>,
    zone: Zone,
) -> (Vec<Row>, [usize; 4]) {
    let mut counts = [0; 4];
    let bounds = DayBounds::new(now, zone);
    let mut buckets: [Vec<usize>; 4] = Default::default();
    for (i, (item, hay)) in items.iter().zip(hays.iter_mut()).enumerate() {
        if !matches_filter(item, hay, terms) {
            continue;
        }
        let status = item_status(item);
        for (t, count) in StatusTab::ALL.iter().zip(counts.iter_mut()) {
            if t.accepts(status) {
                *count += 1;
            }
        }
        if !tab.accepts(status) {
            continue;
        }
        buckets[bounds.section_index(item_time(item))].push(i);
    }
    let mut rows = Vec::new();
    for (section, members) in SECTION_ORDER.iter().zip(buckets) {
        if members.is_empty() {
            continue;
        }
        rows.push(Row::Section(section));
        rows.extend(members.into_iter().map(Row::Run));
    }
    (rows, counts)
}

// --- columns ----------------------------------------------------------------

/// How many runs one page holds. Section headers do not count.
pub const PAGE_SIZE: usize = 50;

/// Narrow-pane thresholds: EFF drops first, then PROJECT. They are the fixed
/// column sums (leading space + cells + gaps), so a column only drops when it
/// cannot fit at all. There is no prompt column: every review prompt starts
/// with the same boilerplate; PROJECT takes the spare width.
pub const EFFORT_MIN_WIDTH: usize = 68; // 1 + (3+1)+(8+1)+(20+1)+(7+1)+(7+1)+(16+1)
pub const PROJECT_MIN_WIDTH: usize = 60; // same minus the EFF column
pub const LIST_FIXED_WIDTH: usize = EFFORT_MIN_WIDTH;

/// The list column titles.
const TITLES: [&str; 6] = ["ST", "KIND", "MODEL", "EFF", "TIME", "PROJECT"];

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

/// Sizes the columns for a pane width. A row is a
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

/// Go: `joinCells` without styles. Lays cells out per the column widths,
/// dropping zero-width columns.
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

/// Go: `joinCells` with styles. Each cell is fitted first and then styled,
/// so styling cannot change the width.
fn join_styled_cells(c: &Columns, cells: [&str; 6], styles: [Style; 6]) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    let mut first = true;
    for ((cell, w), style) in cells.iter().zip(c.widths()).zip(styles) {
        if w == 0 {
            continue;
        }
        if !first {
            spans.push(Span::raw(" "));
        }
        first = false;
        spans.push(Span::styled(fit_cell(cell, w), style));
    }
    Line::from(spans)
}

/// The TIME cell: the elapsed span, prefixed with the queue position for a
/// queued solo run.
pub fn row_time(item: &DisplayItem, now: DateTime<FixedOffset>) -> String {
    if item.is_group() {
        return group_elapsed(item, now);
    }
    let Some(s) = item.primary() else {
        return String::new();
    };
    let t = format_elapsed(s, now);
    if s.status == "queued" && s.queue_position > 0 {
        return format!("#{} {t}", s.queue_position);
    }
    t
}

/// One run as exactly `width` cells.
pub fn render_row(
    item: &DisplayItem,
    c: &Columns,
    width: usize,
    spin: &str,
    selected: bool,
    now: DateTime<FixedOffset>,
    styles: &Styles,
) -> Line<'static> {
    let status = item_status(item);
    let kind = kind_label(item);
    let model = group_model_name(item);
    let effort = group_effort(item);
    let time = row_time(item, now);
    let project = project_name(item.primary().map_or("", |s| s.work_dir.as_str()));
    let cells = [
        status_glyph(status, spin),
        kind.as_str(),
        model.as_str(),
        effort.as_str(),
        time.as_str(),
        project.as_str(),
    ];
    if selected {
        // One style over plain text: per-cell styles would break the bar.
        return Line::styled(fit_cell(&join_cells(c, cells), width), styles.selected);
    }
    let cell_styles = [
        styles.status(status),
        styles.text,
        styles.text,
        styles.dim,
        styles.text,
        styles.dim,
    ];
    fit_cell_line(join_styled_cells(c, cells, cell_styles), width)
}

/// The footer's run total: "1 run", "N runs".
pub fn run_count(n: usize) -> String {
    if n == 1 {
        "1 run".to_string()
    } else {
        format!("{n} runs")
    }
}

// --- the pane ---------------------------------------------------------------

/// The run list state.
#[derive(Debug, Clone)]
pub struct ListPane {
    /// The full, unfiltered set the rows derive from; tab and filter changes
    /// rebuild the rows from it without waiting for a new snapshot.
    pub items: Vec<DisplayItem>,
    /// The lowercase filter haystack per item, built on first use.
    hays: Vec<Option<String>>,
    /// Every row that passes the tab and the filter, across all pages.
    pub rows: Vec<Row>,
    /// Indexes `rows`; it always sits on a run while there is one.
    pub cursor: usize,
    /// Scrolls inside the current page.
    pub offset: usize,
    /// The row index of each run, in order: `run_rows[i]` is the i-th run.
    /// It maps a run ordinal to its row and, by binary search, back.
    run_rows: Vec<usize>,
    pub tab: StatusTab,
    pub filter: TextInput,
    /// Filter matches per tab, for the tab bar.
    pub counts: [usize; 4],
    /// The header's per-status session counts.
    pub stats: HeaderStats,
    /// Whether anything runs or waits.
    pub any_live: bool,
    /// The zone the day sections follow; tests pin it with the clock.
    pub zone: Zone,
}

impl Default for ListPane {
    fn default() -> Self {
        ListPane {
            items: Vec::new(),
            hays: Vec::new(),
            rows: Vec::new(),
            cursor: 0,
            offset: 0,
            run_rows: Vec::new(),
            tab: StatusTab::All,
            filter: TextInput::new("/ ", "filter"),
            counts: [0; 4],
            stats: HeaderStats::default(),
            any_live: false,
            zone: local_zone,
        }
    }
}

impl ListPane {
    /// Replaces the data and keeps the cursor on the same run. The index
    /// alone is not a stable handle: a queued run starting re-sorts the
    /// list, and following the index would silently swap the selected run.
    pub fn set_items(&mut self, items: Vec<DisplayItem>, now: DateTime<FixedOffset>) {
        let anchor = self.selected_key();
        self.hays = vec![None; items.len()];
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

    /// Re-derives the rows and tab counts from the items, the tab and the
    /// filter, then puts the cursor back on `anchor` (an `item_key`).
    /// Reports whether the anchor survived; when it did not, the cursor
    /// clamps to the nearest run.
    pub fn rebuild(&mut self, now: DateTime<FixedOffset>, anchor: &str) -> bool {
        let terms = filter_terms(&self.filter.value());
        (self.rows, self.counts) = rows_and_counts(
            &self.items,
            &mut self.hays,
            self.tab,
            &terms,
            now,
            self.zone,
        );
        self.run_rows = (0..self.rows.len())
            .filter(|&i| self.rows[i].is_run())
            .collect();
        if let Some((kind, id)) = anchor.split_once(':') {
            let group = kind == "group";
            let found = self.rows.iter().position(|r| {
                // item_key without building a string per row.
                r.run()
                    .and_then(|i| self.items[i].primary())
                    .is_some_and(|s| {
                        if group {
                            s.group_id == id
                        } else {
                            s.group_id.is_empty() && s.id == id
                        }
                    })
            });
            if let Some(i) = found {
                self.cursor = i;
                return true;
            }
        }
        self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
        if self.rows.get(self.cursor).is_some_and(|r| !r.is_run()) {
            // Landed on a section header: prefer the run below it.
            if !self.step(1) {
                self.step(-1);
            }
        }
        false
    }

    /// Moves the cursor to the next run in direction `dir` (±1), skipping
    /// section rows. Reports whether a run was found.
    fn step(&mut self, dir: isize) -> bool {
        let mut i = self.cursor as isize + dir;
        while i >= 0 && (i as usize) < self.rows.len() {
            if self.rows[i as usize].is_run() {
                self.cursor = i as usize;
                return true;
            }
            i += dir;
        }
        false
    }

    /// Moves the cursor `delta` runs, skipping section rows; it stops at the
    /// ends.
    pub fn move_by(&mut self, delta: isize) {
        let dir = delta.signum();
        for _ in 0..delta.unsigned_abs() {
            if !self.step(dir) {
                return;
            }
        }
    }

    /// Puts the cursor on the first run.
    pub fn top(&mut self) {
        self.cursor = self.rows.iter().position(Row::is_run).unwrap_or(0);
        self.offset = 0;
    }

    /// Puts the cursor on the last run.
    pub fn bottom(&mut self) {
        self.cursor = self.rows.iter().rposition(Row::is_run).unwrap_or(0);
    }

    /// The run under the cursor.
    pub fn selected(&self) -> Option<&DisplayItem> {
        self.rows
            .get(self.cursor)
            .and_then(|r| r.run())
            .map(|i| &self.items[i])
    }

    /// `item_key` of the selected run, or "" when nothing is selected.
    pub fn selected_key(&self) -> String {
        self.selected()
            .map(super::model::item_key)
            .unwrap_or_default()
    }

    /// Switches the status tab by `delta`. Like a filter change, it goes
    /// back to page 1 with the cursor on the first run.
    pub fn cycle_tab(&mut self, delta: isize, now: DateTime<FixedOffset>) {
        self.tab = self.tab.cycle(delta);
        self.refilter(now);
    }

    /// Rebuilds after the tab or the filter text changed and resets to page
    /// 1 with the cursor on the first run. The old selection may sit on any
    /// page of the new result, so keeping it would leave the user mid-list.
    pub fn refilter(&mut self, now: DateTime<FixedOffset>) {
        self.rebuild(now, "");
        self.top();
    }

    /// The number of pages, at least 1 even when the list is empty.
    pub fn page_count(&self) -> usize {
        self.run_rows.len().div_ceil(PAGE_SIZE).max(1)
    }

    /// The 0-based page that holds the cursor. It is derived from the
    /// cursor, never stored, so anchoring the cursor by run after a refresh
    /// also picks the page that holds the run.
    pub fn page(&self) -> usize {
        if self.run_rows.is_empty() {
            return 0;
        }
        let i = self
            .run_rows
            .partition_point(|&r| r < self.cursor)
            .min(self.run_rows.len() - 1);
        i / PAGE_SIZE
    }

    /// The current page's rows and the cursor's index in them. When the
    /// page starts mid-section, it gets a copy of that section's header on
    /// top, so every run on screen sits under its header.
    pub fn page_rows(&self) -> (Cow<'_, [Row]>, usize) {
        if self.run_rows.is_empty() {
            return (Cow::Borrowed(&self.rows), self.cursor);
        }
        let first = self.page() * PAGE_SIZE;
        let last = (first + PAGE_SIZE).min(self.run_rows.len()) - 1;
        let (start, end) = (self.run_rows[first], self.run_rows[last] + 1);
        let cursor = self.cursor.saturating_sub(start);
        if start > 0 && !self.rows[start - 1].is_run() {
            return (Cow::Borrowed(&self.rows[start - 1..end]), cursor + 1);
        }
        let header = self.rows[..start].iter().rev().find(|r| !r.is_run());
        match header {
            None => (Cow::Borrowed(&self.rows[start..end]), cursor),
            Some(&header) => {
                let mut out = Vec::with_capacity(end - start + 1);
                out.push(header);
                out.extend_from_slice(&self.rows[start..end]);
                (Cow::Owned(out), cursor + 1)
            }
        }
    }

    /// Moves `delta` pages and puts the cursor on the first run of the new
    /// page. It stops at the first and the last page.
    pub fn turn_page(&mut self, delta: isize) {
        let last = self.page_count() as isize - 1;
        let p = (self.page() as isize + delta).clamp(0, last) as usize;
        if p == self.page() || self.run_rows.is_empty() {
            return;
        }
        self.cursor = self.run_rows[p * PAGE_SIZE];
        self.offset = 0;
    }

    /// Scrolls so the cursor is inside a window of `visible` rows of the
    /// current page. When the cursor is the first run of a section, the
    /// header above it stays in view.
    pub fn clamp_offset(&mut self, visible: usize) {
        let visible = visible.max(1);
        let (len, cursor, header_above) = {
            let (rows, cursor) = self.page_rows();
            let len = rows.len();
            (
                len,
                cursor,
                cursor > 0 && cursor < len && !rows[cursor - 1].is_run(),
            )
        };
        let mut offset = self.offset;
        if cursor < offset {
            offset = cursor;
        }
        if cursor >= offset + visible {
            offset = cursor + 1 - visible;
        }
        if cursor == offset && header_above {
            offset -= 1;
        }
        self.offset = offset.min(len.saturating_sub(visible));
    }

    /// Explains an empty body.
    pub fn empty_message(&self) -> String {
        if !self.filter.is_empty() {
            return format!("no runs match {:?} · esc clears", self.filter.value());
        }
        if self.tab != StatusTab::All {
            return format!("no {} runs", self.tab.label().to_lowercase());
        }
        "No sessions yet. Run rival to get started.".to_string()
    }

    /// The status tabs with their counts on the left and the
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

    /// The column titles, the visible rows of the current page
    /// and the page footer as exactly `height` lines of exactly `width`
    /// cells. Only the visible window is built, so 3000 rows cost the same
    /// as 30.
    pub fn view_lines(
        &self,
        width: usize,
        height: usize,
        spin: &str,
        now: DateTime<FixedOffset>,
        styles: &Styles,
    ) -> Vec<Line<'static>> {
        if width == 0 || height == 0 {
            return Vec::new();
        }
        let c = layout_columns(width);
        let mut lines = Vec::with_capacity(height);
        lines.push(Line::styled(
            fit_cell(&join_cells(&c, TITLES), width),
            styles.column_title,
        ));
        // The footer takes the last line whenever there is a line for it.
        let body_end = if height >= 2 { height - 1 } else { height };
        let (rows, cursor) = self.page_rows();
        if rows.is_empty() {
            if lines.len() < body_end {
                lines.push(Line::styled(
                    fit_cell(&format!(" {}", self.empty_message()), width),
                    styles.dim,
                ));
            }
        } else {
            for (i, row) in rows.iter().enumerate().skip(self.offset) {
                if lines.len() >= body_end {
                    break;
                }
                lines.push(match *row {
                    Row::Section(name) => {
                        Line::styled(fit_cell(&format!(" {name}"), width), styles.section)
                    }
                    Row::Run(item) => {
                        render_row(&self.items[item], &c, width, spin, i == cursor, now, styles)
                    }
                });
            }
        }
        while lines.len() < body_end {
            lines.push(Line::raw(" ".repeat(width)));
        }
        if lines.len() < height {
            lines.push(self.footer(width, styles));
        }
        lines
    }

    /// The plain page footer: "‹ prev  page P/N  next ›  · T runs", or only
    /// "T runs" when everything fits on one page.
    pub fn footer_text(&self) -> String {
        let runs = run_count(self.run_rows.len());
        let n = self.page_count();
        if n > 1 {
            return format!("‹ prev  page {}/{n}  next ›  · {runs}", self.page() + 1);
        }
        runs
    }

    /// The footer as exactly `w` cells. prev and next are dim when there is
    /// no page that way; styling applies only when the text fits, so it can
    /// never change the width.
    pub fn footer(&self, w: usize, styles: &Styles) -> Line<'static> {
        let plain = format!(" {}", self.footer_text());
        let n = self.page_count();
        if n <= 1 || width(&plain) > w {
            return Line::styled(fit_cell(&plain, w), styles.dim);
        }
        let p = self.page();
        let link =
            |s: &'static str, ok: bool| Span::styled(s, if ok { styles.text } else { styles.dim });
        let line = Line::from(vec![
            Span::raw(" "),
            link("‹ prev", p > 0),
            Span::raw("  "),
            Span::styled(format!("page {}/{n}", p + 1), styles.text),
            Span::raw("  "),
            link("next ›", p + 1 < n),
            Span::styled(
                format!("  · {}", run_count(self.run_rows.len())),
                styles.dim,
            ),
        ]);
        pad_line(line, w)
    }

    /// Draws [`ListPane::view_lines`] into `area`.
    pub fn render(
        &self,
        area: Rect,
        buf: &mut Buffer,
        spin: &str,
        now: DateTime<FixedOffset>,
        styles: &Styles,
    ) {
        let lines = self.view_lines(
            usize::from(area.width),
            usize::from(area.height),
            spin,
            now,
            styles,
        );
        for (y, line) in (area.y..).zip(lines) {
            buf.set_line(area.x, y, &line, area.width);
        }
    }
}

#[cfg(test)]
mod tests;
