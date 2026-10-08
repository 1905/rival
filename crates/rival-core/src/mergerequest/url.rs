//! URL parsing for MR targets and remote URLs: which inputs parse, and
//! what `parse_target` rebuilds. IP literals follow the strict address
//! grammar. No caller reads the error text, so a parse error is `None`.
//!
//! This is not WHATWG parsing: the host keeps its spelling and port, only the
//! scheme is lowercased, `%XX` in the path decodes without resolving `.` or
//! `..`, and a bare trailing `?` sets `force_query`. Decoded bytes can be
//! invalid UTF-8, so the host and path are `Vec<u8>`.

#[cfg(test)]
mod tests;

/// The URL fields MR parsing reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Url {
    pub scheme: String,
    pub has_user: bool,
    pub host: Vec<u8>,
    pub path: Vec<u8>,
    pub force_query: bool,
}

/// The escape modes that change behavior for these inputs. The path, user and
/// fragment modes unescape alike once query `+` is out of play.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Path,
    PathSegment,
    Host,
    Zone,
    Other,
}

const UPPERHEX: &[u8; 16] = b"0123456789ABCDEF";

fn unhex(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        _ => c - b'A' + 10,
    }
}

/// Whether `c` must be escaped in the path, path-segment and host modes.
fn should_escape(c: u8, mode: Mode) -> bool {
    if c.is_ascii_alphanumeric() {
        return false;
    }
    let host = matches!(mode, Mode::Host | Mode::Zone);
    if host && b"!$&'()*+,;=:[]<>\"".contains(&c) {
        return false;
    }
    match c {
        b'-' | b'_' | b'.' | b'~' => false,
        b'$' | b'&' | b'+' | b',' | b'/' | b':' | b';' | b'=' | b'?' | b'@' => match mode {
            Mode::Path => c == b'?',
            Mode::PathSegment => matches!(c, b'/' | b';' | b',' | b'?'),
            _ => true,
        },
        _ => true,
    }
}

/// Decodes `%XX` escapes; `None` is a bad escape or an invalid host byte.
fn unescape(s: &[u8], mode: Mode) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        if c == b'%' {
            if i + 2 >= s.len() || !s[i + 1].is_ascii_hexdigit() || !s[i + 2].is_ascii_hexdigit() {
                return None;
            }
            let escaped = &s[i..i + 3] == b"%25";
            let v = (unhex(s[i + 1]) << 4) | unhex(s[i + 2]);
            // Hosts may %-encode only non-ASCII bytes (RFC 3986), plus "%25"
            // in an RFC 6874 zone.
            if mode == Mode::Host && unhex(s[i + 1]) < 8 && !escaped {
                return None;
            }
            if mode == Mode::Zone && !escaped && v != b' ' && should_escape(v, Mode::Host) {
                return None;
            }
            out.push(v);
            i += 3;
            continue;
        }
        if matches!(mode, Mode::Host | Mode::Zone) && c < 0x80 && should_escape(c, mode) {
            return None;
        }
        out.push(c);
        i += 1;
    }
    Some(out)
}

/// Decodes every valid `%XX` escape once and keeps a bad escape as it is.
/// For MR detection only: it never fails, and it never decodes twice.
pub(super) fn decode_once(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i] == b'%'
            && i + 2 < s.len()
            && s[i + 1].is_ascii_hexdigit()
            && s[i + 2].is_ascii_hexdigit()
        {
            out.push((unhex(s[i + 1]) << 4) | unhex(s[i + 2]));
            i += 3;
            continue;
        }
        out.push(s[i]);
        i += 1;
    }
    out
}

/// Escapes `s` for the path, path-segment and host modes.
fn escape(s: &[u8], mode: Mode) -> String {
    let mut out = String::with_capacity(s.len());
    for &c in s {
        if should_escape(c, mode) {
            out.push('%');
            out.push(char::from(UPPERHEX[usize::from(c >> 4)]));
            out.push(char::from(UPPERHEX[usize::from(c & 15)]));
        } else {
            out.push(char::from(c));
        }
    }
    out
}

pub(super) fn path_escape(s: &[u8]) -> String {
    escape(s, Mode::PathSegment)
}

