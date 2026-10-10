//! The config window's frame.
//!
//! One rounded box: the title bar (`rival · config`, the config path and
//! `● unsaved`), the section list on the left (a tab row below
//! [`WIDE_MIN_WIDTH`]), the section body, the check panel (one summary line
//! below [`FULL_CHECK_MIN_HEIGHT`] rows, and on the Check section, whose
//! body is the full table) and the key bar. Every line is cut to its
//! column, so nothing overflows at 80×24.

use chrono::{DateTime, FixedOffset};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Widget};

use rival_core::config::ProxyProvider;

use super::config_check::KeyStatus;
use super::config_form::{
    CheckSlot, ConfigForm, Field, MODELS, ModelInfo, NoticeKind, ProbeState, REVIEWERS, Section,
};
use super::styles::{SELECTION_BG, Styles, gradient_word, rgb};
use super::text::{fit_line, pad_line, truncate, width, wrap_cells};
use crate::check::{CheckRow, ProxyLine};

#[cfg(test)]
mod tests;

/// From this width up the sections are a list on the left; below it they
/// fold into a tab row.
pub const WIDE_MIN_WIDTH: usize = 100;
/// Below this height the check panel is one summary line.
pub const FULL_CHECK_MIN_HEIGHT: usize = 30;
/// The section list's width, without the divider.
const NAV_W: u16 = 20;
/// The label column of a field row.
const LABEL_W: usize = 18;
/// The rows the body keeps before the check panel takes more.
const MIN_BODY_H: usize = 14;
/// The most rows the full check panel takes.
const MAX_PANEL_H: usize = 14;

/// What the frame needs besides the form.
#[derive(Debug, Clone, Copy)]
pub struct ViewCtx<'a> {
    pub styles: &'a Styles,
    /// The spinner frame for running check rows.
    pub spin: &'a str,
}

/// Draws the window into `area`.
pub fn render(form: &ConfigForm, area: Rect, buf: &mut Buffer, v: &ViewCtx) {
    let s = v.styles;
    if area.width < 20 || area.height < 8 {
        return;
    }
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(s.border)
        .render(area, buf);
    title_bar(form, area, buf, s);

    let (w, h) = (usize::from(area.width), usize::from(area.height));
    let inner_x = area.x + 1;
    let inner_w = area.width - 2;
    let bottom = area.bottom() - 1; // the bottom border row
    let footer_y = bottom - 1;
    let footer_sep = footer_y - 1;
    hline(buf, area, footer_sep, s);
    let footer = footer_line(form, usize::from(inner_w) - 2, s);
    buf.set_line(inner_x + 1, footer_y, &footer, inner_w - 2);

    let compact = h < FULL_CHECK_MIN_HEIGHT || form.section == Section::Check;
    let wide = w >= WIDE_MIN_WIDTH;
    let panel_w = usize::from(inner_w) - 3;
    let panel = if compact {
        vec![check_summary(form, panel_w, v)]
    } else {
        let cap = (h - 2).saturating_sub(3 + MIN_BODY_H).clamp(3, MAX_PANEL_H);
        check_panel(form, panel_w, cap, v)
    };
    let panel_sep = footer_sep - 1 - panel.len() as u16;
    hline(buf, area, panel_sep, s);
    for (i, line) in panel.iter().enumerate() {
        buf.set_line(inner_x + 2, panel_sep + 1 + i as u16, line, inner_w - 3);
    }

    let top = area.y + 1;
    let body_h = panel_sep - top;
    let body = if wide {
        // The section list and its divider, joined to the borders.
        let div_x = inner_x + NAV_W;
        buf[(div_x, area.y)].set_symbol("┬");
        buf[(div_x, panel_sep)].set_symbol("┴");
        for y in top..panel_sep {
            buf[(div_x, y)].set_symbol("│").set_style(s.border);
        }
        nav(form, Rect::new(inner_x, top, NAV_W, body_h), buf, s);
        Rect::new(div_x + 1, top, area.right() - 2 - div_x, body_h)
    } else {
        let tabs = tab_row(form, usize::from(inner_w) - 2, s);
        buf.set_line(inner_x + 1, top, &tabs, inner_w - 2);
        Rect::new(inner_x, top + 1, inner_w, body_h.saturating_sub(1))
    };
    let body = Rect {
        x: body.x + 2,
        width: body.width.saturating_sub(3),
        ..body
    };
    let lines = section_lines(form, usize::from(body.width), wide, v);
    let scroll = form.section.fields().is_empty().then_some(form.scroll);
    draw_body(&lines, body, buf, scroll);
    if form.help {
        help_overlay(Rect::new(inner_x, top, inner_w, body_h), buf, s);
    }
}

/// `╭─ rival · config ──── ~/.rival/config.yaml  ● unsaved ─╮`.
fn title_bar(form: &ConfigForm, area: Rect, buf: &mut Buffer, s: &Styles) {
    let mut left = vec![Span::raw(" ")];
    left.extend(gradient_word("rival"));
    left.push(Span::styled(" · ", s.dim));
    left.push(Span::styled("config", s.value));
    left.push(Span::raw(" "));
    let left = Line::from(left);
    let left_w = width(&left.to_string());
    buf.set_line(area.x + 2, area.y, &left, area.width.saturating_sub(4));

    let mut right = vec![Span::raw(" ")];
    if form.saving {
        right.push(Span::styled("saving… ", s.running));
    } else if form.dirty() {
        right.push(Span::styled("● unsaved ", s.accent));
    }
    let flag_w: usize = right.iter().map(|sp| width(&sp.content)).sum();
    let room = usize::from(area.width).saturating_sub(left_w + 6 + flag_w);
    if room >= 8 {
        let path = truncate(&form.seed.path_shown, room - 2, "…");
        right.insert(1, Span::styled(format!("{path} "), s.dim));
        if flag_w > 1 {
            right.insert(2, Span::raw(" "));
        }
    }
    let right = Line::from(right);
    let rw = u16::try_from(width(&right.to_string())).unwrap_or(0);
    if rw > 1 {
        buf.set_line(area.right() - 2 - rw, area.y, &right, rw);
    }
}

