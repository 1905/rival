//! Incremental session summary cache.
//!
//! Go: `internal/sessionview/cache.go`.

use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::session::summary::load_summary_file;
use crate::session::{self, Session};

/// How many session files pass between two progress calls.
pub const PROGRESS_EVERY: usize = 100;

struct CachedSession {
    size: i64,
    mod_time: i64,
    session: Arc<Session>,
}

#[derive(Default)]
struct State {
    /// Keyed by file name (`<id>.json`).
    files: BTreeMap<OsString, CachedSession>,
    revision: u64,
}

/// Keeps the TUI responsive without changing the session storage format used
/// by the CLI. Only files whose size or mtime changed are reparsed, and
/// parsed summaries never retain full prompts. `Send + Sync`; every method
/// takes the internal lock.
pub struct Cache {
    dir: PathBuf,
    state: Mutex<State>,
}

impl Cache {
    /// A cache over the session directory `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> Cache {
        Cache {
            dir: dir.into(),
            state: Mutex::new(State::default()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Returns every cached session, newest first, plus a revision counter.
    /// The revision increments only when this call observes a file that was
    /// added, removed, or changed by size or mtime and parsed. Two calls with
    /// no file change return the same number, so a caller can skip redundant
    /// work.
    pub fn load(&self) -> (Vec<Arc<Session>>, u64) {
        self.load_with_progress(|_, _| {})
    }

    /// [`Cache::load`] plus a progress callback. `progress` gets
    /// `(done, total)` over the session files in the directory: every
    /// [`PROGRESS_EVERY`] files and once more at the end. It runs on the
    /// caller's thread with the cache lock held, so it must not call back
    /// into the cache. With no session files it is never called.
    pub fn load_with_progress(
        &self,
        mut progress: impl FnMut(usize, usize),
    ) -> (Vec<Arc<Session>>, u64) {
        let mut st = self.lock();

        // Go: os.ReadDir, which sorts by name and fails on any read error.
        let entries =
            match fs::read_dir(&self.dir).and_then(|rd| rd.collect::<io::Result<Vec<_>>>()) {
                Ok(entries) => entries,
                // Go returns nil here and keeps the cache and revision as they are.
                Err(e) if e.kind() == io::ErrorKind::NotFound => return (Vec::new(), st.revision),
                Err(_) => return (cached_session_values(&st.files), st.revision),
            };

        // Filter first, so total is known before the first progress call.
        let mut files: Vec<_> = entries
            .into_iter()
            .filter(|entry| {
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                !is_dir && session::is_session_file(&entry.file_name().to_string_lossy())
            })
            .collect();
        files.sort_by_key(|entry| entry.file_name());

        let total = files.len();
        let mut seen = HashSet::with_capacity(total);
        let mut changed = false;
        for (i, entry) in files.iter().enumerate() {
            if i > 0 && i % PROGRESS_EVERY == 0 {
                progress(i, total);
            }
            let name = entry.file_name();
            seen.insert(name.clone());

            // Go: DirEntry.Info, an lstat.
            let Ok(info) = entry.metadata() else {
                continue;
            };
            let size = info.len() as i64;
            let mod_time = info.modified().map(unix_nanos).unwrap_or(0);
            if let Some(cached) = st.files.get(&name)
                && cached.size == size
                && cached.mod_time == mod_time
            {
                continue;
            }

            // A read or parse failure keeps the previous summary, if any.
            let Ok(s) = load_summary_file(&self.dir.join(&name), size) else {
                continue;
            };
            st.files.insert(
                name,
                CachedSession {
                    size,
                    mod_time,
                    session: Arc::new(s),
                },
            );
            changed = true;
        }

        if total > 0 {
            progress(total, total);
        }

        let before = st.files.len();
        st.files.retain(|name, _| seen.contains(name));
        if st.files.len() != before {
            changed = true;
        }
        if changed {
            st.revision += 1;
        }

        (cached_session_values(&st.files), st.revision)
    }

    /// Returns one cached session by id, or `None` when it is not cached.
    pub fn get(&self, id: &str) -> Option<Arc<Session>> {
        self.lock()
            .files
            .get(&OsString::from(format!("{id}.json")))
            .map(|cached| Arc::clone(&cached.session))
    }
}

/// Newest `start_time` first. Go iterates a map and uses the unstable
/// `sort.Slice`, so equal start times come in no fixed order there; here
/// they keep file-name order.
fn cached_session_values(files: &BTreeMap<OsString, CachedSession>) -> Vec<Arc<Session>> {
    let mut sessions: Vec<_> = files.values().map(|c| Arc::clone(&c.session)).collect();
    session::sort_newest_first(&mut sessions);
    sessions
}

/// Go: `ModTime().UnixNano()`.
fn unix_nanos(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_nanos()).unwrap_or(i64::MAX),
        Err(e) => i64::try_from(e.duration().as_nanos()).map_or(i64::MIN, |n| -n),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, FixedOffset, Local, SecondsFormat, TimeDelta};
    use std::path::Path;
    use std::thread::sleep;
    use std::time::Duration;

