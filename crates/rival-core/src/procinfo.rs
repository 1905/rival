//! PID liveness checks that are robust against PID reuse.
//!
//! Go: `internal/procinfo`. A PID is paired with its process start time. After
//! a process dies the OS may recycle its PID; comparing start times tells the
//! original process from an unrelated one that later inherited the number.

/// Returns the start time of the process with the given pid, or `None` if it
/// does not exist or cannot be inspected. macOS reports Unix nanoseconds; Linux
/// reports nanoseconds since boot (see [`parse_stat_start`]).
pub fn start_nanos(pid: i32) -> Option<i64> {
    start_nanos_impl(pid)
}

/// Reports whether pid is live and provably the process that recorded
/// `want_start`. Unlike [`alive`] it never degrades: an unrecorded start (0) or
/// an unreadable start time is false. Use it to authorize a signal; [`alive`]
/// is for existence checks that must not false-reap.
pub fn same_process(pid: i32, want_start: i64) -> bool {
    if pid <= 0 || want_start == 0 {
        return false;
    }
    start_nanos(pid) == Some(want_start)
}

/// Reports whether pid is live AND is the same process that recorded
/// `want_start`. If `want_start` is 0 (not recorded, or an unsupported
/// platform) it degrades to a bare existence check. A recycled PID belongs to a
/// process with a different start time, so this returns false for it.
pub fn alive(pid: i32, want_start: i64) -> bool {
    if pid <= 0 || !exists(pid) {
        return false;
    }
    if want_start == 0 {
        return true; // nothing to compare against — best effort
    }
    match start_nanos(pid) {
        Some(got) => got == want_start,
        None => true, // can't read start time right now — don't false-reap a live PID
    }
}

/// Go: `syscall.Kill(pid, 0) == nil`.
#[cfg(unix)]
fn exists(pid: i32) -> bool {
    // SAFETY: signal 0 performs only the existence and permission check.
    unsafe { libc::kill(pid, 0) == 0 }
}

// Windows liveness lands in P5.
#[cfg(not(unix))]
fn exists(_pid: i32) -> bool {
    false
}

#[cfg(target_os = "macos")]
fn start_nanos_impl(pid: i32) -> Option<i64> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_PID, pid];
    let mut size = 0;
    // SAFETY: the MIB and length pointers are valid. A null output asks for size.
    let status = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            4,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if status != 0 || size < std::mem::size_of::<libc::timeval>() {
        return None;
    }
    let mut data = vec![0u8; size];
    // SAFETY: data holds the requested number of writable bytes.
    let status = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            4,
            data.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if status != 0 || size < std::mem::size_of::<libc::timeval>() {
        return None;
    }
    // Darwin sys/sysctl.h: kinfo_proc starts with extern_proc. sys/proc.h:
    // extern_proc starts with the union containing timeval p_starttime.
    // Read only this public ABI prefix; do not duplicate the full kernel struct.
    // SAFETY: the checked buffer contains a timeval; read_unaligned needs no alignment.
    let start = unsafe { data.as_ptr().cast::<libc::timeval>().read_unaligned() };
    Some(
        start
            .tv_sec
            .wrapping_mul(1_000_000_000)
            .wrapping_add(i64::from(start.tv_usec).wrapping_mul(1000)),
    )
}

