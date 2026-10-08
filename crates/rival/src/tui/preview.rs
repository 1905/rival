//! The right-hand pane: the selected run's meta block plus the tail of its
//! log. Go: `internal/dashboard/preview.go`.
//!
//! The meta block is cheap and built on every draw from the item and the
//! clock. The log tail comes from a [`LogSlot`] that a worker fills, so
//! drawing never touches a file.

use std::sync::Arc;

use chrono::{DateTime, FixedOffset};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use rival_core::session::Session;

use super::logview::{LogKey, LogPane, LogRequest, LogResult, LogSlot, PREVIEW_TAIL_LINES};
use super::model::{Ctx, DisplayItem};
use super::session_list::{
    Zone, format_elapsed, group_effort, group_elapsed, kind_label, model_name, project_name,
    section_for, status_glyph,
};
use super::text::{fit_cell_line, hardwrap, truncate, truncate_line};

/// Go: `oneLine`. Collapses every whitespace run to one space.
pub fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Go: `joinMeta`. Joins the non-empty parts with a dim middle dot.
pub fn join_meta(parts: Vec<Span<'static>>, ctx: &Ctx) -> Line<'static> {
    let mut spans = Vec::new();
    for part in parts.into_iter().filter(|p| !p.content.is_empty()) {
        if !spans.is_empty() {
            spans.push(Span::styled(" · ", ctx.styles.dim));
        }
        spans.push(part);
    }
    Line::from(spans)
}

/// Go: `startedLine`. "started 11:40 · pid 81233"; a run from another day
/// gets its date too. Times are local in `zone`.
pub fn started_line(
    t: Option<DateTime<FixedOffset>>,
    pid: i64,
    now: DateTime<FixedOffset>,
    zone: Zone,
) -> String {
    let mut line = "started -".to_string();
    if let Some(t) = t {
        let local = t.with_timezone(&zone(t.naive_utc()));
        let layout = if section_for(Some(t), now, zone) == "TODAY" {
            "%H:%M"
        } else {
            "%b %d %H:%M"
        };
        line = format!("started {}", local.format(layout));
    }
    if pid > 0 {
        line.push_str(&format!(" · pid {pid}"));
    }
    line
}

/// Go: `tailSession`. The member whose log the preview tails: the judge when
/// the group has one (its verdict is the result), else the last member.
pub fn tail_session(item: &DisplayItem) -> Option<&Arc<Session>> {
    item.sessions
        .iter()
        .find(|s| s.mode == "consilium")
        .or_else(|| item.sessions.last())
}

/// Go: `previewMeta`. The block above the log: who, what, when, and for a
/// group one line per member. Every line is cut to `width`.
pub fn preview_meta(item: &DisplayItem, width: usize, ctx: &Ctx) -> Vec<Line<'static>> {
    let Some(s) = item.primary() else {
        return Vec::new();
    };
    let st = ctx.styles;
    let cut = |l: Line<'static>| truncate_line(l, width, "…");
    let dim_line = |text: String| Line::styled(text, st.dim);
    let mut lines = Vec::new();
    if item.is_group() {
        lines.push(cut(join_meta(
            vec![
                Span::styled(project_name(&s.work_dir), st.text),
                Span::styled(kind_label(item), st.text),
                Span::styled(format!("{} models", item.sessions.len()), st.dim),
            ],
            ctx,
        )));
        lines.push(cut(join_meta(
            vec![
                Span::styled(group_effort(item), st.dim),
                Span::styled(group_elapsed(item, ctx.now), st.text),
            ],
            ctx,
        )));
        lines.push(cut(dim_line(started_line(
            s.start_time,
            0,
            ctx.now,
            ctx.zone,
        ))));
        for m in &item.sessions {
            let status_style = st.status(&m.status);
            let mut spans = vec![
                Span::styled(status_glyph(&m.status, "●").to_string(), status_style),
                Span::raw(" "),
                Span::styled(model_name(m).to_string(), st.value),
                Span::raw("  "),
                Span::styled(m.status.clone(), status_style),
            ];
            if m.mode == "consilium" {
                spans.push(Span::styled(" · judge", st.dim));
            }
            lines.push(cut(Line::from(spans)));
        }
    } else {
        lines.push(cut(join_meta(
            vec![
                Span::styled(project_name(&s.work_dir), st.text),
                Span::styled(kind_label(item), st.text),
                Span::styled(s.cli.clone(), st.dim),
            ],
            ctx,
        )));
        lines.push(cut(join_meta(
            vec![
                Span::styled(model_name(s).to_string(), st.value),
                Span::styled(s.effort.clone(), st.dim),
                Span::styled(format_elapsed(s, ctx.now), st.status(&s.status)),
            ],
            ctx,
        )));
        lines.push(cut(dim_line(started_line(
            s.start_time,
            s.pid,
            ctx.now,
            ctx.zone,
        ))));
    }
    let scope = one_line(&s.review_scope);
    if !scope.is_empty() {
        lines.push(cut(dim_line("─ scope ─".into())));
        let mut wrapped: Vec<String> = hardwrap(&scope, width.max(1))
            .split('\n')
            .map(str::to_string)
            .collect();
        if wrapped.len() > 2 {
            wrapped.truncate(2);
            wrapped[1].push('…');
        }
        for w in wrapped {
            lines.push(cut(Line::styled(w, st.text)));
        }
    }
    lines
}

