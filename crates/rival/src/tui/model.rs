//! The dashboard model: state, message routing and the frame.
//! Go: `internal/dashboard/model.go`.
//!
//! The model never touches the terminal, the session files or the clock on
//! its own. The runtime (Task 4.5) feeds it [`Msg`]s from the watcher, the
//! keyboard and its timers, carries out the returned [`Cmd`]s and draws it
//! with [`Model::draw`]. Tests drive it the same way through a ratatui
//! `TestBackend`.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, FixedOffset, Local};
use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding, Widget};

use rival_core::session::Session;
use rival_core::sessionview::{self, Bucket, LoadProgress, SessionEvent};

use super::detail_view::{DetailPane, DetailTab};
use super::keys::{ARROW_DOWN, ARROW_UP, FORCE_QUIT, KeyMap, Mode, help_lines, key_name};
use super::layout::{Layout, MIN_HEIGHT, MIN_WIDTH, compute_layout};
use super::session_list::{ListPane, is_live, item_status};
use super::styles::{HeaderStats, STYLES, Styles, gradient_bar, render_header};
use super::text::{fit_line, line_width, pad_line, truncate};

/// One or more sessions shown as one row: a solo run, or every member of a
/// group run.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayItem {
    pub sessions: Vec<Arc<Session>>,
}

impl DisplayItem {
    /// The first session, used for shared metadata.
    pub fn primary(&self) -> Option<&Arc<Session>> {
        self.sessions.first()
    }

    /// True for a logical grouped run, including a degraded run where only
    /// one requested model passed preflight.
    pub fn is_group(&self) -> bool {
        self.sessions.len() > 1
            || self
                .sessions
                .first()
                .is_some_and(|s| !s.group_id.is_empty())
    }
}

impl From<Bucket> for DisplayItem {
    fn from(b: Bucket) -> Self {
        DisplayItem {
            sessions: b.sessions,
        }
    }
}

/// Merges sessions sharing a group id into display items. The bucketing
/// itself lives in `sessionview`.
pub fn group_sessions(sessions: &[Arc<Session>]) -> Vec<DisplayItem> {
    sessionview::group(sessions)
        .into_iter()
        .map(DisplayItem::from)
        .collect()
}

/// Identifies a display item across refreshes. A group is keyed by its
/// group id, a solo session by its own id; both are stable for the run's
/// lifetime.
pub fn item_key(item: &DisplayItem) -> String {
    match item.primary() {
        None => String::new(),
        Some(s) if !s.group_id.is_empty() => format!("group:{}", s.group_id),
        Some(s) => format!("solo:{}", s.id),
    }
}

/// Whether a row is still running or waiting.
pub fn item_live(item: &DisplayItem) -> bool {
    is_live(item_status(item))
}

/// Everything the model reacts to.
#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    Key(KeyEvent),
    /// Bracketed paste: lands in whichever text input has focus.
    Paste(String),
    Resize {
        width: u16,
        height: u16,
    },
    /// A watcher snapshot.
    Sessions(SessionEvent),
    /// The initial scan's progress, before the first snapshot.
    Progress(LoadProgress),
    /// The 1s refresh tick ([`TICK_INTERVAL`]).
    Tick,
    /// A spinner frame tick ([`SPIN_INTERVAL`]).
    SpinTick,
    /// The watcher failed to start; no snapshot is coming.
    Error(String),
}

/// What the runtime must do after an update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    /// Stop the watcher and leave the terminal.
    Quit,
    /// Send [`Msg::Tick`] after [`TICK_INTERVAL`].
    Tick,
    /// Send [`Msg::SpinTick`] after [`SPIN_INTERVAL`].
    Spin,
}

/// The refresh tick for live timers and log tails.
pub const TICK_INTERVAL: Duration = Duration::from_secs(1);
/// The spinner frame rate (bubbles `MiniDot`: 12 fps).
pub const SPIN_INTERVAL: Duration = Duration::from_millis(1000 / 12);
/// Bubbles' `MiniDot` spinner.
const SPIN_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Caps the loader bar so it reads as a bar, not a rule.
const LOADER_BAR_MAX: usize = 48;

