//! Session directory watcher for the dashboard.
//!
//! Go: `internal/dashboard/watcher.go` (fsnotify). This port uses the
//! `notify` crate, std channels and [`crate::cancel::Context`]; there is no
//! async runtime.
//!
//! [`watch_sessions`] runs the initial scan on the caller's thread, sends the
//! first [`SessionEvent`], and then hands the directory watch to one worker
//! thread owned by the returned [`SessionWatcher`]. The worker stops when the
//! parent context is cancelled, when the event receiver is dropped, or when
//! the [`SessionWatcher`] is dropped. Dropping it also joins the worker, so
//! no thread or OS watch outlives it.

use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::anyhow;
use notify::event::{EventKind, ModifyKind, RenameMode};
use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};

use super::cache::Cache;
use crate::cancel::{CancelFunc, Context, ContextError};
use crate::logging;
use crate::paths::Paths;
use crate::session::{self, Session};

/// Sent when sessions change: the full list, newest first, prompt-free.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionEvent {
    pub sessions: Vec<Arc<Session>>,
}

/// Reports the initial session scan: `done` of `total` session files read.
/// The dashboard shows it as the startup loader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadProgress {
    pub done: usize,
    pub total: usize,
}

/// How long the worker waits for a file event before it checks the context.
const POLL: Duration = Duration::from_millis(50);
/// How long a send into a full event channel waits before it retries.
const SEND_RETRY: Duration = Duration::from_millis(10);

/// The running watch. Dropping it cancels the watch and joins the worker
/// thread, which closes the OS watch.
#[derive(Debug)]
pub struct SessionWatcher {
    cancel: CancelFunc,
    worker: Option<JoinHandle<()>>,
}

impl SessionWatcher {
    /// Cancels the watch and waits for the worker to exit.
    pub fn stop(self) {}

