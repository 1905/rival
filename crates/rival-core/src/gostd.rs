//! Go standard-library behaviors that ported code depends on byte for byte:
//! `time.ParseDuration`, `time.Duration.String`, `strconv.Quote` (`%q`),
//! `strings.EqualFold`, `strings.ToLower`, and `syscall.Errno` text.

use std::fmt::Write as _;

#[path = "gostd_tables.rs"]
mod tables;

const NANOSECOND: u64 = 1;
const MICROSECOND: u64 = 1_000 * NANOSECOND;
const MILLISECOND: u64 = 1_000 * MICROSECOND;
const SECOND: u64 = 1_000 * MILLISECOND;
const MINUTE: u64 = 60 * SECOND;
const HOUR: u64 = 60 * MINUTE;

/// Go: `time.ParseDuration`. Returns signed nanoseconds, like `time.Duration`.
pub fn parse_duration(s: &str) -> Result<i64, String> {
    let orig = s;
    let invalid = || Err(format!("time: invalid duration {}", time_quote(orig)));
    let mut s = s;
    let mut d: u64 = 0;
    let mut neg = false;

    if let Some(&c) = s.as_bytes().first()
        && (c == b'-' || c == b'+')
    {
        neg = c == b'-';
        s = &s[1..];
    }
    if s == "0" {
        return Ok(0);
    }
    if s.is_empty() {
        return invalid();
    }
    while !s.is_empty() {
        let b = s.as_bytes();
        if !(b[0] == b'.' || b[0].is_ascii_digit()) {
            return invalid();
        }
        let pl = s.len();
        let Some((v, rest)) = leading_int(s) else {
            return invalid();
        };
        let mut v = v;
        s = rest;
        let pre = pl != s.len();

        let mut post = false;
        let mut f = 0u64;
        let mut scale = 1f64;
        if s.as_bytes().first() == Some(&b'.') {
            s = &s[1..];
            let pl = s.len();
            let (fx, fscale, rest) = leading_fraction(s);
            f = fx;
            scale = fscale;
            s = rest;
            post = pl != s.len();
        }
        if !pre && !post {
            return invalid();
        }

        let i = s
            .bytes()
            .position(|c| c == b'.' || c.is_ascii_digit())
            .unwrap_or(s.len());
        if i == 0 {
            return Err(format!(
                "time: missing unit in duration {}",
                time_quote(orig)
            ));
        }
        let u = &s[..i];
        s = &s[i..];
        let unit = match u {
            "ns" => NANOSECOND,
            "us" | "\u{b5}s" | "\u{3bc}s" => MICROSECOND,
            "ms" => MILLISECOND,
            "s" => SECOND,
            "m" => MINUTE,
            "h" => HOUR,
            _ => {
                return Err(format!(
                    "time: unknown unit {} in duration {}",
                    time_quote(u),
                    time_quote(orig)
                ));
            }
        };
        if v > (1u64 << 63) / unit {
            return invalid();
        }
        v *= unit;
        if f > 0 {
            v += (f as f64 * (unit as f64 / scale)) as u64;
            if v > 1 << 63 {
                return invalid();
            }
        }
        d = d.wrapping_add(v);
        if d > 1 << 63 {
            return invalid();
        }
    }
    if neg {
        return Ok((d as i64).wrapping_neg());
    }
    if d > (1 << 63) - 1 {
        return invalid();
    }
    Ok(d as i64)
}

fn leading_int(s: &str) -> Option<(u64, &str)> {
    let mut x: u64 = 0;
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if !c.is_ascii_digit() {
            break;
        }
        if x > (1 << 63) / 10 {
            return None;
        }
        x = x * 10 + u64::from(c - b'0');
        if x > 1 << 63 {
            return None;
        }
        i += 1;
    }
    Some((x, &s[i..]))
}

