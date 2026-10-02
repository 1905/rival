//! Queue ticket records.
//!
//! Go: `internal/queue/ticket.go`.

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::Path;

use anyhow::anyhow;
use chrono::{DateTime, FixedOffset};
use serde::Serialize;
use serde_json::value::RawValue;

use crate::gojson::{self, TypeMismatch};
use crate::gostd;
use crate::session::path_error;

pub const STATE_WAITING: &str = "waiting";
pub const STATE_RUNNING: &str = "running";

/// One queued review. Tickets live as JSON files in `~/.rival/queue/` named
/// `<unixnano>-<pid>-<id8>.json` so a plain filename sort yields FIFO order.
/// A ticket is written exactly twice in its life (created as waiting,
/// promoted to running) and removed once — there are no post-promotion
/// writes. Field order and `omitempty` rules match the Go struct, so the JSON
/// bytes match too. Go `int` fields are `i64`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Ticket {
    pub id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub group_id: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub session_ids: Vec<String>,
    pub mode: String,
    pub pid: i64,
    /// Owner process start time (Unix ns); guards against PID reuse.
    #[serde(skip_serializing_if = "is_zero")]
    pub pid_start: i64,
    pub state: String,
    #[serde(with = "gojson::time")]
    pub created_at: DateTime<FixedOffset>,
    #[serde(skip_serializing_if = "Option::is_none", with = "gojson::opt_time")]
    pub started_at: Option<DateTime<FixedOffset>>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub work_dir: String,
    /// Filename within the queue dir, not serialized.
    #[serde(skip)]
    pub(crate) file: String,
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

impl Default for Ticket {
    fn default() -> Self {
        Ticket {
            id: String::new(),
            group_id: String::new(),
            session_ids: Vec::new(),
            mode: String::new(),
            pid: 0,
            pid_start: 0,
            state: String::new(),
            created_at: gojson::zero_time(),
            started_at: None,
            work_dir: String::new(),
            file: String::new(),
        }
    }
}

/// The JSON names, for Go's field lookup.
const JSON_FIELDS: [&str; 10] = [
    "id",
    "group_id",
    "session_ids",
    "mode",
    "pid",
    "pid_start",
    "state",
    "created_at",
    "started_at",
    "work_dir",
];

/// Go: decoding into a string field; `null` keeps the value.
fn set_string(dst: &mut String, raw: &RawValue) -> Result<(), TypeMismatch> {
    if let Some(v) = gojson::decode_string(raw)? {
        *dst = v;
    }
    Ok(())
}

fn set_int(dst: &mut i64, raw: &RawValue) -> Result<(), TypeMismatch> {
    if let Some(v) = gojson::decode_int(raw)? {
        *dst = v;
    }
    Ok(())
}

/// Go: decoding into a `[]string` field, which reuses the slice it already
/// holds. A duplicate key decodes into the earlier array's elements, and
/// truncated elements stay in the backing array until a later, longer array
/// exposes them again. `null` makes the slice nil; `[]` makes it a fresh
/// empty one; a `null` element leaves its slot as it was.
#[derive(Default)]
struct GoStrings {
    /// Every element decoded since the slice was last cleared; the slice is
    /// the first `len` of them.
    backing: Vec<String>,
    len: usize,
}

impl GoStrings {
    /// A mismatch names the Go type it hit: the element's or the slice's.
    fn decode(&mut self, raw: &RawValue) -> Result<(), (TypeMismatch, &'static str)> {
        let text = raw.get();
        match text.as_bytes().first() {
            Some(b'n') => {
                *self = GoStrings::default();
                Ok(())
            }
            Some(b'[') => {
                let items: Vec<Box<RawValue>> = serde_json::from_str(text)
                    .map_err(|_| (TypeMismatch("array".to_string()), "[]string"))?;
                let mut first = None;
                for (i, item) in items.iter().enumerate() {
                    // Go grows only once every slot below capacity is exposed,
                    // so the allocator's capacity never hides an element: a
                    // new slot is simply zero.
                    if i == self.backing.len() {
                        self.backing.push(String::new());
                    }
                    self.len = self.len.max(i + 1);
                    match gojson::decode_string(item) {
                        Ok(Some(v)) => self.backing[i] = v,
                        Ok(None) => {}
                        Err(m) => {
                            first.get_or_insert((m, "string"));
                        }
                    }
                }
                self.len = self.len.min(items.len());
                if items.is_empty() {
                    *self = GoStrings::default();
                }
                first.map_or(Ok(()), Err)
            }
            first => {
                let value = match first {
                    Some(b'"') => "string",
                    Some(b'{') => "object",
                    Some(b't' | b'f') => "bool",
                    _ => "number",
                };
                Err((TypeMismatch(value.to_string()), "[]string"))
            }
        }
    }

