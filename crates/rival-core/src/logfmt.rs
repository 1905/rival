//! Makes raw CLI logs safe to display.
//!
//! Go: `internal/logfmt`. Runtime logs are captured verbatim, so they carry ANSI
//! escapes, carriage-return progress frames and stray control bytes that neither
//! a terminal pane nor a browser renders sensibly. The TUI uses [`sanitize`] and
//! then expands tabs, because a tab is one rune but many terminal cells.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

/// How many spaces [`expand_tabs`] substitutes for one tab.
pub const TAB_WIDTH: i32 = 4;

/// How much of a log's tail a display surface reads. Logs reach tens of
/// megabytes (62 MB observed), and a renderer that re-reads on a timer cannot
/// afford the whole file: wrapping 62 MB measured ~970ms, against a 1s refresh
/// tick. Earlier output stays reachable by opening the log itself.
pub const MAX_TAIL_BYTES: i64 = 256 << 10;

/// Makes one raw log line safe to display: keeps only the last progress frame,
/// strips ANSI escape sequences, and drops C0 control chars and DEL. Tabs
/// survive — expand them separately with [`expand_tabs`].
///
/// The step order is load-bearing: [`sanitize`] splits on "\n", so every line
/// of a CRLF log arrives with a trailing "\r". Trimming that single terminator
/// first keeps the progress-frame rule (keep only what follows the last
/// remaining "\r") from blanking every line of such a log.
pub fn sanitize_line(line: &str) -> String {
    let line = line.strip_suffix('\r').unwrap_or(line);
    let line = match line.rfind('\r') {
        Some(idx) => &line[idx + 1..],
        None => line,
    };
    let stripped = strip_ansi(line);

    let mut b = String::with_capacity(stripped.len());
    for r in stripped.chars() {
        if r == '\t' {
            b.push(r);
            continue;
        }
        if r < '\x20' || r == '\x7f' {
            continue;
        }
        b.push(r);
    }
    b
}

/// Uses the parser's execute callback to preserve tabs, including inside CSI.
fn strip_ansi(s: &str) -> String {
    struct Text(String);
    impl vte::Perform for Text {
        fn print(&mut self, c: char) {
            self.0.push(c);
        }
        fn execute(&mut self, byte: u8) {
            if byte == b'\t' {
                self.0.push('\t');
            }
        }
    }
    let mut text = Text(String::with_capacity(s.len()));
    vte::Parser::new().advance(&mut text, s.as_bytes());
    text.0
}