#[cfg(target_os = "linux")]
fn start_nanos_impl(pid: i32) -> Option<i64> {
    let data = std::fs::read(format!("/proc/{pid}/stat")).ok()?;
    parse_stat_start(&data)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn start_nanos_impl(_pid: i32) -> Option<i64> {
    None
}

/// The kernel USER_HZ. It is effectively always 100 on Linux; Go hardcodes it
/// because cgo (`sysconf(_SC_CLK_TCK)`) is disabled there.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const CLOCK_TICKS_PER_SEC: i64 = 100;

/// Parses `/proc/<pid>/stat` into nanoseconds SINCE BOOT (not wall-clock).
///
/// Field 22 (starttime) is in clock ticks since boot and is fixed for a
/// process's whole lifetime. The boot time (`/proc/stat` btime) is deliberately
/// not added: it is derived from the wall clock and shifts on NTP steps, manual
/// date changes and suspend-resume, so the same live process would report
/// different start times before and after a clock step — false-reaping a live
/// queue holder. The value only has to be self-consistent on one machine.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_stat_start(data: &[u8]) -> Option<i64> {
    // The comm field (2) may contain spaces/parens; everything after the last
    // ')' is space-separated, with starttime as field 22 overall → index 19
    // within the post-')' slice (fields 3..).
    let rparen = data.iter().rposition(|&b| b == b')')?;
    let rest = String::from_utf8_lossy(&data[rparen + 1..]);
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // fields[0] is state (field 3); starttime is field 22 → fields[19].
    if fields.len() < 20 {
        return None;
    }
    let ticks: i64 = fields[19].parse().ok()?;
    // ticks → ns since boot. +1 so a process that started at exactly tick 0
    // never collides with the "unavailable" sentinel.
    Some(
        ticks
            .wrapping_mul(1_000_000_000 / CLOCK_TICKS_PER_SEC)
            .wrapping_add(1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Go uses this PID as one that never exists (above every default pid_max).
    const DEAD_PID: i32 = 1 << 24;

    fn self_pid() -> i32 {
        std::process::id() as i32
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn start_nanos_self() {
        let n = start_nanos(self_pid());
        assert!(
            matches!(n, Some(v) if v > 0),
            "start_nanos(self) = {n:?}, want positive"
        );
    }

    #[test]
    fn alive_cases() {
        let pid = self_pid();
        let Some(start) = start_nanos(pid) else {
            eprintln!("start time unsupported on this platform");
            return;
        };
        assert!(alive(pid, start), "alive(self, correct start) = false");
        // A non-matching start time means the PID was recycled → treat as dead.
        assert!(!alive(pid, start + 1), "PID-reuse guard failed");
        // want_start 0 = no recorded start → bare existence check.
        assert!(
            alive(pid, 0),
            "alive(self, 0) = false, want best-effort true"
        );
        // A dead PID is never alive.
        assert!(!alive(DEAD_PID, 0), "alive(dead pid, 0) = true");
        assert!(
            !alive(0, 0) && !alive(-1, 0),
            "alive(non-positive pid) = true"
        );
    }

    // Codex finding 1: stop authorization never degrades to an existence check.
    #[test]
    fn same_process_cases() {
        let pid = self_pid();
        let Some(start) = start_nanos(pid) else {
            eprintln!("start time unsupported on this platform");
            return;
        };
        assert!(
            same_process(pid, start),
            "same_process(self, correct start) = false"
        );
        assert!(
            !same_process(pid, start + 1),
            "same_process(self, wrong start) = true"
        );
        assert!(
            !same_process(pid, 0),
            "same_process(self, 0) = true: no recorded start never authorizes"
        );
        assert!(
            !same_process(DEAD_PID, start) && !same_process(0, start),
            "same_process(dead or non-positive pid) = true"
        );
    }

    #[test]
    fn start_nanos_dead_pid_is_none() {
        assert_eq!(start_nanos(DEAD_PID), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn start_nanos_self_is_unix_wall_clock() {
        // Go reports Unix nanoseconds on darwin; the test process started
        // within the last day and not in the future.
        let start = start_nanos(self_pid()).unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as i64;
        assert!(start <= now, "start {start} after now {now}");
        assert!(
            now - start < 86_400 * 1_000_000_000,
            "start {start} too old"
        );
        assert_eq!(start % 1000, 0, "microsecond precision expected");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn start_nanos_can_inspect_launchd() {
        // Go uses sysctl, which can inspect another user's process. proc_pidinfo
        // cannot, so switching to it would silently change stop authorization.
        assert!(start_nanos(1).is_some_and(|n| n > 0));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn start_nanos_self_matches_proc_stat() {
        let data = std::fs::read(format!("/proc/{}/stat", self_pid())).unwrap();
        assert_eq!(start_nanos(self_pid()), parse_stat_start(&data));
    }

    /// A stat line with `starttime` (field 22) set to `ticks`.
    fn stat_line(comm: &str, ticks: &str) -> String {
        // Fields 3..21 (19 values), then starttime, then a few trailing ones.
        let mid = "S 1 1 1 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0";
        format!("1234 ({comm}) {mid} {ticks} 12345 678 18446744073709551615\n")
    }

    #[test]
    fn parse_stat_start_cases() {
        let cases: &[(&str, String, Option<i64>)] = &[
            (
                "plain comm",
                stat_line("rival", "4242"),
                Some(42_420_000_001),
            ),
            (
                "tick zero is not the sentinel",
                stat_line("rival", "0"),
                Some(1),
            ),
            (
                "comm with spaces",
                stat_line("my proc", "7"),
                Some(70_000_001),
            ),
            (
                "comm with parens",
                stat_line("a) (b) c", "7"),
                Some(70_000_001),
            ),
            (
                "comm with ) and digits",
                stat_line(") 1 2 3", "9"),
                Some(90_000_001),
            ),
            ("non-numeric starttime", stat_line("rival", "x"), None),
            ("no rparen", "1234 rival S 1 1".to_string(), None),
            (
                "too few fields",
                "1234 (rival) S 1 1 1 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0".to_string(),
                None,
            ),
            (
                "exactly 20 fields",
                "1 (r) S 1 1 1 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0 5".to_string(),
                Some(50_000_001),
            ),
            ("empty", String::new(), None),
        ];
        for (name, line, want) in cases {
            assert_eq!(parse_stat_start(line.as_bytes()), *want, "{name}: {line:?}");
        }
    }

    #[test]
    fn parse_stat_start_invalid_utf8_comm() {
        let mut data = b"1 (\xff\xfe) ".to_vec();
        data.extend_from_slice(b"S 1 1 1 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0 3 0");
        assert_eq!(parse_stat_start(&data), Some(30_000_001));
    }
}