/// `├──────┤` across the box at row `y`.
fn hline(buf: &mut Buffer, area: Rect, y: u16, s: &Styles) {
    for x in area.x..area.right() {
        let sym = if x == area.x {
            "├"
        } else if x == area.right() - 1 {
            "┤"
        } else {
            "─"
        };
        buf[(x, y)].set_symbol(sym).set_style(s.border);
    }
}

/// The section list with the selection bar.
fn nav(form: &ConfigForm, area: Rect, buf: &mut Buffer, s: &Styles) {
    for (i, section) in Section::ALL.iter().enumerate() {
        let y = area.y + 1 + i as u16;
        if y >= area.bottom() {
            break;
        }
        let on = *section == form.section;
        let mark = if on { "▸ " } else { "  " };
        let style = if on { s.selected } else { s.text };
        let mut spans = vec![
            Span::raw(" "),
            Span::styled(mark, if on { s.selected } else { s.dim }),
            Span::styled(section.title(), style),
        ];
        if *section == Section::Check {
            spans.extend(nav_check_badge(form, s, on));
        }
        let bar_w = usize::from(area.width) - 1;
        let mut line = pad_line(Line::from(spans), bar_w);
        if on {
            line = line.style(s.selected);
        }
        buf.set_line(area.x, y, &line, area.width - 1);
    }
}

/// A count after "Check" in the section list once a check ran.
fn nav_check_badge(form: &ConfigForm, s: &Styles, on: bool) -> Vec<Span<'static>> {
    let c = &form.check;
    if c.slots.is_empty() {
        return Vec::new();
    }
    let failed = c.done() - c.passed();
    let base = |st: Style| if on { st.bg(rgb(SELECTION_BG)) } else { st };
    if c.running {
        return vec![Span::styled(
            format!("  {}/{}", c.done(), c.slots.len()),
            base(s.running),
        )];
    }
    if failed > 0 {
        vec![Span::styled(format!("  ✗ {failed}"), base(s.failed))]
    } else {
        vec![Span::styled("  ✓".to_string(), base(s.accent))]
    }
}

/// The sections as a tab row, for narrow frames.
fn tab_row(form: &ConfigForm, w: usize, s: &Styles) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, section) in Section::ALL.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ", s.dim));
        }
        let style = if *section == form.section {
            s.active_tab
        } else {
            s.inactive_tab
        };
        spans.push(Span::styled(
            format!("{} {}", i + 1, section.title()),
            style,
        ));
    }
    fit_line(Line::from(spans), w)
}

/// A body line; the focused field's line gets the selection tint.
struct BodyLine {
    line: Line<'static>,
    focus: bool,
}

impl BodyLine {
    fn plain(line: Line<'static>) -> BodyLine {
        BodyLine { line, focus: false }
    }
}

/// Draws the body, scrolled so the focused line (and the two lines under
/// it, where its help and error go) stay in view; `scroll` sets the first
/// line of a section without fields.
fn draw_body(lines: &[BodyLine], area: Rect, buf: &mut Buffer, scroll: Option<usize>) {
    let h = usize::from(area.height);
    if h == 0 || area.width == 0 {
        return;
    }
    let max = lines.len().saturating_sub(h);
    let offset = match scroll {
        Some(first) => first.min(max),
        None => {
            let focus = lines.iter().position(|l| l.focus).unwrap_or(0);
            (focus + 3).saturating_sub(h).min(max)
        }
    };
    for (i, l) in lines.iter().skip(offset).take(h).enumerate() {
        let y = area.y + i as u16;
        buf.set_line(area.x, y, &l.line, area.width);
        if l.focus {
            // Tint the row, keeping each value's colour.
            for x in area.x.saturating_sub(1)..area.right() {
                buf[(x, y)].set_bg(rgb(SELECTION_BG));
            }
        }
    }
}

/// The body of the current section.
fn section_lines(form: &ConfigForm, w: usize, wide: bool, v: &ViewCtx) -> Vec<BodyLine> {
    let s = v.styles;
    let mut out = Vec::new();
    if wide {
        let (title, about) = match form.section {
            Section::Proxy => ("PROXY", "Claude and Codex through CLIProxyAPI"),
            Section::Models => ("MODELS", "efforts and the plan review defaults"),
            Section::Review => ("REVIEW", "the security review and the skills"),
            Section::Check => ("CHECK", "one short live call to each model"),
        };
        out.push(BodyLine::plain(fit_line(
            Line::from(vec![
                Span::styled(title, s.heading),
                Span::styled(format!("   {about}"), s.dim),
            ]),
            w,
        )));
    }
    out.push(BodyLine::plain(Line::default()));
    match form.section {
        Section::Proxy => proxy_lines(form, w, v, &mut out),
        Section::Models => model_lines(form, w, v, &mut out),
        Section::Review => review_lines(form, w, v, &mut out),
        Section::Check => check_section_lines(form, w, v, &mut out),
    }
    out
}

/// The cursor and the label of a field row.
fn row_head(form: &ConfigForm, field: Field, s: &Styles) -> Vec<Span<'static>> {
    let focused = form.field() == Some(field) && !form.help;
    let label = format!("{:<LABEL_W$}  ", field.label());
    if focused {
        vec![
            Span::styled("▸ ", s.accent),
            Span::styled(label, s.text.add_modifier(Modifier::BOLD)),
        ]
    } else {
        vec![Span::raw("  "), Span::styled(label, s.dim)]
    }
}

