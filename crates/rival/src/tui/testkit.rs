//! Test helpers: key presses, pinned-clock models and TestBackend frames.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, FixedOffset, NaiveDateTime, TimeDelta};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::{Buffer, Cell};
use ratatui::style::{Color, Modifier, Style};

use super::styles::STYLES;
use unicode_width::UnicodeWidthStr;

use rival_core::session::Session;
use rival_core::sessionview::SessionEvent;

use super::model::{DisplayItem, Model, Msg};

/// The pinned "now" every test model uses.
pub fn fixed_now() -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339("2026-10-03T12:00:00+02:00").unwrap()
}

/// The pinned zone every test model uses: [`fixed_now`]'s offset, with no
/// DST switch, so frames do not depend on the host zone.
pub fn fixed_zone(_utc: NaiveDateTime) -> FixedOffset {
    *fixed_now().offset()
}

/// Go test helper `press`: a key press by bubbletea name.
pub fn press(name: &str) -> KeyEvent {
    let none = KeyModifiers::NONE;
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        let mods = if c.is_uppercase() {
            KeyModifiers::SHIFT
        } else {
            none
        };
        return KeyEvent::new(KeyCode::Char(c), mods);
    }
    let (code, mods) = match name {
        "enter" => (KeyCode::Enter, none),
        "esc" => (KeyCode::Esc, none),
        "tab" => (KeyCode::Tab, none),
        "shift+tab" => (KeyCode::BackTab, KeyModifiers::SHIFT),
        "ctrl+c" => (KeyCode::Char('c'), KeyModifiers::CONTROL),
        "backspace" => (KeyCode::Backspace, none),
        "space" => (KeyCode::Char(' '), none),
        "up" => (KeyCode::Up, none),
        "down" => (KeyCode::Down, none),
        "home" => (KeyCode::Home, none),
        "end" => (KeyCode::End, none),
        "pgup" => (KeyCode::PageUp, none),
        "pgdown" => (KeyCode::PageDown, none),
        _ => panic!("unhandled key {name}"),
    };
    KeyEvent::new(code, mods)
}

pub fn key(name: &str) -> Msg {
    Msg::Key(press(name))
}

/// Feeds `msgs` to `m` in order.
pub fn send(m: &mut Model, msgs: impl IntoIterator<Item = Msg>) {
    for msg in msgs {
        m.update(msg);
    }
}

/// One key message per char of `s`.
pub fn type_text(s: &str) -> Vec<Msg> {
    s.chars().map(|c| key(&c.to_string())).collect()
}

/// A model with a pinned clock and zone that has seen one resize but no
/// snapshot.
pub fn loading_model(width: u16, height: u16) -> Model {
    let mut m = Model::new("3.34.0");
    m.clock = fixed_now;
    m.list.zone = fixed_zone;
    m.update(Msg::Resize { width, height });
    m
}

/// A loaded model showing `sessions`.
pub fn list_model(sessions: Vec<Arc<Session>>, width: u16, height: u16) -> Model {
    let mut m = loading_model(width, height);
    m.update(Msg::Sessions(SessionEvent { sessions }));
    m
}

/// Go `listFixture`: one running, one completed today, two old runs.
pub fn list_fixture() -> Vec<Arc<Session>> {
    let now = fixed_now();
    let s = |id: &str,
             cli: &str,
             model: &str,
             mode: &str,
             effort: &str,
             status: &str,
             start,
             workdir: &str| {
        Arc::new(Session {
            id: id.into(),
            cli: cli.into(),
            model: model.into(),
            mode: mode.into(),
            effort: effort.into(),
            status: status.into(),
            start_time: start,
            work_dir: workdir.into(),
            ..Session::default()
        })
    };
    vec![
        s(
            "a0000000-run",
            "codex",
            "gpt-6-astra",
            "review",
            "xhigh",
            "running",
            now - TimeDelta::minutes(1),
            "/src/orbit-web",
        ),
        s(
            "b0000000-done",
            "claude",
            "claude-opus-5-5",
            "plan",
            "medium",
            "completed",
            now - TimeDelta::minutes(2),
            "/src/ledger",
        ),
        s(
            "c0000000-fail",
            "codex",
            "gpt-5.5",
            "review",
            "high",
            "failed",
            now - TimeDelta::days(40),
            "/src/orbit-web",
        ),
        s(
            "d0000000-old",
            "codex",
            "gpt-5.5",
            "review",
            "high",
            "completed",
            now - TimeDelta::days(41),
            "/src/mathquest",
        ),
    ]
}

