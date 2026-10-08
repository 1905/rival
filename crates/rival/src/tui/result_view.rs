//! The Result tab: a finished member's parsed answer. Swift:
//! `RivalApp/ResultPane.swift`, and the parse in `LogSnapshot.load` of
//! `RunDetail.swift`.
//!
//! The parse never runs in `update` or `draw`. [`ResultSlot::request`] names
//! the member's log and the file state of its cached parse; a job worker runs
//! [`load_result`], which stats the file and reads and parses the 256 KB tail
//! only when that state changed. The pane keeps the newest parses in a small
//! cache keyed by session, path, size and mtime, so a member is parsed once
//! per log revision. Every result carries its key and a sequence number, so a
//! late one never replaces the member or the file revision on screen.
//!
//! Answer text comes from provider output. Every string is stripped of
//! terminal controls before it becomes a span; the stored session and the
//! parser's text stay as they are.

use std::collections::{HashSet, VecDeque};
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use rival_core::logfmt;
use rival_core::result::{Finding, RunResult, SEVERITY_NAMES, SeverityGroup, severity_rank};
use rival_core::session::Session;

use super::detail_view::join_ends;
use super::logview::{FileState, ReadTail, display_text, file_state};
use super::markdown;
use super::model::Ctx;
use super::session_list::{format_elapsed, is_live, model_name};
use super::styles::Styles;
use super::text::{line_width, width, wrap_cells};

/// How many parses the pane keeps: enough to flip between the members of a
/// large group without parsing again. Each holds at most a 256 KB tail's
/// answer.
pub const RESULT_CACHE_CAP: usize = 8;

/// The Result tab of a member that is still running or queued.
pub const LIVE_NOTE: &str = "Run is still going — press 2 for Raw.";
/// The title over a parse failure.
pub const PARSE_FAILED: &str = "⚠ Couldn't parse this run's output.";
/// Under every note that has no result to show.
pub const RAW_HINT: &str = "press 2 for Raw";
/// A member without a log path.
pub const NO_LOG: &str = "(no log file recorded)";
/// While the first parse of a member is on a worker.
pub const READING: &str = "(reading result…)";

/// The finding bar and its gap: two cells in front of every finding line.
const GUTTER_W: usize = 2;

// --- the parse job ---------------------------------------------------------------

/// Which log a parse is of. The cache key adds the file state.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResultTarget {
    pub session_id: String,
    pub path: String,
}

impl ResultTarget {
    pub fn of(s: &Session) -> ResultTarget {
        ResultTarget {
            session_id: s.id.clone(),
            path: s.log_file.clone(),
        }
    }
}

/// A parse for a worker. `known` is the file state of the cached parse of
/// the same target: when the file still has it, nothing is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultRequest {
    pub seq: u64,
    pub target: ResultTarget,
    pub known: Option<FileState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResultOutcome {
    /// The file still has the `known` state; keep the cached parse.
    Unchanged,
    /// A fresh parse of the tail. `state` is the file state it was read at:
    /// with the target, the cache key. `None` when the stat failed but the
    /// read did not; the next request then reads again.
    Parsed {
        state: Option<FileState>,
        result: Arc<RunResult>,
    },
    /// The log could not be read; Go's error text.
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultResponse {
    pub seq: u64,
    pub target: ResultTarget,
    pub outcome: ResultOutcome,
}

/// Runs on a job worker. Stats the log; when its state differs from
/// `req.known`, reads the last [`logfmt::MAX_TAIL_BYTES`] and parses the raw
/// (unsanitized) text, as the app does. The state is taken before the read,
/// so a log that grows in between is read again on the next request.
pub fn load_result(req: ResultRequest, read_tail: ReadTail) -> ResultResponse {
    let path = req.target.path.as_str();
    let read = |state| match read_tail(Path::new(path), logfmt::MAX_TAIL_BYTES) {
        Ok((data, _)) => ResultOutcome::Parsed {
            state,
            result: Arc::new(rival_core::result::parse_run_result(
                &String::from_utf8_lossy(&data),
            )),
        },
        Err(e) => ResultOutcome::Failed(format!("open {path}: {}", e)),
    };
    let outcome = match file_state(path) {
        // Let the read report the error, as the Raw tab does.
        Err(_) => read(None),
        Ok(state) if req.known == Some(state) => ResultOutcome::Unchanged,
        Ok(state) => read(Some(state)),
    };
    ResultResponse {
        seq: req.seq,
        target: req.target,
        outcome,
    }
}

/// One cached parse, or the read error to show instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultEntry {
    pub target: ResultTarget,
    pub state: Option<FileState>,
    pub result: Result<Arc<RunResult>, String>,
}