/// The dashboard state.
#[derive(Debug, Clone)]
pub struct Model {
    pub(crate) lay: Layout,
    /// The help bar's height in rows. It depends on the mode, "?" and the
    /// width, so `measure_help` recomputes it when one of those changes.
    pub(crate) help_h: usize,
    /// Turns true on the first snapshot. Until then the body is the loader
    /// and the counts are "…", never a misleading 0.
    pub(crate) loaded: bool,
    pub(crate) load: LoadProgress,
    pub(crate) err_text: String,
    pub(crate) quitting: bool,
    pub(crate) keys: KeyMap,
    /// "?" expands the help bar.
    pub(crate) show_all_help: bool,
    pub(crate) mode: Mode,
    pub(crate) list: ListPane,
    pub(crate) detail: DetailPane,
    pub(crate) spin_frame: usize,
    /// Whether a spinner tick is in flight, so a new event never starts a
    /// second, faster chain.
    pub(crate) spinning: bool,
    /// The same guard for the 1s refresh tick.
    pub(crate) ticking: bool,
    pub(crate) version: String,
    pub(crate) styles: Styles,
    /// The wall clock for day sections and elapsed times; tests pin it.
    pub(crate) clock: fn() -> DateTime<FixedOffset>,
}

fn local_now() -> DateTime<FixedOffset> {
    Local::now().fixed_offset()
}

impl Model {
    /// A model for rival `version`. Call [`Model::init`] once to get the
    /// first commands.
    pub fn new(version: impl Into<String>) -> Model {
        let mut m = Model {
            lay: Layout::default(),
            help_h: 1,
            loaded: false,
            load: LoadProgress { done: 0, total: 0 },
            err_text: String::new(),
            quitting: false,
            keys: KeyMap::default(),
            show_all_help: false,
            mode: Mode::List,
            list: ListPane::default(),
            detail: DetailPane::default(),
            spin_frame: 0,
            // Init starts the spinner chain for the loader.
            spinning: true,
            ticking: false,
            version: version.into(),
            styles: STYLES,
            clock: local_now,
        };
        m.measure_help();
        m
    }

    /// The commands to run at startup: the loader's spinner.
    pub fn init(&self) -> Vec<Cmd> {
        vec![Cmd::Spin]
    }

