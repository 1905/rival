//! The full-screen view of one run: tabs, group members, a follow-able
//! viewport, search and the stop confirm. Go:
//! `internal/dashboard/detail_view.go`.
//!
//! The pane never reads a file. The Raw tab shows what its [`LogSlot`]
//! holds; [`DetailPane::reload`] returns the read a worker should make, and
//! [`DetailPane::accept_log`] takes the result only while it is still for
//! the member and width on screen. The Result tab works the same way with
//! its [`ResultSlot`]: the parse runs on a worker and comes back keyed.

use std::collections::HashMap;
use std::sync::Arc;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use rival_core::session::Session;

use super::input::TextInput;
use super::jobs::{Job, PromptsRequest, PromptsResult};
use super::keys::KeyMap;
use super::logview::{LogKey, LogPane, LogResult, LogSlot, sanitize_log};
use super::model::{Ctx, DisplayItem, item_key};
use super::preview::LOADING_LOG;
use super::result_view::{
    FindingFocus, ResultResponse, ResultSlot, ResultTarget, has_details, result_lines,
};
use super::session_list::{
    Zone, format_elapsed, group_elapsed, is_live, item_status, kind_label, model_name,
    project_name, short_id, status_glyph,
};
use super::styles::{Styles, gradient_word};
use super::text::{fit_cell, fit_line, line_width, pad_line, truncate_line, wrap_cells};
use super::viewport::Viewport;

/// The detail tabs, in key order 1-4. Every detail-screen module uses this
/// one enum. The app's `DetailTab` has the same order.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum DetailTab {
    /// The parsed answer of a finished member.
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

/// The rows the detail screen draws around its viewport: breadcrumb, tab
/// bar, separator, separator, status line. The help bar is counted by the
/// caller, which owns its height.
pub const DETAIL_CHROME_H: usize = 5;

/// Go: `vpHeight`. The viewport height for a pane of `height` rows. Resize
/// and render both use it, so the viewport never holds more rows than the
/// screen keeps.
pub fn vp_height(height: usize) -> usize {
    height.saturating_sub(DETAIL_CHROME_H).max(1)
}

/// The Info tab's label column.
pub const INFO_LABEL_W: usize = 11;

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
    /// loses a session reorders its slice, and an index would silently
    /// switch to another member.
    pub member_id: String,
    pub vp: Viewport,
    /// Whether the Raw tab sticks to the log's tail.
    pub follow: bool,
    pub search: TextInput,
    /// The applied search query.
    pub query: String,
    /// The indexes of the lines that contain the query.
    pub matches: Vec<usize>,
    pub match_idx: usize,
    pub confirm: Option<KillConfirm>,
    /// Full prompts by session id.
    pub prompts: HashMap<String, String>,
    /// The item whose missing prompts a worker is loading.
    pub prompts_pending: Option<String>,
    /// The current tab's content before search highlighting, so a new query
    /// never needs a new read.
    pub lines: Vec<Line<'static>>,
    /// The Raw tab's log read.
    pub log: LogSlot,
    /// The Result tab's parses.
    pub result: ResultSlot,
    /// The focused finding and the open ones on the Result tab.
    pub findings: FindingFocus,
    /// The member seen at the last reload and whether it was live then.
    /// Only that member going from live to finished moves Raw to Result.
    last_seen: Option<(String, bool)>,
    /// A one-shot status message ("nothing running"). The next key clears it.
    pub notice: String,
}

impl Default for DetailPane {
    fn default() -> Self {
        DetailPane {
            tab: DetailTab::default(),
            member_id: String::new(),
            vp: Viewport::default(),
            follow: false,
            search: TextInput::new("/ ", "search"),
            query: String::new(),
            matches: Vec::new(),
            match_idx: 0,
            confirm: None,
            prompts: HashMap::new(),
            prompts_pending: None,
            lines: Vec::new(),
            log: LogSlot::default(),
            result: ResultSlot::default(),
            findings: FindingFocus::default(),
            last_seen: None,
            notice: String::new(),
        }
    }
}

