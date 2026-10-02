//! Test helpers: key presses, pinned-clock models and TestBackend frames.

use std::sync::Arc;

use chrono::{DateTime, FixedOffset, TimeDelta};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use unicode_width::UnicodeWidthStr;

use rival_core::session::Session;
use rival_core::sessionview::SessionEvent;

use super::model::{Model, Msg};

/// The pinned "now" every test model uses.
pub fn fixed_now() -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339("2026-10-03T12:00:00+02:00").unwrap()
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

/// A model with a pinned clock that has seen one resize but no snapshot.
pub fn loading_model(width: u16, height: u16) -> Model {
    let mut m = Model::new("3.34.0");
    m.clock = fixed_now;
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

/// The frame of `m` at its own layout size, as one string.
pub fn frame_text(m: &Model) -> String {
    let w = u16::try_from(m.lay.width).unwrap();
    let h = u16::try_from(m.lay.height).unwrap();
    rows(&draw(m, w, h)).join("\n")
}