/// A solo session with the fields the list shows.
#[allow(clippy::too_many_arguments)]
pub fn run(
    id: &str,
    cli: &str,
    model: &str,
    mode: &str,
    effort: &str,
    status: &str,
    start: DateTime<FixedOffset>,
    workdir: &str,
) -> Session {
    Session {
        id: id.into(),
        cli: cli.into(),
        model: model.into(),
        mode: mode.into(),
        effort: effort.into(),
        status: status.into(),
        start_time: start,
        work_dir: workdir.into(),
        ..Session::default()
    }
}

/// Go `filterFixture`: one run per item. Two TODAY, one YESTERDAY, one
/// OLDER; ids start a, b, c, d.
pub fn filter_fixture(now: DateTime<FixedOffset>) -> Vec<DisplayItem> {
    let solo = |s: Session| DisplayItem {
        sessions: vec![Arc::new(s)],
    };
    vec![
        solo(Session {
            prompt_preview: "review the fingerprint re-key".into(),
            ..run(
                "aaaaaaaa-1",
                "codex",
                "gpt-6-astra",
                "review",
                "xhigh",
                "running",
                now - TimeDelta::minutes(1),
                "/src/orbit-web",
            )
        }),
        solo(Session {
            review_scope: "plans/2026-09-26-service-identity".into(),
            ..run(
                "bbbbbbbb-2",
                "claude",
                "claude-opus-5-5",
                "plan",
                "medium",
                "completed",
                now - TimeDelta::hours(2),
                "/src/ledger",
            )
        }),
        solo(run(
            "cccccccc-3",
            "codex",
            "gpt-5.5",
            "review",
            "high",
            "failed",
            now - TimeDelta::days(1),
            "/src/orbit-web",
        )),
        solo(run(
            "dddddddd-4",
            "codex",
            "gpt-5.5",
            "review",
            "high",
            "completed",
            now - TimeDelta::days(40),
            "/src/mathquest",
        )),
    ]
}

/// Go `manyRuns`: `n` runs, newest first, ids r000, r001, ...: the first 30
/// today, the next 40 yesterday, the rest older. Every third run failed.
pub fn many_runs(n: usize, now: DateTime<FixedOffset>) -> Vec<Arc<Session>> {
    (0..n)
        .map(|i| {
            let start = match i {
                0..30 => now - TimeDelta::milliseconds(i as i64),
                30..70 => now - TimeDelta::days(1),
                _ => now - TimeDelta::days(40),
            };
            let status = if i % 3 == 0 { "failed" } else { "completed" };
            Arc::new(Session {
                duration: "1m".into(),
                ..run(
                    &format!("r{i:03}"),
                    "codex",
                    "gpt-5.5",
                    "review",
                    "high",
                    status,
                    start,
                    "/src/proj",
                )
            })
        })
        .collect()
}

