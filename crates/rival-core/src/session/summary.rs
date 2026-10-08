//! Prompt-free session index.
//!
//! Go: `internal/session/summary.go`.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::anyhow;
use serde_json::value::RawValue;

use super::{Session, io_error, is_session_file, read_file, sort_newest_first};
use crate::paths::Paths;

const SUMMARY_EDGE_BYTES: i64 = 64 << 10;

/// The same newest-first session index as [`Session::load_all`] without
/// retaining embedded prompts. Large legacy files are read only at their
/// edges, where the pretty writer places the metadata around the prompt line.
pub fn load_all_summaries(paths: &Paths) -> Vec<Session> {
    let dir = paths.sessions_dir();
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    // Go: os.ReadDir returns entries sorted by name.
    let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());

    let mut sessions = Vec::with_capacity(entries.len());
    for entry in entries {
        let name = entry.file_name();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir || !is_session_file(&name.to_string_lossy()) {
            continue;
        }
        let Ok(info) = entry.metadata() else {
            continue;
        };
        let Ok(s) = load_summary_file(&dir.join(&name), info.len() as i64) else {
            continue;
        };
        sessions.push(s);
    }
    sort_newest_first(&mut sessions);
    sessions
}

/// Reads one session without retaining its full prompt.
pub fn load_summary_file(path: &Path, size: i64) -> anyhow::Result<Session> {
    if size <= 2 * SUMMARY_EDGE_BYTES {
        let mut s = Session::from_json(&read_file(path)?)?;
        s.prompt = String::new();
        return Ok(s);
    }

    let mut f = crate::gostd::open_file(path).map_err(|e| io_error("open", path, e))?;
    let mut prefix = vec![0u8; SUMMARY_EDGE_BYTES as usize];
    let n = read_full(&mut f, path, &mut prefix)?;
    prefix.truncate(n);

    f.seek(SeekFrom::End(-SUMMARY_EDGE_BYTES))
        .map_err(|e| io_error("seek", path, e))?;
    let mut suffix = vec![0u8; SUMMARY_EDGE_BYTES as usize];
    let n = read_full(&mut f, path, &mut suffix)?;
    suffix.truncate(n);

    let mut raw_fields = BTreeMap::new();
    collect_summary_fields(&mut raw_fields, &prefix);
    collect_summary_fields(&mut raw_fields, &suffix);
    if raw_fields.is_empty() {
        return Err(anyhow!("session metadata not found"));
    }
    // The collected fields form one object, which decodes like a whole file.
    Session::from_json(&serde_json::to_vec(&raw_fields)?)
}

/// Go: `io.ReadFull` where a short read is accepted (`ErrUnexpectedEOF`) but
/// reading nothing at all is a bare `EOF` error.
fn read_full(f: &mut File, path: &Path, buf: &mut [u8]) -> anyhow::Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        match f.read(&mut buf[total..]) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(io_error("read", path, e)),
        }
    }
    if total == 0 && !buf.is_empty() {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "EOF").into());
    }
    Ok(total)
}

const SUMMARY_FIELDS: &[&str] = &[
    "id",
    "group_id",
    "cli",
    "mode",
    "model",
    "effort",
    "review_scope",
    "prompt_preview",
    "prompt_hash",
    "status",
    "start_time",
    "queued_at",
    "queue_position",
    "end_time",
    "exit_code",
    "duration",
    "work_dir",
    "log_file",
    "output_bytes",
    "output_lines",
    "error",
    "account",
    "pid",
    "pid_start",
    "owner_pid",
    "owner_pid_start",
];

fn collect_summary_fields(dst: &mut BTreeMap<String, Box<RawValue>>, data: &[u8]) {
    for line in data.split(|&b| b == b'\n') {
        let line = trim_space(line);
        if line.len() < 4 || line[0] != b'"' {
            continue;
        }
        let Some(colon) = line.iter().position(|&b| b == b':') else {
            continue;
        };
        if colon < 2 {
            continue;
        }
        let Ok(key) = serde_json::from_slice::<String>(&line[..colon]) else {
            continue;
        };
        if !SUMMARY_FIELDS.contains(&key.as_str()) {
            continue;
        }
        let value = trim_space(&line[colon + 1..]);
        let value = value.strip_suffix(b",").unwrap_or(value);
        let Ok(value) = serde_json::from_slice::<Box<RawValue>>(value) else {
            continue;
        };
        dst.insert(key, value);
    }
}