/// A field row, then its error (red) or, when focused, its help (dim).
fn field(
    out: &mut Vec<BodyLine>,
    form: &ConfigForm,
    field: Field,
    value: Vec<Span<'static>>,
    help: &str,
    w: usize,
    s: &Styles,
) {
    field_with(out, form, field, value, help, None, w, s);
}

/// [`field`] with a warning (yellow) that shows when there is no error.
#[allow(clippy::too_many_arguments)]
fn field_with(
    out: &mut Vec<BodyLine>,
    form: &ConfigForm,
    field: Field,
    value: Vec<Span<'static>>,
    help: &str,
    warning: Option<&str>,
    w: usize,
    s: &Styles,
) {
    let mut spans = row_head(form, field, s);
    spans.extend(value);
    let focused = form.field() == Some(field);
    out.push(BodyLine {
        line: fit_line(Line::from(spans), w),
        focus: focused && !form.help,
    });
    let indent = 2 + LABEL_W + 2;
    let edit_err = form
        .edit
        .as_ref()
        .filter(|e| e.field == field)
        .and_then(|e| e.error.clone());
    if let Some(err) = edit_err.as_deref().or(form.error(field)) {
        for (i, part) in wrap_cells(err, w.saturating_sub(indent + 2).max(10))
            .into_iter()
            .enumerate()
        {
            let mark = if i == 0 { "✗ " } else { "  " };
            out.push(BodyLine::plain(fit_line(
                Line::from(vec![
                    Span::raw(" ".repeat(indent)),
                    Span::styled(format!("{mark}{part}"), s.failed),
                ]),
                w,
            )));
        }
    } else if let Some(warning) = warning {
        for (i, part) in wrap_cells(warning, w.saturating_sub(indent + 2).max(10))
            .into_iter()
            .enumerate()
        {
            let mark = if i == 0 { "! " } else { "  " };
            out.push(BodyLine::plain(fit_line(
                Line::from(vec![
                    Span::raw(" ".repeat(indent)),
                    Span::styled(format!("{mark}{part}"), s.warn),
                ]),
                w,
            )));
        }
    } else if focused && !form.help && !help.is_empty() {
        out.push(BodyLine::plain(fit_line(
            Line::from(vec![
                Span::raw(" ".repeat(indent)),
                Span::styled(help.to_string(), s.dim),
            ]),
            w,
        )));
    }
}

fn toggle(on: bool, s: &Styles) -> Span<'static> {
    if on {
        Span::styled("● on ", s.accent)
    } else {
        Span::styled("○ off", s.dim)
    }
}

/// `‹ value ›`, with accent marks and an arrow hint when focused.
fn picker(value: &str, focused: bool, s: &Styles) -> Vec<Span<'static>> {
    let marks = if focused { s.accent } else { s.dim };
    let mut spans = vec![
        Span::styled("‹ ", marks),
        Span::styled(value.to_string(), s.value),
        Span::styled(" ›", marks),
    ];
    if focused {
        spans.push(Span::styled(" ←→", s.dim));
    }
    spans
}

/// The online dot, as the URL row and the check header show it.
fn proxy_status(form: &ConfigForm, s: &Styles) -> Vec<Span<'static>> {
    let any_route =
        form.route_enabled(ProxyProvider::Claude) || form.route_enabled(ProxyProvider::Codex);
    match &form.probe.state {
        ProbeState::Online(n) => vec![
            Span::styled("● online", s.accent),
            Span::styled(format!(" · {n} models"), s.dim),
        ],
        ProbeState::Offline(_) => vec![Span::styled("● offline", s.failed)],
        ProbeState::Waiting => vec![Span::styled("◌ asking the proxy", s.dim)],
        ProbeState::Idle if !any_route || form.seed.proxy_off_env => {
            vec![Span::styled("○ proxy off", s.dim)]
        }
        ProbeState::Idle => match &form.check.proxy {
            Some(ProxyLine::Up { models, .. }) => vec![
                Span::styled("● online", s.accent),
                Span::styled(format!(" · {models} models"), s.dim),
            ],
            Some(ProxyLine::Down { .. }) => vec![Span::styled("● offline", s.failed)],
            _ => vec![Span::styled("○ not asked", s.dim)],
        },
    }
}