/// The pane's parses: a cache of at most [`RESULT_CACHE_CAP`] entries, one
/// per target, and the one request whose reply counts.
#[derive(Debug, Clone, Default)]
pub struct ResultSlot {
    /// Least recently used first.
    cache: VecDeque<ResultEntry>,
    /// The newest request. A reply with any other sequence number answers a
    /// request that was superseded or cancelled.
    in_flight: Option<(ResultTarget, u64)>,
    seq: u64,
}

impl ResultSlot {
    /// The newest parse of `target`, at whatever file state.
    pub fn entry_of(&self, target: &ResultTarget) -> Option<&ResultEntry> {
        self.cache.iter().find(|e| e.target == *target)
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    /// Whether a parse of `target` is on a worker.
    #[cfg(test)]
    pub fn in_flight(&self, target: &ResultTarget) -> bool {
        self.in_flight.as_ref().is_some_and(|(t, _)| t == target)
    }

    /// A parse request for `target`, or `None` when one is already in
    /// flight. With a cached parse the worker only stats an unchanged log.
    pub fn request(&mut self, target: ResultTarget) -> Option<ResultRequest> {
        if self.in_flight.as_ref().is_some_and(|(t, _)| *t == target) {
            return None;
        }
        // A failed read is never trusted; it is read again.
        let known = self
            .entry_of(&target)
            .filter(|e| e.result.is_ok())
            .and_then(|e| e.state);
        self.seq += 1;
        self.in_flight = Some((target.clone(), self.seq));
        Some(ResultRequest {
            seq: self.seq,
            target,
            known,
        })
    }

    /// Applies a worker's result. Only the reply to the newest request
    /// counts: an older one (a member the user left, or a request replaced
    /// in the queue) is dropped and leaves the newest one waiting. The reply
    /// is also dropped when `wanted` (the target the pane shows now)
    /// differs. Returns whether the shown parse changed.
    ///
    /// The cache changes only here, for the request in flight, so the entry
    /// an `Unchanged` reply vouches for is the one its request named.
    pub fn accept(&mut self, res: ResultResponse, wanted: Option<&ResultTarget>) -> bool {
        if self.in_flight.as_ref().map(|(_, seq)| *seq) != Some(res.seq) {
            return false;
        }
        self.in_flight = None;
        if wanted != Some(&res.target) {
            return false;
        }
        let (state, result) = match res.outcome {
            ResultOutcome::Unchanged => {
                // Still current: keep it from being evicted.
                if let Some(i) = self.cache.iter().position(|e| e.target == res.target)
                    && let Some(entry) = self.cache.remove(i)
                {
                    self.cache.push_back(entry);
                }
                return false;
            }
            ResultOutcome::Parsed { state, result } => (state, Ok(result)),
            ResultOutcome::Failed(msg) => (None, Err(msg)),
        };
        self.cache.retain(|e| e.target != res.target);
        self.cache.push_back(ResultEntry {
            target: res.target,
            state,
            result,
        });
        while self.cache.len() > RESULT_CACHE_CAP {
            self.cache.pop_front();
        }
        true
    }

    /// Drops the request in flight; its result is ignored when it lands.
    /// The cache stays, so reopening a run parses nothing again.
    pub fn cancel(&mut self) {
        self.in_flight = None;
    }
}

// --- focus -----------------------------------------------------------------------

/// The focused finding and the findings whose failure scenario and
/// suggestion are open. Indexes count findings in display order.
#[derive(Debug, Clone, Default)]
pub struct FindingFocus {
    pub focus: usize,
    pub expanded: HashSet<usize>,
    /// The content rows of each finding, built with the lines.
    pub rows: Vec<Range<usize>>,
    /// The parse the indexes belong to. Another parse resets them.
    shown: Option<Arc<RunResult>>,
}

impl FindingFocus {
    pub fn reset(&mut self) {
        *self = FindingFocus::default();
    }