fn leading_fraction(s: &str) -> (u64, f64, &str) {
    let b = s.as_bytes();
    let mut i = 0;
    let mut x: u64 = 0;
    let mut scale = 1f64;
    let mut overflow = false;
    while i < b.len() {
        let c = b[i];
        if !c.is_ascii_digit() {
            break;
        }
        i += 1;
        if overflow {
            continue;
        }
        if x > ((1 << 63) - 1) / 10 {
            overflow = true;
            continue;
        }
        let y = x * 10 + u64::from(c - b'0');
        if y > 1 << 63 {
            overflow = true;
            continue;
        }
        x = y;
        scale *= 10.0;
    }
    (x, scale, &s[i..])
}

/// Go's private `time.quote`: non-ASCII and control bytes become `\xNN`.
fn time_quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        if !c.is_ascii() || c < ' ' {
            let mut buf = [0u8; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                let _ = write!(out, "\\x{b:02x}");
            }
        } else {
            if c == '"' || c == '\\' {
                out.push('\\');
            }
            out.push(c);
        }
    }
    out.push('"');
    out
}

/// Go: `time.Duration.String`, e.g. `1h30m0s`, `1.5s`, `100ms`, `0s`.
pub fn format_duration(d: i64) -> String {
    let neg = d < 0;
    let mut u = d.unsigned_abs();
    // Built back to front, like Go's fixed buffer.
    let mut buf: Vec<u8> = Vec::with_capacity(32);
    if u < SECOND {
        buf.push(b's');
        let prec;
        if u == 0 {
            return "0s".to_string();
        } else if u < MICROSECOND {
            prec = 0;
            buf.push(b'n');
        } else if u < MILLISECOND {
            prec = 3;
            // U+00B5 micro sign, reversed byte order.
            buf.extend_from_slice(&[0xB5, 0xC2]);
        } else {
            prec = 6;
            buf.push(b'm');
        }
        u = fmt_frac(&mut buf, u, prec);
        fmt_int(&mut buf, u);
    } else {
        buf.push(b's');
        u = fmt_frac(&mut buf, u, 9);
        fmt_int(&mut buf, u % 60);
        u /= 60;
        if u > 0 {
            buf.push(b'm');
            fmt_int(&mut buf, u % 60);
            u /= 60;
            if u > 0 {
                buf.push(b'h');
                fmt_int(&mut buf, u);
            }
        }
    }
    if neg {
        buf.push(b'-');
    }
    buf.reverse();
    String::from_utf8(buf).expect("duration text is UTF-8")
}

fn fmt_frac(buf: &mut Vec<u8>, mut v: u64, prec: usize) -> u64 {
    let mut print = false;
    for _ in 0..prec {
        let digit = v % 10;
        print = print || digit != 0;
        if print {
            buf.push(digit as u8 + b'0');
        }
        v /= 10;
    }
    if print {
        buf.push(b'.');
    }
    v
}

fn fmt_int(buf: &mut Vec<u8>, mut v: u64) {
    if v == 0 {
        buf.push(b'0');
        return;
    }
    while v > 0 {
        buf.push((v % 10) as u8 + b'0');
        v /= 10;
    }
}

/// Go: `sort.SliceStable`. A direct port of Go's insertion-sort blocks plus
/// SymMerge, so a `less` that is not a strict weak order produces Go's order
/// instead of the panic Rust's own sort may raise.
pub fn slice_stable<T>(data: &mut [T], mut less: impl FnMut(&T, &T) -> bool) {
    let n = data.len();
    let mut block = 20;
    let (mut a, mut b) = (0, block);
    while b <= n {
        insertion_sort(data, a, b, &mut less);
        a = b;
        b += block;
    }
    insertion_sort(data, a, n, &mut less);
    while block < n {
        a = 0;
        b = 2 * block;
        while b <= n {
            sym_merge(data, a, a + block, b, &mut less);
            a = b;
            b += 2 * block;
        }
        let m = a + block;
        if m < n {
            sym_merge(data, a, m, n, &mut less);
        }
        block *= 2;
    }
}

fn insertion_sort<T>(data: &mut [T], a: usize, b: usize, less: &mut impl FnMut(&T, &T) -> bool) {
    for i in a + 1..b {
        let mut j = i;
        while j > a && less(&data[j], &data[j - 1]) {
            data.swap(j, j - 1);
            j -= 1;
        }
    }
}

