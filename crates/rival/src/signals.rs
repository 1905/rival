//! Go `signal.NotifyContext(ctx, os.Interrupt, syscall.SIGTERM)`.
//!
//! While at least one scope is open, SIGINT and SIGTERM notify every open
//! scope instead of killing the process. When the last scope
//! closes, the dispositions that were in place before the first scope come
//! back, so outside a scope the signals act as before (Go: `stop()` →
//! `signal.Stop`).
//!
//! A small self-pipe adapter instead of a crate: `ctrlc`'s termination
//! feature also traps SIGHUP and cannot unregister, and
//! `signal-hook-registry`'s unregister does not restore the previous
//! handler. The handler only writes the signal number to a non-blocking
//! pipe (async-signal-safe); one watcher thread reads it and notifies the
//! open scopes.
//!
//! A scope either cancels a context ([`notify_context`], every command) or
//! calls a function with the signal ([`notify`], the TUI, which must tell
//! SIGINT from SIGTERM as bubbletea does).

use std::sync::Arc;

use rival_core::cancel::{CancelFunc, Context};

/// Which handled signal arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// SIGINT.
    Interrupt,
    /// SIGTERM.
    Terminate,
}

/// What an open scope does when a signal arrives. It runs on the signal
/// watcher thread, never inside the signal handler.
type Sink = Arc<dyn Fn(Signal) + Send + Sync>;

/// Keeps the scope's signal handling alive. Dropping it is Go's deferred
/// `stop()`: the context (if any) is cancelled and, for the last open
/// scope, the previous signal dispositions come back.
#[must_use = "dropping the guard ends signal handling for the scope"]
pub struct NotifyGuard {
    #[cfg(unix)]
    id: u64,
    cancel: Option<CancelFunc>,
}

/// Opens a scope: a child of `parent` that SIGINT or SIGTERM cancels. An
/// error means the handlers could not be installed; nothing is left
/// half-installed and no scope is open.
pub fn notify_context(parent: &Context) -> Result<(Context, NotifyGuard), String> {
    let (ctx, cancel) = parent.with_cancel();
    let sink = {
        let cancel = cancel.clone();
        Arc::new(move |_: Signal| cancel.cancel())
    };
    match open(sink) {
        Ok(mut guard) => {
            guard.cancel = Some(cancel);
            Ok((ctx, guard))
        }
        Err(e) => {
            cancel.cancel();
            Err(e)
        }
    }
}

/// Opens a scope that calls `on_signal` for every SIGINT and SIGTERM while
/// the guard lives. The same install, restore and error rules as
/// [`notify_context`] apply. Not unix: no signal is delivered (Task 5).
pub fn notify(on_signal: impl Fn(Signal) + Send + Sync + 'static) -> Result<NotifyGuard, String> {
    open(Arc::new(on_signal))
}

fn open(sink: Sink) -> Result<NotifyGuard, String> {
    #[cfg(unix)]
    let id = unix::register(sink)?;
    #[cfg(not(unix))]
    drop(sink);
    Ok(NotifyGuard {
        #[cfg(unix)]
        id,
        cancel: None,
    })
}

impl Drop for NotifyGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        unix::unregister(self.id);
        if let Some(cancel) = &self.cancel {
            cancel.cancel();
        }
    }
}