    /// The parse the indexes belong to.
    pub fn shown(&self) -> Option<&Arc<RunResult>> {
        self.shown.as_ref()
    }

    fn show(&mut self, result: Option<&Arc<RunResult>>) {
        let same = match (&self.shown, result) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.reset();
            self.shown = result.cloned();
        }
    }
}

// --- the view --------------------------------------------------------------------

/// The Result tab of `s` at `width`, from its cached parse `entry`. Records
/// where each finding's rows are in `focus.rows`.
pub fn result_lines(
    s: &Session,
    entry: Option<&ResultEntry>,
    focus: &mut FindingFocus,
    width: usize,
    ctx: &Ctx,
) -> Vec<Line<'static>> {
    let st = ctx.styles;
    focus.rows.clear();
    if is_live(&s.status) {
        focus.show(None);
        return vec![key_hint(LIVE_NOTE, st)];
    }
    if s.log_file.is_empty() {
        focus.show(None);
        return note(NO_LOG, width, st);
    }
    let Some(entry) = entry else {
        return vec![styled(READING, st.dim)];
    };
    let result = match &entry.result {
        Err(msg) => {
            focus.show(None);
            return note(&format!("(log unavailable: {msg})"), width, st);
        }
        Ok(result) => result,
    };
    focus.show(Some(result));
    match result.as_ref() {
        RunResult::Findings {
            summary,
            rating,
            groups,
        } => findings_lines(s, summary, *rating, groups, focus, width, ctx),
        RunResult::Markdown { text } => {
            let mut out = header_box(s, None, None, "", width, ctx);
            out.push(Line::default());
            let w = u16::try_from(width).unwrap_or(u16::MAX);
            out.extend(markdown::render(text, w, st).lines);
            out
        }
        RunResult::Failed { reason } => failed_lines(s, reason, width, st),
    }
}

/// One span of `text` in `style`. The style sits on the span, not the line,
/// so it survives when the line's spans move behind a frame or a bar.
fn styled(text: impl Into<std::borrow::Cow<'static, str>>, style: Style) -> Line<'static> {
    Line::from(Span::styled(text, style))
}

/// A dim note with its "2" key in the accent.
fn key_hint(text: &'static str, st: &Styles) -> Line<'static> {
    let (before, after) = text.split_once('2').unwrap_or((text, ""));
    Line::from(vec![
        Span::styled(before, st.dim),
        Span::styled("2", st.accent),
        Span::styled(after, st.dim),
    ])
}

fn raw_hint(st: &Styles) -> Line<'static> {
    key_hint(RAW_HINT, st)
}

/// A dim note and the Raw hint.
fn note(text: &str, width: usize, st: &Styles) -> Vec<Line<'static>> {
    let mut out = wrapped(text, width, st.dim);
    out.push(Line::default());
    out.push(raw_hint(st));
    out
}

/// `text` without terminal controls, word-wrapped to `width`, one style.
fn wrapped(text: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    wrap_cells(&display_text(text), width)
        .into_iter()
        .map(|l| styled(l, style))
        .collect()
}

/// Swift `severityColor`: critical and high in the failure colour, medium
/// amber, the rest dim.
fn severity_style(severity: &str, st: &Styles) -> Style {
    match severity_rank(severity) {
        0 | 1 => st.failed,
        2 => st.running,
        _ => st.dim,
    }
}

/// Swift `ResultNote` with a title: the parse failure, its reason, the
/// session's error and the Raw hint.
fn failed_lines(s: &Session, reason: &str, width: usize, st: &Styles) -> Vec<Line<'static>> {
    let mut out = vec![styled(PARSE_FAILED, st.running), Line::default()];
    out.extend(wrapped(reason, width, st.dim));
    if !s.error_msg.is_empty() {
        out.push(Line::default());
        out.push(styled("error:", st.failed));
        out.extend(wrapped(&s.error_msg, width, st.failed));
    }
    out.push(Line::default());
    out.push(raw_hint(st));
    out
}

