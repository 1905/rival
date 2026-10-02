//! Synchronous cancellation with Go `context` semantics.
//!
//! Go: `context.Background`, `WithCancel`, `WithDeadline`, `WithTimeout`. A
//! context is done once it or any ancestor is cancelled or its deadline
//! passes. A child's deadline is the earlier of its own and its parent's.
//! Cancelling a child never touches its parent. [`Context::wait_timeout`] is
//! Go's `select { case <-ctx.Done(): case <-time.After(d): }` and wakes as
//! soon as the context is done.

use std::fmt;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

/// Go: `context.Canceled` and `context.DeadlineExceeded`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextError {
    Canceled,
    DeadlineExceeded,
}

impl fmt::Display for ContextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ContextError::Canceled => "context canceled",
            ContextError::DeadlineExceeded => "context deadline exceeded",
        })
    }
}

impl std::error::Error for ContextError {}

#[derive(Default)]
struct State {
    err: Option<ContextError>,
    children: Vec<Weak<Inner>>,
}

struct Inner {
    state: Mutex<State>,
    cond: Condvar,
    /// The effective deadline: the earlier of this context's and its parent's.
    deadline: Option<Instant>,
    /// `false` only for [`Context::background`], which can never be done.
    cancellable: bool,
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Marks this context and every descendant done with `err`. The first
    /// error wins, as in Go. Deadlines expire lazily, so a cancel that
    /// arrives after the deadline has passed records `DeadlineExceeded`:
    /// Go's timer would already have fired.
    fn cancel(&self, err: ContextError) {
        let children = {
            let mut st = self.lock();
            if st.err.is_some() {
                return;
            }
            let lapsed = self.deadline.is_some_and(|d| Instant::now() >= d);
            let err = if lapsed {
                ContextError::DeadlineExceeded
            } else {
                err
            };
            st.err = Some(err);
            self.cond.notify_all();
            std::mem::take(&mut st.children)
        };
        let err = self.lock().err.expect("just set");
        for child in children.iter().filter_map(Weak::upgrade) {
            child.cancel(err);
        }
    }

    /// The error, expiring the deadline lazily.
    fn err(&self) -> Option<ContextError> {
        let err = self.lock().err;
        if err.is_some() {
            return err;
        }
        if self.deadline.is_some_and(|d| Instant::now() >= d) {
            self.cancel(ContextError::DeadlineExceeded);
            return self.lock().err;
        }
        None
    }
}

/// A cancellation scope. Cheap to clone; clones share state. `Send + Sync`,
/// so a waiter and a canceller may run on different threads.
#[derive(Clone)]
pub struct Context {
    inner: Arc<Inner>,
}

/// Go: `context.CancelFunc`. Cancels its context and all descendants with
/// [`ContextError::Canceled`]. Idempotent. Dropping it does NOT cancel; call
/// [`CancelFunc::cancel`] (Go's `defer cancel()`).
#[derive(Clone)]
pub struct CancelFunc {
    inner: Arc<Inner>,
}

impl CancelFunc {
    pub fn cancel(&self) {
        self.inner.cancel(ContextError::Canceled);
    }
}

impl fmt::Debug for Context {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Context")
            .field("err", &self.inner.lock().err)
            .field("deadline", &self.inner.deadline)
            .finish()
    }
}

impl fmt::Debug for CancelFunc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CancelFunc")
    }
}

impl Context {
    /// Go: `context.Background()`. Never done, no deadline.
    pub fn background() -> Context {
        Context {
            inner: Arc::new(Inner {
                state: Mutex::new(State::default()),
                cond: Condvar::new(),
                deadline: None,
                cancellable: false,
            }),
        }
    }

    /// Go: `context.WithCancel(ctx)`.
    pub fn with_cancel(&self) -> (Context, CancelFunc) {
        self.child(self.inner.deadline)
    }

    /// Go: `context.WithDeadline(ctx, d)`. A parent deadline that comes
    /// first wins. A deadline already past makes the child done at once with
    /// [`ContextError::DeadlineExceeded`].
    pub fn with_deadline(&self, deadline: Instant) -> (Context, CancelFunc) {
        let effective = match self.inner.deadline {
            Some(parent) if parent <= deadline => parent,
            _ => deadline,
        };
        self.child(Some(effective))
    }