impl DetailPane {
    /// Resets the pane for `item`: first member, follow on, no search. A
    /// finished first member opens on Result, a live one on Raw (the app's
    /// `RunDetailModel.sync`). Returns the load for the full prompts the
    /// list's summaries dropped.
    pub fn open(&mut self, item: Option<&DisplayItem>) -> Option<PromptsRequest> {
        let first = item.and_then(DisplayItem::primary);
        let live = first.is_some_and(|s| is_live(&s.status));
        self.tab = if live {
            DetailTab::Raw
        } else {
            DetailTab::Result
        };
        self.member_id = first.map(|s| s.id.clone()).unwrap_or_default();
        self.last_seen = first.map(|s| (s.id.clone(), live));
        self.findings.reset();
        self.follow = true;
        self.clear_search();
        self.confirm = None;
        self.notice.clear();
        self.prompts.clear();
        self.prompts_pending = None;
        let item = item?;
        let mut missing = Vec::new();
        for s in &item.sessions {
            if s.prompt.is_empty() {
                missing.push(s.id.clone());
            } else {
                self.prompts.insert(s.id.clone(), s.prompt.clone());
            }
        }
        if missing.is_empty() {
            return None;
        }
        let key = item_key(item);
        self.prompts_pending = Some(key.clone());
        Some(PromptsRequest {
            item_key: key,
            ids: missing,
        })
    }

    /// Drops everything that belongs to one run. A read or prompt load
    /// still in flight is dropped when it lands.
    pub fn close(&mut self) {
        self.prompts.clear();
        self.prompts_pending = None;
        self.lines.clear();
        self.log.clear();
        self.result.cancel();
        self.findings.reset();
        self.last_seen = None;
        self.confirm = None;
        self.notice.clear();
        self.clear_search();
        self.vp.set_content(Vec::new());
    }