/// Swift `HeaderCard`: "model · effort · mode · elapsed" with the rating at
/// the right, the severity counts (`groups`), and the summary, framed.
fn header_box(
    s: &Session,
    rating: Option<u8>,
    groups: Option<&[SeverityGroup]>,
    summary: &str,
    width: usize,
    ctx: &Ctx,
) -> Vec<Line<'static>> {
    let st = ctx.styles;
    let inner = width.saturating_sub(4).max(1);
    let mut rows = meta_rows(s, rating, inner, ctx);
    if let Some(groups) = groups {
        rows.extend(count_rows(groups, inner, st));
    }
    let summary = summary.trim();
    if !summary.is_empty() {
        rows.push(Line::default());
        rows.extend(wrapped(summary, inner, st.text));
    }
    framed(rows, width, st)
}

/// The meta line, wrapped when it is long, and the rating: at its right
/// end when both fit, else right-aligned on a row of its own.
fn meta_rows(s: &Session, rating: Option<u8>, inner: usize, ctx: &Ctx) -> Vec<Line<'static>> {
    let st = ctx.styles;
    let elapsed = format_elapsed(s, ctx.now);
    let meta = [model_name(s), &s.effort, &s.mode, &elapsed]
        .iter()
        .map(|p| display_text(p))
        .filter(|p| !p.is_empty() && p != "-")
        .collect::<Vec<_>>()
        .join(" · ");
    let mut rows = if meta.is_empty() {
        Vec::new()
    } else {
        wrap_cells(&meta, inner)
            .into_iter()
            .map(|l| styled(l, st.accent))
            .collect()
    };
    let Some(r) = rating else {
        return rows;
    };
    let r = styled(format!("rating {r}/10"), st.running);
    let rw = line_width(&r);
    match rows.as_mut_slice() {
        [only] if line_width(only) + 2 + rw <= inner => {
            *only = join_ends(only.clone(), r, inner);
        }
        _ => rows.push(join_ends(Line::default(), r, inner)),
    }
    rows
}

/// "● 1 critical  ● 2 high", wrapped between entries, or "No findings.".
fn count_rows(groups: &[SeverityGroup], inner: usize, st: &Styles) -> Vec<Line<'static>> {
    if groups.is_empty() {
        return vec![styled("No findings.", st.dim)];
    }
    let mut rows: Vec<Vec<Span<'static>>> = Vec::new();
    let mut used = 0;
    for g in groups {
        let label = format!(" {} {}", g.findings.len(), g.severity);
        let w = 1 + width(&label);
        match rows.last_mut() {
            Some(row) if used + 2 + w <= inner => {
                row.push(Span::raw("  "));
                used += 2;
            }
            _ => {
                rows.push(Vec::new());
                used = 0;
            }
        }
        let row = rows.last_mut().expect("a row was just ensured");
        row.push(Span::styled("●", severity_style(&g.severity, st)));
        row.push(Span::styled(label, st.text));
        used += w;
    }
    rows.into_iter().map(Line::from).collect()
}

/// `rows` inside a rounded dim frame with a one-cell pad each side, `width`
/// cells wide. Too narrow for a frame, the rows stay bare.
fn framed(rows: Vec<Line<'static>>, width: usize, st: &Styles) -> Vec<Line<'static>> {
    if width < 5 {
        return rows;
    }
    let inner = width - 4;
    let rule = "─".repeat(width - 2);
    let mut out = vec![styled(format!("╭{rule}╮"), st.dim)];
    for row in rows {
        let pad = inner.saturating_sub(line_width(&row));
        let mut spans = vec![Span::styled("│ ", st.dim)];
        spans.extend(row.spans);
        spans.push(Span::raw(" ".repeat(pad)));
        spans.push(Span::styled(" │", st.dim));
        out.push(Line::from(spans));
    }
    out.push(styled(format!("╰{rule}╯"), st.dim));
    out
}

/// The header, then per severity a rule and its findings. Records each
/// finding's rows in `focus.rows`.
fn findings_lines(
    s: &Session,
    summary: &str,
    rating: Option<u8>,
    groups: &[SeverityGroup],
    focus: &mut FindingFocus,
    width: usize,
    ctx: &Ctx,
) -> Vec<Line<'static>> {
    let st = ctx.styles;
    let mut out = header_box(s, rating, Some(groups), summary, width, ctx);
    let total: usize = groups.iter().map(|g| g.findings.len()).sum();
    focus.focus = focus.focus.min(total.saturating_sub(1));
    let mut i = 0;
    for g in groups {
        out.push(Line::default());
        out.push(rule_line(&g.severity, width, st));
        for (n, f) in g.findings.iter().enumerate() {
            if n > 0 {
                out.push(Line::default());
            }
            let lines = finding_lines(f, i == focus.focus, focus.expanded.contains(&i), width, st);
            focus.rows.push(out.len()..out.len() + lines.len());
            out.extend(lines);
            i += 1;
        }
    }
    out
}

