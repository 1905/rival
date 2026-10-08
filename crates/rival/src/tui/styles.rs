//! The dim-phosphor theme, the gradient logo and the header.
//!
//! The colour values match Rival.app's
//! `Theme.swift`. Colour marks state only: running amber, failed red;
//! completed stays quiet.

use std::sync::OnceLock;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::text::{fit_line, width};

/// Body text.
pub const FG: u32 = 0xB4C2B7;
/// Secondary text, borders, help.
pub const DIM: u32 = 0x4F6154;
/// Active marks: tab, focus border, cursor.
pub const ACCENT: u32 = 0x6FC985;
/// Amber: spinner, running status.
pub const RUNNING: u32 = 0xD6A34E;
/// Grey-green: waiting in line.
pub const QUEUED: u32 = 0x6A7F70;
/// Completed: quiet, the normal case.
pub const DONE: u32 = 0x7F9483;
/// Failed.
pub const FAIL: u32 = 0xDB6B6B;
/// Cursor bar fill.
pub const SELECTION_BG: u32 = 0x16241A;
/// Text on the cursor bar.
pub const SELECTION_FG: u32 = 0xE2EDE4;
/// Gradient stops for the logo, the loader bar and the "rival" word:
/// violet → cyan → green.
pub const LOGO_STOPS: [u32; 3] = [0x8B6CD9, 0x4FB8CC, 0x6FC985];

/// A truecolor ratatui colour from `0xRRGGBB`.
pub const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// Every style the TUI draws with. Later views (the list, the detail screen,
/// `markdown::render`) take a `&Styles` instead of hard-coding colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Styles {
    pub text: Style,
    pub dim: Style,
    /// Accent foreground, no weight: focus marks, inline code.
    pub accent: Style,
    /// Bold accent: markdown headings.
    pub heading: Style,
    /// The cursor bar: a dark green tint with light bold text, not a neon
    /// fill. Bold keeps it legible on terminals without truecolor.
    pub selected: Style,
    /// Unfocused box borders.
    pub border: Style,
    /// The focused box border.
    pub focus_border: Style,
    /// Column titles in the list.
    pub column_title: Style,
    /// Day section rows in the list.
    pub section: Style,
    pub active_tab: Style,
    pub inactive_tab: Style,
    pub running: Style,
    pub queued: Style,
    pub completed: Style,
    pub failed: Style,
    /// Notes inside log text.
    pub label: Style,
    /// An emphasised value: model id, session id.
    pub value: Style,
    /// Fenced code blocks in markdown.
    pub code_block: Style,
    /// Inline code in markdown.
    pub inline_code: Style,
    /// A detail search hit: black on the accent. Colours only, so the line
    /// keeps its width.
    pub matched: Style,
}

impl Styles {
    /// The dim-phosphor theme.
    pub const fn phosphor() -> Styles {
        let text = Style::new().fg(rgb(FG));
        let dim = Style::new().fg(rgb(DIM));
        let accent = Style::new().fg(rgb(ACCENT));
        Styles {
            text,
            dim,
            accent,
            heading: accent.add_modifier(Modifier::BOLD),
            selected: Style::new()
                .fg(rgb(SELECTION_FG))
                .bg(rgb(SELECTION_BG))
                .add_modifier(Modifier::BOLD),
            border: dim,
            focus_border: accent,
            column_title: dim.add_modifier(Modifier::BOLD),
            section: dim.add_modifier(Modifier::BOLD),
            active_tab: accent
                .add_modifier(Modifier::BOLD)
                .add_modifier(Modifier::UNDERLINED),
            inactive_tab: dim,
            running: Style::new().fg(rgb(RUNNING)).add_modifier(Modifier::BOLD),
            queued: Style::new().fg(rgb(QUEUED)),
            completed: Style::new().fg(rgb(DONE)),
            failed: Style::new().fg(rgb(FAIL)),
            label: dim,
            value: text.add_modifier(Modifier::BOLD),
            code_block: dim,
            inline_code: accent,
            matched: Style::new().fg(rgb(0x000000)).bg(rgb(ACCENT)),
        }
    }

    /// The style of a run status. An unknown status uses body text.
    pub fn status(&self, status: &str) -> Style {
        match status {
            "running" => self.running,
            "completed" => self.completed,
            "failed" => self.failed,
            "queued" => self.queued,
            _ => self.text,
        }
    }
}

/// The theme the TUI draws with.
pub const STYLES: Styles = Styles::phosphor();

/// Swift: `blendHex`. A linear sRGB blend across evenly spaced `stops` at `t`
/// in 0..=1 (clamped).
pub fn blend_hex(stops: &[u32], t: f64) -> u32 {
    let Some(&first) = stops.first() else {
        return 0;
    };
    if stops.len() < 2 {
        return first;
    }
    let pos = t.clamp(0.0, 1.0) * (stops.len() - 1) as f64;
    let i = (pos as usize).min(stops.len() - 2);
    let f = pos - i as f64;
    let mix = |shift: u32| {
        let ch = |c: u32| f64::from((c >> shift) & 0xFF);
        ((ch(stops[i]) * (1.0 - f) + ch(stops[i + 1]) * f).round() as u32) << shift
    };
    mix(16) | mix(8) | mix(0)
}

/// Swift: `Theme.logoColor`. The 45° logo gradient colour of cell `x`, `y`
/// in a `w`×`h` block.
pub fn logo_color(x: usize, y: usize, w: usize, h: usize) -> Color {
    let fx = if w > 1 {
        x as f64 / (w - 1) as f64
    } else {
        0.0
    };
    let fy = if h > 1 {
        y as f64 / (h - 1) as f64
    } else {
        0.0
    };
    rgb(blend_hex(&LOGO_STOPS, (fx + fy) / 2.0))
}