    pub fn clear_search(&mut self) {
        self.query.clear();
        self.matches.clear();
        self.match_idx = 0;
        self.search.reset();
        self.search.blur();
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

    /// Moves to the next (+1) or previous (-1) member, wrapping. A new
    /// member starts at its tail, with follow on. Returns whether the member
    /// changed: re-picking the one shown (a run of one) changes nothing, as
    /// in the app's `RunDetailModel.selectMember`.
    pub fn cycle_member(&mut self, item: Option<&DisplayItem>, delta: isize) -> bool {
        let Some(item) = item.filter(|i| i.sessions.len() >= 2) else {
            return false;
        };
        let n = item.sessions.len() as isize;
        let i = (self.member_index(item) as isize + delta).rem_euclid(n) as usize;
        if item.sessions[i].id == self.member_id {
            return false;
        }
        self.member_id = item.sessions[i].id.clone();
        self.follow = true;
        true
    }

    /// Go: `resize`. Fits the viewport to a `width`×`height` pane. A reader
    /// following the tail stays on it; anyone else keeps their place,
    /// clamped to the new end.
    pub fn resize(&mut self, width: usize, height: usize) {
        self.vp.set_width(width);
        self.vp.set_height(vp_height(height));
        if self.tab == DetailTab::Raw && self.follow {
            self.vp.goto_bottom();
            return;
        }
        self.vp.set_y_offset(self.vp.y_offset() as isize);
    }

    /// The log read the Raw tab needs: the current member's log wrapped to
    /// the viewport width.
    pub fn wanted_log(&self, item: Option<&DisplayItem>) -> Option<LogKey> {
        let width = self.vp.width();
        let s = self.current(item)?;
        (width > 0).then(|| LogKey::new(s, width, 0))
    }

    /// The parse the Result tab needs: the current member's log, once the
    /// member is finished and has one.
    pub fn wanted_result(&self, item: Option<&DisplayItem>) -> Option<ResultTarget> {
        let s = self.current(item)?;
        (!is_live(&s.status) && !s.log_file.is_empty()).then(|| ResultTarget::of(s))
    }

    /// Go: `reload`. Rebuilds the current tab's content for `item`. `reset`
    /// resets the scroll position: Raw jumps to the tail, the other tabs to
    /// the top. Otherwise Raw follows the tail only while follow is on, and
    /// a scrolled-up reader keeps their place.
    ///
    /// When the member on screen went from live to finished while the user
    /// reads Raw at its tail, the tab moves to Result (the app's
    /// `RunDetailModel.sync`). Switching to a finished member is not a
    /// finish.
    ///
    /// Returns the job that refreshes the tab: the Raw log read or the
    /// Result parse. The worker only stats an unchanged file.
    pub fn reload(&mut self, item: Option<&DisplayItem>, ctx: &Ctx, reset: bool) -> Option<Job> {
        let mut reset = reset;
        self.last_seen = match self.current(item) {
            Some(s) => {
                let live = is_live(&s.status);
                let finished = self
                    .last_seen
                    .as_ref()
                    .is_some_and(|(id, was_live)| *id == s.id && *was_live && !live);
                if finished && self.tab == DetailTab::Raw && self.follow {
                    self.tab = DetailTab::Result;
                    reset = true;
                }
                Some((s.id.clone(), live))
            }
            None => None,
        };
        let job = match self.tab {
            DetailTab::Raw => self
                .wanted_log(item)
                .and_then(|key| self.log.request(LogPane::Detail, key, true))
                .map(Job::Log),
            DetailTab::Result => self
                .wanted_result(item)
                .and_then(|target| self.result.request(target))
                .map(Job::Result),
            _ => None,
        };
        self.rebuild(item, ctx, reset);
        job
    }

    /// Builds the current tab from what the pane already holds.
    fn rebuild(&mut self, item: Option<&DisplayItem>, ctx: &Ctx, reset: bool) {
        let width = self.vp.width();
        let Some(s) = self.current(item).filter(|_| width > 0).cloned() else {
            self.lines.clear();
            self.vp.set_content(Vec::new());
            return;
        };
        // A vanished member fell back to the first; stay there if it returns.
        self.member_id = s.id.clone();
        self.lines = match self.tab {
            DetailTab::Result => {
                let entry = self.result.entry_of(&ResultTarget::of(&s));
                result_lines(&s, entry, &mut self.findings, width, ctx)
            }
            DetailTab::Raw => output_lines(&s, width, &self.log, ctx.styles),
            DetailTab::Prompt => prompt_lines(
                &s,
                &self.prompts,
                self.prompts_pending.is_some(),
                width,
                ctx.styles,
            ),
            DetailTab::Info => info_lines(&s, width, ctx),
        };
        self.apply_content(ctx.styles);
        if self.tab == DetailTab::Raw && (reset || self.follow) {
            // Landing on the tail by a reset means following it too.
            self.follow = true;
            self.vp.goto_bottom();
        } else if reset {
            self.vp.goto_top();
        }
    }

    /// Takes a worker's log read if it is still for the member and width on
    /// screen, and redraws the Raw tab with it.
    pub fn accept_log(&mut self, res: LogResult, item: Option<&DisplayItem>, ctx: &Ctx) {
        let wanted = self.wanted_log(item);
        if self.log.accept(res, wanted.as_ref()) && self.tab == DetailTab::Raw {
            self.rebuild(item, ctx, false);
        }
    }

    /// Takes a worker's parse if it is still for the member on screen, and
    /// redraws the Result tab with it.
    pub fn accept_result(&mut self, res: ResultResponse, item: Option<&DisplayItem>, ctx: &Ctx) {
        let wanted = self.wanted_result(item);
        if self.result.accept(res, wanted.as_ref()) && self.tab == DetailTab::Result {
            self.rebuild(item, ctx, false);
        }
    }

    /// The Result tab's own keys while it lists findings: up/down (j/k)
    /// move the focus, enter/space open or close the focused finding's
    /// failure scenario and suggestion. The focused finding is scrolled
    /// into view. Returns false for any other key, or when there are no
    /// findings; the viewport then scrolls as on the other tabs.
    pub fn result_key(
        &mut self,
        key: &str,
        keys: &KeyMap,
        item: Option<&DisplayItem>,
        ctx: &Ctx,
    ) -> bool {
        let n = self.findings.rows.len();
        if self.tab != DetailTab::Result || n == 0 {
            return false;
        }
        let f = &mut self.findings;
        if keys.down.matches(key) {
            f.focus = (f.focus + 1).min(n - 1);
        } else if keys.up.matches(key) {
            f.focus = f.focus.saturating_sub(1);
        } else if keys.toggle.matches(key) {
            let i = f.focus;
            if f.shown().is_some_and(|r| has_details(r, i)) && !f.expanded.remove(&i) {
                f.expanded.insert(i);
            }
        } else {
            return false;
        }
        self.rebuild(item, ctx, false);
        self.scroll_to_focus();
        true
    }

    /// Scrolls the least that shows the focused finding: its last row when
    /// it ends below the view, but never past its first row. The first
    /// finding brings the header back.
    fn scroll_to_focus(&mut self) {
        let Some(rows) = self.findings.rows.get(self.findings.focus).cloned() else {
            return;
        };
        if self.findings.focus == 0 {
            self.vp.goto_top();
        }
        let (top, h) = (self.vp.y_offset(), self.vp.height());
        if rows.end > top + h {
            self.vp.set_y_offset(rows.end.saturating_sub(h) as isize);
        }
        // The severity rule sits right above a group's first finding.
        let start = rows.start.saturating_sub(2);
        if start < self.vp.y_offset() {
            self.vp.set_y_offset(start as isize);
        }
    }

    /// Takes the loaded prompts if they are for the run still open.
    pub fn accept_prompts(&mut self, res: PromptsResult, item: Option<&DisplayItem>, ctx: &Ctx) {
        if self.prompts_pending.as_deref() != Some(res.item_key.as_str()) {
            return;
        }
        self.prompts_pending = None;
        self.prompts.extend(res.prompts);
        if self.tab == DetailTab::Prompt {
            self.rebuild(item, ctx, false);
        }
    }

    /// Go: `applyContent`. Pushes the lines into the viewport with search
    /// matches highlighted. The viewport clamps its offset, so a shrinking
    /// log never leaves it scrolled past its end.
    pub fn apply_content(&mut self, styles: &Styles) {
        self.matches = find_matches(&self.lines, &self.query);
        if self.match_idx >= self.matches.len() {
            self.match_idx = 0;
        }
        let mut out = self.lines.clone();
        for &i in &self.matches {
            out[i] = highlight_line(&out[i], &self.query, styles.matched);
        }
        self.vp.set_content(out);
    }

    /// Go: `runSearch`. Sets the query and jumps to its first match.
    pub fn run_search(&mut self, query: &str, styles: &Styles) {
        self.query = query.trim().to_string();
        self.match_idx = 0;
        self.apply_content(styles);
        self.jump_to_match();
    }

    /// Go: `stepMatch`. The next (+1) or previous (-1) match, wrapping.
    pub fn step_match(&mut self, delta: isize) {
        let n = self.matches.len() as isize;
        if n == 0 {
            return;
        }
        self.match_idx = (self.match_idx as isize + delta).rem_euclid(n) as usize;
        self.jump_to_match();
    }

    /// Go: `jumpToMatch`. Scrolls the current match a third of the way
    /// down. Reading a match means reading old output, so follow pauses.
    fn jump_to_match(&mut self) {
        let Some(&line) = self.matches.get(self.match_idx) else {
            return;
        };
        self.vp
            .set_y_offset(line as isize - (self.vp.height() / 3) as isize);
        // Not at_bottom(): a match near the tail clamps to the bottom, and
        // following there would scroll the match away on the next output.
        self.follow = false;
    }

    /// Go: `view`. The whole detail screen for `item`: exactly
    /// `area.height` rows, none wider than `area.width`. The viewport
    /// already holds the content; drawing reads no file.
    pub fn render(
        &self,
        item: Option<&DisplayItem>,
        area: Rect,
        buf: &mut Buffer,
        spin: &str,
        ctx: &Ctx,
    ) {
        let (w, h) = (usize::from(area.width), usize::from(area.height));
        let Some(item) = item.filter(|i| i.primary().is_some()) else {
            return;
        };
        if w == 0 || h == 0 {
            return;
        }
        let sep = Line::styled("─".repeat(w), ctx.styles.dim);
        let mut lines = vec![
            self.breadcrumb(item, w, spin, ctx),
            self.tab_bar(item, w, ctx.styles),
            sep.clone(),
        ];
        let visible = self.vp.visible();
        for i in 0..vp_height(h) {
            lines.push(
                visible
                    .get(i)
                    .cloned()
                    .map_or_else(Line::default, |l| fit_line(l, w)),
            );
        }
        lines.push(sep);
        lines.push(self.status_line(w, ctx.styles));
        for (y, line) in (area.y..).zip(lines.into_iter().take(h)) {
            buf.set_line(area.x, y, &pad_line(line, w), area.width);
        }
    }

    /// Go: `breadcrumb`. " rival › project › kind id8" on the left and
    /// "glyph status elapsed   follow ●" on the right.
    fn breadcrumb(&self, item: &DisplayItem, w: usize, spin: &str, ctx: &Ctx) -> Line<'static> {
        let st = ctx.styles;
        let Some(s) = item.primary() else {
            return Line::default();
        };
        let id = if s.group_id.is_empty() {
            &s.id
        } else {
            &s.group_id
        };
        let arrow = || Span::styled(" › ", st.dim);
        let mut left = vec![Span::raw(" ")];
        left.extend(gradient_word("rival"));
        left.extend([
            arrow(),
            Span::styled(project_name(&s.work_dir), st.text),
            arrow(),
            Span::styled(format!("{} {}", kind_label(item), short_id(id)), st.text),
        ]);
        let status = item_status(item);
        let elapsed = if item.is_group() {
            group_elapsed(item, ctx.now)
        } else {
            format_elapsed(s, ctx.now)
        };
        let glyph = match status_glyph(status, spin) {
            "" => "●",
            g => g,
        };
        let follow = if self.follow { "●" } else { "○" };
        let right = Line::from(vec![
            Span::styled(format!("{glyph} {status} {elapsed}"), st.status(status)),
            Span::raw("   "),
            Span::styled("follow ", st.dim),
            Span::styled(follow, st.text),
            Span::raw(" "),
        ]);
        join_ends(Line::from(left), right, w)
    }