/// "CRITICAL ─────" across `width`.
fn rule_line(severity: &str, w: usize, st: &Styles) -> Line<'static> {
    let title = display_text(&severity.to_uppercase());
    let rest = w.saturating_sub(width(&title) + 1);
    Line::from(vec![
        Span::styled(
            title,
            severity_style(severity, st).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled("─".repeat(rest), st.dim),
    ])
}

/// Swift `FindingCard.location`: "file:line", the file, "line N", or "—".
pub fn location(f: &Finding) -> String {
    match (f.file.is_empty(), f.line > 0) {
        (true, true) => format!("line {}", f.line),
        (true, false) => "—".to_string(),
        (false, true) => format!("{}:{}", f.file, f.line),
        (false, false) => f.file.clone(),
    }
}

/// Swift `FindingCard.meta`: category, confidence, and a severity off the
/// ladder.
pub fn finding_meta(f: &Finding) -> Vec<String> {
    let mut parts = Vec::new();
    if !f.category.is_empty() {
        parts.push(f.category.clone());
    }
    if f.confidence > 0 {
        parts.push(format!("conf {}", f.confidence));
    }
    if severity_rank(&f.severity) == SEVERITY_NAMES.len() && !f.severity.is_empty() {
        parts.push(f.severity.clone());
    }
    parts
}

/// One finding behind its bar: location and meta, the bold title, the body,
/// then the failure scenario and the suggestion, open or behind "▸".
fn finding_lines(
    f: &Finding,
    focused: bool,
    expanded: bool,
    w: usize,
    st: &Styles,
) -> Vec<Line<'static>> {
    let cw = w.saturating_sub(GUTTER_W).max(1);
    let mut body: Vec<Line<'static>> = wrapped(&location(f), cw, st.accent);
    let meta = finding_meta(f)
        .iter()
        .map(|p| display_text(p))
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    if !meta.is_empty() {
        let tail = format!(" · {meta}");
        match body.last_mut() {
            Some(last) if line_width(last) + width(&tail) <= cw => {
                last.spans.push(Span::styled(tail, st.dim));
            }
            _ => body.extend(wrapped(&meta, cw, st.dim)),
        }
    }
    for (text, style) in [(&f.title, st.value), (&f.body, st.text)] {
        let text = text.trim();
        if !text.is_empty() {
            body.extend(wrapped(text, cw, style));
        }
    }
    for (label, text) in [
        ("failure scenario", &f.failure_scenario),
        ("suggestion", &f.suggestion),
    ] {
        let Some(text) = text else { continue };
        let mark = if expanded { "▾" } else { "▸" };
        body.push(styled(format!("{mark} {label}"), st.dim));
        if expanded {
            for l in wrapped(text.trim(), cw.saturating_sub(2).max(1), st.text) {
                let mut spans = vec![Span::raw("  ")];
                spans.extend(l.spans);
                body.push(Line::from(spans));
            }
        }
    }
    let bar = if focused {
        Span::styled("▌ ", st.accent.add_modifier(Modifier::BOLD))
    } else {
        Span::styled("│ ", severity_style(&f.severity, st))
    };
    body.into_iter()
        .map(|l| {
            let mut spans = vec![bar.clone()];
            spans.extend(l.spans);
            Line::from(spans)
        })
        .collect()
}

/// Whether finding `i` has anything to open.
pub fn has_details(result: &RunResult, i: usize) -> bool {
    let RunResult::Findings { groups, .. } = result else {
        return false;
    };
    groups
        .iter()
        .flat_map(|g| &g.findings)
        .nth(i)
        .is_some_and(|f| f.failure_scenario.is_some() || f.suggestion.is_some())
}

#[cfg(test)]
mod tests;