/// The text form of an `https` URL with no user, query or fragment and
/// a path that starts with `/`: what `parse_target` leaves after clearing
/// the raw path, query and fragment. `force_query` still adds `?`.
pub(super) fn https_string(host: &[u8], path: &[u8], force_query: bool) -> String {
    let mut out = format!(
        "https://{}{}",
        escape(host, Mode::Host),
        escape(path, Mode::Path)
    );
    if force_query {
        out.push('?');
    }
    out
}

fn cut(s: &[u8], sep: u8) -> (&[u8], &[u8]) {
    match s.iter().position(|&b| b == sep) {
        Some(i) => (&s[..i], &s[i + 1..]),
        None => (s, &[]),
    }
}

/// `None` is "missing protocol scheme".
fn get_scheme(raw: &[u8]) -> Option<(&[u8], &[u8])> {
    for (i, &c) in raw.iter().enumerate() {
        match c {
            b'a'..=b'z' | b'A'..=b'Z' => {}
            b'0'..=b'9' | b'+' | b'-' | b'.' if i > 0 => {}
            b':' if i == 0 => return None,
            b':' => return Some((&raw[..i], &raw[i + 1..])),
            _ => return Some((&[], raw)),
        }
    }
    Some((&[], raw))
}

/// Parses a URL reference; `None` is a parse error.
pub(super) fn parse(raw: &[u8]) -> Option<Url> {
    let (raw, frag) = cut(raw, b'#');
    if raw.iter().any(|&b| b < b' ' || b == 0x7f) {
        return None;
    }
    let (scheme, mut rest) = get_scheme(raw)?;
    let mut url = Url {
        // getScheme accepts only ASCII letters, digits and "+-.".
        scheme: String::from_utf8_lossy(scheme).to_ascii_lowercase(),
        ..Url::default()
    };
    if rest.ends_with(b"?") && rest.iter().filter(|&&b| b == b'?').count() == 1 {
        url.force_query = true;
        rest = &rest[..rest.len() - 1];
    } else {
        rest = cut(rest, b'?').0;
    }
    // An invalid fragment escape fails the whole parse.
    if !frag.is_empty() {
        unescape(frag, Mode::Other)?;
    }
    if !rest.starts_with(b"/") {
        if !url.scheme.is_empty() {
            // Opaque: no host, no path.
            return Some(url);
        }
        if cut(rest, b'/').0.contains(&b':') {
            return None;
        }
    }
    if (!url.scheme.is_empty() || !rest.starts_with(b"///")) && rest.starts_with(b"//") {
        let authority = &rest[2..];
        let end = authority
            .iter()
            .position(|&b| b == b'/')
            .unwrap_or(authority.len());
        rest = &authority[end..];
        let authority = &authority[..end];
        let at = authority.iter().rposition(|&b| b == b'@');
        url.host = parse_host(at.map_or(authority, |i| &authority[i + 1..]))?;
        if let Some(i) = at {
            let userinfo = &authority[..i];
            if !valid_userinfo(userinfo) {
                return None;
            }
            let (user, password) = cut(userinfo, b':');
            unescape(user, Mode::Other)?;
            unescape(password, Mode::Other)?;
            url.has_user = true;
        }
    }
    url.path = unescape(rest, Mode::Other)?;
    Some(url)
}

/// Parses `host[:port]`, IP literals with RFC 6874 zones.
fn parse_host(host: &[u8]) -> Option<Vec<u8>> {
    match host.iter().rposition(|&b| b == b'[') {
        Some(0) => {
            let close = host.iter().rposition(|&b| b == b']')?;
            let colon_port = &host[close + 1..];
            if !valid_optional_port(colon_port) {
                return None;
            }
            let hostname = &host[1..close];
            let mut out = vec![b'['];
            match hostname.windows(3).position(|w| w == b"%25") {
                Some(zone) => {
                    out.extend(unescape(&hostname[..zone], Mode::Host)?);
                    out.extend(unescape(&hostname[zone..], Mode::Zone)?);
                }
                None => out.extend(unescape(hostname, Mode::Host)?),
            }
            // Only an IPv6 address may be bracketed; IPv4-mapped is fine.
            if parse_addr(&out[1..]) != Some(Addr::V6) {
                return None;
            }
            out.push(b']');
            out.extend(unescape(colon_port, Mode::Host)?);
            Some(out)
        }
        Some(_) => None,
        None => {
            if let Some(i) = host.iter().rposition(|&b| b == b':')
                && !valid_optional_port(&host[i..])
            {
                return None;
            }
            unescape(host, Mode::Host)
        }
    }
}

