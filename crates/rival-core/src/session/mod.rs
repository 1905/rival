//! Session records under `~/.rival/sessions`.
//!
//! Go: `internal/session`. Each run writes `<id>.json` (Go `MarshalIndent`
//! bytes) and appends output to `<id>.log`. Several processes write the same
//! record, so every save goes through a unique temp file and a rename.

pub mod reaper;
pub mod summary;

#[cfg(test)]
mod tests;

use std::borrow::Borrow;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::path::Path;
use std::time::Instant;

use anyhow::anyhow;
use chrono::{DateTime, FixedOffset, Local, TimeDelta};
use serde::Serialize;
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};

use crate::config;
use crate::gojson::{self, GoString, TypeMismatch};
use crate::gostd;
use crate::paths::Paths;
use crate::procinfo;

// Session modes. The dashboards label a run by its mode, so a new run type
// needs its own value here rather than reusing an existing one.
pub const MODE_PLAN: &str = "plan";
pub const MODE_ANTISLOP: &str = "antislop";
pub const MODE_SECURITY: &str = "security";

/// Reports whether mode names a task rather than a transport.
/// A task mode identifies the run in both dashboards, so a runtime must not
/// overwrite it with a transport name such as "native" or "docker".
pub fn is_task_mode(mode: &str) -> bool {
    mode == MODE_PLAN || mode == MODE_ANTISLOP || mode == MODE_SECURITY
}

/// One run. Field order and `omitempty` rules match the Go struct, so the
/// JSON bytes match too. Go `int` fields are `i64`. Value fields with
/// `omitempty` skip their zero value; Go pointer fields are `Option` and
/// skip only `None`. Records decode through [`Session::from_json`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Session {
    pub id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub group_id: String,
    pub cli: String,
    pub mode: String,
    pub model: String,
    pub effort: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub review_scope: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub prompt: String,
    /// The first 100 bytes of the prompt. A rune split at byte 100 keeps its
    /// stray bytes, which Go writes as `\ufffd` escapes.
    #[serde(skip_serializing_if = "GoString::is_empty")]
    pub prompt_preview: GoString,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub prompt_hash: String,
    pub status: String,
    #[serde(with = "gojson::time")]
    pub start_time: DateTime<FixedOffset>,
    #[serde(skip_serializing_if = "Option::is_none", with = "gojson::opt_time")]
    pub queued_at: Option<DateTime<FixedOffset>>,
    #[serde(skip_serializing_if = "is_zero")]
    pub queue_position: i64,
    #[serde(skip_serializing_if = "Option::is_none", with = "gojson::opt_time")]
    pub end_time: Option<DateTime<FixedOffset>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub duration: String,
    pub work_dir: String,
    pub log_file: String,
    pub output_bytes: i64,
    pub output_lines: i64,
    #[serde(rename = "error", skip_serializing_if = "String::is_empty")]
    pub error_msg: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub account: String,
    pub pid: i64,
    /// Start time of `pid` (Unix ns); guards against PID reuse.
    #[serde(skip_serializing_if = "is_zero")]
    pub pid_start: i64,
    /// The rival process driving this session. `pid` is overwritten with the
    /// provider child's PID once the subprocess starts, so without this field
    /// the reaper cannot tell "provider exited, rival is about to write the
    /// final status" (a normal end-of-run window) from "everything is dead".
    /// Sessions written by older releases have 0 here.
    #[serde(skip_serializing_if = "is_zero")]
    pub owner_pid: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub owner_pid_start: i64,
    /// Not in the record: the monotonic clock behind `start_time`.
    #[serde(skip)]
    pub start_mono: MonoStart,
}

/// Go: `time.Now()` — a wall-clock reading plus the monotonic reading Go
/// keeps inside an in-process `time.Time`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Now {
    pub(crate) wall: DateTime<FixedOffset>,
    pub(crate) mono: Instant,
}

impl Now {
    fn read() -> Now {
        Now {
            wall: Local::now().fixed_offset(),
            mono: Instant::now(),
        }
    }
}

/// The monotonic reading taken with `start_time` when this process set it.
/// Go's `now.Sub(StartTime)` uses the monotonic clock in that case, so a
/// wall-clock step during a run does not change the duration. It applies
/// only while `start_time` still holds the wall time it was taken with; a
/// loaded record has none and uses wall time, as in Go. It is not serialized
/// and always compares equal.
#[derive(Clone, Copy, Default)]
pub struct MonoStart(Option<Now>);