fn sym_merge<T>(
    data: &mut [T],
    a: usize,
    m: usize,
    b: usize,
    less: &mut impl FnMut(&T, &T) -> bool,
) {
    if m - a == 1 {
        let (mut i, mut j) = (m, b);
        while i < j {
            let h = (i + j) / 2;
            if less(&data[h], &data[a]) {
                i = h + 1;
            } else {
                j = h;
            }
        }
        for k in a..i.saturating_sub(1) {
            data.swap(k, k + 1);
        }
        return;
    }
    if b - m == 1 {
        let (mut i, mut j) = (a, m);
        while i < j {
            let h = (i + j) / 2;
            if !less(&data[m], &data[h]) {
                i = h + 1;
            } else {
                j = h;
            }
        }
        let mut k = m;
        while k > i {
            data.swap(k, k - 1);
            k -= 1;
        }
        return;
    }
    let mid = (a + b) / 2;
    let n = mid + m;
    let (mut start, mut r) = if m > mid { (n - b, mid) } else { (a, m) };
    let p = n - 1;
    while start < r {
        let c = (start + r) / 2;
        if !less(&data[p - c], &data[c]) {
            start = c + 1;
        } else {
            r = c;
        }
    }
    let end = n - start;
    if start < m && m < end {
        rotate(data, start, m, end);
    }
    if a < start && start < mid {
        sym_merge(data, a, start, mid, less);
    }
    if mid < end && end < b {
        sym_merge(data, mid, end, b, less);
    }
}

fn rotate<T>(data: &mut [T], a: usize, m: usize, b: usize) {
    let mut i = m - a;
    let mut j = b - m;
    while i != j {
        if i > j {
            swap_range(data, m - i, m, j);
            i -= j;
        } else {
            swap_range(data, m - i, m + j - i, i);
            j -= i;
        }
    }
    swap_range(data, m - i, m, i);
}

fn swap_range<T>(data: &mut [T], a: usize, b: usize, n: usize) {
    for i in 0..n {
        data.swap(a + i, b + i);
    }
}

/// Go: `strconv.Quote`, the `%q` verb for strings.
/// Uses the same Unicode 15.0.0 printability tables as Go 1.25.14.
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            _ if is_print(c) => out.push(c),
            '\x07' => out.push_str("\\a"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\x0b' => out.push_str("\\v"),
            _ if c < ' ' || c == '\x7f' => {
                let _ = write!(out, "\\x{:02x}", c as u32);
            }
            _ if (c as u32) < 0x10000 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            _ => {
                let _ = write!(out, "\\U{:08x}", c as u32);
            }
        }
    }
    out.push('"');
    out
}

fn is_print(c: char) -> bool {
    use tables::*;
    let r = c as u32;
    if r <= 0xff {
        return (0x20..=0x7e).contains(&r) || ((0xa1..=0xff).contains(&r) && r != 0xad);
    }
    if r < 0x10000 {
        let r = r as u16;
        let i = PRINT16.partition_point(|&v| v < r);
        return i < PRINT16.len()
            && r >= PRINT16[i & !1]
            && r <= PRINT16[i | 1]
            && NOT_PRINT16.binary_search(&r).is_err();
    }
    let i = PRINT32.partition_point(|&v| v < r);
    i < PRINT32.len()
        && r >= PRINT32[i & !1]
        && r <= PRINT32[i | 1]
        && (r >= 0x20000 || NOT_PRINT32.binary_search(&((r - 0x10000) as u16)).is_err())
}

/// Go: `strings.EqualFold`, including special simple-fold cycles.
pub fn equal_fold(a: &str, b: &str) -> bool {
    let mut ai = a.chars();
    let mut bi = b.chars();
    loop {
        match (ai.next(), bi.next()) {
            (None, None) => return true,
            (Some(x), Some(y)) => {
                if x == y {
                    continue;
                }
                let mut folded = simple_fold(x);
                while folded != x && folded != y {
                    folded = simple_fold(folded);
                }
                if folded != y {
                    return false;
                }
            }
            _ => return false,
        }
    }
}