fn proxy_lines(form: &ConfigForm, w: usize, v: &ViewCtx, out: &mut Vec<BodyLine>) {
    let s = v.styles;
    let focused = |f: Field| form.field() == Some(f) && !form.help;
    let value_w = w.saturating_sub(2 + LABEL_W + 2);

    for (f, provider, models) in [
        (Field::ClaudeRoute, ProxyProvider::Claude, "Opus, Fable"),
        (Field::CodexRoute, ProxyProvider::Codex, "Codex, Sol"),
    ] {
        let on = form.route_enabled(provider);
        let note = if on {
            format!("   {models} go through the proxy")
        } else {
            format!("   {models} use their own login")
        };
        let help = if form.seed.proxy_off_env {
            "RIVAL_PROXY=off is set: every run goes direct"
        } else {
            "space or ←→ switches it"
        };
        field(
            out,
            form,
            f,
            vec![toggle(on, s), Span::styled(note, s.dim)],
            help,
            w,
            s,
        );
    }

    // URL, with the online dot on the right.
    let editing_url = form.edit.as_ref().filter(|e| e.field == Field::Url);
    let status = proxy_status(form, s);
    let status_w: usize = status.iter().map(|sp| width(&sp.content)).sum();
    let mut value = match editing_url {
        Some(e) => {
            e.input
                .line_in(s, value_w.saturating_sub(status_w + 3).max(8))
                .spans
        }
        None if !form.seed.url_env.is_empty() => vec![
            Span::styled(form.seed.url_env.clone(), s.value),
            Span::styled("  from RIVAL_PROXY_URL", s.dim),
        ],
        None if form.draft.proxy.url.is_empty() => vec![Span::styled("not set", s.dim)],
        None => vec![Span::styled(form.draft.proxy.url.clone(), s.value)],
    };
    let used: usize = value.iter().map(|sp| width(&sp.content)).sum();
    let pad = value_w.saturating_sub(used + status_w).max(2);
    value.push(Span::raw(" ".repeat(pad)));
    value.extend(status);
    let mut help = "the base URL, without /v1 · enter edits".to_string();
    if !form.seed.base_url_env.is_empty() {
        help.push_str(" · d imports ANTHROPIC_BASE_URL");
    }
    field(out, form, Field::Url, value, &help, w, s);
    if let ProbeState::Offline(e) = &form.probe.state
        && form.error(Field::Url).is_none()
    {
        out.push(BodyLine::plain(fit_line(
            Line::from(vec![
                Span::raw(" ".repeat(2 + LABEL_W + 2)),
                Span::styled(e.clone(), s.failed),
            ]),
            w,
        )));
    }

    // Key: masked, never shown.
    let value = match form.edit.as_ref().filter(|e| e.field == Field::Key) {
        Some(e) => masked_input(e.input.len(), e.input.cursor(), s),
        None => key_value(form, s),
    };
    let help = if matches!(form.seed.key, KeyStatus::Set { env: true, .. }) {
        "RIVAL_PROXY_KEY is set and wins over the key file".to_string()
    } else {
        "enter types or pastes a new key; s saves it (0600)".to_string()
    };
    field(out, form, Field::Key, value, &help, w, s);
    if let KeyStatus::Error(e) = &form.seed.key
        && form.pending_key().is_none()
    {
        out.push(BodyLine::plain(fit_line(
            Line::from(vec![
                Span::raw(" ".repeat(2 + LABEL_W + 2)),
                Span::styled(e.clone(), s.failed),
            ]),
            w,
        )));
    }

    for (f, provider) in [
        (Field::ClaudePrefix, ProxyProvider::Claude),
        (Field::CodexPrefix, ProxyProvider::Codex),
    ] {
        let value = match form.edit.as_ref().filter(|e| e.field == f) {
            Some(e) => e.input.line_in(s, value_w.saturating_sub(2).max(8)).spans,
            None => {
                let current = form.prefix(provider);
                let mut spans = picker(shown_prefix(current), focused(f), s);
                let mut others: Vec<String> = form
                    .prefix_choices(provider)
                    .into_iter()
                    .filter(|p| !p.is_empty() && p != current)
                    .collect();
                others.sort();
                if !others.is_empty() {
                    spans.push(Span::styled(
                        format!("   also: {}", others.join(", ")),
                        s.dim,
                    ));
                }
                if !form.route_enabled(provider) {
                    spans.push(Span::styled("   route off", s.dim));
                }
                spans
            }
        };
        let who = match provider {
            ProxyProvider::Claude => "Claude",
            ProxyProvider::Codex => "Codex",
        };
        let help = format!("the proxy account for {who} models · e types one");
        let warning = form
            .prefix_warning(provider)
            .filter(|_| form.route_enabled(provider) && form.edit.is_none());
        field_with(out, form, f, value, &help, warning.as_deref(), w, s);
    }

    out.push(BodyLine::plain(Line::default()));
    out.push(BodyLine::plain(Line::from(Span::styled(
        "REQUESTS GO TO",
        s.section,
    ))));
    let display_w = MODELS.iter().map(|m| width(m.display)).max().unwrap_or(0);
    for m in MODELS.iter().filter(|m| m.provider.is_some()) {
        let proxied = form.proxied(m.provider);
        let (tag, tag_style) = if proxied {
            ("proxy ", s.accent)
        } else {
            ("direct", s.dim)
        };
        out.push(BodyLine::plain(fit_line(
            Line::from(vec![
                Span::raw("  "),
                Span::styled(format!("{:<7}", m.name), s.text),
                Span::styled(format!("{:<display_w$}  ", m.display), s.dim),
                Span::styled(tag, tag_style),
                Span::styled("  → ", s.dim),
                Span::styled(form.wire(m), s.value),
            ]),
            w,
        )));
    }
    if form.seed.proxy_off_env {
        out.push(BodyLine::plain(fit_line(
            Line::from(Span::styled(
                "  RIVAL_PROXY=off is set: every run goes direct",
                s.warn,
            )),
            w,
        )));
    }
}

fn shown_prefix(prefix: &str) -> &str {
    if prefix.is_empty() { "none" } else { prefix }
}

/// A key being typed: one bullet per char and the cursor cell.
fn masked_input(len: usize, cursor: usize, s: &Styles) -> Vec<Span<'static>> {
    let cursor_style = s.accent.add_modifier(Modifier::REVERSED);
    let shown = len.min(32);
    let cursor = cursor.min(shown);
    let mut spans = vec![Span::styled("•".repeat(cursor), s.value)];
    if cursor < shown {
        spans.push(Span::styled("•", cursor_style));
        spans.push(Span::styled("•".repeat(shown - cursor - 1), s.value));
    } else {
        spans.push(Span::styled(" ", cursor_style));
    }
    if len == 0 {
        spans.push(Span::styled(" type or paste the key", s.dim));
    }
    spans
}