    /// Waits for the worker to exit without cancelling it first. It exits
    /// once the parent context is cancelled or the event receiver is gone.
    pub fn join(mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for SessionWatcher {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Watches the session directory of `paths` and sends events on changes.
///
/// `progress`, when given, gets [`LoadProgress`] messages during the initial
/// scan and is closed (dropped) once this function returns, on success or
/// error. Progress sends never block: a full channel drops the update,
/// because the next one (or the first [`SessionEvent`]) supersedes it.
///
/// The initial [`SessionEvent`] is sent before this returns; that send waits
/// for room in `events` until `ctx` is done, which returns
/// [`ContextError`] (downcastable from the error).
pub fn watch_sessions(
    ctx: &Context,
    paths: &Paths,
    events: SyncSender<SessionEvent>,
    progress: Option<SyncSender<LoadProgress>>,
) -> anyhow::Result<SessionWatcher> {
    let dir = paths.sessions_dir();
    create_dir_all(&dir)?;

    let (fs_tx, fs_rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(fs_tx)?;
    // On error the watcher drops here, as Go closes it.
    watcher.watch(&dir, RecursiveMode::NonRecursive)?;

    // One shared cache serves every reload below. It reparses only the files
    // whose size or mtime changed, instead of re-reading every session JSON
    // (prompts included) on each event.
    let cache = Cache::new(&dir);

    // Send initial state.
    let (sessions, _) = cache.load_with_progress(|done, total| {
        if let Some(progress) = &progress {
            let _ = progress.try_send(LoadProgress { done, total });
        }
    });
    drop(progress);
    match send_or_cancel(ctx, &events, SessionEvent { sessions }) {
        Send::Sent => {}
        Send::Cancelled(err) => return Err(err.into()),
        Send::Disconnected => return Err(anyhow!("session event receiver closed")),
    }

    let (ctx, cancel) = ctx.with_cancel();
    let worker = thread::Builder::new()
        .name("rival-session-watch".into())
        .spawn(move || {
            // The watcher lives exactly as long as this loop.
            let _watcher: RecommendedWatcher = watcher;
            run(&ctx, &cache, &fs_rx, &events);
        })
        .map_err(|e| anyhow!("spawn session watcher: {e}"))?;

    Ok(SessionWatcher {
        cancel,
        worker: Some(worker),
    })
}

/// Go: `os.MkdirAll(dir, 0700)`.
fn create_dir_all(dir: &Path) -> anyhow::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder
        .create(dir)
        .map_err(|e| anyhow!(session::path_error("mkdir", dir, &e)))
}

fn run(
    ctx: &Context,
    cache: &Cache,
    fs_rx: &Receiver<notify::Result<notify::Event>>,
    events: &SyncSender<SessionEvent>,
) {
    let mut filter = RefreshFilter::default();
    while !ctx.is_done() {
        let event = match fs_rx.recv_timeout(POLL) {
            Ok(Ok(event)) => event,
            Ok(Err(err)) => {
                logging::warn().err(err).msg("watcher error");
                continue;
            }
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let Some(update) = filter.handle(&event, cache) else {
            continue;
        };
        // A blocked send would stall this thread and leak it past
        // cancellation, because the bounded channel fills under log churn.
        if !matches!(send_or_cancel(ctx, events, update), Send::Sent) {
            return;
        }
    }
}

/// Decides which file events reload the cache and which reloads reach the
/// dashboard.
#[derive(Debug, Default)]
struct RefreshFilter {
    /// Starts at 0 although the initial scan already moved the cache's
    /// revision, as in Go: the first later log event therefore refreshes
    /// even when nothing changed.
    last_revision: u64,
}

impl RefreshFilter {
    fn handle(&mut self, event: &notify::Event, cache: &Cache) -> Option<SessionEvent> {
        let names = event_names(event)?;
        let is_json = names.iter().any(|p| {
            p.file_name()
                .is_some_and(|n| session::is_session_file(&n.to_string_lossy()))
        });
        let is_log = names.iter().any(|p| p.to_string_lossy().ends_with(".log"));
        if !is_json && !is_log {
            return None;
        }
        let (sessions, revision) = cache.load();
        // Log appends fire constantly during a run. When nothing the
        // dashboard displays changed, skip the redraw.
        if revision == self.last_revision && !is_json {
            return None;
        }
        self.last_revision = revision;
        Some(SessionEvent { sessions })
    }
}

/// The paths of an event Go's filter (`Write`, `Create` or `Remove`) would
/// see, or `None` for any other event. fsnotify reports a rename into the
/// directory as `Create` on the new name and ignores the old name; inotify
/// tells the two apart, FSEvents and Windows report both as `Name(Any)`.
fn event_names(event: &notify::Event) -> Option<&[std::path::PathBuf]> {
    match event.kind {
        EventKind::Create(_)
        | EventKind::Remove(_)
        | EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Any)
        | EventKind::Modify(ModifyKind::Name(RenameMode::To | RenameMode::Any)) => {
            Some(&event.paths)
        }
        // [from, to]: only the destination.
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => {
            Some(event.paths.last().map(std::slice::from_ref).unwrap_or(&[]))
        }
        _ => None,
    }
}

enum Send {
    Sent,
    Cancelled(ContextError),
    Disconnected,
}

/// Go: `select { case ch <- v: case <-ctx.Done(): }`. Retries a full
/// channel until it has room or `ctx` is done. A dropped receiver ends the
/// wait, since nothing could read the value.
fn send_or_cancel<T>(ctx: &Context, tx: &SyncSender<T>, mut value: T) -> Send {
    loop {
        match tx.try_send(value) {
            Ok(()) => return Send::Sent,
            Err(TrySendError::Disconnected(_)) => return Send::Disconnected,
            Err(TrySendError::Full(v)) => value = v,
        }
        if let Some(err) = ctx.wait_timeout(SEND_RETRY) {
            return Send::Cancelled(err);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, CreateKind, DataChange, MetadataKind, RemoveKind};
    use std::fs;
    use std::path::PathBuf;
    use std::time::Instant;

    const WAIT: Duration = Duration::from_secs(10);

    fn write_session(dir: &Path, id: &str) {
        let body = format!(
            "{{\n  \"id\": \"{id}\",\n  \"cli\": \"codex\",\n  \"mode\": \"review\",\n  \"model\": \"m\",\n  \"effort\": \"high\",\n  \"prompt\": \"secret\",\n  \"status\": \"running\",\n  \"start_time\": \"{}\"\n}}",
            chrono::Local::now().to_rfc3339()
        );
        // Write and rename, as Session::save does.
        let tmp = dir.join(format!("{id}.json.tmp-1"));
        fs::write(&tmp, body).unwrap();
        fs::rename(&tmp, dir.join(format!("{id}.json"))).unwrap();
    }

    fn paths(tmp: &Path) -> Paths {
        Paths::from_home(tmp)
    }

    fn event(kind: EventKind, path: &str) -> notify::Event {
        notify::Event::new(kind).add_path(PathBuf::from(path))
    }

    fn write_event(path: &str) -> notify::Event {
        event(
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            path,
        )
    }

    /// Receives the next event whose session ids match `want`, skipping
    /// stale refreshes that FSEvents may still deliver.
    fn recv_until(rx: &Receiver<SessionEvent>, want: &[&str]) -> SessionEvent {
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let ev = rx
                .recv_timeout(left)
                .unwrap_or_else(|e| panic!("no event with {want:?}: {e}"));
            let mut ids: Vec<_> = ev.sessions.iter().map(|s| s.id.as_str()).collect();
            ids.sort();
            if ids == want {
                return ev;
            }
        }
    }

    // Go: TestWatchSessionsClosesProgressOnEmptyDir. The progress channel is
    // closed once the initial scan is sent, also with no sessions at all.
    #[test]
    fn watch_sessions_closes_progress_on_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let (ctx, cancel) = Context::background().with_cancel();
        let (events_tx, events) = mpsc::sync_channel(1);
        let (progress_tx, progress) = mpsc::sync_channel(1);
        let watcher =
            watch_sessions(&ctx, &paths(tmp.path()), events_tx, Some(progress_tx)).unwrap();

        let ev = events.recv_timeout(WAIT).expect("no initial SessionEvent");
        assert!(ev.sessions.is_empty());
        assert_eq!(
            progress.recv_timeout(WAIT),
            Err(RecvTimeoutError::Disconnected),
            "progress not closed after the initial scan"
        );
        assert!(paths(tmp.path()).sessions_dir().is_dir(), "dir not created");
        cancel.cancel();
        watcher.join();
    }

    // The scan reports every 100 files and at the end, but never blocks: a
    // one-slot channel nobody reads keeps the first update and drops the rest.
    #[test]
    fn watch_sessions_progress_is_nonblocking() {
        let tmp = tempfile::tempdir().unwrap();
        let p = paths(tmp.path());
        fs::create_dir_all(p.sessions_dir()).unwrap();
        for i in 0..150 {
            write_session(&p.sessions_dir(), &format!("s{i:03}"));
        }
        let (events_tx, events) = mpsc::sync_channel(1);
        let (progress_tx, progress) = mpsc::sync_channel(1);
        let watcher =
            watch_sessions(&Context::background(), &p, events_tx, Some(progress_tx)).unwrap();

        let ev = events.recv_timeout(WAIT).unwrap();
        assert_eq!(ev.sessions.len(), 150);
        assert!(ev.sessions.iter().all(|s| s.prompt.is_empty()));
        let got: Vec<_> = progress.iter().collect();
        assert_eq!(
            got,
            [LoadProgress {
                done: 100,
                total: 150
            }]
        );
        drop(watcher);
    }

    #[test]
    fn json_change_triggers_a_refresh() {
        let tmp = tempfile::tempdir().unwrap();
        let p = paths(tmp.path());
        let (events_tx, events) = mpsc::sync_channel(10);
        let watcher = watch_sessions(&Context::background(), &p, events_tx, None).unwrap();
        assert!(events.recv_timeout(WAIT).unwrap().sessions.is_empty());

        write_session(&p.sessions_dir(), "a");
        let ev = recv_until(&events, &["a"]);
        assert!(
            ev.sessions[0].prompt.is_empty(),
            "event retained the prompt"
        );

        fs::remove_file(p.sessions_dir().join("a.json")).unwrap();
        recv_until(&events, &[]);
        drop(watcher);
    }

    #[test]
    fn cancelling_the_parent_stops_and_joins_the_worker() {
        let tmp = tempfile::tempdir().unwrap();
        let (ctx, cancel) = Context::background().with_cancel();
        let (events_tx, events) = mpsc::sync_channel(1);
        let watcher = watch_sessions(&ctx, &paths(tmp.path()), events_tx, None).unwrap();
        events.recv_timeout(WAIT).unwrap();

        cancel.cancel();
        let start = Instant::now();
        watcher.join();
        assert!(start.elapsed() < WAIT);
        // The worker owned the only sender; it is gone with the worker.
        assert_eq!(
            events.recv_timeout(WAIT),
            Err(RecvTimeoutError::Disconnected)
        );
    }

    #[test]
    fn dropping_the_watcher_stops_the_worker() {
        let tmp = tempfile::tempdir().unwrap();
        let (events_tx, events) = mpsc::sync_channel(1);
        let watcher =
            watch_sessions(&Context::background(), &paths(tmp.path()), events_tx, None).unwrap();
        events.recv_timeout(WAIT).unwrap();
        drop(watcher);
        assert_eq!(
            events.recv_timeout(WAIT),
            Err(RecvTimeoutError::Disconnected)
        );
    }

    // A worker stuck on a full channel nobody reads still stops on drop.
    #[test]
    fn full_event_channel_does_not_block_shutdown() {
        let tmp = tempfile::tempdir().unwrap();
        let p = paths(tmp.path());
        let (events_tx, events) = mpsc::sync_channel(1);
        let watcher = watch_sessions(&Context::background(), &p, events_tx, None).unwrap();
        // The initial event fills the one slot; nobody reads it.
        for i in 0..5 {
            write_session(&p.sessions_dir(), &format!("s{i}"));
        }
        thread::sleep(Duration::from_millis(300));
        let start = Instant::now();
        drop(watcher);
        assert!(start.elapsed() < WAIT);
        drop(events);
    }

    #[test]
    fn initial_send_returns_on_cancel_and_closes_progress() {
        let tmp = tempfile::tempdir().unwrap();
        let (ctx, cancel) = Context::background().with_cancel();
        // A rendezvous channel nobody reads: the initial send cannot proceed.
        let (events_tx, _events) = mpsc::sync_channel(0);
        let (progress_tx, progress) = mpsc::sync_channel(1);
        let canceller = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            cancel.cancel();
        });
        let err = watch_sessions(&ctx, &paths(tmp.path()), events_tx, Some(progress_tx))
            .expect_err("initial send must stop on cancel");
        canceller.join().unwrap();
        assert_eq!(
            err.downcast_ref::<ContextError>(),
            Some(&ContextError::Canceled)
        );
        assert_eq!(err.to_string(), "context canceled");
        assert_eq!(progress.recv(), Err(mpsc::RecvError));
    }

