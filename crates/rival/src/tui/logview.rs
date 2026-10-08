//! Reading session logs for display, plus the "open log" helpers.
//!
//! Everything here blocks on the file system, so only [`super::jobs`] calls
//! it, on the runtime's workers. The model keeps a [`LogSlot`] per pane and
//! never reads a file itself.

use std::fmt::Write as _;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use rival_core::logfmt;
use rival_core::session::Session;

use super::session_list::model_name;
use super::text::hardwrap;

/// Strips terminal control sequences and then expands
/// tabs: a tab is one rune but many cells, so leaving it in place makes
/// wrapped lines overflow the terminal.
pub fn sanitize_log(raw: &str) -> String {
    logfmt::expand_tabs(&logfmt::sanitize(raw), logfmt::TAB_WIDTH)
}

/// Makes text from a model's answer safe to become a span. `sanitize` strips
/// ANSI/OSC sequences and C0 controls but keeps C1 controls; those go too.
/// Tabs and newlines stay.
pub fn strip_controls(s: &str) -> String {
    let mut out = logfmt::sanitize(s);
    out.retain(|c| c == '\t' || c == '\n' || !c.is_control());
    out
}

/// [`strip_controls`] with tabs expanded, for text that is wrapped by cell.
pub fn display_text(s: &str) -> String {
    logfmt::expand_tabs(&strip_controls(s), logfmt::TAB_WIDTH)
}

/// Reads a file's tail. Tests swap this seam to count file reads.
pub type ReadTail = fn(&Path, i64) -> io::Result<(Vec<u8>, bool)>;

/// Caps the preview's read. It keeps [`PREVIEW_TAIL_LINES`] lines of any
/// sane log, and the preview never shows more.
pub const PREVIEW_TAIL_BYTES: i64 = 32 << 10;

/// Caps how many log lines the preview keeps. Far more than any pane shows,
/// and it bounds the wrap work on a 1s tick.
pub const PREVIEW_TAIL_LINES: usize = 200;

/// How many extra raw lines a `last_n` cut keeps before sanitizing. Trailing
/// lines that sanitize to nothing (a bare colour reset) are trimmed
/// afterwards, and the slack keeps them from costing tail rows.
const RAW_CUT_SLACK: usize = 16;

/// The note above a log cut to its tail.
pub const OMITTED_MARKER: &str = "... earlier output omitted — press o to open the full log";

/// One log, sanitized and wrapped. The first `marker_rows` lines are the
/// [`OMITTED_MARKER`], which the views draw in the label style.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogLines {
    pub marker_rows: usize,
    pub lines: Vec<String>,
}

/// Reads one log's tail, strips terminal control
/// sequences, and hard-wraps by display width so wide runes and tabs cannot
/// push a line past `wrap_width`. A missing log is an error, an empty one has
/// no lines.
///
/// `last_n > 0` is the preview: it reads at most [`PREVIEW_TAIL_BYTES`] and
/// keeps only the last `last_n` source lines, cut before sanitizing and
/// wrapping, so a small pane does not process 256 KB it will never show. The
/// omitted marker is then left out, since the caller cut the text itself.
pub fn read_log_lines(
    path: &str,
    wrap_width: usize,
    last_n: usize,
    read_tail: ReadTail,
) -> Result<LogLines, String> {
    // Tail-only: the detail view rebuilds this on every 1s tick, and
    // wrapping a whole multi-megabyte log takes about as long as the tick.
    let mut max_bytes = logfmt::MAX_TAIL_BYTES;
    if last_n > 0 {
        max_bytes = max_bytes.min(PREVIEW_TAIL_BYTES);
    }
    let (data, truncated) =
        read_tail(Path::new(path), max_bytes).map_err(|e| format!("open {path}: {}", e))?;
    if data.is_empty() {
        return Ok(LogLines::default());
    }

    let data = String::from_utf8_lossy(&data);
    let mut raw: &str = &data;
    if last_n > 0 {
        // Sanitizing works line by line, so cutting raw lines first is safe.
        raw = raw.trim_end_matches('\n');
        if let Some(i) = nth_last_newline(raw, last_n + RAW_CUT_SLACK) {
            raw = &raw[i + 1..];
        }
    }
    // No public model naming: the TUI shows the raw model id everywhere, so
    // the log names the same model the list does.
    let sanitized = sanitize_log(raw);
    let mut text = sanitized.trim_end_matches('\n');
    if last_n > 0
        && let Some(i) = nth_last_newline(text, last_n)
    {
        text = &text[i + 1..];
    }
    let wrap = |s: &str| -> Vec<String> {
        let wrapped = if wrap_width > 0 {
            hardwrap(s, wrap_width)
        } else {
            s.to_string()
        };
        wrapped.split('\n').map(str::to_string).collect()
    };
    let mut out = LogLines::default();
    if last_n == 0 && truncated {
        out.lines = wrap(OMITTED_MARKER);
        out.marker_rows = out.lines.len();
    }
    out.lines.extend(wrap(text));
    Ok(out)
}

