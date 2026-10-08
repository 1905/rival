//! Queue ticket records.
//!
//! Go: `internal/queue/ticket.go`.

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::Path;

use anyhow::anyhow;
use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};

use crate::gostd;
use crate::json;
use crate::session::path_error;

pub const STATE_WAITING: &str = "waiting";
pub const STATE_RUNNING: &str = "running";

/// One queued review. Tickets live as JSON files in `~/.rival/queue/` named
/// `<unixnano>-<pid>-<id8>.json` so a plain filename sort yields FIFO order.
/// A ticket is written exactly twice in its life (created as waiting,
/// promoted to running) and removed once — there are no post-promotion
/// writes. The JSON keys keep the order and the omit rules of the Go
/// release's ticket.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ticket {
    #[serde(deserialize_with = "json::nullable")]
    pub id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    #[serde(deserialize_with = "json::nullable")]
    pub group_id: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(deserialize_with = "json::nullable")]
    pub session_ids: Vec<String>,
    #[serde(deserialize_with = "json::nullable")]
    pub mode: String,
    #[serde(deserialize_with = "json::nullable")]
    pub pid: i64,
    /// Owner process start time (Unix ns); guards against PID reuse.
    #[serde(skip_serializing_if = "is_zero")]
    #[serde(deserialize_with = "json::nullable")]
    pub pid_start: i64,
    #[serde(deserialize_with = "json::nullable")]
    pub state: String,
    /// `None` only in a damaged ticket: every writer sets it.
    #[serde(skip_serializing_if = "Option::is_none", with = "json::opt_time")]
    pub created_at: Option<DateTime<FixedOffset>>,
    #[serde(skip_serializing_if = "Option::is_none", with = "json::opt_time")]
    pub started_at: Option<DateTime<FixedOffset>>,
    #[serde(skip_serializing_if = "String::is_empty")]
    #[serde(deserialize_with = "json::nullable")]
    pub work_dir: String,
    /// Filename within the queue dir, not serialized.
    #[serde(skip)]
    #[serde(deserialize_with = "json::nullable")]
    pub(crate) file: String,
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

impl Ticket {
    /// Decodes a ticket. A missing key or `null` gives the field's default
    /// and unknown keys are ignored. Any error means the scanner skips the
    /// file.
    pub fn from_json(data: &[u8]) -> Result<Ticket, String> {
        json::decode(data).map_err(|e| e.to_string())
    }

    /// The ticket bytes: pretty JSON with a two-space indent.
    pub fn to_json(&self) -> anyhow::Result<Vec<u8>> {
        serde_json::to_vec_pretty(self).map_err(|e| anyhow!("marshal ticket: {e}"))
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