    /// Go: `tabBar`. The tabs on the left and, for a group, the member bar on
    /// the right: " 1 Result  2 Raw  3 Prompt  4 Info      [ gpt-5.5 ] judge".
    fn tab_bar(&self, item: &DisplayItem, w: usize, st: &Styles) -> Line<'static> {
        let mut left = Vec::new();
        for (i, tab) in DetailTab::ALL.iter().enumerate() {
            let style = if *tab == self.tab {
                st.active_tab
            } else {
                st.inactive_tab
            };
            left.push(Span::raw(" "));
            left.push(Span::styled(format!("{} {}", i + 1, tab.label()), style));
            left.push(Span::raw(" "));
        }
        if !item.is_group() {
            return truncate_line(Line::from(left), w, "");
        }
        let cur = self.member_index(item);
        let mut right = Vec::new();
        for (i, s) in item.sessions.iter().enumerate() {
            let label = member_label(s);
            if i == cur {
                right.push(Span::styled(format!("[ {label} ]"), st.active_tab));
            } else {
                right.push(Span::styled(label.to_string(), st.inactive_tab));
            }
            right.push(Span::raw(" "));
        }
        let (l, r) = (Line::from(left), Line::from(right));
        if line_width(&l) + line_width(&r) + 2 > w {
            // Too narrow for both at the ends: members follow the tabs
            // directly and the tail is cut.
            let mut spans = l.spans;
            spans.push(Span::raw(" "));
            spans.extend(r.spans);
            return truncate_line(Line::from(spans), w, "…");
        }
        join_ends(l, r, w)
    }

    /// Go: `statusLine`. The row under the viewport: the confirm bar, the
    /// search input, the search result, a one-shot notice, or the scroll
    /// position.
    fn status_line(&self, w: usize, st: &Styles) -> Line<'static> {
        let line = if let Some(confirm) = &self.confirm {
            let n = confirm.targets.len();
            let noun = if n == 1 { "session" } else { "sessions" };
            Line::from(vec![
                Span::raw(" "),
                Span::styled(format!("stop {n} running {noun}? y/n"), st.running),
            ])
        } else if self.search.focused() {
            let mut line = self.search.line(st);
            line.spans.insert(0, Span::raw(" "));
            line
        } else if !self.query.is_empty() {
            let mut spans = vec![
                Span::raw(" "),
                Span::styled(format!("/{}", self.query), st.text),
                Span::raw("  "),
            ];
            match self.matches.len() {
                0 => spans.push(Span::styled("no matches · esc clear", st.dim)),
                n => {
                    spans.push(Span::styled(
                        format!("{}/{n} matches", self.match_idx + 1),
                        st.text,
                    ));
                    spans.push(Span::styled(" · n next · N prev · esc clear", st.dim));
                }
            }
            Line::from(spans)
        } else if !self.notice.is_empty() {
            Line::from(vec![
                Span::raw(" "),
                Span::styled(self.notice.clone(), st.dim),
            ])
        } else {
            let total = self.vp.total_line_count();
            if total > 0 {
                let first = self.vp.y_offset() + 1;
                let last = total.min(self.vp.y_offset() + self.vp.height());
                Line::styled(format!(" lines {first}-{last} of {total}"), st.dim)
            } else {
                Line::default()
            }
        };
        truncate_line(line, w, "")
    }
}