/// The byte index of the n-th newline from the end of
/// `s`, or `None` when `s` has fewer than `n` newlines.
pub fn nth_last_newline(s: &str, n: usize) -> Option<usize> {
    let mut end = s.len();
    let mut found = None;
    for _ in 0..n {
        let i = s[..end].rfind('\n')?;
        found = Some(i);
        end = i;
    }
    found
}

/// The file state a cached read was built from. A log whose size and mtime
/// are unchanged is neither re-read nor re-wrapped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileState {
    pub size: u64,
    pub mtime_nanos: i128,
}

/// The file state for the cache key.
pub fn file_state(path: &str) -> io::Result<FileState> {
    let meta = fs::metadata(path)?;
    let mtime = meta.modified()?;
    let mtime_nanos = match mtime.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_nanos() as i128,
        Err(e) => -(e.duration().as_nanos() as i128),
    };
    Ok(FileState {
        size: meta.len(),
        mtime_nanos,
    })
}

/// What one log read is for: which session's file, wrapped how. A result
/// is accepted only while its key is still the one the pane wants, so a late
/// read for another member or an old width is dropped.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LogKey {
    pub session_id: String,
    pub path: String,
    pub width: usize,
    pub last_n: usize,
}

impl LogKey {
    pub fn new(s: &Session, width: usize, last_n: usize) -> LogKey {
        LogKey {
            session_id: s.id.clone(),
            path: s.log_file.clone(),
            width,
            last_n,
        }
    }

    /// Whether a cached read is of `s`'s log, at any wrap.
    pub fn is_of(&self, s: &Session) -> bool {
        self.session_id == s.id && self.path == s.log_file
    }
}

/// Which pane a log read belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogPane {
    Detail,
    Preview,
}

/// A log read for a worker. `known` is the file state of the cached read
/// with the same key: when the file still has it, nothing is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRequest {
    pub pane: LogPane,
    pub seq: u64,
    pub key: LogKey,
    pub known: Option<FileState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogOutcome {
    /// The file still has the `known` state; keep the cached lines.
    Unchanged,
    /// A fresh read. `state` is `None` when the stat failed but the read did
    /// not: the lines show, but the next request reads again.
    Loaded {
        state: Option<FileState>,
        lines: LogLines,
    },
    /// The read failed; the text is the error message.
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogResult {
    pub pane: LogPane,
    pub seq: u64,
    pub key: LogKey,
    pub outcome: LogOutcome,
}

/// The log cache read, on a worker. Stats the file; reads it only when its
/// state differs from `req.known`.
pub fn load_log(req: LogRequest, read_tail: ReadTail) -> LogResult {
    let read = |state| match read_log_lines(&req.key.path, req.key.width, req.key.last_n, read_tail)
    {
        Ok(lines) => LogOutcome::Loaded { state, lines },
        Err(msg) => LogOutcome::Failed(msg),
    };
    let outcome = match file_state(&req.key.path) {
        // Let the read report the error, so the message stays the same.
        Err(_) => read(None),
        Ok(state) if req.known == Some(state) => LogOutcome::Unchanged,
        Ok(state) => read(Some(state)),
    };
    LogResult {
        pane: req.pane,
        seq: req.seq,
        key: req.key,
        outcome,
    }
}

/// A cached read and the key and file state it was built from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub key: LogKey,
    pub state: Option<FileState>,
    pub result: Result<LogLines, String>,
}

/// One pane's log reads: the cached result, the read in flight and the
/// sequence numbers that drop late results.
#[derive(Debug, Clone, Default)]
pub struct LogSlot {
    entry: Option<LogEntry>,
    in_flight: Option<(LogKey, u64)>,
    seq: u64,
    accepted: u64,
}

impl LogSlot {
    /// The cached read, if any.
    #[cfg(test)]
    pub fn entry(&self) -> Option<&LogEntry> {
        self.entry.as_ref()
    }

    /// The cached read of `s`'s log, at whatever wrap it was built for.
    pub fn entry_of(&self, s: &Session) -> Option<&LogEntry> {
        self.entry.as_ref().filter(|e| e.key.is_of(s))
    }