    /// Writes a minimal session JSON file and returns its path.
    fn write_session(dir: &Path, id: &str, status: &str, start: DateTime<FixedOffset>) -> PathBuf {
        let path = dir.join(format!("{id}.json"));
        let body = format!(
            r#"{{
  "id": "{id}",
  "cli": "codex",
  "model": "gpt-5.6-sol",
  "mode": "review",
  "status": "{status}",
  "effort": "high",
  "work_dir": "/tmp",
  "prompt": "a long stored prompt that summaries drop",
  "start_time": "{}"
}}"#,
            start.to_rfc3339_opts(SecondsFormat::Nanos, true)
        );
        fs::write(&path, body).unwrap();
        path
    }

    fn now() -> DateTime<FixedOffset> {
        Local::now().fixed_offset()
    }

    fn ids(sessions: &[Arc<Session>]) -> Vec<&str> {
        sessions.iter().map(|s| s.id.as_str()).collect()
    }

    // Go: TestCacheLoadsAllSessionsNewestFirst.
    #[test]
    fn cache_loads_all_sessions_newest_first() {
        let tmp = tempfile::tempdir().unwrap();
        let now = now();
        write_session(tmp.path(), "older", "completed", now - TimeDelta::hours(1));
        write_session(tmp.path(), "newer", "completed", now);

        let (sessions, rev) = Cache::new(tmp.path()).load();
        assert_eq!(ids(&sessions), ["newer", "older"]);
        assert_ne!(rev, 0, "revision stayed 0 after the initial load");
        assert!(
            sessions.iter().all(|s| s.prompt.is_empty()),
            "summaries retained the prompt"
        );
    }

    // Go: TestCacheRevisionOnlyMovesOnChange.
    #[test]
    fn cache_revision_only_moves_on_change() {
        let tmp = tempfile::tempdir().unwrap();
        write_session(tmp.path(), "a", "completed", now());
        let cache = Cache::new(tmp.path());

        let (_, first) = cache.load();
        let (_, second) = cache.load();
        assert_eq!(first, second, "revision moved without a file change");

        // A rewrite with new content and a new mtime must bump the revision.
        sleep(Duration::from_millis(10));
        write_session(tmp.path(), "a", "failed", now());
        let (sessions, third) = cache.load();
        assert_ne!(
            third, second,
            "revision did not move after the file changed"
        );
        assert_eq!(sessions.len(), 1);
        assert_eq!(
            sessions[0].status, "failed",
            "cache did not reparse the changed file"
        );
    }

    // Go: TestCacheDropsDeletedFiles.
    #[test]
    fn cache_drops_deleted_files() {
        let tmp = tempfile::tempdir().unwrap();
        write_session(tmp.path(), "a", "completed", now());
        let path = write_session(tmp.path(), "b", "completed", now());
        let cache = Cache::new(tmp.path());

        let (sessions, first) = cache.load();
        assert_eq!(sessions.len(), 2);
        fs::remove_file(&path).unwrap();
        let (sessions, second) = cache.load();
        assert_eq!(ids(&sessions), ["a"], "deleted session still cached");
        assert_ne!(first, second, "an observed removal must move the revision");
        assert!(cache.get("b").is_none(), "get returned a deleted session");
    }

    // Go: TestCacheSkipsUnparsableFileAndTempFiles.
    #[test]
    fn cache_skips_unparsable_file_and_temp_files() {
        let tmp = tempfile::tempdir().unwrap();
        write_session(tmp.path(), "good", "completed", now());
        fs::write(tmp.path().join("broken.json"), "{not json").unwrap();
        fs::write(tmp.path().join("partial.json.tmp"), "{}").unwrap();
        fs::create_dir(tmp.path().join("dir.json")).unwrap();

        let (sessions, _) = Cache::new(tmp.path()).load();
        assert_eq!(ids(&sessions), ["good"]);
    }

    // Go: TestCacheOnAbsentDirectoryReturnsNil.
    #[test]
    fn cache_on_absent_directory_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let (sessions, rev) = Cache::new(tmp.path().join("missing")).load();
        assert!(sessions.is_empty());
        assert_eq!(rev, 0);
    }

    // Go: TestCacheGetReturnsCachedSession.
    #[test]
    fn cache_get_returns_cached_session() {
        let tmp = tempfile::tempdir().unwrap();
        write_session(tmp.path(), "wanted", "running", now());
        let cache = Cache::new(tmp.path());
        cache.load();

        let got = cache.get("wanted").expect("wanted is cached");
        assert_eq!(got.id, "wanted");
        assert!(got.prompt.is_empty());
        assert!(cache.get("absent").is_none());
    }

    // Go: TestCacheProgressIsThrottledAndReachesTotal.
    #[test]
    fn cache_progress_is_throttled_and_reaches_total() {
        let tmp = tempfile::tempdir().unwrap();
        const N: usize = 250;
        for i in 0..N {
            write_session(tmp.path(), &format!("s{i:03}"), "completed", now());
        }
        let mut calls = Vec::new();
        let (sessions, _) =
            Cache::new(tmp.path()).load_with_progress(|done, total| calls.push((done, total)));
        assert_eq!(sessions.len(), N);
        assert_eq!(calls, [(100, N), (200, N), (N, N)]);
    }

    // Go: TestCacheProgressSilentOnEmptyDir.
    #[test]
    fn cache_progress_silent_on_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let mut called = false;
        Cache::new(tmp.path()).load_with_progress(|_, _| called = true);
        assert!(!called, "progress called with no session files");
    }

    #[test]
    fn cache_keeps_previous_summary_when_a_rewrite_fails_to_parse() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_session(tmp.path(), "a", "running", now());
        let cache = Cache::new(tmp.path());
        let (_, first) = cache.load();

        sleep(Duration::from_millis(10));
        fs::write(&path, "{torn").unwrap();
        let (sessions, second) = cache.load();
        assert_eq!(second, first, "a failed parse must not move the revision");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].status, "running");
        assert_eq!(cache.get("a").unwrap().status, "running");

        // The next good write is picked up, since the failed one was not cached.
        sleep(Duration::from_millis(10));
        write_session(tmp.path(), "a", "completed", now());
        let (sessions, third) = cache.load();
        assert_ne!(third, second);
        assert_eq!(sessions[0].status, "completed");
    }

    #[test]
    fn cache_unchanged_file_reuses_the_cached_summary() {
        let tmp = tempfile::tempdir().unwrap();
        write_session(tmp.path(), "a", "completed", now());
        let cache = Cache::new(tmp.path());
        let (first, _) = cache.load();
        let (second, _) = cache.load();
        assert!(
            Arc::ptr_eq(&first[0], &second[0]),
            "unchanged file was reparsed"
        );
    }

    // Go keeps the cache and revision when the directory disappears, and
    // returns nil; a later load with the directory back sees no change.
    #[test]
    fn cache_absent_directory_after_load_keeps_state() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("sessions");
        fs::create_dir(&dir).unwrap();
        write_session(&dir, "a", "completed", now());
        let cache = Cache::new(&dir);
        let (_, first) = cache.load();

        let moved = tmp.path().join("moved");
        fs::rename(&dir, &moved).unwrap();
        let (sessions, second) = cache.load();
        assert!(sessions.is_empty());
        assert_eq!(second, first);
        assert!(cache.get("a").is_some(), "Go keeps the cached entries");

        fs::rename(&moved, &dir).unwrap();
        let (sessions, third) = cache.load();
        assert_eq!(ids(&sessions), ["a"]);
        assert_eq!(third, first);
    }

    #[test]
    fn cache_unreadable_directory_returns_cached_values() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("not-a-dir");
        fs::write(&file, "x").unwrap();
        // ENOTDIR is not "not exist": Go returns the cached values.
        let (sessions, rev) = Cache::new(&file).load();
        assert!(sessions.is_empty());
        assert_eq!(rev, 0);
    }

    #[test]
    fn unix_nanos_matches_go() {
        assert_eq!(unix_nanos(UNIX_EPOCH + Duration::new(2, 5)), 2_000_000_005);
        assert_eq!(unix_nanos(UNIX_EPOCH - Duration::new(1, 0)), -1_000_000_000);
    }
}