fn simple_fold(c: char) -> char {
    let r = c as u32;
    if let Ok(i) = tables::CASE_ORBIT.binary_search_by_key(&r, |&(from, _)| from) {
        return char::from_u32(tables::CASE_ORBIT[i].1).unwrap();
    }
    let lower = simple_lower(c);
    if lower != c {
        lower
    } else {
        convert_case(c, 0)
    }
}

fn convert_case(c: char, case: usize) -> char {
    let r = c as u32;
    let i = tables::CASE_RANGES.partition_point(|&(_, hi, _)| hi < r);
    let Some(&(lo, _, deltas)) = tables::CASE_RANGES.get(i) else {
        return c;
    };
    if r < lo {
        return c;
    }
    let delta = deltas[case];
    let mapped = if delta > 0x10ffff {
        lo + (((r - lo) & !1) | case as u32)
    } else {
        (r as i32 + delta) as u32
    };
    char::from_u32(mapped).unwrap()
}

fn simple_lower(c: char) -> char {
    convert_case(c, 1)
}

/// Go: `strings.ToLower` — per-rune simple mapping, no context rules
/// (so `İ` lowers to `i` and a final `Σ` to `σ`).
pub fn to_lower(s: &str) -> String {
    s.chars().map(simple_lower).collect()
}

/// Go: the text of a `syscall.Errno` — the C `strerror` message with its
/// first letter lowercased (`is a directory`, `permission denied`). On
/// Windows Go prints the `FormatMessage` text as it is (`The system cannot
/// find the file specified.`), which is std's text without its suffix.
pub fn os_error_text(err: &std::io::Error) -> String {
    let Some(code) = err.raw_os_error() else {
        return err.to_string();
    };
    let full = std::io::Error::from_raw_os_error(code).to_string();
    let text = full
        .strip_suffix(&format!(" (os error {code})"))
        .unwrap_or(&full);
    if cfg!(windows) {
        return text.to_string();
    }
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Go `os.Open`: read-only. Go's Windows `syscall.Open` adds
/// `FILE_FLAG_BACKUP_SEMANTICS` to every read-only open, so a directory
/// opens and its first read fails (`read <path>: Incorrect function.`);
/// std's plain open would fail instead (`open <path>: Access is denied.`).
pub fn open_file(path: impl AsRef<std::path::Path>) -> std::io::Result<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(windows)]
    std::os::windows::fs::OpenOptionsExt::custom_flags(
        &mut opts,
        windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS,
    );
    opts.open(path)
}

/// Go `errors.Is(err, fs.ErrNotExist)` for an OS error: `ENOENT` on Unix;
/// on Windows `ERROR_FILE_NOT_FOUND`, `ERROR_PATH_NOT_FOUND` (a missing
/// parent directory) or `ERROR_BAD_NETPATH`.
pub fn is_not_exist(err: &std::io::Error) -> bool {
    is_not_exist_code(cfg!(windows), err.raw_os_error())
}

/// [`is_not_exist`] for a raw code, with the platform explicit so both
/// tables test everywhere.
pub fn is_not_exist_code(windows: bool, code: Option<i32>) -> bool {
    match code {
        // ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_BAD_NETPATH.
        Some(c) if windows => matches!(c, 2 | 3 | 53),
        // ENOENT is 2 on every supported Unix.
        Some(c) => c == 2,
        None => false,
    }
}

/// Go's `Errno` text for the OS errors that tests provoke. Windows prints
/// the English `FormatMessage` text.
#[cfg(test)]
pub(crate) mod errtext {
    /// A missing file in an existing directory.
    pub const NO_SUCH_FILE: &str = if cfg!(windows) {
        "The system cannot find the file specified."
    } else {
        "no such file or directory"
    };

    /// A missing parent directory (`ERROR_PATH_NOT_FOUND` on Windows).
    pub const NO_SUCH_PATH: &str = if cfg!(windows) {
        "The system cannot find the path specified."
    } else {
        "no such file or directory"
    };