/// The saved (or typed) key: `••••••••a91f`, where it lives, its mode.
fn key_value(form: &ConfigForm, s: &Styles) -> Vec<Span<'static>> {
    let pending = form.pending_key().is_some();
    match form.key_view() {
        KeyStatus::Missing => vec![
            Span::styled("missing", s.warn),
            Span::styled("   enter sets it", s.dim),
        ],
        KeyStatus::Error(_) => vec![Span::styled("unusable", s.failed)],
        KeyStatus::Set { tail, env, mode } => {
            let masked = if tail.is_empty() {
                "••••".to_string()
            } else {
                format!("••••••••{tail}")
            };
            let mut spans = vec![Span::styled(masked, s.value)];
            if env {
                spans.push(Span::styled("   from RIVAL_PROXY_KEY", s.dim));
                if pending {
                    spans.push(Span::styled(" (wins over the typed key)", s.warn));
                }
            } else if pending {
                spans.push(Span::styled("   new · s writes it", s.accent));
            } else {
                spans.push(Span::styled(
                    format!("   {}", form.seed.key_path_shown),
                    s.dim,
                ));
                match mode {
                    Some(m) if m & 0o077 == 0 => {
                        spans.push(Span::styled(format!("  {m:04o} ✓"), s.accent));
                    }
                    Some(m) => spans.push(Span::styled(format!("  {m:04o} ✗"), s.failed)),
                    None => {}
                }
            }
            spans
        }
    }
}

fn model_lines(form: &ConfigForm, w: usize, v: &ViewCtx, out: &mut Vec<BodyLine>) {
    let s = v.styles;
    out.push(BodyLine::plain(fit_line(
        Line::from(Span::styled(
            format!(
                "  {:<8}{:<13}{:<8}{:<13}{}",
                "MODEL", "RUNTIME", "ROUTE", "EFFORT", "PLAN"
            ),
            s.column_title,
        )),
        w,
    )));
    for (i, m) in MODELS.iter().enumerate() {
        let f = Field::Model(i);
        let focused = form.field() == Some(f) && !form.help;
        let installed = form.seed.installed.get(i).copied().unwrap_or(true);
        let base = if installed { s.text } else { s.dim };
        let mut spans = if focused {
            vec![
                Span::styled("▸ ", s.accent),
                Span::styled(format!("{:<8}", m.name), base.add_modifier(Modifier::BOLD)),
            ]
        } else {
            vec![
                Span::raw("  "),
                Span::styled(format!("{:<8}", m.name), base),
            ]
        };
        spans.push(Span::styled(
            format!("{:<13}", m.runtime),
            if installed { s.text } else { s.dim },
        ));
        let (route, route_style) = if form.proxied(m.provider) {
            ("proxy", s.accent)
        } else {
            ("direct", s.dim)
        };
        spans.push(Span::styled(format!("{route:<8}"), route_style));
        spans.extend(effort_cell(form, m, focused, s));
        spans.push(Span::styled(
            if !m.plan {
                " — "
            } else if form.plan_default(m) {
                "[■]"
            } else {
                "[ ]"
            },
            if form.plan_default(m) {
                s.accent
            } else {
                s.dim
            },
        ));
        if !installed {
            spans.push(Span::styled("  not installed", s.dim));
        }
        out.push(BodyLine {
            line: fit_line(Line::from(spans), w),
            focus: focused,
        });
    }
    out.push(BodyLine::plain(Line::default()));
    if let Some(Field::Model(i)) = form.field()
        && let Some(m) = MODELS.get(i)
    {
        let mut spans = vec![
            Span::styled(format!("  {} ", m.display), s.text),
            Span::styled("sends ", s.dim),
            Span::styled(form.wire(m), s.value),
        ];
        if m.ladder.len() < 2 {
            spans.push(Span::styled(
                format!(" · runs at {} only", m.ladder[0]),
                s.dim,
            ));
        }
        out.push(BodyLine::plain(fit_line(Line::from(spans), w)));
    }
    out.push(BodyLine::plain(fit_line(
        Line::from(Span::styled(
            "  PLAN marks what rival command plan runs without -m",
            s.dim,
        )),
        w,
    )));
}

/// The effort column, 13 cells: a picker on the focused row.
fn effort_cell(form: &ConfigForm, m: &ModelInfo, focused: bool, s: &Styles) -> Vec<Span<'static>> {
    let effort = form.effort(m);
    if focused && m.ladder.len() > 1 {
        let mut spans = picker(&effort, true, s);
        spans.pop(); // the arrow hint lives in the key bar here
        let used = width(&effort) + 4;
        spans.push(Span::raw(" ".repeat(13usize.saturating_sub(used))));
        return spans;
    }
    let text = if m.ladder.len() < 2 {
        format!("  {effort} fixed")
    } else {
        format!("  {effort}")
    };
    vec![Span::styled(
        format!("{text:<13}"),
        if m.ladder.len() < 2 { s.dim } else { s.text },
    )]
}

fn review_lines(form: &ConfigForm, w: usize, v: &ViewCtx, out: &mut Vec<BodyLine>) {
    let s = v.styles;
    let reviewer = form.reviewer().to_string();
    let about = REVIEWERS
        .iter()
        .find(|(n, _)| *n == reviewer)
        .map_or("", |(_, a)| a);
    let mut value = picker(&reviewer, form.field() == Some(Field::Reviewer), s);
    value.push(Span::styled(format!("   {about}"), s.dim));
    field(
        out,
        form,
        Field::Reviewer,
        value,
        "the model behind rival command security",
        w,
        s,
    );
    field(
        out,
        form,
        Field::AutoFix,
        vec![
            toggle(form.draft.auto_fix_critical_high, s),
            Span::styled("   skills fix confirmed critical/high", s.dim),
        ],
        "medium and low findings are never fixed without asking",
        w,
        s,
    );
    out.push(BodyLine::plain(Line::default()));
    out.push(BodyLine::plain(Line::from(vec![
        Span::styled("ROLE PROMPTS", s.section),
        Span::styled("   read-only", s.dim),
    ])));
    if form.draft.roles.is_empty() {
        out.push(BodyLine::plain(Line::from(Span::styled(
            "  none set — every role uses the built-in prompt",
            s.dim,
        ))));
    }
    for (role, prompt) in &form.draft.roles {
        let first = prompt.lines().next().unwrap_or("").trim();
        out.push(BodyLine::plain(fit_line(
            Line::from(vec![
                Span::styled(format!("  {role:<12}"), s.text),
                Span::styled(format!("{} chars  ", prompt.chars().count()), s.dim),
                Span::styled(first.to_string(), s.dim),
            ]),
            w,
        )));
    }
    out.push(BodyLine::plain(fit_line(
        Line::from(vec![
            Span::styled("  edit roles: in ", s.dim),
            Span::styled(form.seed.path_shown.clone(), s.text),
        ]),
        w,
    )));
}