/// Go: `bytes.TrimSpace` — trims Unicode white space; an invalid UTF-8 byte
/// stops the trim.
fn trim_space(mut s: &[u8]) -> &[u8] {
    while let Some(c) = first_char(s) {
        if !c.is_whitespace() {
            break;
        }
        s = &s[c.len_utf8()..];
    }
    while let Some(c) = last_char(s) {
        if !c.is_whitespace() {
            break;
        }
        s = &s[..s.len() - c.len_utf8()];
    }
    s
}

fn first_char(s: &[u8]) -> Option<char> {
    let head = &s[..s.len().min(4)];
    let valid = match std::str::from_utf8(head) {
        Ok(v) => v,
        Err(e) => std::str::from_utf8(&head[..e.valid_up_to()]).ok()?,
    };
    valid.chars().next()
}

fn last_char(s: &[u8]) -> Option<char> {
    (1..=s.len().min(4)).find_map(|k| {
        let tail = std::str::from_utf8(&s[s.len() - k..]).ok()?;
        let mut chars = tail.chars();
        let c = chars.next()?;
        chars.next().is_none().then_some(c)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Local;

    fn stored(prompt: String) -> Session {
        Session {
            id: "summary-id".into(),
            group_id: "summary-group".into(),
            cli: "opencode".into(),
            mode: "megareview".into(),
            model: "test-model".into(),
            effort: "high".into(),
            prompt,
            prompt_preview: "large prompt preview".into(),
            status: "running".into(),
            start_time: Some(Local::now().fixed_offset()),
            work_dir: "/tmp/work".into(),
            log_file: "/tmp/internal.log".into(),
            pid: 123,
            pid_start: 456,
            owner_pid: 789,
            owner_pid_start: 101112,
            ..Session::default()
        }
    }

    fn write(dir: &Path, s: &Session) -> (std::path::PathBuf, i64) {
        let data = s.to_json().unwrap();
        let path = dir.join("session.json");
        fs::write(&path, &data).unwrap();
        (path, data.len() as i64)
    }

    // Go: TestLoadSummaryFileSkipsLargePrompt.
    #[test]
    fn load_summary_file_skips_large_prompt() {
        let tmp = tempfile::tempdir().unwrap();
        let stored = stored("large prompt ".repeat(20000));
        let (path, size) = write(tmp.path(), &stored);
        assert!(size > 2 * SUMMARY_EDGE_BYTES, "edge path not exercised");

        let got = load_summary_file(&path, size).unwrap();
        assert!(
            got.prompt.is_empty(),
            "summary retained {} prompt bytes",
            got.prompt.len()
        );
        assert_eq!(
            (&got.id, &got.group_id, &got.prompt_preview),
            (&stored.id, &stored.group_id, &stored.prompt_preview),
            "summary identity = {got:?}"
        );
        assert_eq!(
            (got.pid, got.pid_start, got.owner_pid, got.owner_pid_start),
            (
                stored.pid,
                stored.pid_start,
                stored.owner_pid,
                stored.owner_pid_start
            ),
            "summary process metadata = {got:?}"
        );
        assert_eq!(got.log_file, stored.log_file);
        assert_eq!(got.start_time, stored.start_time);
    }

    #[test]
    fn load_summary_file_small_record_drops_prompt_only() {
        let tmp = tempfile::tempdir().unwrap();
        let mut stored = stored("short prompt".into());
        stored.exit_code = Some(0);
        let (path, size) = write(tmp.path(), &stored);
        assert!(size <= 2 * SUMMARY_EDGE_BYTES);

        let got = load_summary_file(&path, size).unwrap();
        assert_eq!(
            got,
            Session {
                prompt: String::new(),
                ..stored
            }
        );
    }

    #[test]
    fn load_summary_file_large_record_keeps_every_metadata_field() {
        let tmp = tempfile::tempdir().unwrap();
        let mut stored = stored("x".repeat(200_000));
        stored.review_scope = "diff".into();
        stored.prompt_hash = "abc".into();
        stored.queued_at = stored.start_time;
        stored.queue_position = 3;
        stored.end_time = stored.start_time;
        stored.exit_code = Some(0);
        stored.duration = "1m2s".into();
        stored.output_bytes = 9;
        stored.output_lines = 2;
        stored.error_msg = "boom".into();
        stored.account = "acc".into();
        let (path, size) = write(tmp.path(), &stored);

        let got = load_summary_file(&path, size).unwrap();
        assert_eq!(
            got,
            Session {
                prompt: String::new(),
                ..stored
            }
        );
    }

    #[test]
    fn load_summary_file_large_file_without_metadata_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("x.json");
        fs::write(&path, "z".repeat(200_000)).unwrap();
        let err = load_summary_file(&path, 200_000).unwrap_err();
        assert_eq!(err.to_string(), "session metadata not found");
    }

    #[test]
    fn load_summary_file_large_record_reads_exact_keys_and_the_last_line() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("s.json");
        let mut data = Vec::new();
        data.extend_from_slice(b"{\n  \"ID\": \"upper\",\n  \"id\": \"x\",\n  \"prompt\": \"");
        data.extend(std::iter::repeat_n(b'z', 200_000));
        data.extend_from_slice(b"\",\n  \"prompt_preview\": \"\xe6\x97\",\n  \"status\": \"a\",\n");
        data.extend_from_slice(b"  \"status\": \"b\",\n  \"queued_at\": null,\n");
        data.extend_from_slice(b"  \"start_time\": \"0001-01-01T00:00:00Z\"\n}");
        fs::write(&path, &data).unwrap();

        let got = load_summary_file(&path, data.len() as i64).unwrap();
        // Only exact summary keys are collected, so "ID" is skipped.
        assert_eq!(got.id, "x");
        // A line with invalid UTF-8 is not valid JSON and is skipped.
        assert_eq!(got.prompt_preview, "");
        assert_eq!(got.start_time, None);
        // A later line overwrites an earlier one.
        assert_eq!(got.status, "b");
        assert_eq!(got.queued_at, None);
        assert!(got.prompt.is_empty());
    }

    #[test]
    fn collect_summary_fields_skips_prompt_bad_values_and_unknown_keys() {
        let data = b"{\n  \"id\": \"a\",\n  \"prompt\": \"secret\",\n  \"pid\": 12,\n  \"mode\": broken,\n  \"other\": 1,\n  \"status\": \"running\"\n}";
        let mut dst = BTreeMap::new();
        collect_summary_fields(&mut dst, data);
        let got: Vec<_> = dst.iter().map(|(k, v)| (k.as_str(), v.get())).collect();
        assert_eq!(
            got,
            [("id", "\"a\""), ("pid", "12"), ("status", "\"running\"")]
        );
    }

    #[test]
    fn trim_space_matches_go() {
        assert_eq!(trim_space(b" \t\x0b\x0c\r\n x \n"), b"x");
        assert_eq!(trim_space("\u{a0}\u{85}x\u{2003}".as_bytes()), b"x");
        assert_eq!(trim_space(b"\xff x \xfe"), b"\xff x \xfe");
        assert_eq!(trim_space(b"   "), b"");
    }

    #[test]
    fn load_all_summaries_skips_temp_files_and_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: tmp.path().join(".rival"),
        };
        let dir = paths.sessions_dir();
        fs::create_dir_all(dir.join("d.json")).unwrap();
        let mut a = stored("p".into());
        a.id = "a".into();
        fs::write(dir.join("a.json"), a.to_json().unwrap()).unwrap();
        fs::write(dir.join("a.json.tmp"), br#"{"id":"tmp"}"#).unwrap();
        fs::write(dir.join("b.json"), b"not json").unwrap();

        let got = load_all_summaries(&paths);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "a");
        assert!(got[0].prompt.is_empty());
    }
}