    /// The read of a directory that [`super::open_file`] opened
    /// (`ERROR_INVALID_FUNCTION` on Windows).
    pub const IS_A_DIRECTORY: &str = if cfg!(windows) {
        "Incorrect function."
    } else {
        "is a directory"
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_not_exist_matches_go_err_not_exist() {
        assert!(is_not_exist_code(false, Some(2)));
        assert!(!is_not_exist_code(false, Some(3)), "ESRCH is not ENOENT");
        assert!(
            !is_not_exist_code(false, Some(20)),
            "ENOTDIR stays an error"
        );
        assert!(!is_not_exist_code(false, None));
        for code in [2, 3, 53] {
            assert!(is_not_exist_code(true, Some(code)), "{code}");
        }
        assert!(
            !is_not_exist_code(true, Some(5)),
            "access denied stays an error"
        );
        #[cfg(unix)]
        assert_eq!(libc::ENOENT, 2);
        let missing = std::env::temp_dir()
            .join("rival-no-such-parent-dir")
            .join("x")
            .join("config.yaml");
        assert!(is_not_exist(&std::fs::File::open(missing).unwrap_err()));
    }

    const S: i64 = SECOND as i64;
    const M: i64 = MINUTE as i64;
    const H: i64 = HOUR as i64;

    #[test]
    fn parse_duration_matches_go() {
        let ok: &[(&str, i64)] = &[
            ("0", 0),
            ("-0", 0),
            ("+0", 0),
            ("9223372036854775808ns9223372036854775808ns", 0),
            ("0s", 0),
            ("10m", 10 * M),
            ("30m", 30 * M),
            ("-5m", -5 * M),
            ("+5s", 5 * S),
            ("1.5h", 90 * M),
            ("1h30m", 90 * M),
            ("2h45m", 2 * H + 45 * M),
            ("300ms", 300 * MILLISECOND as i64),
            ("1us", 1_000),
            ("1\u{b5}s", 1_000),
            ("1\u{3bc}s", 1_000),
            ("1ns", 1),
            (".5s", S / 2),
            ("1.s", S),
            ("1.0000000001s", S),
            ("9223372036854775807ns", i64::MAX),
            ("-9223372036854775808ns", i64::MIN),
        ];
        for &(input, want) in ok {
            assert_eq!(parse_duration(input), Ok(want), "{input}");
        }
        let bad: &[(&str, &str)] = &[
            ("", r#"time: invalid duration """#),
            ("-", r#"time: invalid duration "-""#),
            ("banana", r#"time: invalid duration "banana""#),
            (".", r#"time: invalid duration ".""#),
            (".s", r#"time: invalid duration ".s""#),
            ("1", r#"time: missing unit in duration "1""#),
            ("1x", r#"time: unknown unit "x" in duration "1x""#),
            ("1d", r#"time: unknown unit "d" in duration "1d""#),
            ("10 m", r#"time: unknown unit " m" in duration "10 m""#),
            (
                "9223372036854775808ns",
                r#"time: invalid duration "9223372036854775808ns""#,
            ),
            ("3000000h", r#"time: invalid duration "3000000h""#),
            (
                "1é",
                r#"time: unknown unit "\xc3\xa9" in duration "1\xc3\xa9""#,
            ),
        ];
        for &(input, want) in bad {
            assert_eq!(parse_duration(input), Err(want.to_string()), "{input:?}");
        }
    }

    // Go's own time_test.go durationTests.
    #[test]
    fn format_duration_matches_go() {
        let cases: &[(&str, i64)] = &[
            ("0s", 0),
            ("1ns", 1),
            ("1.1\u{b5}s", 1_100),
            ("2.2ms", 2_200_000),
            ("3.3s", 3_300_000_000),
            ("4m5s", 4 * M + 5 * S),
            ("4m5.001s", 4 * M + 5001 * MILLISECOND as i64),
            ("5h6m7.001s", 5 * H + 6 * M + 7001 * MILLISECOND as i64),
            ("8m0.000000001s", 8 * M + 1),
            ("2562047h47m16.854775807s", i64::MAX),
            ("-2562047h47m16.854775808s", i64::MIN),
            ("30m0s", 30 * M),
            ("1h35m0s", 95 * M),
            ("-1.5s", -3 * S / 2),
        ];
        for &(want, d) in cases {
            assert_eq!(format_duration(d), want, "{d}");
        }
    }

    #[test]
    fn slice_stable_sorts_and_keeps_equal_order() {
        // Sizes cover one insertion block, a partial block and several merges.
        for n in [0usize, 1, 2, 19, 20, 21, 40, 41, 97, 200] {
            let mut data: Vec<(u32, usize)> =
                (0..n).map(|i| (((i * 7919) % 13) as u32, i)).collect();
            let mut want = data.clone();
            want.sort_by_key(|&(k, _)| k);
            slice_stable(&mut data, |a, b| a.0 < b.0);
            assert_eq!(data, want, "n={n}");
        }
    }

    #[test]
    fn quote_matches_strconv() {
        let cases = [
            ("", r#""""#),
            ("high", r#""high""#),
            ("a\"b\\c", r#""a\"b\\c""#),
            ("\x07\x08\x0c\n\r\t\x0b", r#""\a\b\f\n\r\t\v""#),
            ("\x00\x1b\x7f", r#""\x00\x1b\x7f""#),
            ("café", r#""café""#),
            ("\u{ad}", r#""\u00ad""#),
            ("\u{a0}", r#""\u00a0""#),
            ("\u{feff}", r#""\ufeff""#),
            ("\u{2028}", r#""\u2028""#),
            ("日本", r#""日本""#),
            ("\u{378}", r#""\u0378""#),
            ("\u{600}", r#""\u0600""#),
            ("\u{301}", "\"\u{301}\""),
            ("\u{10ffff}", r#""\U0010ffff""#),
        ];
        for (input, want) in cases {
            assert_eq!(quote(input), want, "{input:?}");
        }
    }

    #[test]
    fn equal_fold_matches_go() {
        assert!(equal_fold("false", "FALSE"));
        assert!(equal_fold("FaLsE", "false"));
        // Go folds the long s (U+017F) into s and the Kelvin sign into k.
        assert!(equal_fold("fal\u{17f}e", "false"));
        assert!(equal_fold("\u{212a}", "k"));
        assert!(equal_fold("USER", "user"));
        assert!(equal_fold("ß", "ẞ"));
        assert!(equal_fold("Σ", "ς"));
        assert!(!equal_fold("İ", "i"));
        assert!(!equal_fold("ı", "I"));
        assert!(!equal_fold("false ", "false"));
        assert!(!equal_fold("fals", "false"));
        assert!(!equal_fold("0", "false"));
    }

    #[test]
    fn to_lower_uses_simple_mapping() {
        assert_eq!(to_lower("HIGH"), "high");
        assert_eq!(to_lower("MoDeL:"), "model:");
        assert_eq!(to_lower("\u{130}"), "i");
        assert_eq!(to_lower("ΟΔΟΣ"), "οδοσ");
    }

    /// A raw OS error is an errno on Unix but a Win32 code on Windows, where
    /// libc's CRT errno values name unrelated errors (21 is
    /// `ERROR_NOT_READY`).
    #[cfg(unix)]
    #[test]
    fn os_error_text_matches_syscall_errno() {
        let eisdir = std::io::Error::from_raw_os_error(libc::EISDIR);
        assert_eq!(os_error_text(&eisdir), "is a directory");
        let eacces = std::io::Error::from_raw_os_error(libc::EACCES);
        assert_eq!(os_error_text(&eacces), "permission denied");
        let enoent = std::io::Error::from_raw_os_error(libc::ENOENT);
        assert_eq!(os_error_text(&enoent), "no such file or directory");
    }

    /// Go's Windows `Errno.Error`: the English `FormatMessage` text, kept
    /// as it is.
    #[cfg(windows)]
    #[test]
    fn os_error_text_matches_syscall_errno() {
        for (code, want) in [
            (1, "Incorrect function."),
            (2, "The system cannot find the file specified."),
            (3, "The system cannot find the path specified."),
            (5, "Access is denied."),
            (6, "The handle is invalid."),
        ] {
            let err = std::io::Error::from_raw_os_error(code);
            assert_eq!(os_error_text(&err), want, "{code}");
        }
    }
}
