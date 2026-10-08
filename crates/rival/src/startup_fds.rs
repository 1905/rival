//! Records which standard descriptors were closed when the process started.
//!
//! Rust's Unix runtime reopens a closed fd 0, 1 or 2 on `/dev/null` before
//! `main`. Rival must still see the closed fd: a `rival --detach` with a
//! closed stderr fails to start its child
//! (`start ...: Bad file descriptor (os error 9)`) and exits 1.
//! A loader constructor runs before that sanitizing step and records the
//! original state here. It calls only `fcntl` and stores atomics: no
//! allocation, std I/O, environment or runtime services.

use std::sync::atomic::{AtomicU8, Ordering};

/// Bit `fd` is set when standard descriptor `fd` was closed at startup.
static CLOSED_AT_START: AtomicU8 = AtomicU8::new(0);
/// Set once the constructor ran, so a missing constructor is detectable.
#[cfg_attr(
    not(all(test, any(target_os = "linux", target_os = "macos"))),
    allow(dead_code, reason = "diagnostic for the constructor tests")
)]
static RECORDED: AtomicU8 = AtomicU8::new(0);

/// The constructor body. `fcntl(F_GETFD)` fails with `EBADF` only for a
/// closed descriptor.
#[cfg(any(target_os = "linux", target_os = "macos"))]
extern "C" fn record_standard_fds() {
    let mut closed = 0u8;
    for fd in 0..3 {
        // SAFETY: F_GETFD only reads the descriptor flags.
        if unsafe { libc::fcntl(fd, libc::F_GETFD) } == -1 {
            closed |= 1 << fd;
        }
    }
    CLOSED_AT_START.store(closed, Ordering::Relaxed);
    RECORDED.store(1, Ordering::Release);
}

// The loader runs every pointer in these sections before `main`, which is
// before std's descriptor sanitizing. `#[used]` keeps the pointer in the
// object file; the Rust Reference does not promise the final link keeps it.
// `scripts/check-startup-fds.py` checks the linked debug and LTO release
// binaries, and `records_descriptors_closed_before_std_reopens_them` checks
// the test binary at run time.
#[cfg(target_os = "linux")]
#[used]
#[unsafe(link_section = ".init_array")]
static RECORD_STANDARD_FDS: extern "C" fn() = record_standard_fds;

#[cfg(target_os = "macos")]
#[used]
#[unsafe(link_section = "__DATA,__mod_init_func,mod_init_funcs")]
static RECORD_STANDARD_FDS: extern "C" fn() = record_standard_fds;

/// Whether standard descriptor `fd` (0, 1 or 2) was closed when the process
/// started. Always false where no constructor runs (Windows).
pub fn closed_at_start(fd: i32) -> bool {
    (0..3).contains(&fd) && CLOSED_AT_START.load(Ordering::Relaxed) & (1 << fd) != 0
}

/// Whether any of fds 0-2 was closed at startup.
pub fn any_closed_at_start() -> bool {
    CLOSED_AT_START.load(Ordering::Relaxed) != 0
}

/// Whether the constructor ran in this process.
#[cfg_attr(
    not(all(test, any(target_os = "linux", target_os = "macos"))),
    allow(dead_code, reason = "diagnostic for the constructor tests")
)]
pub fn recorded() -> bool {
    RECORDED.load(Ordering::Acquire) == 1
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt;
    use std::path::Path;
    use std::process::{Command, Stdio};

    /// Set on the helper child only. Names the report file.
    const HELPER_OUT: &str = "RIVAL_STARTUP_FDS_HELPER_OUT";
    const HELPER_TEST: &str = "startup_fds::tests::startup_fds_helper_child";

    #[test]
    fn constructor_ran_in_the_test_binary() {
        assert!(recorded(), "the startup constructor did not run");
        // libtest runs with all three descriptors open.
        assert!(!any_closed_at_start());
        assert!(!closed_at_start(3) && !closed_at_start(-1));
    }

    /// Runs only inside the helper process.
    #[test]
    #[ignore = "helper process for the startup fd tests"]
    fn startup_fds_helper_child() {
        let Some(out) = std::env::var_os(HELPER_OUT) else {
            return;
        };
        let report = format!(
            "recorded={} fd0={} fd1={} fd2={}",
            recorded(),
            closed_at_start(0),
            closed_at_start(1),
            closed_at_start(2)
        );
        std::fs::write(Path::new(&out), report).unwrap();
        // SAFETY: ends the helper without libtest's summary on fd 1.
        unsafe { libc::_exit(0) };
    }

    /// Re-runs this test binary with `close_fds` closed before exec and
    /// returns the helper's report.
    fn run_helper(close_fds: &'static [i32]) -> String {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("report");
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args(["--exact", HELPER_TEST, "--ignored", "--quiet"])
            .env(HELPER_OUT, &out)
            .env("HOME", dir.path())
            .env("RIVAL_HOME", dir.path().join(".rival"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // SAFETY: close is async-signal-safe; the fds are the child's copies.
        unsafe {
            cmd.pre_exec(move || {
                for &fd in close_fds {
                    libc::close(fd);
                }
                Ok(())
            });
        }
        let status = rival_core::executor::process::spawn(&mut cmd)
            .and_then(|mut child| child.wait())
            .unwrap();
        assert!(status.success(), "helper failed: {status}");
        std::fs::read_to_string(&out).unwrap()
    }

    #[test]
    fn records_descriptors_closed_before_std_reopens_them() {
        assert_eq!(
            run_helper(&[]),
            "recorded=true fd0=false fd1=false fd2=false"
        );
        assert_eq!(
            run_helper(&[2]),
            "recorded=true fd0=false fd1=false fd2=true"
        );
        assert_eq!(
            run_helper(&[0, 1]),
            "recorded=true fd0=true fd1=true fd2=false"
        );
    }
}