    /// Whether the user asked to quit.
    pub fn quitting(&self) -> bool {
        self.quitting
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    fn now(&self) -> DateTime<FixedOffset> {
        (self.clock)()
    }

    /// Handles one message and returns what the runtime must do next.
    pub fn update(&mut self, msg: Msg) -> Vec<Cmd> {
        let (prev_mode, prev_help) = (self.mode, self.show_all_help);
        let spin = matches!(msg, Msg::SpinTick);
        let cmds = self.route(msg);
        // The help bar's height depends on the mode and on "?". When either
        // changes the detail viewport must be resized, or it keeps a height
        // the view then clips.
        if self.mode != prev_mode || self.show_all_help != prev_help {
            self.measure_help();
            self.resize_detail();
        }
        // A spinner frame changes nothing the list shows.
        if !spin {
            self.reconcile();
        }
        cmds
    }

    /// Runs once after every update: scrolls the list so the cursor stays in
    /// view. Task 4.4 adds the preview refresh here.
    fn reconcile(&mut self) {
        let rows = self.list_rows();
        self.list.clamp_offset(rows);
    }

    fn route(&mut self, msg: Msg) -> Vec<Cmd> {
        match msg {
            Msg::Key(ev) => {
                let Some(key) = key_name(&ev) else {
                    return Vec::new();
                };
                if FORCE_QUIT.matches(&key) {
                    return self.quit();
                }
                match self.mode {
                    Mode::Filter => self.filter_key(&key, &ev),
                    Mode::Detail => self.detail_key(&key),
                    Mode::Search => self.search_key(&key, &ev),
                    Mode::Confirm => self.confirm_key(&key),
                    Mode::List => self.list_key(&key),
                }
            }
            Msg::Paste(text) => {
                // A paste arrives here, not as keys, so it must refilter too.
                match self.mode {
                    Mode::Filter => {
                        let before = self.list.filter.value();
                        self.list.filter.insert(&text);
                        if self.list.filter.value() != before {
                            let now = self.now();
                            self.list.refilter(now);
                        }
                    }
                    Mode::Search => self.detail.search.insert(&text),
                    _ => {}
                }
                Vec::new()
            }
            Msg::Resize { width, height } => {
                self.lay = compute_layout(usize::from(width), usize::from(height));
                self.measure_help();
                self.sync_detail(false);
                Vec::new()
            }
            Msg::Sessions(ev) => {
                self.loaded = true;
                self.apply_sessions(&ev.sessions);
                self.ensure_live_timers()
            }
            Msg::Tick => {
                // Re-read the open run while it can still grow. Keep ticking
                // while anything runs.
                if self.in_detail() && self.list.selected().is_some_and(item_live) {
                    self.sync_detail(false);
                }
                if self.list.any_live {
                    return vec![Cmd::Tick];
                }
                self.ticking = false;
                Vec::new()
            }
            Msg::Progress(p) => {
                if !self.loaded {
                    self.load = p;
                }
                Vec::new()
            }
            Msg::SpinTick => {
                // Dropping the tick ends the chain; the next snapshot with a
                // running row restarts it. The loader keeps it alive.
                if self.loaded && !self.list.any_live {
                    self.spinning = false;
                    return Vec::new();
                }
                self.spin_frame = (self.spin_frame + 1) % SPIN_FRAMES.len();
                vec![Cmd::Spin]
            }
            Msg::Error(text) => {
                self.err_text = text;
                // No snapshot is coming; let the loader's spinner chain end.
                self.loaded = true;
                Vec::new()
            }
        }
    }

    fn quit(&mut self) -> Vec<Cmd> {
        self.quitting = true;
        vec![Cmd::Quit]
    }

    /// Starts whichever of the refresh tick and the spinner is not already
    /// running, while anything is live. Each chain renews itself and ends on
    /// its own once nothing runs, so a second one would double its rate.
    fn ensure_live_timers(&mut self) -> Vec<Cmd> {
        let mut cmds = Vec::new();
        if !self.list.any_live {
            return cmds;
        }
        if !self.ticking {
            self.ticking = true;
            cmds.push(Cmd::Tick);
        }
        if !self.spinning {
            self.spinning = true;
            cmds.push(Cmd::Spin);
        }
        cmds
    }

    /// Rebuilds the list from a watcher snapshot. The list keeps the cursor
    /// on the same run; when that run vanished while its detail view was
    /// open, the user drops back to the list rather than seeing another
    /// run's log under the old heading.
    fn apply_sessions(&mut self, sessions: &[Arc<Session>]) {
        let anchor = self.list.selected_key();
        let now = self.now();
        self.list.set_items(group_sessions(sessions), now);
        if self.in_detail() {
            if self.list.selected_key() != anchor {
                self.close_detail();
            } else {
                self.sync_detail(false);
            }
        }
    }

    /// Whether the detail screen shows: browsing it, typing a search, or
    /// answering the stop confirm.
    pub fn in_detail(&self) -> bool {
        matches!(self.mode, Mode::Detail | Mode::Search | Mode::Confirm)
    }

    fn close_detail(&mut self) {
        self.mode = Mode::List;
        self.detail.close();
    }

    /// Resizes the detail pane and re-targets it at the current selection.
    /// `reset` puts the scroll back to the tab's start.
    fn sync_detail(&mut self, reset: bool) {
        if !self.in_detail() {
            return;
        }
        self.resize_detail();
        self.detail.reload(self.list.selected(), reset);
    }

    /// Fits the detail pane to the current geometry without re-reading.
    fn resize_detail(&mut self) {
        if self.in_detail() {
            self.detail.resize(self.lay.width, self.content_height());
        }
    }

    fn list_key(&mut self, key: &str) -> Vec<Cmd> {
        let k = self.keys;
        let now = self.now();
        if k.quit.matches(key) {
            return self.quit();
        } else if k.up.matches(key) {
            self.list.move_by(-1);
        } else if k.down.matches(key) {
            self.list.move_by(1);
        } else if k.top.matches(key) {
            self.list.top();
        } else if k.bottom.matches(key) {
            self.list.bottom();
        } else if k.next_page.matches(key) {
            self.list.turn_page(1);
        } else if k.prev_page.matches(key) {
            self.list.turn_page(-1);
        } else if k.next_tab.matches(key) {
            self.list.cycle_tab(1, now);
        } else if k.prev_tab.matches(key) {
            self.list.cycle_tab(-1, now);
        } else if k.help.matches(key) {
            self.show_all_help = !self.show_all_help;
        } else if k.filter.matches(key) {
            self.mode = Mode::Filter;
            self.list.filter.focus();
        } else if k.back.matches(key) {
            // esc outside the input clears a kept filter.
            if !self.list.filter.is_empty() {
                self.list.filter.reset();
                self.list.refilter(now);
            }
        } else if k.open.matches(key) && self.list.selected().is_some() {
            self.mode = Mode::Detail;
            self.detail.open(self.list.selected());
            // Open at the tail: live output and final verdicts both live at
            // the end of the log.
            self.sync_detail(true);
        }
        Vec::new()
    }

    fn filter_key(&mut self, key: &str, ev: &KeyEvent) -> Vec<Cmd> {
        let now = self.now();
        if self.keys.open.matches(key) {
            self.list.filter.blur();
            self.mode = Mode::List;
        } else if self.keys.back.matches(key) {
            self.list.filter.reset();
            self.list.filter.blur();
            self.list.refilter(now);
            self.mode = Mode::List;
        } else if ARROW_UP.matches(key) {
            self.list.move_by(-1);
        } else if ARROW_DOWN.matches(key) {
            self.list.move_by(1);
        } else {
            let before = self.list.filter.value();
            self.list.filter.handle_key(ev);
            if self.list.filter.value() != before {
                self.list.refilter(now);
            }
        }
        Vec::new()
    }

    fn detail_key(&mut self, key: &str) -> Vec<Cmd> {
        let k = self.keys;
        self.detail.notice.clear();
        if k.quit.matches(key) {
            return self.quit();
        } else if k.back.matches(key) || key == "backspace" {
            // esc peels one layer: a search first, then the screen.
            if !self.detail.query.is_empty() {
                self.detail.clear_search();
            } else {
                self.close_detail();
            }
        } else if k.help.matches(key) {
            self.show_all_help = !self.show_all_help;
        } else if let Some(tab) = k.detail_tab(key) {
            self.set_detail_tab(tab);
        } else if k.next_tab.matches(key) {
            self.set_detail_tab(self.detail.tab.cycle(1));
        } else if k.prev_tab.matches(key) {
            self.set_detail_tab(self.detail.tab.cycle(-1));
        } else if k.next_member.matches(key) {
            self.detail.cycle_member(self.list.selected(), 1);
            self.sync_detail(true);
        } else if k.prev_member.matches(key) {
            self.detail.cycle_member(self.list.selected(), -1);
            self.sync_detail(true);
        } else if k.search.matches(key) {
            self.mode = Mode::Search;
            let query = self.detail.query.clone();
            self.detail.search.set_value(&query);
            self.detail.search.cursor_end();
            self.detail.search.focus();
        }
        // Follow, top/bottom, scrolling, n/N matches, open log and stop are
        // detail behaviour: Task 4.4 routes them here.
        Vec::new()
    }

    fn set_detail_tab(&mut self, tab: DetailTab) {
        self.detail.tab = tab;
        self.sync_detail(true);
    }

    /// Every key but enter, esc and ctrl+c is text, so "q" and "n" type
    /// themselves.
    fn search_key(&mut self, key: &str, ev: &KeyEvent) -> Vec<Cmd> {
        if self.keys.open.matches(key) {
            self.detail.search.blur();
            self.mode = Mode::Detail;
            let query = self.detail.search.value();
            self.detail.run_search(&query);
        } else if self.keys.back.matches(key) {
            self.detail.clear_search();
            self.mode = Mode::Detail;
        } else {
            self.detail.search.handle_key(ev);
        }
        Vec::new()
    }

    /// Answers the stop confirm: every key closes the bar, and only y may
    /// stop, so a stray key press can never kill a run. Task 4.4 adds the y
    /// path (re-check against the current snapshot, then the signal).
    fn confirm_key(&mut self, _key: &str) -> Vec<Cmd> {
        self.detail.confirm = None;
        self.mode = Mode::Detail;
        Vec::new()
    }

    // --- geometry -----------------------------------------------------------

    /// The help lines for the current mode, "?" and width.
    fn help_view(&self) -> Vec<Line<'static>> {
        help_lines(
            &self.keys.help(self.mode),
            self.show_all_help,
            self.lay.width,
            &self.styles,
        )
    }