/// Go `previewFixture` without the log files the list never reads: one
/// running, one completed, one failed run, all today.
pub fn preview_fixture() -> Vec<Arc<Session>> {
    let now = fixed_now();
    vec![
        Arc::new(Session {
            pid: 101,
            ..run(
                "a0000000-live",
                "codex",
                "gpt-6-astra",
                "review",
                "xhigh",
                "running",
                now - TimeDelta::minutes(1),
                "/src/orbit-web",
            )
        }),
        Arc::new(Session {
            duration: "2m36s".into(),
            ..run(
                "b0000000-done",
                "claude",
                "claude-opus-5-5",
                "plan",
                "medium",
                "completed",
                now - TimeDelta::minutes(2),
                "/src/ledger",
            )
        }),
        Arc::new(Session {
            duration: "23s".into(),
            ..run(
                "c0000000-fail",
                "codex",
                "gpt-5.5",
                "review",
                "high",
                "failed",
                now - TimeDelta::minutes(3),
                "/src/mathquest",
            )
        }),
    ]
}

/// Draws `m` on a `width`×`height` TestBackend and returns the buffer.
pub fn draw(m: &Model, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| m.draw(f)).unwrap();
    terminal.backend().buffer().clone()
}

/// The rows of `buf` as plain text, wide glyphs counted once.
pub fn rows(buf: &Buffer) -> Vec<String> {
    let area = buf.area;
    (area.top()..area.bottom())
        .map(|y| {
            let mut row = String::new();
            let mut x = area.left();
            while x < area.right() {
                let sym = buf[(x, y)].symbol();
                row.push_str(sym);
                x += u16::try_from(sym.width().max(1)).unwrap();
            }
            row
        })
        .collect()
}

/// One letter for the style of a cell, so a golden frame pins colour and
/// weight as well as text. Styles the theme does not name map to "g" (the
/// logo gradient) when they only set a foreground, else "?".
fn style_code(cell: &Cell) -> char {
    let s = &STYLES;
    let cursor = s.accent.add_modifier(Modifier::REVERSED);
    let named = [
        ('S', s.selected),
        ('A', s.active_tab),
        ('C', cursor),
        ('D', s.section),
        ('r', s.running),
        ('q', s.queued),
        ('c', s.completed),
        ('f', s.failed),
        ('T', s.value),
        ('t', s.text),
        ('d', s.dim),
        ('a', s.accent),
        ('.', Style::new()),
    ];
    let key = (cell.fg, cell.bg, cell.modifier);
    for (code, style) in named {
        let want = (
            style.fg.unwrap_or(Color::Reset),
            style.bg.unwrap_or(Color::Reset),
            style.add_modifier,
        );
        if key == want {
            return code;
        }
    }
    if cell.bg == Color::Reset && matches!(cell.fg, Color::Rgb(..)) {
        'g'
    } else {
        '?'
    }
}

/// The complete frame for a golden file: every row as text, then every
/// cell's style code, one character per cell. Each row ends in "|", so an
/// editor that strips trailing spaces cannot change the file unnoticed.
pub fn golden_frame(buf: &Buffer) -> String {
    let area = buf.area;
    let mut out = String::new();
    for row in rows(buf) {
        out.push_str(&row);
        out.push_str("|\n");
    }
    out.push_str("-- styles --\n");
    for y in area.top()..area.bottom() {
        out.extend((area.left()..area.right()).map(|x| style_code(&buf[(x, y)])));
        out.push_str("|\n");
    }
    out
}

/// Where golden frame `name` lives.
pub fn golden_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/tui/testdata")
        .join(format!("{name}.golden"))
}

/// Compares `got` with the committed golden frame `name`. Tests never
/// rewrite the file; the ignored `regenerate_list_golden_frames` test does.
pub fn assert_golden(name: &str, got: &str) {
    let path = golden_path(name);
    let want = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "read {}: {e}; regenerate with \
             cargo test -p rival regenerate_list_golden_frames -- --ignored",
            path.display()
        )
    });
    if got != want {
        panic!(
            "frame {name} differs from {}:\n--- got\n{got}\n--- want\n{want}",
            path.display()
        );
    }
}

/// The frame of `m` at its own layout size, as one string.
pub fn frame_text(m: &Model) -> String {
    let w = u16::try_from(m.lay.width).unwrap();
    let h = u16::try_from(m.lay.height).unwrap();
    rows(&draw(m, w, h)).join("\n")
}
