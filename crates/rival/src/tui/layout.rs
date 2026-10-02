//! Frame geometry. Go: `internal/dashboard/layout.go`.

use super::session_list::LIST_FIXED_WIDTH;
use super::styles::{BANNER_LINES, COMPACT_HEADER_BELOW_HEIGHT};

/// Below `MIN_WIDTH`×`MIN_HEIGHT` the frame cannot hold the header, tab bar,
/// one row and help, so a notice replaces it.
pub const MIN_WIDTH: usize = 60;
pub const MIN_HEIGHT: usize = 16;
/// From this width up the body splits into the list and the preview.
pub const PREVIEW_MIN_WIDTH: usize = 120;

/// The frame geometry, computed once per resize. The view and the update
/// both read it: if they sized the body differently, a scrolled view would
/// render rows the frame then clips.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Layout {
    pub width: usize,
    pub height: usize,
    pub header_h: usize,
    pub body_h: usize,
    pub list_w: usize,
    pub preview_w: usize,
    pub show_preview: bool,
    pub compact: bool,
    pub too_small: bool,
}

/// Go: `computeLayout`. Splits the terminal into header, tab bar, body and
/// help bar: `header_h + 1 + body_h + 1 == height`. From
/// [`PREVIEW_MIN_WIDTH`] up the body splits into the list (55%, but never
/// narrower than its fixed columns plus the border) and the preview, with a
/// 1-col gap between the two boxes: `list_w + 1 + preview_w == width`.
/// Below it the list takes the full width.
pub fn compute_layout(width: usize, height: usize) -> Layout {
    let mut l = Layout {
        width,
        height,
        list_w: width,
        ..Layout::default()
    };
    l.too_small = width < MIN_WIDTH || height < MIN_HEIGHT;
    if width >= PREVIEW_MIN_WIDTH {
        l.show_preview = true;
        // A 55% split alone leaves the list under its fixed columns at
        // 120-145 cols, which silently dropped EFF and PROJECT.
        l.list_w = (LIST_FIXED_WIDTH + 2).max(width * 55 / 100);
        l.preview_w = width - l.list_w - 1;
    }
    l.compact = height < COMPACT_HEADER_BELOW_HEIGHT;
    l.header_h = if l.compact { 1 } else { BANNER_LINES.len() };
    l.body_h = height.saturating_sub(l.header_h + 2);
    l
}

#[cfg(test)]
mod tests;