/// Go: `joinEnds`. `left` and `right` at the two ends of a `w`-cell line.
/// When both do not fit, `left` is cut first: the status on the right
/// matters more.
pub fn join_ends(left: Line<'static>, right: Line<'static>, w: usize) -> Line<'static> {
    let rw = line_width(&right);
    if rw >= w {
        return truncate_line(right, w, "");
    }
    let mut left = left;
    if line_width(&left) + rw > w {
        left = truncate_line(left, w - rw, "…");
    }
    let lw = line_width(&left);
    let mut spans = left.spans;
    spans.push(Span::raw(" ".repeat(w - lw - rw)));
    spans.extend(right.spans);
    Line::from(spans)
}

/// Go: `memberLabel`. A member's tab name: its model id, or "judge" for the
/// consilium judge.
pub fn member_label(s: &Session) -> &str {
    if s.mode == "consilium" {
        "judge"
    } else {
        model_name(s)
    }
}

/// Go: `outputLines`. The member's log, pre-wrapped, followed by its error
/// when it failed. The error goes last because follow parks the reader
/// there.
pub fn output_lines(s: &Session, width: usize, log: &LogSlot, st: &Styles) -> Vec<Line<'static>> {
    let mut lines = match log.entry_of(s).map(|e| &e.result) {
        None => vec![Line::styled(LOADING_LOG, st.dim)],
        Some(Err(msg)) => vec![Line::styled(format!("(log unavailable: {msg})"), st.dim)],
        Some(Ok(log)) if log.lines.is_empty() => vec![Line::styled("(empty log)", st.dim)],
        Some(Ok(log)) => log
            .lines
            .iter()
            .enumerate()
            .map(|(i, l)| {
                if i < log.marker_rows {
                    Line::styled(l.clone(), st.label)
                } else {
                    Line::raw(l.clone())
                }
            })
            .collect(),
    };
    if s.status == "failed" {
        lines.extend(error_lines(s, width, st));
    }
    lines
}