    #[test]
    fn initial_send_to_a_dropped_receiver_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let (events_tx, events) = mpsc::sync_channel(1);
        drop(events);
        let err = watch_sessions(&Context::background(), &paths(tmp.path()), events_tx, None)
            .expect_err("no receiver");
        assert_eq!(err.to_string(), "session event receiver closed");
    }

    #[test]
    fn setup_failure_returns_the_error_and_closes_progress() {
        let tmp = tempfile::tempdir().unwrap();
        let p = paths(tmp.path());
        // A file where the .rival directory must go.
        fs::write(&p.root, "x").unwrap();
        let (events_tx, events) = mpsc::sync_channel(1);
        let (progress_tx, progress) = mpsc::sync_channel(1);
        let err = watch_sessions(&Context::background(), &p, events_tx, Some(progress_tx))
            .expect_err("mkdir must fail");
        assert!(
            err.to_string()
                .starts_with(&format!("mkdir {}: ", p.sessions_dir().display())),
            "{err}"
        );
        assert_eq!(progress.recv(), Err(mpsc::RecvError));
        assert_eq!(events.recv(), Err(mpsc::RecvError));
    }

    #[test]
    fn send_or_cancel_waits_for_room_and_stops_on_cancel() {
        let (ctx, cancel) = Context::background().with_cancel();
        let (tx, rx) = mpsc::sync_channel(1);
        assert!(matches!(send_or_cancel(&ctx, &tx, 1), Send::Sent));

        // Full: a reader frees the slot after a while.
        let reader = thread::spawn(move || {
            thread::sleep(Duration::from_millis(30));
            let first = rx.recv().unwrap();
            (first, rx)
        });
        assert!(matches!(send_or_cancel(&ctx, &tx, 2), Send::Sent));
        let (first, rx) = reader.join().unwrap();
        assert_eq!(first, 1);

        // Full and nobody reads: cancel ends the wait.
        let canceller = thread::spawn(move || {
            thread::sleep(Duration::from_millis(30));
            cancel.cancel();
        });
        assert!(matches!(
            send_or_cancel(&ctx, &tx, 3),
            Send::Cancelled(ContextError::Canceled)
        ));
        canceller.join().unwrap();

        drop(rx);
        let fresh = Context::background();
        assert!(matches!(send_or_cancel(&fresh, &tx, 4), Send::Disconnected));
    }

    #[test]
    fn event_names_follow_the_go_filter() {
        let cases = [
            (EventKind::Create(CreateKind::File), true),
            (EventKind::Remove(RemoveKind::File), true),
            (EventKind::Modify(ModifyKind::Data(DataChange::Any)), true),
            (EventKind::Modify(ModifyKind::Any), true),
            (EventKind::Modify(ModifyKind::Name(RenameMode::To)), true),
            (EventKind::Modify(ModifyKind::Name(RenameMode::Any)), true),
            (EventKind::Modify(ModifyKind::Name(RenameMode::From)), false),
            (
                EventKind::Modify(ModifyKind::Metadata(MetadataKind::Permissions)),
                false,
            ),
            (EventKind::Access(AccessKind::Any), false),
            (EventKind::Other, false),
            (EventKind::Any, false),
        ];
        for (kind, want) in cases {
            let ev = event(kind, "/s/a.json");
            assert_eq!(event_names(&ev).is_some(), want, "{kind:?}");
        }

        let both = notify::Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
            .add_path(PathBuf::from("/s/a.json"))
            .add_path(PathBuf::from("/s/a.txt"));
        assert_eq!(event_names(&both), Some(&[PathBuf::from("/s/a.txt")][..]));
    }

    #[test]
    fn refresh_filter_follows_go() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write_session(dir, "a");
        let cache = Cache::new(dir);
        // The initial snapshot, as watch_sessions sends it.
        let (_, initial) = cache.load();
        assert_eq!(initial, 1);
        let mut filter = RefreshFilter::default();
        let log = dir.join("a.log").to_string_lossy().into_owned();
        let json = dir.join("a.json").to_string_lossy().into_owned();

        // Go quirk: last_revision starts at 0, so the first log event after
        // the initial snapshot refreshes although nothing changed.
        let ev = filter
            .handle(&write_event(&log), &cache)
            .expect("first log refresh");
        assert_eq!(ev.sessions.len(), 1);
        // Later log appends with no session change are suppressed.
        assert!(filter.handle(&write_event(&log), &cache).is_none());
        // A JSON event always refreshes, changed or not.
        assert!(filter.handle(&write_event(&json), &cache).is_some());
        // Temp files and unrelated names never reload.
        let tmp_name = dir.join("a.json.tmp-9").to_string_lossy().into_owned();
        assert!(filter.handle(&write_event(&tmp_name), &cache).is_none());
        assert!(
            filter
                .handle(&write_event("/s/notes.txt"), &cache)
                .is_none()
        );
        // A chmod on a JSON file is outside Go's filter.
        let chmod = event(
            EventKind::Modify(ModifyKind::Metadata(MetadataKind::Permissions)),
            &json,
        );
        assert!(filter.handle(&chmod, &cache).is_none());

        // A log event after a real change refreshes once, then is suppressed.
        thread::sleep(Duration::from_millis(10));
        write_session(dir, "b");
        let ev = filter.handle(&write_event(&log), &cache).expect("changed");
        assert_eq!(ev.sessions.len(), 2);
        assert!(filter.handle(&write_event(&log), &cache).is_none());
    }
}