    /// A read for `key`, or `None` when one is already in flight for it, or
    /// when the cache holds it and `refresh` is off. A refresh still reads
    /// nothing when the file is unchanged: the worker only stats it.
    pub fn request(&mut self, pane: LogPane, key: LogKey, refresh: bool) -> Option<LogRequest> {
        if self.in_flight.as_ref().is_some_and(|(k, _)| *k == key) {
            return None;
        }
        let cached = self.entry.as_ref().filter(|e| e.key == key);
        if cached.is_some() && !refresh {
            return None;
        }
        // Only successful reads are cached; a failed one is read again.
        let known = cached.filter(|e| e.result.is_ok()).and_then(|e| e.state);
        self.seq += 1;
        self.in_flight = Some((key.clone(), self.seq));
        Some(LogRequest {
            pane,
            seq: self.seq,
            key,
            known,
        })
    }

    /// Applies a worker's result. It is dropped when a newer result was
    /// already applied or when `wanted` (the key the pane needs now) differs.
    /// Returns whether the cached lines changed.
    pub fn accept(&mut self, res: LogResult, wanted: Option<&LogKey>) -> bool {
        if self
            .in_flight
            .as_ref()
            .is_some_and(|(_, seq)| *seq == res.seq)
        {
            self.in_flight = None;
        }
        if res.seq <= self.accepted || wanted != Some(&res.key) {
            return false;
        }
        self.accepted = res.seq;
        let entry = match res.outcome {
            LogOutcome::Unchanged => return false,
            LogOutcome::Loaded { state, lines } => LogEntry {
                key: res.key,
                state,
                result: Ok(lines),
            },
            LogOutcome::Failed(msg) => LogEntry {
                key: res.key,
                state: None,
                result: Err(msg),
            },
        };
        self.entry = Some(entry);
        true
    }

    /// Forgets everything; a result still in flight is dropped on arrival.
    pub fn clear(&mut self) {
        self.entry = None;
        self.in_flight = None;
        self.accepted = self.seq;
    }
}

// --- open log ----------------------------------------------------------------

/// Copies the raw log, with no public model renaming, to
/// a temp file in `dir`, so the model id in the file matches the screen.
pub fn create_log_view(dir: &Path, s: &Session) -> io::Result<PathBuf> {
    let data = fs::read(&s.log_file)?;
    create_text_view(dir, &data)
}

/// Every member's raw log under a heading with
/// its model, role and effort, in one temp file in `dir`.
pub fn create_group_log_view<S: AsRef<Session>>(dir: &Path, sessions: &[S]) -> io::Result<PathBuf> {
    let mut content = Vec::new();
    for s in sessions {
        let s = s.as_ref();
        let mut label = group_log_label(s);
        if s.status == "failed" && !s.error_msg.is_empty() {
            label.push_str(" (FAILED)");
        }
        let mut head = format!("=== {label} ===\n");
        if !s.error_msg.is_empty() {
            let _ = writeln!(head, "Error: {}", s.error_msg);
        }
        content.extend_from_slice(head.as_bytes());
        match fs::read(&s.log_file) {
            Err(e) => {
                if s.error_msg.is_empty() {
                    let msg = format!("(log unavailable: open {}: {})\n", s.log_file, e);
                    content.extend_from_slice(msg.as_bytes());
                }
            }
            Ok(data) => {
                content.extend_from_slice(&data);
                if data.last().is_some_and(|&b| b != b'\n') {
                    content.push(b'\n');
                }
            }
        }
        content.push(b'\n');
    }
    create_text_view(dir, &content)
}

/// Heads one member in the combined group log: its raw
/// model id, its role and its effort.
pub fn group_log_label(s: &Session) -> String {
    let role = if s.mode == "consilium" {
        "JUDGE"
    } else {
        "REVIEW"
    };
    let mut label = format!("{} {role}", model_name(s));
    if !s.effort.is_empty() {
        label.push_str(" · EFFORT ");
        label.push_str(&s.effort);
    }
    label
}

/// Writes `content` to a new `rival-log-*.txt` temp file in `dir`. A file
/// that cannot be written whole is removed again.
pub fn create_text_view(dir: &Path, content: &[u8]) -> io::Result<PathBuf> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let mut tries = 0;
    let (mut file, path) = loop {
        let path = dir.join(format!("rival-log-{}.txt", uuid::Uuid::new_v4().simple()));
        match opts.open(&path) {
            Ok(f) => break (f, path),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists && tries < 10 => tries += 1,
            Err(e) => return Err(e),
        }
    };
    if let Err(e) = file.write_all(content) {
        drop(file);
        let _ = fs::remove_file(&path);
        return Err(e);
    }
    Ok(path)
}

#[cfg(test)]
mod tests;