/// Go: `errorLines`. The error under an "error:" heading, wrapped to
/// `width`, or nothing when there is none.
pub fn error_lines(s: &Session, width: usize, st: &Styles) -> Vec<Line<'static>> {
    if s.error_msg.is_empty() {
        return Vec::new();
    }
    let mut out = vec![Line::default(), Line::styled("error:", st.failed)];
    out.extend(
        wrap_cells(&sanitize_log(&s.error_msg), width)
            .into_iter()
            .map(|l| Line::styled(l, st.failed)),
    );
    out
}

/// Go: `promptLines`. The member's full prompt, word-wrapped. When the
/// stored record cannot be read it is the 100-byte preview plus a note;
/// while the load is still running the note says so.
pub fn prompt_lines(
    s: &Session,
    prompts: &HashMap<String, String>,
    loading: bool,
    width: usize,
    st: &Styles,
) -> Vec<Line<'static>> {
    if let Some(full) = prompts.get(&s.id).filter(|p| !p.is_empty()) {
        return wrap_cells(&sanitize_log(full), width)
            .into_iter()
            .map(Line::raw)
            .collect();
    }
    let mut lines: Vec<Line<'static>> = if s.prompt_preview.is_empty() {
        Vec::new()
    } else {
        wrap_cells(&sanitize_log(&s.prompt_preview), width)
            .into_iter()
            .map(Line::raw)
            .collect()
    };
    let note = if loading {
        "(loading full prompt…)"
    } else {
        "(full prompt unavailable)"
    };
    lines.push(Line::styled(note, st.dim));
    lines
}

/// A stored time in the local zone, Go layout "2006-01-02 15:04:05"; empty
/// for no time.
fn local_stamp(t: Option<chrono::DateTime<chrono::FixedOffset>>, zone: Zone) -> String {
    match t {
        Some(t) => t
            .with_timezone(&zone(t.naive_utc()))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string(),
        None => String::new(),
    }
}