/// The colour of cell `i` of `n` along the logo gradient.
fn ramp_color(i: usize, n: usize) -> Color {
    let t = if n > 1 {
        i as f64 / (n - 1) as f64
    } else {
        0.0
    };
    rgb(blend_hex(&LOGO_STOPS, t))
}

/// The ASCII logo for the wide header.
pub const BANNER_LINES: [&str; 5] = [
    r"         _             __",
    r"   _____(_)   ______ _/ /",
    r"  / ___/ / | / / __ `/ /",
    r" / /  / /| |/ / /_/ / /",
    r"/_/  /_/ |___/\__,_/_/",
];

/// The widest banner line in cells.
pub fn banner_width() -> usize {
    BANNER_LINES.iter().map(|l| width(l)).max().unwrap_or(0)
}

/// The logo with its 45° gradient, one colour per cell, every line padded to
/// [`banner_width`] so the stats beside it line up. Built once: the logo
/// never changes.
pub fn logo_lines() -> &'static [Line<'static>] {
    static LOGO: OnceLock<Vec<Line<'static>>> = OnceLock::new();
    LOGO.get_or_init(|| {
        let w = banner_width();
        let h = BANNER_LINES.len();
        BANNER_LINES
            .iter()
            .enumerate()
            .map(|(y, line)| {
                let chars: Vec<char> = line.chars().collect();
                let spans: Vec<Span<'static>> = (0..w)
                    .map(|x| match chars.get(x) {
                        Some(&c) if c != ' ' => Span::styled(
                            c.to_string(),
                            Style::new()
                                .fg(logo_color(x, y, w, h))
                                .add_modifier(Modifier::BOLD),
                        ),
                        _ => Span::raw(" "),
                    })
                    .collect();
                Line::from(spans)
            })
            .collect()
    })
}

/// Each char of `word` along the logo gradient, bold.
pub fn gradient_word(word: &str) -> Vec<Span<'static>> {
    let chars: Vec<char> = word.chars().collect();
    chars
        .iter()
        .enumerate()
        .map(|(i, c)| {
            Span::styled(
                c.to_string(),
                Style::new()
                    .fg(ramp_color(i, chars.len()))
                    .add_modifier(Modifier::BOLD),
            )
        })
        .collect()
}

/// A `w`-cell progress bar: the filled part in the logo gradient across the
/// whole bar, the rest dim.
pub fn gradient_bar(w: usize, pct: f64, styles: &Styles) -> Line<'static> {
    if w == 0 {
        return Line::default();
    }
    let filled = ((pct * w as f64 + 0.5) as usize).min(w);
    let mut spans: Vec<Span<'static>> = (0..filled)
        .map(|i| Span::styled("█", Style::new().fg(ramp_color(i, w))))
        .collect();
    if filled < w {
        spans.push(Span::styled("░".repeat(w - filled), styles.dim));
    }
    Line::from(spans)
}

/// The session counts shown in the header.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeaderStats {
    pub running: usize,
    pub queued: usize,
    pub completed: usize,
    pub failed: usize,
    pub total: usize,
    pub version: String,
    /// Shows "…" for every count until the first snapshot, never a
    /// misleading 0.
    pub loading: bool,
}

/// The terminal height under which the 5-row logo collapses to a one-line
/// header, so short terminals keep their rows for runs.
pub const COMPACT_HEADER_BELOW_HEIGHT: usize = 30;

/// The header. Wide form: the gradient logo with the stats block
/// right-aligned beside it. Compact form: one line with "rival" in the
/// gradient and the same stats. No line is wider than `w`.
pub fn render_header(
    w: usize,
    compact: bool,
    st: &HeaderStats,
    spin: &str,
    styles: &Styles,
) -> Vec<Line<'static>> {
    if w == 0 {
        return Vec::new();
    }
    let spin = if spin.is_empty() { "●" } else { spin };
    let n = |v: usize| {
        if st.loading {
            "…".to_string()
        } else {
            v.to_string()
        }
    };
    let running = Span::styled(format!("{spin} {} running", n(st.running)), styles.running);
    let queued = Span::styled(format!("◌ {} queued", n(st.queued)), styles.queued);
    let done = Span::styled(format!("✓ {}", n(st.completed)), styles.completed);
    let failed = Span::styled(format!("✗ {}", n(st.failed)), styles.failed);
    let total = Span::styled(format!("{} sessions", n(st.total)), styles.dim);
    let version = Span::styled(st.version.clone(), styles.dim);
    let gap = || Span::raw("  ");

    if compact {
        let mut spans = gradient_word("rival");
        for s in [running, queued, done, failed, total, version] {
            spans.push(gap());
            spans.push(s);
        }
        return vec![fit_line(Line::from(spans), w)];
    }

    let stats: [Vec<Span<'static>>; 5] = [
        Vec::new(),
        vec![running, gap(), queued],
        vec![done, gap(), failed],
        vec![total, gap(), version],
        Vec::new(),
    ];
    let stats_w = stats
        .iter()
        .map(|s| s.iter().map(|span| width(&span.content)).sum::<usize>())
        .max()
        .unwrap_or(0);
    // Right-align the stats block as a whole, so its lines share a left edge.
    let pad = w.saturating_sub(banner_width() + stats_w).max(2);
    logo_lines()
        .iter()
        .zip(stats)
        .map(|(logo, stat)| {
            let mut line = logo.clone();
            if !stat.is_empty() {
                line.spans.push(Span::raw(" ".repeat(pad)));
                line.spans.extend(stat);
            }
            fit_line(line, w)
        })
        .collect()
}

#[cfg(test)]
mod tests;