fn check_section_lines(form: &ConfigForm, w: usize, v: &ViewCtx, out: &mut Vec<BodyLine>) {
    let s = v.styles;
    for text in [
        "Sends \"Reply with exactly: ok\" to each model at low effort, through",
        "the route the draft picks. Nothing is saved first.",
    ] {
        out.push(BodyLine::plain(fit_line(
            Line::from(Span::styled(text, s.dim)),
            w,
        )));
    }
    out.push(BodyLine::plain(Line::default()));
    let mut head = vec![Span::styled("proxy  ", s.dim)];
    head.extend(proxy_status(form, s));
    if let Some(ProxyLine::Down { error, .. }) = &form.check.proxy {
        head.push(Span::styled(format!("  {error}"), s.failed));
    }
    out.push(BodyLine::plain(fit_line(Line::from(head), w)));
    out.push(BodyLine::plain(Line::default()));
    if form.check.slots.is_empty() {
        out.push(BodyLine::plain(Line::from(Span::styled(
            "  not run yet",
            s.dim,
        ))));
    } else {
        for line in check_rows(form, w, usize::MAX, true, v) {
            out.push(BodyLine::plain(line));
        }
        out.push(BodyLine::plain(Line::default()));
        out.push(BodyLine::plain(fit_line(check_status(form, s), w)));
    }
    out.push(BodyLine::plain(Line::default()));
    out.push(BodyLine::plain(fit_line(
        Line::from(vec![
            Span::styled("  c", s.accent),
            Span::styled(
                " codex, sol, claude, fable and the security reviewer",
                s.dim,
            ),
        ]),
        w,
    )));
    out.push(BodyLine::plain(fit_line(
        Line::from(vec![
            Span::styled("  a", s.accent),
            Span::styled(" every model, K3 and Grok too", s.dim),
        ]),
        w,
    )));
    if form.check.running {
        out.push(BodyLine::plain(Line::from(vec![
            Span::styled("  x", s.accent),
            Span::styled(" cancels the running check", s.dim),
        ])));
    }
}

/// `last run 14:02 · 4 of 5 ok`, or the progress of a running check.
fn check_status(form: &ConfigForm, s: &Styles) -> Line<'static> {
    let c = &form.check;
    if c.running {
        let what = if c.cancelling {
            "cancelling"
        } else {
            "running"
        };
        return Line::from(Span::styled(
            format!("{what} · {} of {} done", c.done(), c.slots.len()),
            s.running,
        ));
    }
    let Some(at) = c.finished else {
        return Line::from(Span::styled("not run yet", s.dim));
    };
    let total = c.slots.len();
    let ok = c.passed();
    Line::from(vec![
        Span::styled(format!("last run {} · ", clock(at)), s.dim),
        Span::styled(
            format!("{ok} of {total} ok"),
            if ok == total { s.accent } else { s.failed },
        ),
    ])
}

fn clock(at: DateTime<FixedOffset>) -> String {
    at.format("%H:%M").to_string()
}

/// The full check panel: a header, then the rows, at most `cap` lines.
fn check_panel(form: &ConfigForm, w: usize, cap: usize, v: &ViewCtx) -> Vec<Line<'static>> {
    let s = v.styles;
    let mut left = vec![Span::styled("CHECK", s.section), Span::raw("   ")];
    left.extend(proxy_status(form, s));
    let right = if form.check.slots.is_empty() {
        Vec::new()
    } else {
        check_status(form, s).spans
    };
    let mut lines = vec![spread(left, right, w)];
    if form.check.slots.is_empty() {
        lines.push(fit_line(
            Line::from(vec![
                Span::styled("not run yet · ", s.dim),
                Span::styled("c", s.accent),
                Span::styled(" checks the default models on the draft, ", s.dim),
                Span::styled("a", s.accent),
                Span::styled(" every model", s.dim),
            ]),
            w,
        ));
        return lines;
    }
    let rows = check_rows(form, w, 2, false, v);
    let room = cap.saturating_sub(1);
    if rows.len() > room {
        lines.extend(rows.into_iter().take(room.saturating_sub(1)));
        lines.push(Line::from(Span::styled(
            "… more on the Check section (4)",
            s.dim,
        )));
    } else {
        lines.extend(rows);
    }
    lines
}

/// `left`, then `right` flush with the right edge, cut to `w`.
fn spread(left: Vec<Span<'static>>, right: Vec<Span<'static>>, w: usize) -> Line<'static> {
    let lw: usize = left.iter().map(|sp| width(&sp.content)).sum();
    let rw: usize = right.iter().map(|sp| width(&sp.content)).sum();
    let mut spans = left;
    if lw + 2 + rw <= w {
        spans.push(Span::raw(" ".repeat(w - lw - rw)));
        spans.extend(right);
    }
    fit_line(Line::from(spans), w)
}

