//! A one-line text input: the list filter and the detail search.
//! It handles the standard text-input keys the TUI needs.

use crossterm::event::KeyEvent;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};

use super::keys::key_name;
use super::styles::Styles;
use super::text::width;

/// The most chars the input holds.
const CHAR_LIMIT: usize = 200;
/// Text cells shown after the prompt.
const VIEW_WIDTH: usize = 24;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextInput {
    prompt: &'static str,
    placeholder: &'static str,
    value: Vec<char>,
    /// The cursor as a char index into `value`, 0..=len.
    cursor: usize,
    focused: bool,
}

impl TextInput {
    pub fn new(prompt: &'static str, placeholder: &'static str) -> TextInput {
        TextInput {
            prompt,
            placeholder,
            value: Vec::new(),
            cursor: 0,
            focused: false,
        }
    }

    pub fn value(&self) -> String {
        self.value.iter().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.value.is_empty()
    }

    /// Replaces the text and puts the cursor at its end.
    pub fn set_value(&mut self, value: &str) {
        self.value.clear();
        self.cursor = 0;
        self.insert(value);
    }

    /// Clears the text.
    pub fn reset(&mut self) {
        self.value.clear();
        self.cursor = 0;
    }

    pub fn focus(&mut self) {
        self.focused = true;
    }

    pub fn blur(&mut self) {
        self.focused = false;
    }

    pub fn focused(&self) -> bool {
        self.focused
    }

    pub fn cursor_end(&mut self) {
        self.cursor = self.value.len();
    }

    /// The cursor as a char index into the value.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The value's length in chars.
    pub fn len(&self) -> usize {
        self.value.len()
    }

    /// Inserts pasted or typed text at the cursor. Tabs and line breaks
    /// become spaces and other control chars are dropped, so a paste cannot
    /// break the line. Text past the char limit is dropped.
    pub fn insert(&mut self, text: &str) {
        for c in text.chars() {
            let c = match c {
                '\t' | '\n' | '\r' => ' ',
                c if c.is_control() => continue,
                c => c,
            };
            if self.value.len() >= CHAR_LIMIT {
                break;
            }
            self.value.insert(self.cursor, c);
            self.cursor += 1;
        }
    }