/// The preview pane. `tail` caches the log tail by file state: the meta
/// block is re-rendered on every draw, the log only re-read when it changed.
#[derive(Debug, Clone, Default)]
pub struct PreviewPane {
    pub tail: LogSlot,
}

impl PreviewPane {
    /// The meta block and the output heading.
    fn head(item: &DisplayItem, width: usize, ctx: &Ctx) -> Vec<Line<'static>> {
        let mut lines = preview_meta(item, width, ctx);
        lines.push(Line::styled(
            truncate("─ output (tail) ─", width, ""),
            ctx.styles.dim,
        ));
        lines
    }

    /// The log read the pane needs for `item` at `width`×`height`: the
    /// tail session, at most [`PREVIEW_TAIL_LINES`] of the rows left under
    /// the head. `None` when no row is left.
    pub fn tail_key(
        item: Option<&DisplayItem>,
        width: usize,
        height: usize,
        ctx: &Ctx,
    ) -> Option<LogKey> {
        let item = item.filter(|i| i.primary().is_some())?;
        if width == 0 || height == 0 {
            return None;
        }
        let head = Self::head(item, width, ctx).len();
        if head >= height {
            return None;
        }
        let rows = height - head;
        let s = tail_session(item)?;
        Some(LogKey::new(s, width, rows.min(PREVIEW_TAIL_LINES)))
    }

    /// Go: `refresh`. A read for the tail `item` needs, if the cache does not
    /// hold it or `refresh` asks to check the file again.
    pub fn request(
        &mut self,
        item: Option<&DisplayItem>,
        width: usize,
        height: usize,
        ctx: &Ctx,
        refresh: bool,
    ) -> Option<LogRequest> {
        let key = Self::tail_key(item, width, height, ctx)?;
        self.tail.request(LogPane::Preview, key, refresh)
    }

    /// Applies a worker's read if it is still the tail the pane shows.
    pub fn accept(
        &mut self,
        res: LogResult,
        item: Option<&DisplayItem>,
        width: usize,
        height: usize,
        ctx: &Ctx,
    ) -> bool {
        let wanted = Self::tail_key(item, width, height, ctx);
        self.tail.accept(res, wanted.as_ref())
    }

    /// Go: `renderPreview`. The head, then the tail of the log in the rows
    /// left.
    pub fn lines(
        &self,
        item: Option<&DisplayItem>,
        width: usize,
        height: usize,
        ctx: &Ctx,
    ) -> Vec<Line<'static>> {
        let Some(item) = item.filter(|i| i.primary().is_some()) else {
            return Vec::new();
        };
        if width == 0 || height == 0 {
            return Vec::new();
        }
        let mut lines = Self::head(item, width, ctx);
        if lines.len() >= height {
            lines.truncate(height);
            return lines;
        }
        let rows = height - lines.len();
        if let Some(s) = tail_session(item) {
            lines.extend(self.tail_lines(s, width, rows, ctx));
        }
        lines
    }

    /// Go: `previewTail`. The last `rows` wrapped lines of `s`'s log, or a dim
    /// note when there is nothing to show yet.
    fn tail_lines(&self, s: &Session, width: usize, rows: usize, ctx: &Ctx) -> Vec<Line<'static>> {
        let dim = ctx.styles.dim;
        let Some(entry) = self.tail.entry_of(s) else {
            return vec![Line::styled(LOADING_LOG, dim)];
        };
        match &entry.result {
            Err(msg) => vec![Line::styled(
                truncate(&format!("(log unavailable: {msg})"), width, "…"),
                dim,
            )],
            Ok(log) if log.lines.is_empty() => vec![Line::styled("(empty log)", dim)],
            Ok(log) => {
                let skip = log.lines.len().saturating_sub(rows);
                log.lines[skip..]
                    .iter()
                    .map(|l| Line::raw(l.clone()))
                    .collect()
            }
        }
    }

    /// Go: `view`. Exactly `area.height` rows of exactly `area.width` cells.
    pub fn render(&self, item: Option<&DisplayItem>, area: Rect, buf: &mut Buffer, ctx: &Ctx) {
        let (w, h) = (usize::from(area.width), usize::from(area.height));
        if w == 0 || h == 0 {
            return;
        }
        let mut lines = self.lines(item, w, h, ctx).into_iter();
        for y in area.top()..area.bottom() {
            let line = fit_cell_line(lines.next().unwrap_or_default(), w);
            buf.set_line(area.x, y, &line, area.width);
        }
    }
}

/// Shown while a pane waits for the first read of a log.
pub const LOADING_LOG: &str = "(loading log…)";

#[cfg(test)]
mod tests;