/// The one-line check summary of short frames.
fn check_summary(form: &ConfigForm, w: usize, v: &ViewCtx) -> Line<'static> {
    let s = v.styles;
    let c = &form.check;
    let mut left = vec![Span::styled("CHECK", s.section), Span::raw("  ")];
    if c.slots.is_empty() {
        left.push(Span::styled("not run · ", s.dim));
        left.push(Span::styled("c", s.accent));
        left.push(Span::styled(" checks the draft", s.dim));
    } else {
        let failed = c.done() - c.passed();
        let running = c.slots.len() - c.done();
        left.push(Span::styled(format!("✓ {}", c.passed()), s.accent));
        if failed > 0 {
            left.push(Span::styled(format!("  ✗ {failed}"), s.failed));
        }
        if running > 0 {
            left.push(Span::styled(format!("  {} {running}", v.spin), s.running));
        }
        left.push(Span::styled("  · ", s.dim));
        left.extend(check_status(form, s).spans);
    }
    spread(left, proxy_status(form, s), w)
}

/// Column widths of the check rows.
struct Cols {
    name: usize,
    wire: usize,
    msg_x: usize,
}

fn cols(slots: &[CheckSlot], w: usize) -> Cols {
    let mut name = slots
        .iter()
        .map(|s| width(&s.name))
        .max()
        .unwrap_or(0)
        .max(6);
    let mut wire = slots
        .iter()
        .map(|s| width(&s.wire))
        .max()
        .unwrap_or(0)
        .max(8);
    // mark, gaps, route and time: "✓ " + name + "  " + route(6) + "  " +
    // wire + "  " + time(5) + "  ".
    let fixed = |name: usize, wire: usize| 2 + name + 2 + 6 + 2 + wire + 2 + 5 + 2;
    while fixed(name, wire) + 18 > w && wire > 14 {
        wire -= 1;
    }
    while fixed(name, wire) + 18 > w && name > 6 {
        name -= 1;
    }
    Cols {
        name,
        wire,
        msg_x: fixed(name, wire),
    }
}

/// The check rows: mark, name, route, wire id, latency (right-aligned) and
/// the reply or the error, wrapped under the message column (at most
/// `max_wrap` lines). `hints` adds the hint under a failed row.
fn check_rows(
    form: &ConfigForm,
    w: usize,
    max_wrap: usize,
    hints: bool,
    v: &ViewCtx,
) -> Vec<Line<'static>> {
    let s = v.styles;
    let c = cols(&form.check.slots, w);
    // Too little room beside the row: the message goes under it.
    let below = w.saturating_sub(c.msg_x) < 32;
    let (msg_x, msg_w) = if below {
        (4, w.saturating_sub(4).max(8))
    } else {
        (c.msg_x, w.saturating_sub(c.msg_x).max(8))
    };
    let mut out = Vec::new();
    for slot in &form.check.slots {
        let (mark, mark_style) = match &slot.row {
            None => (v.spin.to_string(), s.running),
            Some(r) if r.ok => ("✓".to_string(), s.accent),
            Some(_) => ("✗".to_string(), s.failed),
        };
        let time = match &slot.row {
            None => "…".to_string(),
            Some(r) if r.called => format!("{:.1}s", r.ms as f64 / 1000.0),
            Some(_) => "—".to_string(),
        };
        let route_style = if slot.route == "proxy" {
            s.accent
        } else {
            s.dim
        };
        let mut spans = vec![
            Span::styled(format!("{mark} "), mark_style),
            Span::styled(cell(&slot.name, c.name), s.text),
            Span::raw("  "),
            Span::styled(format!("{:<6}", slot.route), route_style),
            Span::raw("  "),
            Span::styled(cell(&slot.wire, c.wire), s.value),
            Span::raw("  "),
            Span::styled(format!("{time:>5}"), s.dim),
            Span::raw("  "),
        ];
        let (label, msg, msg_style) = message(slot.row.as_ref(), form.check.cancelling, s);
        let short = slot.row.as_ref().is_none_or(|r| r.ok && !r.unexpected);
        let mut room = msg_w;
        if below && !short {
            // A failure or an odd reply gets its own lines.
            out.push(fit_line(Line::from(spans), w));
            spans = vec![Span::raw(" ".repeat(msg_x))];
        }
        if let Some(label) = label {
            room = room.saturating_sub(width(&label.content) + 1);
            spans.push(label);
            spans.push(Span::raw(" "));
        }
        let wrapped = wrap_cells(&msg, room.max(8));
        let mut parts = wrapped.into_iter();
        spans.push(Span::styled(parts.next().unwrap_or_default(), msg_style));
        out.push(fit_line(Line::from(spans), w));
        for part in parts.take(max_wrap.saturating_sub(1)) {
            out.push(fit_line(
                Line::from(vec![
                    Span::raw(" ".repeat(msg_x)),
                    Span::styled(part, msg_style),
                ]),
                w,
            ));
        }
        if hints
            && let Some(r) = &slot.row
            && !r.ok
            && !r.hint.is_empty()
        {
            for part in wrap_cells(&r.hint, msg_w) {
                out.push(fit_line(
                    Line::from(vec![
                        Span::raw(" ".repeat(msg_x)),
                        Span::styled(part, s.dim),
                    ]),
                    w,
                ));
            }
        }
    }
    out
}

/// `s` cut or padded to exactly `w` cells.
fn cell(s: &str, w: usize) -> String {
    let t = truncate(s, w, "…");
    let pad = w.saturating_sub(width(&t));
    format!("{t}{}", " ".repeat(pad))
}

/// A row's message: an optional label span (`limit`), the text and its
/// style.
fn message(
    row: Option<&CheckRow>,
    cancelling: bool,
    s: &Styles,
) -> (Option<Span<'static>>, String, Style) {
    match row {
        None if cancelling => (None, "cancelling".to_string(), s.dim),
        None => (None, "checking".to_string(), s.dim),
        Some(r) if r.ok && r.unexpected => (None, format!("{}  (not ok)", r.reply), s.warn),
        Some(r) if r.ok => (None, r.reply.clone(), s.text),
        Some(r) if r.limit => (
            Some(Span::styled("limit", s.failed.add_modifier(Modifier::BOLD))),
            r.error.clone(),
            s.failed,
        ),
        Some(r) => (None, r.error.clone(), s.failed),
    }
}