    fn into_vec(mut self) -> Vec<String> {
        self.backing.truncate(self.len);
        self.backing
    }
}

impl Ticket {
    /// Go: `json.Unmarshal(data, &t)` into a zero `Ticket`. Keys match
    /// exactly, else case-insensitively; duplicate keys assign in order;
    /// unknown keys are ignored. Any error means the scanner skips the file.
    pub fn from_json(data: &[u8]) -> Result<Ticket, String> {
        let mut t = Ticket::default();
        let mut session_ids = GoStrings::default();
        let mut saved = None;
        for (key, raw) in gojson::decode_object(data, "queue.Ticket")? {
            let Some(name) = gojson::match_field(&JSON_FIELDS, &key) else {
                continue;
            };
            let mismatch = match name {
                "id" => set_string(&mut t.id, &raw).err().map(|m| (m, "string")),
                "group_id" => set_string(&mut t.group_id, &raw)
                    .err()
                    .map(|m| (m, "string")),
                "session_ids" => session_ids.decode(&raw).err(),
                "mode" => set_string(&mut t.mode, &raw).err().map(|m| (m, "string")),
                "pid" => set_int(&mut t.pid, &raw).err().map(|m| (m, "int")),
                "pid_start" => set_int(&mut t.pid_start, &raw).err().map(|m| (m, "int64")),
                "state" => set_string(&mut t.state, &raw).err().map(|m| (m, "string")),
                "created_at" => {
                    if let Some(v) = gojson::decode_time(&raw)? {
                        t.created_at = v;
                    }
                    None
                }
                "started_at" => {
                    t.started_at = gojson::decode_time(&raw)?;
                    None
                }
                "work_dir" => set_string(&mut t.work_dir, &raw)
                    .err()
                    .map(|m| (m, "string")),
                _ => unreachable!("{name} is not in JSON_FIELDS"),
            };
            if let Some((TypeMismatch(value), go_type)) = mismatch
                && saved.is_none()
            {
                saved = Some(format!(
                    "json: cannot unmarshal {value} into Go struct field Ticket.{name} of type {go_type}"
                ));
            }
        }
        t.session_ids = session_ids.into_vec();
        saved.map_or(Ok(t), Err)
    }

    /// Go: `json.MarshalIndent(t, "", "  ")`.
    pub fn to_json(&self) -> anyhow::Result<Vec<u8>> {
        gojson::marshal_indent(self).map_err(|e| anyhow!("marshal ticket: {e}"))
    }
}

/// Persists a ticket atomically (`<file>.tmp` + rename), matching the
/// session save pattern so scanners never see a half-written file. Unlike
/// a session save, the temp name is fixed: only the owning process writes
/// its ticket.
pub(crate) fn write_ticket(dir: &Path, t: &Ticket) -> anyhow::Result<()> {
    let data = t.to_json()?;
    let tmp = dir.join(format!("{}.tmp", t.file));
    let fin = dir.join(&t.file);
    // Go: os.WriteFile(tmp, data, 0600).
    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let written = opts
        .open(&tmp)
        .map_err(|e| path_error("open", &tmp, &e))
        .and_then(|mut f| {
            f.write_all(&data)
                .map_err(|e| path_error("write", &tmp, &e))
        });
    if let Err(e) = written {
        return Err(anyhow!("write ticket tmp: {e}"));
    }
    if let Err(e) = fs::rename(&tmp, &fin) {
        let _ = fs::remove_file(&tmp);
        return Err(anyhow!(
            "rename ticket: rename {} {}: {}",
            tmp.display(),
            fin.display(),
            gostd::os_error_text(&e)
        ));
    }
    Ok(())
}

/// Go: `t.UnixNano()`, wrapping outside the representable range as Go does.
fn unix_nano(t: &DateTime<FixedOffset>) -> i64 {
    t.timestamp()
        .wrapping_mul(1_000_000_000)
        .wrapping_add(i64::from(t.timestamp_subsec_nanos()))
}

/// `<unixnano>-<pid>-<id8>.json`. The nano part is zero-padded so a
/// lexicographic filename sort is chronological FIFO.
///
/// Go slices the first 8 bytes of the id; a non-ASCII id cut inside a rune
/// keeps its stray bytes there, which a Rust `String` cannot hold, so they
/// become U+FFFD. Rival's ids are UUIDs.
pub(crate) fn ticket_filename(now: &DateTime<FixedOffset>, pid: i64, id: &str) -> String {
    let short = if id.len() > 8 {
        String::from_utf8_lossy(&id.as_bytes()[..8])
    } else {
        id.into()
    };
    format!("{:019}-{pid}-{short}.json", unix_nano(now))
}