    /// Applies a key press while focused. Returns whether the key was an
    /// editing key; a blurred input takes nothing.
    pub fn handle_key(&mut self, ev: &KeyEvent) -> bool {
        if !self.focused {
            return false;
        }
        let Some(name) = key_name(ev) else {
            return false;
        };
        match name.as_str() {
            "left" | "ctrl+b" => self.cursor = self.cursor.saturating_sub(1),
            "right" | "ctrl+f" => self.cursor = (self.cursor + 1).min(self.value.len()),
            "home" | "ctrl+a" => self.cursor = 0,
            "end" | "ctrl+e" => self.cursor_end(),
            "backspace" | "ctrl+h" => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.value.remove(self.cursor);
                }
            }
            "delete" | "ctrl+d" => {
                if self.cursor < self.value.len() {
                    self.value.remove(self.cursor);
                }
            }
            "ctrl+u" => {
                self.value.drain(..self.cursor);
                self.cursor = 0;
            }
            "ctrl+k" => self.value.truncate(self.cursor),
            "ctrl+w" | "alt+backspace" => self.delete_word_backward(),
            "space" => self.insert(" "),
            _ => {
                let mut chars = name.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => self.insert(&c.to_string()),
                    _ => return false,
                }
            }
        }
        true
    }

    fn delete_word_backward(&mut self) {
        let mut start = self.cursor;
        while start > 0 && self.value[start - 1].is_whitespace() {
            start -= 1;
        }
        while start > 0 && !self.value[start - 1].is_whitespace() {
            start -= 1;
        }
        self.value.drain(start..self.cursor);
        self.cursor = start;
    }

    /// The prompt and the text, scrolled so the cursor stays inside the
    /// input's width. A focused input shows its cursor as a reversed cell; an
    /// empty one shows the placeholder.
    pub fn line(&self, styles: &Styles) -> Line<'static> {
        self.line_in(styles, VIEW_WIDTH)
    }

    /// [`TextInput::line`] with `view_width` text cells after the prompt.
    pub fn line_in(&self, styles: &Styles, view_width: usize) -> Line<'static> {
        let view_width = view_width.max(1);
        let prompt_style = if self.focused {
            styles.accent
        } else {
            styles.dim
        };
        let mut spans = vec![Span::styled(self.prompt, prompt_style)];
        let cursor_style = styles.accent.add_modifier(Modifier::REVERSED);
        if self.value.is_empty() {
            if self.focused {
                let mut ph = self.placeholder.chars();
                let first = ph.next().map(String::from).unwrap_or_else(|| " ".into());
                spans.push(Span::styled(first, cursor_style));
                spans.push(Span::styled(ph.collect::<String>(), styles.dim));
            } else {
                spans.push(Span::styled(self.placeholder, styles.dim));
            }
            return Line::from(spans);
        }
        // Scroll: keep the cursor cell (one past the text at the end) inside
        // VIEW_WIDTH cells.
        let mut start = 0;
        let cells = |from: usize, to: usize| -> usize {
            width(&self.value[from..to].iter().collect::<String>())
        };
        let cursor_w = usize::from(self.focused);
        while start < self.cursor && cells(start, self.cursor) + cursor_w > view_width {
            start += 1;
        }
        let mut end = start;
        while end < self.value.len() && cells(start, end + 1) + cursor_w <= view_width {
            end += 1;
        }
        let end = end.max(self.cursor.min(self.value.len()));
        if !self.focused {
            spans.push(Span::styled(
                self.value[start..end].iter().collect::<String>(),
                styles.text,
            ));
            return Line::from(spans);
        }
        let before: String = self.value[start..self.cursor].iter().collect();
        spans.push(Span::styled(before, styles.text));
        match self.value.get(self.cursor) {
            Some(c) if self.cursor < end => {
                spans.push(Span::styled(c.to_string(), cursor_style));
                let after: String = self.value[self.cursor + 1..end].iter().collect();
                spans.push(Span::styled(after, styles.text));
            }
            _ => spans.push(Span::styled(" ", cursor_style)),
        }
        Line::from(spans)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::styles::STYLES;
    use crossterm::event::{KeyCode, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn typed(input: &mut TextInput, s: &str) {
        for c in s.chars() {
            input.handle_key(&key(KeyCode::Char(c)));
        }
    }

    #[test]
    fn blurred_input_ignores_keys() {
        let mut input = TextInput::new("/ ", "filter");
        assert!(!input.handle_key(&key(KeyCode::Char('q'))));
        assert_eq!(input.value(), "");
    }

    #[test]
    fn editing_keys() {
        let mut input = TextInput::new("/ ", "filter");
        input.focus();
        typed(&mut input, "qjn G");
        assert_eq!(input.value(), "qjn G");
        input.handle_key(&key(KeyCode::Left));
        input.handle_key(&key(KeyCode::Backspace));
        assert_eq!(input.value(), "qjnG");
        input.handle_key(&key(KeyCode::Home));
        input.handle_key(&key(KeyCode::Delete));
        assert_eq!(input.value(), "jnG");
        input.handle_key(&ctrl('e'));
        typed(&mut input, " word");
        input.handle_key(&ctrl('w'));
        assert_eq!(input.value(), "jnG ");
        input.handle_key(&ctrl('u'));
        assert_eq!(input.value(), "");
        // Keys that are not text change nothing.
        assert!(!input.handle_key(&key(KeyCode::Tab)));
        assert!(!input.handle_key(&key(KeyCode::Enter)));
    }

    #[test]
    fn paste_flattens_lines_and_respects_the_limit() {
        let mut input = TextInput::new("/ ", "filter");
        input.insert("a\tb\nc\u{7}");
        assert_eq!(input.value(), "a b c");
        input.insert(&"x".repeat(300));
        assert_eq!(input.value().chars().count(), CHAR_LIMIT);
    }

    #[test]
    fn line_scrolls_to_keep_the_cursor_visible() {
        let mut input = TextInput::new("/ ", "filter");
        input.focus();
        input.set_value(&"a".repeat(40));
        let line = input.line(&STYLES);
        assert_eq!(line.width(), width("/ ") + VIEW_WIDTH);
        input.blur();
        assert!(input.line(&STYLES).width() <= width("/ ") + VIEW_WIDTH);
        input.reset();
        assert_eq!(input.line(&STYLES).to_string(), "/ filter");
    }
}