/// The key bar: the confirm bar, a notice, or the keys for the focus.
fn footer_line(form: &ConfigForm, w: usize, s: &Styles) -> Line<'static> {
    if form.confirm_exit {
        return fit_line(
            Line::from(vec![
                Span::styled("save changes?", s.running),
                Span::raw("   "),
                Span::styled("y", s.accent),
                Span::styled(" save and close  ", s.dim),
                Span::styled("n", s.accent),
                Span::styled(" discard  ", s.dim),
                Span::styled("esc", s.accent),
                Span::styled(" stay", s.dim),
            ]),
            w,
        );
    }
    if let Some(n) = &form.notice {
        let (mark, style) = match n.kind {
            NoticeKind::Ok => ("✓ ", s.accent),
            NoticeKind::Error => ("✗ ", s.failed),
            NoticeKind::Info => ("· ", s.text),
        };
        return fit_line(
            Line::from(vec![Span::styled(format!("{mark}{}", n.text), style)]),
            w,
        );
    }
    // (key, what, style, rank): the lowest rank goes first when the bar is
    // too narrow; the order stays.
    let mut keys: Vec<(&str, String, Style, u8)> = Vec::new();
    let mut add = |k: &'static str, d: &str, rank: u8| keys.push((k, d.to_string(), s.dim, rank));
    if form.help {
        add("any key", "close help", 9);
    } else if let Some(edit) = &form.edit {
        add("enter", "save field", 9);
        add("esc", "cancel", 9);
        if edit.field == Field::Key {
            add("paste", "works; the key is never shown", 1);
        }
    } else {
        add("tab", "section", 6);
        let field = form.field();
        add("↑↓", if field.is_some() { "field" } else { "scroll" }, 2);
        match field {
            Some(f) if f.is_toggle() => add("space", "toggle", 7),
            Some(Field::Model(i)) => {
                add("←→", "effort", 7);
                if MODELS.get(i).is_some_and(|m| m.plan) {
                    add("space", "plan", 7);
                }
            }
            Some(Field::ClaudePrefix | Field::CodexPrefix) => {
                add("←→", "choose", 7);
                add("e", "type", 3);
            }
            Some(f) if f.is_picker() => add("←→", "choose", 7),
            Some(f) if f.is_text() => add("enter", "edit", 7),
            _ => {}
        }
        if field == Some(Field::Url) && !form.seed.base_url_env.is_empty() {
            add("d", "import", 3);
        }
        if form.check.running {
            add("x", "cancel check", 8);
        } else {
            add("c", "check", 5);
            add("a", "all", 1);
        }
        let save_style = if form.blocked().is_some() {
            s.failed
        } else {
            s.dim
        };
        let save = if form.blocked().is_some() {
            "save blocked"
        } else {
            "save"
        };
        keys.push(("s", save.to_string(), save_style, 9));
        let mut add =
            |k: &'static str, d: &str, rank: u8| keys.push((k, d.to_string(), s.dim, rank));
        if form.dirty() {
            add("u", "undo", 4);
        }
        add("?", "help", 8);
        add("esc", "back", 9);
    }
    let cost = |keys: &[(&str, String, Style, u8)]| -> usize {
        keys.iter()
            .map(|(k, d, _, _)| width(k) + 1 + width(d))
            .sum::<usize>()
            + 2 * keys.len().saturating_sub(1)
    };
    while cost(&keys) > w {
        let Some(i) = (0..keys.len()).min_by_key(|&i| (keys[i].3, std::cmp::Reverse(i))) else {
            break;
        };
        keys.remove(i);
    }
    let mut spans = Vec::new();
    for (i, (k, d, style, _)) in keys.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(k, s.accent));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(d, style));
    }
    fit_line(Line::from(spans), w)
}

/// The keys of the window, over the body.
fn help_overlay(area: Rect, buf: &mut Buffer, s: &Styles) {
    const KEYS: [(&str, &str); 16] = [
        ("tab ⇧tab", "next / previous section"),
        ("1-4", "jump to a section"),
        ("↑↓ j k", "move between fields"),
        ("enter", "edit a field, or toggle"),
        ("space", "toggle; plan default on a model"),
        ("←→ h l", "choose: prefix, effort, reviewer"),
        ("e", "type a prefix the proxy does not list"),
        ("d", "import ANTHROPIC_BASE_URL into URL"),
        ("c", "check the default models on the draft"),
        ("a", "check every model"),
        ("x", "cancel a running check"),
        ("s", "save the draft (and a typed key)"),
        ("u", "drop the draft"),
        ("esc q", "back; asks first when unsaved"),
        ("?", "this help"),
        ("ctrl+c", "quit rival"),
    ];
    let h = (KEYS.len() + 4).min(usize::from(area.height));
    let w = 52.min(usize::from(area.width).saturating_sub(4));
    if h < 5 || w < 30 {
        return;
    }
    let rect = Rect::new(
        area.x + (area.width - w as u16) / 2,
        area.y + (area.height - h as u16) / 2,
        w as u16,
        h as u16,
    );
    Clear.render(rect, buf);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(s.focus_border)
        .title(Line::from(Span::styled(" keys ", s.heading)));
    let inner = block.inner(rect);
    block.render(rect, buf);
    for (i, (k, d)) in KEYS.iter().enumerate().take(usize::from(inner.height) - 1) {
        let line = fit_line(
            Line::from(vec![
                Span::styled(format!(" {k:<10}"), s.accent),
                Span::styled(d.to_string(), s.text),
            ]),
            usize::from(inner.width),
        );
        buf.set_line(inner.x, inner.y + 1 + i as u16, &line, inner.width);
    }
}