#[cfg(unix)]
mod unix {
    use std::io::{self, Read};
    use std::os::fd::FromRawFd;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicI32, Ordering};

    use super::{Signal, Sink};

    const SIGNALS: [libc::c_int; 2] = [libc::SIGINT, libc::SIGTERM];

    /// The pipe's write end, or -1 before the pipe exists.
    static PIPE_WRITE: AtomicI32 = AtomicI32::new(-1);

    struct State {
        next_id: u64,
        scopes: Vec<(u64, Sink)>,
        /// The dispositions saved when the first scope opened.
        saved: Option<[libc::sigaction; 2]>,
    }

    static STATE: Mutex<State> = Mutex::new(State {
        next_id: 0,
        scopes: Vec::new(),
        saved: None,
    });

    fn lock() -> std::sync::MutexGuard<'static, State> {
        STATE.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn os_err(what: &str) -> String {
        format!(
            "{what}: {}",
            rival_core::gostd::os_error_text(&io::Error::last_os_error())
        )
    }

    extern "C" fn on_signal(sig: libc::c_int) {
        // SAFETY: errno access and write(2) are async-signal-safe; the pipe
        // is non-blocking, so a full pipe drops the byte instead of hanging.
        unsafe {
            let errno = *errno_location();
            let fd = PIPE_WRITE.load(Ordering::Relaxed);
            if fd >= 0 {
                let byte = sig as u8;
                libc::write(fd, (&raw const byte).cast(), 1);
            }
            *errno_location() = errno;
        }
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    unsafe fn errno_location() -> *mut libc::c_int {
        unsafe { libc::__errno_location() }
    }

    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    unsafe fn errno_location() -> *mut libc::c_int {
        unsafe { libc::__error() }
    }

    /// Creates the pipe and its watcher thread once. On failure everything
    /// created so far is closed.
    fn ensure_pipe() -> Result<(), String> {
        if PIPE_WRITE.load(Ordering::Acquire) >= 0 {
            return Ok(());
        }
        let mut fds = [0 as libc::c_int; 2];
        // SAFETY: pipe writes two descriptors into the array.
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err(os_err("pipe"));
        }
        let close_both = || {
            // SAFETY: closing descriptors this function created.
            unsafe {
                libc::close(fds[0]);
                libc::close(fds[1]);
            }
        };
        // SAFETY: flag updates on descriptors we own.
        let flags_ok = unsafe {
            libc::fcntl(fds[0], libc::F_SETFD, libc::FD_CLOEXEC) != -1
                && libc::fcntl(fds[1], libc::F_SETFD, libc::FD_CLOEXEC) != -1
                && {
                    let fl = libc::fcntl(fds[1], libc::F_GETFL);
                    fl != -1 && libc::fcntl(fds[1], libc::F_SETFL, fl | libc::O_NONBLOCK) != -1
                }
        };
        if !flags_ok {
            let err = os_err("fcntl");
            close_both();
            return Err(err);
        }
        let read_fd = fds[0];
        let spawned = std::thread::Builder::new()
            .name("rival-signals".into())
            .spawn(move || {
                // SAFETY: the watcher owns the read end from here.
                let mut reader = unsafe { std::fs::File::from_raw_fd(read_fd) };
                let mut buf = [0u8; 16];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => return,
                        Ok(n) => {
                            // Called outside the lock: a sink may be slow.
                            let scopes: Vec<Sink> =
                                lock().scopes.iter().map(|(_, s)| s.clone()).collect();
                            for &byte in &buf[..n] {
                                let sig = if libc::c_int::from(byte) == libc::SIGINT {
                                    Signal::Interrupt
                                } else {
                                    Signal::Terminate
                                };
                                for sink in &scopes {
                                    sink(sig);
                                }
                            }
                        }
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                        Err(_) => return,
                    }
                }
            });
        if let Err(e) = spawned {
            close_both();
            return Err(format!("start signal watcher: {e}"));
        }
        PIPE_WRITE.store(fds[1], Ordering::Release);
        Ok(())
    }

    pub(super) fn register(sink: Sink) -> Result<u64, String> {
        let mut st = lock();
        if st.saved.is_none() {
            ensure_pipe()?;
            st.saved = Some(install()?);
        }
        let id = st.next_id;
        st.next_id += 1;
        st.scopes.push((id, sink));
        Ok(id)
    }

    pub(super) fn unregister(id: u64) {
        let mut st = lock();
        st.scopes.retain(|(i, _)| *i != id);
        if st.scopes.is_empty()
            && let Some(saved) = st.saved.take()
        {
            // A failed restore cannot be reported from a drop; the handler
            // stays, and with no scope open it only writes to the pipe.
            restore(&saved);
        }
    }

    /// Installs [`on_signal`] for SIGINT and SIGTERM; returns the old
    /// dispositions. On failure the signals keep their old dispositions.
    fn install() -> Result<[libc::sigaction; 2], String> {
        // SAFETY: zeroed sigaction structs are valid; sigaction copies them.
        unsafe {
            let mut old: [libc::sigaction; 2] = std::mem::zeroed();
            let mut act: libc::sigaction = std::mem::zeroed();
            act.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
            act.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut act.sa_mask);
            for (i, sig) in SIGNALS.iter().enumerate() {
                if libc::sigaction(*sig, &act, &mut old[i]) != 0 {
                    let err = os_err("sigaction");
                    restore(&old[..i]);
                    return Err(err);
                }
            }
            Ok(old)
        }
    }

    fn restore(saved: &[libc::sigaction]) {
        for (sig, old) in SIGNALS.iter().zip(saved) {
            // SAFETY: restores a disposition sigaction returned earlier.
            unsafe { libc::sigaction(*sig, old, std::ptr::null_mut()) };
        }
    }

    /// The handler currently installed for `sig`.
    #[cfg(test)]
    pub(super) fn current_handler(sig: libc::c_int) -> libc::sighandler_t {
        // SAFETY: querying only.
        unsafe {
            let mut old: libc::sigaction = std::mem::zeroed();
            libc::sigaction(sig, std::ptr::null(), &mut old);
            old.sa_sigaction
        }
    }

    #[cfg(test)]
    pub(super) fn handler_address() -> libc::sighandler_t {
        on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t
    }

    /// No scope open, no saved dispositions and no pipe.
    #[cfg(test)]
    pub(super) fn is_idle() -> bool {
        let st = lock();
        st.scopes.is_empty() && st.saved.is_none() && PIPE_WRITE.load(Ordering::Acquire) < 0
    }
}