/// Go: `infoLines`. Every stored field of `s`, one per row, then the full
/// error. Values wrap under their label rather than being cut.
pub fn info_lines(s: &Session, width: usize, ctx: &Ctx) -> Vec<Line<'static>> {
    let st = ctx.styles;
    let val_w = width.saturating_sub(INFO_LABEL_W + 1).max(1);
    let indent = " ".repeat(INFO_LABEL_W + 1);
    let mut out = Vec::new();
    let mut add = |label: &str, value: String, style: Style| {
        let value = if value.is_empty() {
            "-".to_string()
        } else {
            value
        };
        for (i, l) in wrap_cells(&value, val_w).into_iter().enumerate() {
            let mut spans = if i == 0 {
                vec![
                    Span::styled(fit_cell(label, INFO_LABEL_W), st.dim),
                    Span::raw(" "),
                ]
            } else {
                vec![Span::raw(indent.clone())]
            };
            spans.push(Span::styled(l, style));
            out.push(Line::from(spans));
        }
    };
    let exit = s.exit_code.map(|c| c.to_string()).unwrap_or_default();
    let pid = if s.pid > 0 {
        s.pid.to_string()
    } else {
        String::new()
    };
    add("id", s.id.clone(), st.value);
    add("group id", s.group_id.clone(), st.text);
    add("cli", s.cli.clone(), st.text);
    add("model", model_name(s).to_string(), st.value);
    add("effort", s.effort.clone(), st.text);
    add("mode", s.mode.clone(), st.text);
    add("status", s.status.clone(), st.status(&s.status));
    add("exit", exit, st.text);
    add("started", local_stamp(s.start_time, ctx.zone), st.text);
    add("ended", local_stamp(s.end_time, ctx.zone), st.text);
    add("duration", format_elapsed(s, ctx.now), st.text);
    add("queued at", local_stamp(s.queued_at, ctx.zone), st.text);
    add("workdir", s.work_dir.clone(), st.text);
    add("scope", s.review_scope.clone(), st.text);
    add("account", s.account.clone(), st.text);
    add("pid", pid, st.text);
    add(
        "output",
        format!("{} bytes, {} lines", s.output_bytes, s.output_lines),
        st.text,
    );
    add("log", s.log_file.clone(), st.text);
    out.extend(error_lines(s, width, st));
    out
}

/// Lowers `s` one char at a time and keeps the first char of each
/// lowercase form, so `İ` becomes `i` and the char count does not change.
fn lower_chars(s: &str) -> String {
    s.chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect()
}

/// The plain text of a styled line.
fn plain(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

/// Go: `findMatches`. The indexes of the lines that contain `query`,
/// ignoring case (Go's per-rune lowering) and styling. An empty query
/// matches nothing.
pub fn find_matches(lines: &[Line<'static>], query: &str) -> Vec<usize> {
    let q = lower_chars(query);
    if q.is_empty() {
        return Vec::new();
    }
    lines
        .iter()
        .enumerate()
        .filter(|(_, l)| lower_chars(&plain(l)).contains(&q))
        .map(|(i, _)| i)
        .collect()
}

/// Go: `highlightLine`. Gives every case-insensitive occurrence of `query`
/// the match style. Colours only, so the line keeps its width and text.
pub fn highlight_line(line: &Line<'static>, query: &str, matched: Style) -> Line<'static> {
    let q: Vec<char> = lower_chars(query).chars().collect();
    if q.is_empty() {
        return line.clone();
    }
    // `lower_chars` maps char to char, so the lowered text has the same
    // char count.
    let lower: Vec<char> = lower_chars(&plain(line)).chars().collect();
    let mut hit = vec![false; lower.len()];
    let mut i = 0;
    while i + q.len() <= lower.len() {
        if lower[i..i + q.len()] == q[..] {
            hit[i..i + q.len()].iter_mut().for_each(|h| *h = true);
            i += q.len();
        } else {
            i += 1;
        }
    }
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut pos = 0;
    for span in &line.spans {
        let mut run = String::new();
        let mut run_hit = None;
        for c in span.content.chars() {
            let h = hit.get(pos).copied().unwrap_or(false);
            if run_hit.is_some_and(|r| r != h) {
                let style = if run_hit == Some(true) {
                    matched
                } else {
                    span.style
                };
                spans.push(Span::styled(std::mem::take(&mut run), style));
            }
            run_hit = Some(h);
            run.push(c);
            pos += 1;
        }
        if !run.is_empty() {
            let style = if run_hit == Some(true) {
                matched
            } else {
                span.style
            };
            spans.push(Span::styled(run, style));
        }
    }
    let mut out = Line::from(spans).style(line.style);
    out.alignment = line.alignment;
    out
}

#[cfg(test)]
mod tests;