    /// Go: `context.WithTimeout(ctx, d)`.
    pub fn with_timeout(&self, timeout: Duration) -> (Context, CancelFunc) {
        match Instant::now().checked_add(timeout) {
            Some(deadline) => self.with_deadline(deadline),
            // Beyond the clock's range: no reachable deadline of its own.
            None => self.with_cancel(),
        }
    }

    /// Go: `context.WithTimeout(ctx, time.Duration(nanos))` for a signed
    /// budget such as `Config::run_timeout_budget`. Zero or negative means a
    /// deadline already past, so the child is done at once.
    pub fn with_timeout_nanos(&self, nanos: i64) -> (Context, CancelFunc) {
        if nanos <= 0 {
            return self.with_deadline(Instant::now());
        }
        self.with_timeout(Duration::from_nanos(nanos as u64))
    }

    fn child(&self, deadline: Option<Instant>) -> (Context, CancelFunc) {
        let inner = Arc::new(Inner {
            state: Mutex::new(State::default()),
            cond: Condvar::new(),
            deadline,
            cancellable: true,
        });
        // Expire a passed parent deadline first, so the check below sees it.
        let _ = self.inner.err();
        {
            let mut parent = self.inner.lock();
            match parent.err {
                Some(err) => inner.lock().err = Some(err),
                None if self.inner.cancellable => {
                    parent.children.retain(|w| w.strong_count() > 0);
                    parent.children.push(Arc::downgrade(&inner));
                }
                None => {}
            }
        }
        let _ = inner.err(); // a deadline already past
        let cancel = CancelFunc {
            inner: Arc::clone(&inner),
        };
        (Context { inner }, cancel)
    }

    /// Go: `ctx.Err()`. `None` while the context is live.
    pub fn err(&self) -> Option<ContextError> {
        self.inner.err()
    }

    pub fn is_done(&self) -> bool {
        self.err().is_some()
    }

    /// Go: `ctx.Deadline()`.
    pub fn deadline(&self) -> Option<Instant> {
        self.inner.deadline
    }

    /// Sleeps up to `timeout`, returning early once the context is done.
    /// `Some(err)` means the context is done; `None` means the full timeout
    /// passed. A context already done returns at once.
    pub fn wait_timeout(&self, timeout: Duration) -> Option<ContextError> {
        let end = Instant::now().checked_add(timeout);
        let mut st = self.inner.lock();
        loop {
            if let Some(err) = st.err {
                return Some(err);
            }
            let now = Instant::now();
            if self.inner.deadline.is_some_and(|d| now >= d) {
                drop(st);
                return self.err();
            }
            if end.is_some_and(|e| now >= e) {
                return None;
            }
            let wake = match (end, self.inner.deadline) {
                (Some(e), Some(d)) => Some(e.min(d)),
                (e, d) => e.or(d),
            };
            st = match wake {
                Some(w) => {
                    self.inner
                        .cond
                        .wait_timeout(st, w - now)
                        .unwrap_or_else(|p| p.into_inner())
                        .0
                }
                None => self.inner.cond.wait(st).unwrap_or_else(|p| p.into_inner()),
            };
        }
    }