/// Signal tests change process-wide dispositions and raise real signals, so
/// each runs in its own child process (this test binary, re-executed on an
/// ignored helper). No other test in the child can be cancelled by them.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const HELPER_OUT: &str = "RIVAL_SIGNALS_HELPER_OUT";
    const HELPER_MODE: &str = "RIVAL_SIGNALS_HELPER_MODE";
    const HELPER_TEST: &str = "signals::tests::signals_helper_child";

    /// Runs inside the helper process only.
    #[test]
    #[ignore = "helper process for the signal tests"]
    fn signals_helper_child() {
        let (Some(out), Some(mode)) = (
            std::env::var_os(HELPER_OUT),
            std::env::var(HELPER_MODE).ok(),
        ) else {
            return;
        };
        let mut report = Vec::new();
        match mode.as_str() {
            "dispositions" => {
                let before = unix::current_handler(libc::SIGINT);
                report.push(format!(
                    "before_is_ours={}",
                    before == unix::handler_address()
                ));
                {
                    let (_ctx, _guard) = notify_context(&Context::background()).unwrap();
                    report.push(format!(
                        "open_int={} open_term={}",
                        unix::current_handler(libc::SIGINT) == unix::handler_address(),
                        unix::current_handler(libc::SIGTERM) == unix::handler_address()
                    ));
                    {
                        let (_c2, _g2) = notify_context(&Context::background()).unwrap();
                    }
                    report.push(format!(
                        "after_inner={}",
                        unix::current_handler(libc::SIGINT) == unix::handler_address()
                    ));
                }
                report.push(format!(
                    "restored={}",
                    unix::current_handler(libc::SIGINT) == before
                        && unix::current_handler(libc::SIGTERM) == libc::SIG_DFL
                ));
            }
            "sigint" | "sigterm" => {
                let sig = if mode == "sigint" {
                    libc::SIGINT
                } else {
                    libc::SIGTERM
                };
                let (ctx, guard) = notify_context(&Context::background()).unwrap();
                let (other, other_guard) = notify_context(&Context::background()).unwrap();
                // SAFETY: the scope's handler is installed; it only writes
                // to the pipe.
                unsafe { libc::raise(sig) };
                let woke = ctx.wait_timeout(Duration::from_secs(10)).is_some();
                let other_woke = other.wait_timeout(Duration::from_secs(10)).is_some();
                report.push(format!("cancelled={woke} all_scopes={other_woke}"));
                drop(other_guard);
                drop(guard);
                std::fs::write(Path::new(&out), report.join("\n")).unwrap();
                // Outside any scope the default disposition is back: this
                // signal ends the helper.
                // SAFETY: raising a signal at our own process.
                unsafe { libc::raise(sig) };
                std::thread::sleep(Duration::from_secs(10));
                report.push("survived".into());
            }
            "notify" => {
                // A function scope sees which signal arrived; a context
                // scope open at the same time is still cancelled.
                let (tx, rx) = std::sync::mpsc::channel();
                let guard = notify(move |sig| {
                    let _ = tx.send(sig);
                })
                .unwrap();
                let (ctx, ctx_guard) = notify_context(&Context::background()).unwrap();
                let wait = Duration::from_secs(10);
                // SAFETY: the scope's handler is installed; it only writes
                // to the pipe.
                unsafe { libc::raise(libc::SIGINT) };
                let first = rx.recv_timeout(wait);
                // SAFETY: as above.
                unsafe { libc::raise(libc::SIGTERM) };
                let second = rx.recv_timeout(wait);
                report.push(format!(
                    "first={first:?} second={second:?} ctx_cancelled={}",
                    ctx.wait_timeout(wait).is_some()
                ));
                drop(ctx_guard);
                report.push(format!(
                    "still_ours={}",
                    unix::current_handler(libc::SIGTERM) == unix::handler_address()
                ));
                drop(guard);
                report.push(format!(
                    "restored={}",
                    unix::current_handler(libc::SIGTERM) == libc::SIG_DFL
                ));
            }
            "setup_failure" => {
                let ours = unix::handler_address();
                let before = unix::current_handler(libc::SIGINT);
                // SAFETY: rlimit and descriptor calls on this helper process
                // only; dup(0) finds the lowest free descriptor number.
                let (limit, lowest_free) = unsafe {
                    let mut limit: libc::rlimit = std::mem::zeroed();
                    assert_eq!(libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit), 0);
                    let fd = libc::dup(0);
                    assert!(fd >= 0);
                    libc::close(fd);
                    (limit, fd)
                };
                // No new descriptor fits under the limit, so pipe(2) fails.
                let tight = libc::rlimit {
                    rlim_cur: lowest_free as libc::rlim_t,
                    rlim_max: limit.rlim_max,
                };
                // SAFETY: lowers the soft limit of this helper only.
                let limited = unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &tight) } == 0;
                let result = notify_context(&Context::background());
                // SAFETY: restores the soft limit saved above.
                let restored = unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) } == 0;
                report.push(format!("limited={limited} restored={restored}"));
                match result {
                    Ok(_) => report.push("err=none".into()),
                    Err(e) => report.push(format!("err={e}")),
                }
                report.push(format!(
                    "handler_installed={} dispositions_kept={} idle={}",
                    unix::current_handler(libc::SIGINT) == ours
                        || unix::current_handler(libc::SIGTERM) == ours,
                    unix::current_handler(libc::SIGINT) == before
                        && unix::current_handler(libc::SIGTERM) == libc::SIG_DFL,
                    unix::is_idle()
                ));
                // SAFETY: probes the lowest free descriptor again.
                let fd = unsafe { libc::dup(0) };
                report.push(format!("fds_leaked={}", fd != lowest_free));
                // SAFETY: closes the probe descriptor.
                unsafe { libc::close(fd) };
                // Nothing half-installed blocks a later scope.
                let (ctx, guard) = notify_context(&Context::background()).unwrap();
                report.push(format!(
                    "retry_installed={}",
                    unix::current_handler(libc::SIGINT) == ours
                ));
                drop(guard);
                report.push(format!("retry_closed={}", ctx.is_done()));
            }
            _ => report.push(format!("unknown mode {mode}")),
        }
        std::fs::write(Path::new(&out), report.join("\n")).unwrap();
        // SAFETY: ends the helper without libtest's summary.
        unsafe { libc::_exit(0) };
    }

    /// Runs the helper in `mode`; returns its report and wait status.
    fn run_helper(mode: &str) -> (String, std::process::ExitStatus) {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("report");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                HELPER_TEST,
                "--ignored",
                "--quiet",
                "--test-threads=1",
            ])
            .env(HELPER_OUT, &out)
            .env(HELPER_MODE, mode)
            .env("HOME", dir.path())
            .env("RIVAL_HOME", dir.path().join(".rival"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("signal helper {mode} timed out");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        (std::fs::read_to_string(&out).unwrap_or_default(), status)
    }

    #[test]
    fn scope_installs_and_restores_dispositions() {
        let (report, status) = run_helper("dispositions");
        assert!(status.success(), "{status}");
        assert_eq!(
            report,
            "before_is_ours=false\nopen_int=true open_term=true\nafter_inner=true\nrestored=true"
        );
    }

    #[test]
    fn sigint_and_sigterm_cancel_every_scope_then_default_returns() {
        use std::os::unix::process::ExitStatusExt;
        for (mode, sig) in [("sigint", libc::SIGINT), ("sigterm", libc::SIGTERM)] {
            let (report, status) = run_helper(mode);
            assert_eq!(report, "cancelled=true all_scopes=true", "{mode}");
            assert_eq!(status.signal(), Some(sig), "{mode}: {status}");
        }
    }

    #[test]
    fn notify_tells_sigint_from_sigterm() {
        let (report, status) = run_helper("notify");
        assert!(status.success(), "{status}");
        assert_eq!(
            report,
            "first=Ok(Interrupt) second=Ok(Terminate) ctx_cancelled=true\n\
             still_ours=true\n\
             restored=true"
        );
    }

    #[test]
    fn setup_failure_returns_err_without_installing_handlers() {
        let (report, status) = run_helper("setup_failure");
        assert!(status.success(), "{status}");
        assert_eq!(
            report,
            "limited=true restored=true\n\
             err=pipe: too many open files\n\
             handler_installed=false dispositions_kept=true idle=true\n\
             fds_leaked=false\n\
             retry_installed=true\n\
             retry_closed=true"
        );
    }

    #[test]
    fn dropping_the_guard_cancels_its_context() {
        // No signal and no disposition check: safe in the shared process.
        let (ctx, guard) = notify_context(&Context::background()).unwrap();
        assert!(!ctx.is_done());
        drop(guard);
        assert!(ctx.is_done());
    }
}