impl MonoStart {
    pub(crate) fn at(now: Now) -> MonoStart {
        MonoStart(Some(now))
    }
}

impl PartialEq for MonoStart {
    fn eq(&self, _: &MonoStart) -> bool {
        true
    }
}

impl fmt::Debug for MonoStart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.0.is_some() {
            "MonoStart(set)"
        } else {
            "MonoStart(none)"
        })
    }
}

/// The JSON names, for Go's field lookup.
const JSON_FIELDS: [&str; 27] = [
    "id",
    "group_id",
    "cli",
    "mode",
    "model",
    "effort",
    "review_scope",
    "prompt",
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

/// A decode target: a field and its Go type.
enum Slot<'a> {
    Str(&'a mut String),
    Bytes(&'a mut GoString),
    Int(&'a mut i64, &'static str),
    OptInt(&'a mut Option<i64>),
    Time(&'a mut DateTime<FixedOffset>),
    OptTime(&'a mut Option<DateTime<FixedOffset>>),
}

/// Go: decoding into a string field; `null` keeps the value.
fn set_string<T: From<String>>(dst: &mut T, raw: &RawValue) -> Result<(), TypeMismatch> {
    if let Some(v) = gojson::decode_string(raw)? {
        *dst = T::from(v);
    }
    Ok(())
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

impl Default for Session {
    fn default() -> Self {
        Session {
            id: String::new(),
            group_id: String::new(),
            cli: String::new(),
            mode: String::new(),
            model: String::new(),
            effort: String::new(),
            review_scope: String::new(),
            prompt: String::new(),
            prompt_preview: GoString::default(),
            prompt_hash: String::new(),
            status: String::new(),
            start_time: gojson::zero_time(),
            queued_at: None,
            queue_position: 0,
            end_time: None,
            exit_code: None,
            duration: String::new(),
            work_dir: String::new(),
            log_file: String::new(),
            output_bytes: 0,
            output_lines: 0,
            error_msg: String::new(),
            account: String::new(),
            pid: 0,
            pid_start: 0,
            owner_pid: 0,
            owner_pid_start: 0,
            start_mono: MonoStart::default(),
        }
    }
}

/// The inputs of Go's `NewQueued(cli, mode, model, effort, workdir, prompt,
/// reviewScope, groupID)`.
#[derive(Debug, Clone, Copy, Default)]
pub struct NewSession<'a> {
    pub cli: &'a str,
    pub mode: &'a str,
    pub model: &'a str,
    pub effort: &'a str,
    pub workdir: &'a str,
    pub prompt: &'a str,
    pub review_scope: &'a str,
    pub group_id: &'a str,
}

/// Go: `procinfo.StartNanos(pid)` with the error dropped.
fn pid_start_nanos(pid: i64) -> i64 {
    procinfo::start_nanos(proc_pid(pid)).unwrap_or(0)
}

/// A recorded PID as the OS sees it. A value outside `i32` can name no live
/// process, so it maps to 0, which procinfo treats as dead.
pub(crate) fn proc_pid(pid: i64) -> i32 {
    i32::try_from(pid).unwrap_or(0)
}

/// Go: `t.Sub(u)`, saturating at the `time.Duration` range.
pub fn sub_nanos(t: DateTime<FixedOffset>, u: DateTime<FixedOffset>) -> i64 {
    let d = t.signed_duration_since(u);
    d.num_nanoseconds().unwrap_or(if d > TimeDelta::zero() {
        i64::MAX
    } else {
        i64::MIN
    })
}

/// Go: `time.Duration.Round(m)`. Halfway values round away from zero.
fn round_duration(d: i64, m: i64) -> i64 {
    if m <= 0 {
        return d;
    }
    let less_than_half = |x: i64| (x as u64).wrapping_add(x as u64) < m as u64;
    let mut r = d % m;
    if d < 0 {
        r = -r;
        if less_than_half(r) {
            return d + r;
        }
        let d1 = d.wrapping_sub(m).wrapping_add(r);
        if d1 < d {
            return d1;
        }
        return i64::MIN;
    }
    if less_than_half(r) {
        return d - r;
    }
    let d1 = d.wrapping_add(m).wrapping_sub(r);
    if d1 > d {
        return d1;
    }
    i64::MAX
}

/// Go: `t.Sub(u)` for two monotonic readings, saturating.
fn mono_sub(t: Instant, u: Instant) -> i64 {
    match t.checked_duration_since(u) {
        Some(d) => i64::try_from(d.as_nanos()).unwrap_or(i64::MAX),
        None => i64::try_from(u.duration_since(t).as_nanos()).map_or(i64::MIN, |n| -n),
    }
}

/// Go: `d.Round(time.Second).String()`.
pub fn duration_text(nanos: i64) -> String {
    gostd::format_duration(round_duration(nanos, 1_000_000_000))
}

/// Go's `*PathError` text: `<op> <path>: <errno text>`.
pub(crate) fn path_error(op: &str, path: &Path, err: &io::Error) -> String {
    format!("{op} {}: {}", path.display(), gostd::os_error_text(err))
}

/// An I/O error that prints as Go's `*PathError` and still downcasts to
/// the `io::Error`.
pub(crate) fn io_error(op: &str, path: &Path, err: io::Error) -> anyhow::Error {
    let text = path_error(op, path, &err);
    anyhow::Error::new(err).context(text)
}

/// Go: `os.ReadFile`.
pub(crate) fn read_file(path: &Path) -> anyhow::Result<Vec<u8>> {
    let mut f = crate::gostd::open_file(path).map_err(|e| io_error("open", path, e))?;
    let mut data = Vec::new();
    f.read_to_end(&mut data)
        .map_err(|e| io_error("read", path, e))?;
    Ok(data)
}

/// Go: `os.CreateTemp(dir, pattern)` with a trailing `*`: the random part
/// is the decimal form of a random u32, and the file is created 0600.
fn create_temp(dir: &Path, prefix: &str) -> Result<(File, std::path::PathBuf), String> {
    let pattern = format!("{prefix}*");
    if prefix
        .bytes()
        .any(|b| b == b'/' || (cfg!(windows) && b == b'\\'))
    {
        return Err(format!(
            "createtemp {pattern}: pattern contains path separator"
        ));
    }
    for _ in 0..10_000 {
        let random = u32::from_le_bytes(
            uuid::Uuid::new_v4().as_bytes()[..4]
                .try_into()
                .expect("4 bytes"),
        );
        let path = dir.join(format!("{prefix}{random}"));
        let mut opts = OpenOptions::new();
        opts.read(true).write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        match opts.open(&path) {
            Ok(f) => return Ok((f, path)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(path_error("open", &path, &e)),
        }
    }
    Err(format!(
        "createtemp {}: file already exists",
        dir.join(pattern).display()
    ))
}

impl Session {
    /// Go: `NewQueued`. Creates a session in "queued" state — visible in the
    /// TUI while the process waits for a queue slot. Call [`Session::mark_running`]
    /// when the slot is acquired.
    pub fn new_queued(paths: &Paths, input: NewSession<'_>) -> anyhow::Result<Session> {
        Self::create(paths, input, "queued")
    }

    fn create(paths: &Paths, input: NewSession<'_>, status: &str) -> anyhow::Result<Session> {
        let dir = paths.sessions_dir();
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        if let Err(e) = builder.create(&dir) {
            return Err(anyhow!(
                "create session dir: {}",
                path_error("mkdir", &dir, &e)
            ));
        }

        let id = uuid::Uuid::new_v4().to_string();
        let log_file = dir.join(format!("{id}.log"));

        // Go slices the prompt at a byte offset; encoding/json then writes each
        // byte of a split rune as a \ufffd escape.
        let bytes = input.prompt.as_bytes();
        let preview = GoString::from_bytes(&bytes[..bytes.len().min(config::PROMPT_PREVIEW_LEN)]);

        let hash: String = Sha256::digest(input.prompt.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();

        let now = Now::read();
        let pid = i64::from(std::process::id());
        let mut s = Session {
            id,
            group_id: input.group_id.to_string(),
            cli: input.cli.to_string(),
            mode: input.mode.to_string(),
            model: input.model.to_string(),
            effort: input.effort.to_string(),
            review_scope: input.review_scope.to_string(),
            prompt: input.prompt.to_string(),
            prompt_preview: preview,
            prompt_hash: hash,
            status: status.to_string(),
            start_time: now.wall,
            start_mono: MonoStart::at(now),
            work_dir: input.workdir.to_string(),
            log_file: log_file.to_string_lossy().into_owned(),
            // The rival process's own PID until the subprocess starts — keeps
            // the reaper accurate during a potentially long queue wait (PID 0
            // would be treated as dead and the session insta-failed). pid_start
            // guards the PID against reuse after this process dies.
            pid,
            pid_start: pid_start_nanos(pid),
            // The owner never changes: as long as this rival process is alive
            // it is responsible for finalizing the session, and the reaper
            // must not.
            owner_pid: pid,
            owner_pid_start: pid_start_nanos(pid),
            ..Session::default()
        };
        if status == "queued" {
            s.queued_at = Some(now.wall);
        }

        s.save(paths)?;
        Ok(s)
    }

    /// Transitions a queued session to running. `start_time` is reset so
    /// `duration` measures runtime, not queue wait; `queued_at` preserves the wait.
    pub fn mark_running(&mut self, paths: &Paths) -> anyhow::Result<()> {
        self.mark_running_at(paths, Now::read())
    }

    pub(crate) fn mark_running_at(&mut self, paths: &Paths, now: Now) -> anyhow::Result<()> {
        self.status = "running".to_string();
        self.start_time = now.wall;
        self.start_mono = MonoStart::at(now);
        self.queue_position = 0;
        self.save(paths)
    }

    /// Updates the displayed queue position. No-op (and no file write /
    /// fsnotify event) when unchanged.
    pub fn set_queue_position(&mut self, paths: &Paths, pos: i64) -> anyhow::Result<()> {
        if self.queue_position == pos {
            return Ok(());
        }
        self.queue_position = pos;
        self.save(paths)
    }

    /// Go: `json.MarshalIndent(s, "", "  ")`.
    pub fn to_json(&self) -> anyhow::Result<Vec<u8>> {
        gojson::marshal_indent(self).map_err(|e| anyhow!("marshal session: {e}"))
    }

    /// Writes the session JSON atomically: a unique `<id>.json.tmp-*` file,
    /// then rename. The unique name matters because several processes save
    /// the same session (the owning rival, the TUI stop, the reaper, the Mac
    /// app); with one shared temp name, two concurrent writers could
    /// interleave into a partial file and rename it into place. The temp name
    /// never ends in ".json", so readers that glob or suffix-match "*.json"
    /// skip it.
    pub fn save(&self, paths: &Paths) -> anyhow::Result<()> {
        let dir = paths.sessions_dir();
        let data = self.to_json()?;

        let (mut f, tmp) = create_temp(&dir, &format!("{}.json.tmp-", self.id))
            .map_err(|e| anyhow!("create session tmp: {e}"))?;
        let werr = f.write_all(&data);
        drop(f);
        if let Err(e) = werr {
            let _ = fs::remove_file(&tmp);
            return Err(anyhow!(
                "write session tmp: {}",
                path_error("write", &tmp, &e)
            ));
        }
        let dst = dir.join(format!("{}.json", self.id));
        if let Err(e) = fs::rename(&tmp, &dst) {
            let _ = fs::remove_file(&tmp); // clean up orphaned temp file
            return Err(anyhow!(
                "rename session: rename {} {}: {}",
                tmp.display(),
                dst.display(),
                gostd::os_error_text(&e)
            ));
        }
        Ok(())
    }

    /// Go: `now.Sub(s.StartTime)` — monotonic while `start_time` is the
    /// reading this process took, wall time otherwise.
    fn elapsed(&self, now: Now) -> i64 {
        match self.start_mono.0 {
            Some(start) if start.wall == self.start_time => mono_sub(now.mono, start.mono),
            _ => sub_nanos(now.wall, self.start_time),
        }
    }

    /// Marks the session as completed.
    pub fn complete(
        &mut self,
        paths: &Paths,
        exit_code: i64,
        output_bytes: i64,
        output_lines: i64,
    ) -> anyhow::Result<()> {
        self.complete_at(paths, Now::read(), exit_code, output_bytes, output_lines)
    }

    pub(crate) fn complete_at(
        &mut self,
        paths: &Paths,
        now: Now,
        exit_code: i64,
        output_bytes: i64,
        output_lines: i64,
    ) -> anyhow::Result<()> {
        self.status = "completed".to_string();
        self.exit_code = Some(exit_code);
        self.end_time = Some(now.wall);
        self.duration = duration_text(self.elapsed(now));
        self.output_bytes = output_bytes;
        self.output_lines = output_lines;
        self.save(paths)
    }

    /// Marks the session as failed.
    pub fn fail(&mut self, paths: &Paths, exit_code: i64, err_msg: &str) -> anyhow::Result<()> {
        self.fail_at(paths, Now::read(), exit_code, err_msg)
    }

    pub(crate) fn fail_at(
        &mut self,
        paths: &Paths,
        now: Now,
        exit_code: i64,
        err_msg: &str,
    ) -> anyhow::Result<()> {
        self.status = "failed".to_string();
        self.exit_code = Some(exit_code);
        self.end_time = Some(now.wall);
        self.duration = duration_text(self.elapsed(now));
        self.error_msg = err_msg.to_string();
        self.save(paths)
    }

    /// Go: `json.Unmarshal(data, &s)` into a zero `Session`. Keys match
    /// exactly, else case-insensitively; duplicate keys assign in order;
    /// `null` leaves a value field alone and clears a pointer field; invalid
    /// UTF-8 in strings becomes U+FFFD; unknown keys are ignored.
    pub fn from_json(data: &[u8]) -> anyhow::Result<Session> {
        let mut s = Session::default();
        s.merge_json(data).map_err(anyhow::Error::msg)?;
        Ok(s)
    }

    /// Assigns each member in document order. A type mismatch is kept (the
    /// first one wins) while decoding goes on; a time error stops at once.
    fn merge_json(&mut self, data: &[u8]) -> Result<(), String> {
        let mut saved = None;
        for (key, raw) in gojson::decode_object(data, "session.Session")? {
            let Some(name) = gojson::match_field(&JSON_FIELDS, &key) else {
                continue;
            };
            let mismatch = match self.slot(name) {
                Slot::Str(dst) => set_string(dst, &raw).err().map(|m| (m, "string")),
                Slot::Bytes(dst) => set_string(dst, &raw).err().map(|m| (m, "string")),
                Slot::Int(dst, go_type) => gojson::decode_int(&raw)
                    .map(|v| {
                        if let Some(v) = v {
                            *dst = v;
                        }
                    })
                    .err()
                    .map(|m| (m, go_type)),
                Slot::OptInt(dst) => gojson::decode_int(&raw)
                    .map(|v| *dst = v)
                    .err()
                    .map(|m| (m, "int")),
                Slot::Time(dst) => {
                    if let Some(t) = gojson::decode_time(&raw)? {
                        *dst = t;
                    }
                    None
                }
                Slot::OptTime(dst) => {
                    *dst = gojson::decode_time(&raw)?;
                    None
                }
            };
            if let Some((TypeMismatch(value), go_type)) = mismatch
                && saved.is_none()
            {
                saved = Some(format!(
                    "json: cannot unmarshal {value} into Go struct field Session.{name} of type {go_type}"
                ));
            }
        }
        saved.map_or(Ok(()), Err)
    }

    fn slot(&mut self, name: &str) -> Slot<'_> {
        match name {
            "id" => Slot::Str(&mut self.id),
            "group_id" => Slot::Str(&mut self.group_id),
            "cli" => Slot::Str(&mut self.cli),
            "mode" => Slot::Str(&mut self.mode),
            "model" => Slot::Str(&mut self.model),
            "effort" => Slot::Str(&mut self.effort),
            "review_scope" => Slot::Str(&mut self.review_scope),
            "prompt" => Slot::Str(&mut self.prompt),
            "prompt_preview" => Slot::Bytes(&mut self.prompt_preview),
            "prompt_hash" => Slot::Str(&mut self.prompt_hash),
            "status" => Slot::Str(&mut self.status),
            "start_time" => Slot::Time(&mut self.start_time),
            "queued_at" => Slot::OptTime(&mut self.queued_at),
            "queue_position" => Slot::Int(&mut self.queue_position, "int"),
            "end_time" => Slot::OptTime(&mut self.end_time),
            "exit_code" => Slot::OptInt(&mut self.exit_code),
            "duration" => Slot::Str(&mut self.duration),
            "work_dir" => Slot::Str(&mut self.work_dir),
            "log_file" => Slot::Str(&mut self.log_file),
            "output_bytes" => Slot::Int(&mut self.output_bytes, "int64"),
            "output_lines" => Slot::Int(&mut self.output_lines, "int"),
            "error" => Slot::Str(&mut self.error_msg),
            "account" => Slot::Str(&mut self.account),
            "pid" => Slot::Int(&mut self.pid, "int"),
            "pid_start" => Slot::Int(&mut self.pid_start, "int64"),
            "owner_pid" => Slot::Int(&mut self.owner_pid, "int"),
            "owner_pid_start" => Slot::Int(&mut self.owner_pid_start, "int64"),
            _ => unreachable!("{name} is not in JSON_FIELDS"),
        }
    }

    /// Reads and returns all sessions, sorted newest first.
    pub fn load_all(paths: &Paths) -> Vec<Session> {
        // Go: filepath.Glob(dir/*.json), which returns names in sorted order.
        let Ok(entries) = fs::read_dir(paths.sessions_dir()) else {
            return Vec::new();
        };
        let mut names: Vec<_> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.file_name())
            .filter(|name| name.to_string_lossy().ends_with(".json"))
            .collect();
        names.sort();

        let mut sessions = Vec::new();
        for name in names {
            if is_temp_name(&name.to_string_lossy()) {
                continue;
            }
            let Ok(data) = fs::read(paths.sessions_dir().join(&name)) else {
                continue;
            };
            let Ok(s) = Session::from_json(&data) else {
                continue;
            };
            sessions.push(s);
        }
        sort_newest_first(&mut sessions);
        sessions
    }

    /// Reads one complete session record by id. A read error prints as Go's
    /// `*PathError` and downcasts to `io::Error`.
    pub fn load(paths: &Paths, id: &str) -> anyhow::Result<Session> {
        let path = crate::paths::clean(&paths.sessions_dir().join(format!("{id}.json")));
        Session::from_json(&read_file(&path)?)
    }

    /// Opens the session log file for appending.
    pub fn open_log(&self) -> io::Result<File> {
        let mut opts = OpenOptions::new();
        opts.create(true).append(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        opts.open(&self.log_file)
    }
}

/// Newest `start_time` first. Rust's stable sort keeps the name order for
/// ties; Go's `sort.Slice` may not for more than 12 sessions.
pub(crate) fn sort_newest_first<S: Borrow<Session>>(sessions: &mut [S]) {
    sessions.sort_by_key(|s| std::cmp::Reverse(s.borrow().start_time));
}

/// Reports whether name is a save temp file: the legacy `<id>.json.tmp` or
/// the unique `<id>.json.tmp-*`.
fn is_temp_name(name: &str) -> bool {
    name.contains(".json.tmp")
}

/// Reports whether name (a base name) is a finished session record, not a
/// temp file or anything else in the sessions dir.
pub fn is_session_file(name: &str) -> bool {
    name.ends_with(".json") && !is_temp_name(name)
}

/// Restores the order in which a grouped run requested its models.
/// `queued_at` is the creation timestamp and, unlike `start_time`, is not
/// reset as members are promoted to running. The deterministic fallbacks keep
/// legacy sessions stable when they do not have queue metadata.
pub fn sort_group_members<S: Borrow<Session>>(sessions: &mut [S]) {
    gostd::slice_stable(sessions, |a, b| {
        let (a, b) = (a.borrow(), b.borrow());
        let (rank_a, rank_b) = (group_mode_rank(&a.mode), group_mode_rank(&b.mode));
        if rank_a != rank_b {
            return rank_a < rank_b;
        }
        if let (Some(qa), Some(qb)) = (a.queued_at, b.queued_at)
            && qa != qb
        {
            return qa < qb;
        }
        let (rank_a, rank_b) = (group_model_rank(a), group_model_rank(b));
        if rank_a != rank_b {
            return rank_a < rank_b;
        }
        if a.start_time != b.start_time {
            return a.start_time < b.start_time;
        }
        a.id < b.id
    });
}

fn group_mode_rank(mode: &str) -> i32 {
    if mode == "consilium" { 1 } else { 0 }
}

fn group_model_rank(s: &Session) -> i32 {
    match config::engine_label(&s.cli, &s.model).as_str() {
        config::SOL_LABEL => 0, // read-compat: display of sessions recorded before Sol's removal
        "kimi-k3" => 1,
        config::CLAUDE_LABEL => 2,
        config::GROK_LABEL => 3,
        _ => 100,
    }
}