    /// Blocks until the context is done (Go: `<-ctx.Done()`). Blocks forever
    /// on [`Context::background`].
    pub fn wait(&self) -> ContextError {
        loop {
            if let Some(err) = self.wait_timeout(Duration::from_secs(3600)) {
                return err;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn errors_print_go_text() {
        assert_eq!(ContextError::Canceled.to_string(), "context canceled");
        assert_eq!(
            ContextError::DeadlineExceeded.to_string(),
            "context deadline exceeded"
        );
    }

    #[test]
    fn background_is_never_done() {
        let bg = Context::background();
        assert_eq!(bg.err(), None);
        assert_eq!(bg.deadline(), None);
        assert_eq!(bg.wait_timeout(Duration::from_millis(5)), None);
    }

    #[test]
    fn parent_cancel_propagates_and_wakes_child_waiter() {
        let (parent, cancel_parent) = Context::background().with_cancel();
        let (child, _cancel_child) = parent.with_timeout(Duration::from_secs(60));
        let (grandchild, _c) = child.with_cancel();
        let waiter = thread::spawn(move || {
            let start = Instant::now();
            let err = grandchild.wait_timeout(Duration::from_secs(30));
            (err, start.elapsed())
        });
        thread::sleep(Duration::from_millis(20));
        cancel_parent.cancel();
        let (err, waited) = waiter.join().unwrap();
        assert_eq!(err, Some(ContextError::Canceled));
        assert!(
            waited < Duration::from_secs(5),
            "waiter woke late: {waited:?}"
        );
        assert_eq!(child.err(), Some(ContextError::Canceled));
    }

    #[test]
    fn child_cancel_or_timeout_leaves_parent_live() {
        let (parent, _cancel) = Context::background().with_cancel();
        let (child, cancel_child) = parent.with_cancel();
        cancel_child.cancel();
        cancel_child.cancel(); // idempotent
        assert_eq!(child.err(), Some(ContextError::Canceled));
        assert_eq!(parent.err(), None);

        let (short, _c) = parent.with_timeout(Duration::from_millis(10));
        assert_eq!(
            short.wait_timeout(Duration::from_secs(5)),
            Some(ContextError::DeadlineExceeded)
        );
        assert_eq!(parent.err(), None);
    }

    #[test]
    fn earliest_deadline_wins() {
        let (parent, _c) = Context::background().with_timeout(Duration::from_millis(30));
        let parent_deadline = parent.deadline().unwrap();

        // A later child deadline is clipped to the parent's.
        let (late, _c) = parent.with_timeout(Duration::from_secs(60));
        assert_eq!(late.deadline(), Some(parent_deadline));
        // An earlier one is kept.
        let (early, _c) = parent.with_timeout(Duration::from_millis(1));
        assert!(early.deadline().unwrap() < parent_deadline);
        // A plain cancel child inherits the parent's deadline.
        let (plain, _c) = parent.with_cancel();
        assert_eq!(plain.deadline(), Some(parent_deadline));

        assert_eq!(
            late.wait_timeout(Duration::from_secs(5)),
            Some(ContextError::DeadlineExceeded)
        );
        assert_eq!(parent.err(), Some(ContextError::DeadlineExceeded));
        assert_eq!(plain.err(), Some(ContextError::DeadlineExceeded));
    }

    #[test]
    fn parent_deadline_propagates_deadline_exceeded() {
        let (parent, _c) = Context::background().with_timeout(Duration::from_millis(10));
        let (child, _c) = parent.with_cancel();
        assert_eq!(child.wait(), ContextError::DeadlineExceeded);
    }

    #[test]
    fn non_positive_budget_is_an_immediate_deadline() {
        let bg = Context::background();
        for nanos in [0, -1, i64::MIN] {
            let (ctx, _c) = bg.with_timeout_nanos(nanos);
            assert_eq!(ctx.err(), Some(ContextError::DeadlineExceeded), "{nanos}");
        }
        let (ctx, _c) = bg.with_timeout_nanos(i64::MAX);
        assert_eq!(ctx.err(), None);
    }

    #[test]
    fn child_of_done_parent_is_done_with_parent_error() {
        let (parent, cancel) = Context::background().with_cancel();
        cancel.cancel();
        let (child, _c) = parent.with_timeout(Duration::from_secs(60));
        assert_eq!(child.err(), Some(ContextError::Canceled));

        let (expired, _c) = Context::background().with_timeout_nanos(0);
        let (child, _c) = expired.with_cancel();
        assert_eq!(child.err(), Some(ContextError::DeadlineExceeded));
    }

    #[test]
    fn cancel_after_lapsed_deadline_keeps_deadline_exceeded() {
        // Nobody called err() before the cancel: the deadline still wins.
        let (ctx, cancel) = Context::background().with_timeout(Duration::from_millis(5));
        thread::sleep(Duration::from_millis(20));
        cancel.cancel();
        assert_eq!(ctx.err(), Some(ContextError::DeadlineExceeded));
    }

    #[test]
    fn child_deadline_before_parent_cancel_keeps_deadline_exceeded() {
        let (parent, cancel_parent) = Context::background().with_cancel();
        let (child, _c) = parent.with_timeout(Duration::from_millis(5));
        let (grandchild, _c) = child.with_cancel();
        thread::sleep(Duration::from_millis(20));
        cancel_parent.cancel();
        assert_eq!(parent.err(), Some(ContextError::Canceled));
        assert_eq!(child.err(), Some(ContextError::DeadlineExceeded));
        assert_eq!(grandchild.err(), Some(ContextError::DeadlineExceeded));
    }

    #[test]
    fn wait_timeout_returns_none_when_time_passes() {
        let (ctx, _c) = Context::background().with_cancel();
        let start = Instant::now();
        assert_eq!(ctx.wait_timeout(Duration::from_millis(15)), None);
        assert!(start.elapsed() >= Duration::from_millis(15));
    }
}
