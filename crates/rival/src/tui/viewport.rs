//! A vertical scroller over pre-wrapped lines. Go: the bubbles `viewport`
//! the detail screen uses, with its pager keys.
//!
//! Content is wrapped to the width before it arrives, so there is no soft
//! wrap and no horizontal scroll: every line already fits.

use ratatui::text::Line;

use super::text::line_width;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Viewport {
    lines: Vec<Line<'static>>,
    y_offset: usize,
    width: usize,
    height: usize,
}

impl Viewport {
    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn set_width(&mut self, w: usize) {
        self.width = w;
    }

    pub fn set_height(&mut self, h: usize) {
        self.height = h;
    }

    pub fn y_offset(&self) -> usize {
        self.y_offset
    }

    pub fn total_line_count(&self) -> usize {
        self.lines.len()
    }

    /// Go: `GetContent`. The plain text of every line.
    #[cfg(test)]
    pub fn content_text(&self) -> String {
        self.lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn max_y_offset(&self) -> usize {
        self.lines.len().saturating_sub(self.height)
    }

    pub fn at_top(&self) -> bool {
        self.y_offset == 0
    }

    pub fn at_bottom(&self) -> bool {
        self.y_offset >= self.max_y_offset()
    }

    /// Go: `SetContentLines`. One empty line counts as no content. The offset
    /// is clamped, so a shrinking log never leaves the view past its end.
    pub fn set_content(&mut self, lines: Vec<Line<'static>>) {
        self.lines = lines;
        if self.lines.len() == 1 && line_width(&self.lines[0]) == 0 {
            self.lines.clear();
        }
        if self.y_offset > self.max_y_offset() {
            self.goto_bottom();
        }
    }

    /// Go: `SetYOffset`, clamped to the content.
    pub fn set_y_offset(&mut self, n: isize) {
        self.y_offset = (n.max(0) as usize).min(self.max_y_offset());
    }

    pub fn goto_top(&mut self) {
        self.y_offset = 0;
    }

    pub fn goto_bottom(&mut self) {
        self.y_offset = self.max_y_offset();
    }

    pub fn scroll_down(&mut self, n: usize) {
        if self.at_bottom() || n == 0 || self.lines.is_empty() {
            return;
        }
        self.y_offset = (self.y_offset + n).min(self.max_y_offset());
    }

    pub fn scroll_up(&mut self, n: usize) {
        if self.at_top() || n == 0 || self.lines.is_empty() {
            return;
        }
        self.y_offset = self.y_offset.saturating_sub(n);
    }

    /// The pager keys. "f" is follow on the detail screen, so page down is
    /// only pgdown and space. Returns whether `key` is a viewport key.
    pub fn handle_key(&mut self, key: &str) -> bool {
        match key {
            "pgdown" | "space" => self.scroll_down(self.height),
            "pgup" | "b" => self.scroll_up(self.height),
            "d" | "ctrl+d" => self.scroll_down(self.height / 2),
            "u" | "ctrl+u" => self.scroll_up(self.height / 2),
            "down" | "j" => self.scroll_down(1),
            "up" | "k" => self.scroll_up(1),
            _ => return false,
        }
        true
    }

    /// The rows on screen, at most `height` of them.
    pub fn visible(&self) -> &[Line<'static>] {
        if self.height == 0 || self.width == 0 {
            return &[];
        }
        let top = self.y_offset.min(self.lines.len());
        let bottom = (top + self.height).min(self.lines.len());
        &self.lines[top..bottom]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vp(n: usize, height: usize) -> Viewport {
        let mut v = Viewport::default();
        v.set_width(10);
        v.set_height(height);
        v.set_content((0..n).map(|i| Line::raw(format!("line {i}"))).collect());
        v
    }

    #[test]
    fn offsets_clamp_to_the_content() {
        let mut v = vp(30, 10);
        assert!(v.at_top() && !v.at_bottom());
        v.goto_bottom();
        assert_eq!(v.y_offset(), 20);
        v.set_y_offset(99);
        assert_eq!(v.y_offset(), 20);
        v.set_y_offset(-5);
        assert_eq!(v.y_offset(), 0);
        // A shrinking content pulls the offset back to the new end.
        v.goto_bottom();
        v.set_content((0..12).map(|i| Line::raw(format!("{i}"))).collect());
        assert_eq!(v.y_offset(), 2);
        // Content shorter than the view: top and bottom at once.
        let v = vp(3, 10);
        assert!(v.at_top() && v.at_bottom());
        assert_eq!(v.visible().len(), 3);
    }

    #[test]
    fn one_empty_line_is_no_content() {
        let mut v = vp(0, 5);
        v.set_content(vec![Line::raw("")]);
        assert_eq!(v.total_line_count(), 0);
        v.set_content(vec![Line::raw(""), Line::raw("")]);
        assert_eq!(v.total_line_count(), 2);
    }

    #[test]
    fn pager_keys() {
        let mut v = vp(100, 10);
        for (key, want) in [
            ("j", 1),
            ("down", 2),
            ("space", 12),
            ("pgdown", 22),
            ("d", 27),
            ("ctrl+d", 32),
            ("k", 31),
            ("up", 30),
            ("u", 25),
            ("ctrl+u", 20),
            ("b", 10),
            ("pgup", 0),
            ("pgup", 0),
        ] {
            assert!(v.handle_key(key), "{key}");
            assert_eq!(v.y_offset(), want, "after {key}");
        }
        for key in ["f", "g", "G", "h", "l", "n"] {
            assert!(!v.handle_key(key), "{key} is not a viewport key");
        }
        v.goto_bottom();
        v.handle_key("pgdown");
        assert_eq!(v.y_offset(), 90, "page down stops at the end");
        let rows: Vec<String> = v.visible().iter().map(ToString::to_string).collect();
        assert_eq!(rows.first().unwrap(), "line 90");
        assert_eq!(rows.last().unwrap(), "line 99");
    }

    #[test]
    fn nothing_visible_without_a_size() {
        let mut v = vp(5, 0);
        assert!(v.visible().is_empty());
        v.set_height(3);
        v.set_width(0);
        assert!(v.visible().is_empty());
    }
}