/// Whether the port part is empty or `:` followed by digits.
fn valid_optional_port(port: &[u8]) -> bool {
    match port.split_first() {
        None => true,
        Some((b':', digits)) => digits.iter().all(u8::is_ascii_digit),
        Some(_) => false,
    }
}

/// It ranges over runes and allows no non-ASCII rune
/// (an invalid byte is U+FFFD), so checking bytes is the same.
fn valid_userinfo(s: &[u8]) -> bool {
    s.iter()
        .all(|&c| c.is_ascii_alphanumeric() || b"-._:~!$&'()*+,;=%@".contains(&c))
}

impl Url {
    /// The host without a valid numeric port and without IP-literal brackets.
    pub(super) fn hostname(&self) -> &[u8] {
        let mut host = &self.host[..];
        if let Some(colon) = host.iter().rposition(|&b| b == b':')
            && valid_optional_port(&host[colon..])
        {
            host = &host[..colon];
        }
        if host.len() >= 2 && host[0] == b'[' && host[host.len() - 1] == b']' {
            host = &host[1..host.len() - 1];
        }
        host
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Addr {
    V4,
    V6,
}

/// Parses an IP address, reporting only the family; `None` is an error.
fn parse_addr(s: &[u8]) -> Option<Addr> {
    match s.iter().find(|&&c| matches!(c, b'.' | b':' | b'%'))? {
        b'.' => ipv4_fields(s).then_some(Addr::V4),
        b':' => parse_ipv6(s),
        _ => None,
    }
}

/// Whether `s` is four decimal octets with no leading zeros.
fn ipv4_fields(s: &[u8]) -> bool {
    let mut val = 0u32;
    let mut pos = 0;
    let mut dig_len = 0;
    for (i, &c) in s.iter().enumerate() {
        if c.is_ascii_digit() {
            if dig_len == 1 && val == 0 {
                return false;
            }
            val = val * 10 + u32::from(c - b'0');
            dig_len += 1;
            if val > 255 {
                return false;
            }
        } else if c == b'.' {
            if i == 0 || i == s.len() - 1 || s[i - 1] == b'.' || pos == 3 {
                return false;
            }
            pos += 1;
            val = 0;
            dig_len = 0;
        } else {
            return false;
        }
    }
    pos == 3
}

fn parse_ipv6(input: &[u8]) -> Option<Addr> {
    let mut s = input;
    if let Some(i) = s.iter().position(|&b| b == b'%') {
        if i + 1 == s.len() {
            return None; // an explicit zone must not be empty
        }
        s = &s[..i];
    }
    let mut ellipsis = false;
    if s.starts_with(b"::") {
        ellipsis = true;
        s = &s[2..];
        if s.is_empty() {
            return Some(Addr::V6);
        }
    }
    let mut i = 0;
    while i < 16 {
        let digits = s.iter().take_while(|c| c.is_ascii_hexdigit()).count();
        // At most four digits; their value then fits 16 bits.
        if digits == 0 || digits > 4 {
            return None;
        }
        if s.get(digits) == Some(&b'.') {
            // An embedded IPv4 replaces the last two fields.
            if (!ellipsis && i != 12) || i + 4 > 16 || !ipv4_fields(s) {
                return None;
            }
            s = &[];
            i += 4;
            break;
        }
        i += 2;
        s = &s[digits..];
        if s.is_empty() {
            break;
        }
        if s[0] != b':' || s.len() == 1 {
            return None;
        }
        s = &s[1..];
        if s[0] == b':' {
            if ellipsis {
                return None;
            }
            ellipsis = true;
            s = &s[1..];
            if s.is_empty() {
                break;
            }
        }
    }
    // The whole string is used, and "::" stands for at least one zero field.
    let full = s.is_empty() && if i < 16 { ellipsis } else { !ellipsis };
    full.then_some(Addr::V6)
}