    /// Stores how many rows the help bar takes. Call it whenever the mode,
    /// "?" or the width changes.
    fn measure_help(&mut self) {
        self.help_h = self.help_view().len();
    }

    /// The list pane height: the layout body minus any rows the expanded
    /// help borrows.
    pub(crate) fn list_body_height(&self) -> usize {
        self.lay
            .body_h
            .saturating_sub(self.help_h.saturating_sub(1))
            .max(1)
    }

    /// The list's size inside its border. The border exists only in the
    /// split view; alone, the list takes the whole body.
    pub(crate) fn list_inner_height(&self) -> usize {
        if self.lay.show_preview {
            return self.list_body_height().saturating_sub(2).max(1);
        }
        self.list_body_height()
    }

    pub(crate) fn list_inner_width(&self) -> usize {
        if self.lay.show_preview {
            return self.lay.list_w.saturating_sub(2).max(1);
        }
        self.lay.width
    }

    /// How many data rows fit between the list's column titles and its page
    /// footer.
    pub(crate) fn list_rows(&self) -> usize {
        self.list_inner_height().saturating_sub(2).max(1)
    }

    /// The preview text width: the box minus its border and a 1-col pad each
    /// side.
    pub(crate) fn preview_inner_width(&self) -> usize {
        self.lay.preview_w.saturating_sub(4).max(1)
    }

