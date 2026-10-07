//! The live stdout mirror of `rival run`. The provider's stdout lines go
//! through a bounded queue to a thread that owns the real stdout.
//!
//! Go wrote each line straight to `os.Stdout`. A reader that keeps the pipe
//! open but stops reading then blocked that write forever, past a cancel or
//! `RIVAL_RUN_TIMEOUT`, so the run never ended and kept its queue slot.
//! Here a full queue waits on the run's context instead: once the context is
//! done, the rest of the live copy is dropped (the session log keeps all of
//! it) and the run ends as usual.

use std::collections::VecDeque;
use std::io::{self, Write};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use rival_core::cancel::Context;
use rival_core::executor::PIPE_DRAIN_GRACE;
use rival_core::logging;

#[cfg(test)]
mod tests;

/// Queued bytes before a write waits for the stdout thread.
const MAX_QUEUED: usize = 1 << 20;

/// How often a waiting write checks the run's context.
const POLL: Duration = Duration::from_millis(50);

#[derive(Default)]
struct State {
    lines: VecDeque<Vec<u8>>,
    bytes: usize,
    /// No more lines will come.
    closed: bool,
    /// The context ended while the queue was full; lines are dropped.
    abandoned: bool,
    /// The stdout thread's write error, reported for every later line as a
    /// direct write would have.
    failed: Option<(io::ErrorKind, String)>,
    /// The stdout thread has returned.
    done: bool,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    cond: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The stdout thread and its queue.
pub(crate) struct LiveMirror {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl LiveMirror {
    /// Starts the thread that writes queued lines to `out`.
    pub(crate) fn spawn(mut out: Box<dyn Write + Send>) -> io::Result<LiveMirror> {
        let shared = Arc::new(Shared::default());
        let pump = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("rival-mirror".to_string())
            .spawn(move || {
                loop {
                    let line = {
                        let mut st = pump.lock();
                        while st.lines.is_empty() && !st.closed {
                            st = pump.cond.wait(st).unwrap_or_else(|e| e.into_inner());
                        }
                        let Some(line) = st.lines.pop_front() else {
                            break;
                        };
                        st.bytes -= line.len();
                        pump.cond.notify_all();
                        line
                    };
                    if let Err(e) = out.write_all(&line).and_then(|()| out.flush()) {
                        let mut st = pump.lock();
                        st.failed = Some((e.kind(), e.to_string()));
                        st.lines.clear();
                        st.bytes = 0;
                        break;
                    }
                }
                pump.lock().done = true;
                pump.cond.notify_all();
            })?;
        Ok(LiveMirror {
            shared,
            thread: Some(thread),
        })
    }

    /// The writer the provider's stdout lines go to during a run.
    pub(crate) fn sink<'a>(&'a self, ctx: &'a Context) -> Sink<'a> {
        Sink {
            shared: &self.shared,
            ctx,
        }
    }

    /// Ends the mirror. While `ctx` is live it waits until every queued line
    /// is written, so later output follows the live copy. Once `ctx` is done
    /// it waits at most [`PIPE_DRAIN_GRACE`], then leaves a stuck thread
    /// behind; the process exit ends it.
    pub(crate) fn finish(mut self, ctx: &Context) {
        let mut st = self.shared.lock();
        st.closed = true;
        self.shared.cond.notify_all();
        let mut grace = None;
        while !st.done {
            if grace.is_none() && ctx.is_done() {
                grace = Some(std::time::Instant::now() + PIPE_DRAIN_GRACE);
            }
            if grace.is_some_and(|g| std::time::Instant::now() >= g) {
                logging::warn()
                    .dur("grace", PIPE_DRAIN_GRACE)
                    .msg("stdout is not being read — leaving the live output");
                drop(st);
                drop(self.thread.take());
                return;
            }
            st = self
                .shared
                .cond
                .wait_timeout(st, POLL)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        drop(st);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Queues lines for the stdout thread. A write waits while the queue is
/// full, but never past the end of the run's context.
pub(crate) struct Sink<'a> {
    shared: &'a Shared,
    ctx: &'a Context,
}

impl Write for Sink<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut st = self.shared.lock();
        loop {
            if let Some((kind, text)) = &st.failed {
                return Err(io::Error::new(*kind, text.clone()));
            }
            if st.abandoned {
                return Ok(buf.len());
            }
            if st.lines.is_empty() || st.bytes + buf.len() <= MAX_QUEUED {
                st.bytes += buf.len();
                st.lines.push_back(buf.to_vec());
                self.shared.cond.notify_all();
                return Ok(buf.len());
            }
            if self.ctx.is_done() {
                st.abandoned = true;
                logging::warn().msg("stdout is not being read — dropping the live output");
                return Ok(buf.len());
            }
            st = self
                .shared
                .cond
                .wait_timeout(st, POLL)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