/// Sanitizes each line while preserving newline separators.
pub fn sanitize(raw: &str) -> String {
    raw.split('\n')
        .map(sanitize_line)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Replaces every tab with `width` spaces. Terminal panes need this before
/// wrapping: a tab is one rune but many cells, so leaving it in place makes
/// wrapped lines overflow. A non-positive width removes tabs entirely.
pub fn expand_tabs(s: &str, width: i32) -> String {
    let width = width.max(0) as usize;
    s.replace('\t', &" ".repeat(width))
}

/// Returns up to `max_bytes` from the end of `path` and whether the file was
/// truncated.
///
/// A tail that starts mid-file is aligned past the first newline: the raw
/// offset otherwise decapitates whatever multi-byte rune or ANSI escape
/// straddles it, and a beheaded escape renders as literal garbage. Escapes do
/// not span lines, so one alignment fixes both. A single enormous line has no
/// newline to align to — serving the unaligned tail beats serving nothing.
pub fn read_tail(path: &Path, max_bytes: i64) -> io::Result<(Vec<u8>, bool)> {
    let mut f = File::open(path)?;
    let size = f.metadata()?.len() as i64;
    let truncated = size > max_bytes;
    if truncated {
        f.seek(SeekFrom::End(-max_bytes))?;
    }

    let mut data = Vec::new();
    f.take(max_bytes.max(0) as u64).read_to_end(&mut data)?;
    if truncated
        && let Some(idx) = data.iter().position(|&b| b == b'\n')
        && idx + 1 < data.len()
    {
        data.drain(..=idx);
    }
    Ok((to_valid_utf8(&data), truncated))
}

/// Go's `strings.ToValidUTF8(s, "")`: drops every invalid byte sequence.
fn to_valid_utf8(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    for chunk in data.utf8_chunks() {
        out.extend_from_slice(chunk.valid().as_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn sanitize_cases() {
        let cases: &[(&str, &str, &str)] = &[
            (
                "tabs survive sanitize",
                "if x {\n\treturn\n}",
                "if x {\n\treturn\n}",
            ),
            ("progress frames collapse to the last", "10%\r99%", "99%"),
            ("csi color stripped", "\x1b[31mred\x1b[0m", "red"),
            ("osc bel stripped", "\x1b]0;title\x07text", "text"),
            ("osc st stripped", "\x1b]8;;url\x1b\\link", "link"),
            ("backspace and nul dropped", "a\x08b\x00c", "abc"),
            ("newlines preserved", "one\ntwo\nthree", "one\ntwo\nthree"),
            ("plain text unchanged", "plain log line", "plain log line"),
            // CRLF: sanitize splits on "\n", so each line still ends in "\r".
            // The trailing-terminator trim must run before the progress-frame
            // rule or every line of a CRLF log collapses to empty.
            ("crlf lines survive", "alpha\r\nbeta\r\n", "alpha\nbeta\n"),
            (
                "trailing frame keeps its text",
                "working...\r",
                "working...",
            ),
            ("crlf plus progress frames", "10%\r99%\r\n", "99%\n"),
            // Rust-only: tabs next to escapes, non-ASCII text, DEL.
            ("tab beside escapes", "\x1b[1m\tbold\x1b[0m\tx", "\tbold\tx"),
            ("non-ascii kept", "привет ✓ 日本", "привет ✓ 日本"),
            ("del dropped", "a\x7fb", "ab"),
        ];
        for (name, raw, want) in cases {
            assert_eq!(sanitize(raw), *want, "{name}: sanitize({raw:?})");
        }
    }

    #[test]
    fn sanitize_line_drops_embedded_newline() {
        assert_eq!(sanitize_line("a\nb"), "ab");
    }

    #[test]
    fn expand_tabs_widths() {
        assert_eq!(expand_tabs("a\tb", TAB_WIDTH), "a    b");
        assert_eq!(expand_tabs("a\tb", 0), "ab");
        assert_eq!(expand_tabs("a\tb", -1), "ab");
    }

    fn write_temp(data: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.txt");
        fs::write(&path, data).unwrap();
        (dir, path)
    }

    #[test]
    fn read_tail_short_file_whole() {
        let (_dir, path) = write_temp(b"one\ntwo\n");
        let (data, truncated) = read_tail(&path, MAX_TAIL_BYTES).unwrap();
        assert_eq!(data, b"one\ntwo\n");
        assert!(!truncated);
    }

    #[test]
    fn read_tail_aligns_past_first_newline() {
        let (_dir, path) = write_temp(b"aaaa\nbbbb\ncccc\n");
        // The last 12 bytes are "a\nbbbb\ncccc\n"; the partial "a" line goes.
        let (data, truncated) = read_tail(&path, 12).unwrap();
        assert_eq!(data, b"bbbb\ncccc\n");
        assert!(truncated);
    }

    #[test]
    fn read_tail_without_newline_keeps_unaligned_tail() {
        let (_dir, path) = write_temp(b"abcdefghij");
        let (data, truncated) = read_tail(&path, 4).unwrap();
        assert_eq!(data, b"ghij");
        assert!(truncated);
    }

    #[test]
    fn read_tail_newline_as_last_byte_is_not_aligned() {
        // idx+1 == len: aligning would serve nothing, so the tail stays.
        let (_dir, path) = write_temp(b"abcdef\n");
        let (data, truncated) = read_tail(&path, 3).unwrap();
        assert_eq!(data, b"ef\n");
        assert!(truncated);
    }

    #[test]
    fn read_tail_drops_invalid_utf8() {
        // A tail cut mid-rune with no newline keeps the beheaded bytes; they
        // are removed, not replaced.
        let (_dir, path) = write_temp("xé日".as_bytes());
        // "日" is 3 bytes, "é" is 2: the last 4 bytes start mid-"é".
        let (data, truncated) = read_tail(&path, 4).unwrap();
        assert_eq!(data, "日".as_bytes());
        assert!(truncated);

        let (_dir2, path2) = write_temp(b"ok\xffgo\xe2\x82!");
        let (data, truncated) = read_tail(&path2, MAX_TAIL_BYTES).unwrap();
        assert_eq!(data, b"okgo!");
        assert!(!truncated);
    }

    #[test]
    fn read_tail_exact_size_not_truncated() {
        let (_dir, path) = write_temp(b"abcd\nef");
        let (data, truncated) = read_tail(&path, 7).unwrap();
        assert_eq!(data, b"abcd\nef");
        assert!(!truncated);
    }

    #[test]
    fn read_tail_missing_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_tail(&dir.path().join("nope"), MAX_TAIL_BYTES).is_err());
    }
}

#[test]
fn tab_inside_ansi_escape_preserves_parser_state() {
    assert_eq!(sanitize_line("\x1b[3\t1mX"), "\tX");
}