    /// The detail body height: everything between the header and the help
    /// bar. The view and `sync_detail` both use it.
    pub(crate) fn content_height(&self) -> usize {
        self.lay
            .height
            .saturating_sub(self.lay.header_h + self.help_h)
    }

    // --- view ---------------------------------------------------------------

    /// The header's session counts.
    fn header_stats(&self) -> HeaderStats {
        HeaderStats {
            version: self.version.clone(),
            loading: !self.loaded,
            ..self.list.stats.clone()
        }
    }

    /// The current spinner frame, or "" when nothing runs.
    pub(crate) fn spin_frame(&self) -> &'static str {
        if self.loaded && !self.list.any_live {
            return "";
        }
        SPIN_FRAMES[self.spin_frame]
    }

    /// Draws the frame.
    pub fn draw(&self, frame: &mut Frame) {
        let area = frame.area();
        self.render(area, frame.buffer_mut());
    }

    /// Draws the frame into `area` of `buf`. The geometry comes from the
    /// last resize; anything outside `area` is clipped, so a stale size can
    /// never write out of bounds.
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        let area = area.intersection(buf.area);
        if area.is_empty() || self.quitting {
            return;
        }
        let mut out = Rows::new(area, buf);
        if !self.err_text.is_empty() {
            out.line(Line::raw(format!("Error: {}", self.err_text)));
            return;
        }
        if self.lay.width == 0 || self.lay.height == 0 {
            out.line(Line::raw("Initializing..."));
            return;
        }
        let w = self.lay.width;
        if self.lay.too_small {
            let notice = format!("terminal too small (need {MIN_WIDTH}×{MIN_HEIGHT})");
            out.line(Line::raw(truncate(&notice, w, "")));
            return;
        }

        let spin = self.spin_frame();
        for line in render_header(
            w,
            self.lay.compact,
            &self.header_stats(),
            spin,
            &self.styles,
        ) {
            out.line(pad_line(line, w));
        }
        if self.in_detail() {
            let h = self.content_height();
            let rect = out.take(h);
            self.detail
                .render(self.list.selected(), self.mode, rect, out.buf, &self.styles);
        } else {
            out.line(self.list.tab_bar(w, !self.loaded, &self.styles));
            let rect = out.take(self.list_body_height());
            if self.loaded {
                self.render_body(rect, out.buf, spin);
            } else {
                self.render_loader(rect, out.buf, spin);
            }
        }
        for line in self.help_view() {
            out.line(line);
        }
    }

    /// The list alone, or from the preview width up the list and the
    /// preview in rounded boxes with a 1-col gap. The list has focus here,
    /// so its border is accent and the preview's is dim.
    fn render_body(&self, area: Rect, buf: &mut Buffer, spin: &str) {
        if !self.lay.show_preview {
            self.list.render(area, buf, spin, &self.styles);
            return;
        }
        let list_w = u16::try_from(self.lay.list_w).unwrap_or(u16::MAX);
        let left = Rect {
            width: list_w.min(area.width),
            ..area
        };
        let gap = left.width.saturating_add(1).min(area.width);
        let right = Rect {
            x: area.x + gap,
            width: area.width - gap,
            ..area
        };
        let list_box = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(self.styles.focus_border);
        let inner = list_box.inner(left);
        list_box.render(left, buf);
        self.list.render(inner, buf, spin, &self.styles);
        // Task 4.4 draws the preview inside this box.
        Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(self.styles.border)
            .padding(Padding::horizontal(1))
            .render(right, buf);
    }

    /// Fills the list body until the first snapshot: the spinner and
    /// "reading sessions done/total" over a bar in the logo gradient,
    /// centred in the body.
    fn render_loader(&self, area: Rect, buf: &mut Buffer, spin: &str) {
        let (w, h) = (usize::from(area.width), usize::from(area.height));
        if w == 0 || h == 0 {
            return;
        }
        let mut label = "reading sessions".to_string();
        let mut pct = 0.0;
        if self.load.total > 0 {
            label.push_str(&format!(" {}/{}", self.load.done, self.load.total));
            pct = self.load.done as f64 / self.load.total as f64;
        }
        let bar_w = LOADER_BAR_MAX.min(w.saturating_sub(4).max(1));
        let title = Line::from(vec![
            Span::styled(spin.to_string(), self.styles.running),
            Span::raw(" "),
            Span::styled(label, self.styles.text),
        ]);
        let mut block = vec![fit_line(title, bar_w)];
        if h >= 3 {
            block.push(Line::default());
            block.push(gradient_bar(bar_w, pct, &self.styles));
        }
        let top = (h - block.len().min(h)) / 2;
        let left = (w - bar_w.min(w)) / 2;
        for (i, line) in block.into_iter().enumerate().take(h - top) {
            // Centre each line inside the bar's width, then the bar in the
            // body.
            let pad = (bar_w - line_width(&line).min(bar_w)) / 2;
            let x = area.x + u16::try_from(left + pad).unwrap_or(0);
            let y = area.y + u16::try_from(top + i).unwrap_or(0);
            buf.set_line(x, y, &line, area.width.saturating_sub(x - area.x));
        }
    }
}

/// Writes whole rows top-down into an area and never past its bottom.
struct Rows<'a> {
    area: Rect,
    y: u16,
    buf: &'a mut Buffer,
}

impl<'a> Rows<'a> {
    fn new(area: Rect, buf: &'a mut Buffer) -> Rows<'a> {
        Rows { area, y: 0, buf }
    }

    /// Draws one line in the next row.
    fn line(&mut self, line: Line<'static>) {
        let rect = self.take(1);
        if rect.height > 0 {
            self.buf.set_line(rect.x, rect.y, &line, rect.width);
        }
    }

    /// Reserves the next `n` rows, clipped to the area.
    fn take(&mut self, n: usize) -> Rect {
        let n = u16::try_from(n).unwrap_or(u16::MAX);
        let height = n.min(self.area.height - self.y);
        let rect = Rect {
            x: self.area.x,
            y: self.area.y + self.y,
            width: self.area.width,
            height,
        };
        self.y += height;
        rect
    }
}

#[cfg(test)]
mod tests;
