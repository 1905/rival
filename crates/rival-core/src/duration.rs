//! Duration text: the `30m` / `1h30m` / `500ms` syntax of
//! `RIVAL_RUN_TIMEOUT` and the `--timeout` flags.

use std::fmt::Write as _;

#[cfg(test)]
mod tests;

const NANOSECOND: u64 = 1;
const MICROSECOND: u64 = 1_000 * NANOSECOND;
const MILLISECOND: u64 = 1_000 * MICROSECOND;
const SECOND: u64 = 1_000 * MILLISECOND;
const MINUTE: u64 = 60 * SECOND;
const HOUR: u64 = 60 * MINUTE;

/// Parses a duration such as `30m`, `1h30m`, `1.5s` or `500ms` into signed
/// nanoseconds. Units: `ns`, `us` (or `µs`), `ms`, `s`, `m`, `h`. A bare `0`
/// needs no unit. A result outside `i64` is an invalid duration.
pub fn parse(s: &str) -> Result<i64, String> {
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
        d = match d.checked_add(v) {
            Some(d) if d <= 1 << 63 => d,
            _ => return invalid(),
        };
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

/// Quotes `s` for an error text: non-ASCII and control bytes become `\xNN`.
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

/// Formats signed nanoseconds, e.g. `1h30m0s`, `1.5s`, `100ms`, `0s`.
pub fn format(d: i64) -> String {
    let neg = d < 0;
    let mut u = d.unsigned_abs();
    // Built back to front.
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
