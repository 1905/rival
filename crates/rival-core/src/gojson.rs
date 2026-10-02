//! Go `encoding/json` behaviors that ported records depend on byte for byte:
//! `json.MarshalIndent(v, "", "  ")` output, `time.Time` JSON text, and what
//! the decoder accepts (case-insensitive keys, duplicate keys, `null`, invalid
//! UTF-8). The rules follow the legacy `encode.go`/`decode.go` that Go 1.25,
//! the release toolchain, builds with.

use std::borrow::Cow;
use std::fmt;
use std::io;

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, TimeZone, Timelike};
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::ser::{Formatter, PrettyFormatter};
use serde_json::value::RawValue;

use crate::gostd;

/// Go: `json.MarshalIndent(v, "", "  ")`. Two-space indent, no final newline,
/// and Go's HTML-safe escaping of `<`, `>`, `&`, U+2028 and U+2029.
pub fn marshal_indent<T: Serialize + ?Sized>(value: &T) -> serde_json::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(512);
    let mut ser = serde_json::Serializer::with_formatter(
        &mut out,
        GoFormatter(PrettyFormatter::with_indent(b"  ")),
    );
    value.serialize(&mut ser)?;
    Ok(out)
}

/// A pretty formatter with Go's extra string escapes.
struct GoFormatter<'a>(PrettyFormatter<'a>);

impl Formatter for GoFormatter<'_> {
    fn begin_array<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.0.begin_array(w)
    }
    fn end_array<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.0.end_array(w)
    }
    fn begin_array_value<W: ?Sized + io::Write>(
        &mut self,
        w: &mut W,
        first: bool,
    ) -> io::Result<()> {
        self.0.begin_array_value(w, first)
    }
    fn end_array_value<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.0.end_array_value(w)
    }
    fn begin_object<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.0.begin_object(w)
    }
    fn end_object<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.0.end_object(w)
    }
    fn begin_object_key<W: ?Sized + io::Write>(
        &mut self,
        w: &mut W,
        first: bool,
    ) -> io::Result<()> {
        self.0.begin_object_key(w, first)
    }
    fn begin_object_value<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.0.begin_object_value(w)
    }
    fn end_object_value<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.0.end_object_value(w)
    }

    fn write_string_fragment<W: ?Sized + io::Write>(
        &mut self,
        w: &mut W,
        fragment: &str,
    ) -> io::Result<()> {
        let mut start = 0;
        for (i, c) in fragment.char_indices() {
            let esc = match c {
                '<' => "\\u003c",
                '>' => "\\u003e",
                '&' => "\\u0026",
                '\u{2028}' => "\\u2028",
                '\u{2029}' => "\\u2029",
                _ => continue,
            };
            w.write_all(&fragment.as_bytes()[start..i])?;
            w.write_all(esc.as_bytes())?;
            start = i + c.len_utf8();
        }
        w.write_all(&fragment.as_bytes()[start..])
    }
}

/// Decodes the rune at the start of `s` the way Go's `utf8.DecodeRune` does:
/// `None` with width 1 for an invalid byte, `None` with width 0 for empty input.
fn decode_rune(s: &[u8]) -> (Option<char>, usize) {
    let Some(&lead) = s.first() else {
        return (None, 0);
    };
    let width = match lead {
        0x00..=0x7f => 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return (None, 1),
    };
    match s.get(..width).and_then(|b| std::str::from_utf8(b).ok()) {
        Some(text) => (text.chars().next(), width),
        None => (None, 1),
    }
}

/// Go: `appendString(dst, src, escapeHTML=true)` from the legacy encoder.
/// Each byte of an invalid UTF-8 sequence becomes the six ASCII bytes
/// `�`; a valid U+FFFD stays literal.
pub fn quote_bytes(src: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(src.len() + 2);
    out.push('"');
    let mut i = 0;
    while i < src.len() {
        match decode_rune(&src[i..]) {
            (Some(c), n) if c.is_ascii() => {
                match c {
                    '\\' | '"' => {
                        out.push('\\');
                        out.push(c);
                    }
                    '\u{8}' => out.push_str("\\b"),
                    '\u{c}' => out.push_str("\\f"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    '<' | '>' | '&' | '\0'..='\u{1f}' => {
                        let b = c as u8;
                        out.push_str("\\u00");
                        out.push(HEX[usize::from(b >> 4)] as char);
                        out.push(HEX[usize::from(b & 0xf)] as char);
                    }
                    _ => out.push(c),
                }
                i += n;
            }
            (Some('\u{2028}'), n) => {
                out.push_str("\\u2028");
                i += n;
            }
            (Some('\u{2029}'), n) => {
                out.push_str("\\u2029");
                i += n;
            }
            (Some(c), n) => {
                out.push(c);
                i += n;
            }
            (None, _) => {
                out.push_str("\\ufffd");
                i += 1;
            }
        }
    }
    out.push('"');
    out
}

/// Go: a `[]byte` → `string` conversion as `encoding/json` decodes it. Every
/// byte of an invalid UTF-8 sequence becomes its own U+FFFD.
pub fn lossy_utf8(mut bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    loop {
        match std::str::from_utf8(bytes) {
            Ok(s) => {
                out.push_str(s);
                return out;
            }
            Err(e) => {
                let valid = e.valid_up_to();
                out.push_str(std::str::from_utf8(&bytes[..valid]).expect("valid prefix"));
                out.push('\u{FFFD}');
                bytes = &bytes[valid + 1..];
            }
        }
    }
}

/// A Go `string`: any bytes, usually UTF-8. Go writes an invalid byte as the
/// escape `�` and a valid U+FFFD literally, so the bytes are kept until
/// serialization. A decoded value is always valid UTF-8, as in Go.
#[derive(Clone, Default, PartialEq, Eq, Hash)]
pub struct GoString(Vec<u8>);

impl GoString {
    pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> GoString {
        GoString(bytes.into())
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The text with each invalid byte as U+FFFD (what Go's decoder reads back).
    pub fn to_str_lossy(&self) -> Cow<'_, str> {
        match std::str::from_utf8(&self.0) {
            Ok(s) => Cow::Borrowed(s),
            Err(_) => Cow::Owned(lossy_utf8(&self.0)),
        }
    }
}

impl From<&str> for GoString {
    fn from(s: &str) -> GoString {
        GoString(s.as_bytes().to_vec())
    }
}

impl From<String> for GoString {
    fn from(s: String) -> GoString {
        GoString(s.into_bytes())
    }
}

impl PartialEq<str> for GoString {
    fn eq(&self, other: &str) -> bool {
        self.0 == other.as_bytes()
    }
}

impl PartialEq<&str> for GoString {
    fn eq(&self, other: &&str) -> bool {
        self.0 == other.as_bytes()
    }
}

impl PartialEq<String> for GoString {
    fn eq(&self, other: &String) -> bool {
        self.0 == other.as_bytes()
    }
}

impl fmt::Debug for GoString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match std::str::from_utf8(&self.0) {
            Ok(s) => fmt::Debug::fmt(s, f),
            Err(_) => write!(f, "GoString({:?})", self.0),
        }
    }
}

impl fmt::Display for GoString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_str_lossy())
    }
}

/// Valid UTF-8 serializes as a plain string. Otherwise the Go string literal
/// goes out as a raw JSON fragment, so only serde_json writes it byte-exact.
impl Serialize for GoString {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match std::str::from_utf8(&self.0) {
            Ok(text) => s.serialize_str(text),
            Err(_) => RawValue::from_string(quote_bytes(&self.0))
                .map_err(serde::ser::Error::custom)?
                .serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for GoString {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<GoString, D::Error> {
        String::deserialize(d).map(GoString::from)
    }
}

// ---- decoding ----

/// Go's `maxNestingDepth`: the scanner rejects deeper documents.
const MAX_NESTING_DEPTH: usize = 10_000;

/// Rewrites a document so serde_json reads it as Go's decoder does. Inside
/// strings, each invalid UTF-8 byte becomes U+FFFD and an unpaired surrogate
/// escape becomes `�` (Go's `unquote`). Nesting deeper than Go's limit
/// fails with Go's message. Everything outside strings is copied unchanged,
/// so syntax errors stay syntax errors.
fn prepare(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(data.len());
    let mut depth = 0usize;
    let mut in_string = false;
    let mut i = 0;
    while i < data.len() {
        let b = data[i];
        if !in_string {
            match b {
                b'"' => in_string = true,
                b'{' | b'[' => {
                    depth += 1;
                    if depth > MAX_NESTING_DEPTH {
                        return Err("exceeded max depth".to_string());
                    }
                }
                b'}' | b']' => depth = depth.saturating_sub(1),
                _ => {}
            }
            out.push(b);
            i += 1;
            continue;
        }
        match b {
            b'"' => {
                in_string = false;
                out.push(b);
                i += 1;
            }
            b'\\' => match hex_escape(&data[i..]) {
                Some(r) if (0xd800..0xe000).contains(&r) => {
                    let paired = (0xd800..0xdc00).contains(&r)
                        && hex_escape(&data[i + 6..])
                            .is_some_and(|r2| (0xdc00..0xe000).contains(&r2));
                    if paired {
                        out.extend_from_slice(&data[i..i + 12]);
                        i += 12;
                    } else {
                        out.extend_from_slice(b"\\ufffd");
                        i += 6;
                    }
                }
                _ => {
                    // A plain escape (or a bad one serde_json will reject).
                    let end = (i + 2).min(data.len());
                    out.extend_from_slice(&data[i..end]);
                    i = end;
                }
            },
            0x00..=0x7f => {
                out.push(b);
                i += 1;
            }
            _ => match decode_rune(&data[i..]) {
                (Some(c), n) => {
                    out.extend_from_slice(&data[i..i + n]);
                    debug_assert_eq!(c.len_utf8(), n);
                    i += n;
                }
                (None, _) => {
                    out.extend_from_slice("\u{FFFD}".as_bytes());
                    i += 1;
                }
            },
        }
    }
    Ok(out)
}

/// Go: `getu4` — the code unit of a `\uXXXX` escape at the start of `s`.
fn hex_escape(s: &[u8]) -> Option<u32> {
    if s.len() < 6 || s[0] != b'\\' || s[1] != b'u' {
        return None;
    }
    let text = std::str::from_utf8(&s[2..6]).ok()?;
    if !text.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(text, 16).ok()
}

/// One object member, in document order, duplicates kept.
pub type Member = (String, Box<RawValue>);

struct Members(Vec<Member>);

impl<'de> Deserialize<'de> for Members {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Members, D::Error> {
        struct MembersVisitor;
        impl<'de> Visitor<'de> for MembersVisitor {
            type Value = Members;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Members, A::Error> {
                let mut members = Vec::new();
                while let Some(member) = map.next_entry::<String, Box<RawValue>>()? {
                    members.push(member);
                }
                Ok(Members(members))
            }
        }
        d.deserialize_map(MembersVisitor)
    }
}

/// The `Value` word of Go's `UnmarshalTypeError` for a raw JSON value.
fn kind(raw: &str) -> &'static str {
    match raw.as_bytes().first() {
        Some(b'"') => "string",
        Some(b'{') => "object",
        Some(b'[') => "array",
        Some(b't' | b'f') => "bool",
        Some(b'n') => "null",
        _ => "number",
    }
}

/// Go: `json.Unmarshal(data, &v)` for a struct `v`, up to the field
/// assignments. Returns the members in document order with duplicates, so
/// the caller can assign them in turn as Go does. A top-level `null` has no
/// members (Go leaves the struct unchanged). `go_type` names the struct in
/// Go's type-mismatch message, e.g. `session.Session`.
pub fn decode_object(data: &[u8], go_type: &str) -> Result<Vec<Member>, String> {
    let data = prepare(data)?;
    let value: Box<RawValue> = serde_json::from_slice(&data).map_err(|e| e.to_string())?;
    match kind(value.get()) {
        "null" => Ok(Vec::new()),
        "object" => serde_json::from_str::<Members>(value.get())
            .map(|m| m.0)
            .map_err(|e| e.to_string()),
        other => Err(format!(
            "json: cannot unmarshal {other} into Go value of type {go_type}"
        )),
    }
}

/// Go: `json.Valid` for a value Go will decode later. Returns the value as a
/// raw fragment in the form [`decode_object`] reads.
pub fn valid_value(data: &[u8]) -> Option<Box<RawValue>> {
    serde_json::from_slice(&prepare(data).ok()?).ok()
}

/// Go's decoder field lookup: an exact name match first, else a
/// case-insensitive one (`strings.EqualFold` rules, so U+212A matches `k`
/// and U+017F matches `s`).
pub fn match_field<'a>(names: &[&'a str], key: &str) -> Option<&'a str> {
    names
        .iter()
        .find(|name| **name == key)
        .or_else(|| names.iter().find(|name| gostd::equal_fold(name, key)))
        .copied()
}

/// A JSON value that does not fit a Go field. Holds the `Value` word of
/// Go's `UnmarshalTypeError` ("string", "bool", "number 1.5", ...).
#[derive(Debug, PartialEq)]
pub struct TypeMismatch(pub String);

/// Go: decoding into a `string` field. `Ok(None)` is `null`, which Go skips.
pub fn decode_string(raw: &RawValue) -> Result<Option<String>, TypeMismatch> {
    match kind(raw.get()) {
        "null" => Ok(None),
        "string" => serde_json::from_str(raw.get())
            .map(Some)
            .map_err(|_| TypeMismatch("string".to_string())),
        other => Err(TypeMismatch(other.to_string())),
    }
}

/// Go: decoding into an `int` or `int64` field (64-bit): `strconv.ParseInt`
/// of the number literal. `Ok(None)` is `null`.
pub fn decode_int(raw: &RawValue) -> Result<Option<i64>, TypeMismatch> {
    match kind(raw.get()) {
        "null" => Ok(None),
        "number" => raw
            .get()
            .parse::<i64>()
            .map(Some)
            .map_err(|_| TypeMismatch(format!("number {}", raw.get()))),
        other => Err(TypeMismatch(other.to_string())),
    }
}

/// Go: `time.Time.UnmarshalJSON` on the raw value. It strips the quotes
/// without unescaping. `Ok(None)` is `null`: a value field keeps its time,
/// a pointer field becomes nil.
pub fn decode_time(raw: &RawValue) -> Result<Option<DateTime<FixedOffset>>, String> {
    let data = raw.get().as_bytes();
    if data == b"null" {
        return Ok(None);
    }
    if data.len() < 2 || data[0] != b'"' || data[data.len() - 1] != b'"' {
        return Err("Time.UnmarshalJSON: input is not a JSON string".to_string());
    }
    parse_time(&data[1..data.len() - 1]).map(Some)
}

// ---- time ----

/// Go's zero `time.Time`: 0001-01-01T00:00:00Z.
pub fn zero_time() -> DateTime<FixedOffset> {
    NaiveDate::from_ymd_opt(1, 1, 1)
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .expect("valid date")
        .and_utc()
        .fixed_offset()
}

/// Go: `time.Time.MarshalJSON` text without quotes (RFC 3339 with the
/// fraction's trailing zeros trimmed, `Z` for a zero offset).
pub fn format_time(t: &DateTime<FixedOffset>) -> Result<String, String> {
    let year = t.year();
    if !(0..=9999).contains(&year) {
        return Err("Time.MarshalJSON: year outside of range [0,9999]".to_string());
    }
    let mut out = format!(
        "{year:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        t.month(),
        t.day(),
        t.hour(),
        t.minute(),
        t.second()
    );
    let nanos = t.nanosecond() % 1_000_000_000;
    if nanos != 0 {
        let frac = format!("{nanos:09}");
        out.push('.');
        out.push_str(frac.trim_end_matches('0'));
    }
    let offset = t.offset().local_minus_utc();
    if offset == 0 {
        out.push('Z');
    } else {
        let sign = if offset < 0 { '-' } else { '+' };
        let mins = offset.unsigned_abs() / 60;
        out.push_str(&format!("{sign}{:02}:{:02}", mins / 60, mins % 60));
    }
    Ok(out)
}

const RFC3339: &str = "2006-01-02T15:04:05Z07:00";

/// Parsed time fields; `offset` is seconds east of UTC.
struct TimeFields {
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    min: u32,
    sec: u32,
    nsec: u32,
    offset: i32,
}

/// Go: `parseStrictRFC3339`, the parser behind `time.Time.UnmarshalJSON`.
/// The fast RFC 3339 path runs first. On failure Go falls back to
/// `time.Parse(time.RFC3339, s)` with its strict checks disabled, which also
/// accepts a one-digit hour, a comma before the fraction, and zone offsets up
/// to +24:60. Lowercase `t`/`z` and a space separator are rejected.
///
/// Rust-only limit: an offset of 24h or more (Go accepts up to +24:60) does
/// not fit `FixedOffset` and is an error.
pub fn parse_time(s: &[u8]) -> Result<DateTime<FixedOffset>, String> {
    let f = match parse_rfc3339(s) {
        Some(f) => f,
        None => parse_layout(s)?,
    };
    let naive = NaiveDate::from_ymd_opt(f.year, f.month, f.day)
        .and_then(|d| d.and_hms_nano_opt(f.hour, f.min, f.sec, f.nsec));
    let zone = FixedOffset::east_opt(f.offset);
    match (naive, zone) {
        (Some(naive), Some(zone)) => zone
            .from_local_datetime(&naive)
            .single()
            .ok_or_else(|| format!("parsing time {}: out of range", time_quote(s))),
        (None, _) => Err(format!("parsing time {}: out of range", time_quote(s))),
        (Some(_), None) => Err(format!(
            "parsing time {}: time zone offset not supported",
            time_quote(s)
        )),
    }
}

fn is_digit(s: &[u8], i: usize) -> bool {
    s.get(i).is_some_and(u8::is_ascii_digit)
}

fn is_leap(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// Go: `daysIn(month, year)`.
fn days_in(month: u32, year: i32) -> u32 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Go: `parseNanoseconds(value, nbytes)` once the caller has checked the
/// separator and digits. Digits after the ninth are dropped.
fn parse_nanoseconds(value: &[u8], nbytes: usize) -> u32 {
    let nbytes = nbytes.min(10);
    let mut ns: u32 = 0;
    for &c in &value[1..nbytes] {
        ns = ns * 10 + u32::from(c - b'0');
    }
    for _ in nbytes..10 {
        ns *= 10;
    }
    ns
}

/// Go: the fast path `parseRFC3339(s, Local)`.
fn parse_rfc3339(s: &[u8]) -> Option<TimeFields> {
    let mut ok = true;
    let mut parse_uint = |b: &[u8], min: u32, max: u32| -> u32 {
        let mut x: u32 = 0;
        for &c in b {
            if !c.is_ascii_digit() {
                ok = false;
                return min;
            }
            x = x * 10 + u32::from(c - b'0');
        }
        if x < min || max < x {
            ok = false;
            return min;
        }
        x
    };
    if s.len() < RFC3339.len() - "Z07:00".len() {
        return None;
    }
    let year = parse_uint(&s[0..4], 0, 9999) as i32;
    let month = parse_uint(&s[5..7], 1, 12);
    let day = parse_uint(&s[8..10], 1, days_in(month, year));
    let hour = parse_uint(&s[11..13], 0, 23);
    let min = parse_uint(&s[14..16], 0, 59);
    let sec = parse_uint(&s[17..19], 0, 59);
    let separators =
        s[4] == b'-' && s[7] == b'-' && s[10] == b'T' && s[13] == b':' && s[16] == b':';
    let mut s = &s[19..];

    let mut nsec = 0;
    if s.len() >= 2 && s[0] == b'.' && is_digit(s, 1) {
        let mut n = 2;
        while n < s.len() && is_digit(s, n) {
            n += 1;
        }
        nsec = parse_nanoseconds(s, n);
        s = &s[n..];
    }

    let mut offset = 0;
    if s != b"Z" {
        if s.len() != "-07:00".len() {
            return None;
        }
        let hr = parse_uint(&s[1..3], 0, 23);
        let mm = parse_uint(&s[4..6], 0, 59);
        if !((s[0] == b'-' || s[0] == b'+') && s[3] == b':') {
            return None;
        }
        offset = ((hr * 60 + mm) * 60) as i32;
        if s[0] == b'-' {
            offset = -offset;
        }
    }
    if !ok || !separators {
        return None;
    }
    Some(TimeFields {
        year,
        month,
        day,
        hour,
        min,
        sec,
        nsec,
        offset,
    })
}

/// Go: `getnum(s, fixed)`.
fn getnum(s: &[u8], fixed: bool) -> Option<(u32, &[u8])> {
    if !is_digit(s, 0) {
        return None;
    }
    let first = u32::from(s[0] - b'0');
    if !is_digit(s, 1) {
        return (!fixed).then_some((first, &s[1..]));
    }
    Some((first * 10 + u32::from(s[1] - b'0'), &s[2..]))
}

/// Go: `time.parse(RFC3339, value, UTC, Local)`, unrolled for the layout
/// `2006-01-02T15:04:05Z07:00`, with Go's `ParseError` texts.
fn parse_layout(value: &[u8]) -> Result<TimeFields, String> {
    let cannot = |value_elem: &[u8], layout_elem: &str| {
        format!(
            "parsing time {} as {}: cannot parse {} as {}",
            time_quote(value),
            time_quote(RFC3339.as_bytes()),
            time_quote(value_elem),
            time_quote(layout_elem.as_bytes())
        )
    };
    let out_of_range =
        |what: &str| format!("parsing time {}: {what} out of range", time_quote(value));
    // Go: `skip(value, prefix)` for a one-byte literal.
    let expect = |v: &[u8], prefix: u8| -> Result<(), String> {
        match v.first() {
            Some(&c) if c == prefix => Ok(()),
            _ => Err(cannot(v, std::str::from_utf8(&[prefix]).expect("ASCII"))),
        }
    };

    // stdLongYear
    let mut v = value;
    if v.len() < 4 || !is_digit(v, 0) || !v[1..4].iter().all(u8::is_ascii_digit) {
        return Err(cannot(v, "2006"));
    }
    let year = v[..4]
        .iter()
        .fold(0i32, |x, &c| x * 10 + i32::from(c - b'0'));
    v = &v[4..];
    expect(v, b'-')?;
    v = &v[1..];

    // stdZeroMonth
    let (month, rest) = getnum(v, true).ok_or_else(|| cannot(v, "01"))?;
    if !(1..=12).contains(&month) {
        return Err(out_of_range("month"));
    }
    expect(rest, b'-')?;
    v = &rest[1..];

    // stdZeroDay; the range is checked after the whole layout.
    let (day, rest) = getnum(v, true).ok_or_else(|| cannot(v, "02"))?;
    expect(rest, b'T')?;
    v = &rest[1..];

    // stdHour: one or two digits.
    let (hour, rest) = getnum(v, false).ok_or_else(|| cannot(v, "15"))?;
    if hour >= 24 {
        return Err(out_of_range("hour"));
    }
    expect(rest, b':')?;
    v = &rest[1..];

    // stdZeroMinute
    let (min, rest) = getnum(v, true).ok_or_else(|| cannot(v, "04"))?;
    if min >= 60 {
        return Err(out_of_range("minute"));
    }
    expect(rest, b':')?;
    v = &rest[1..];

    // stdZeroSecond, with a fraction after '.' or ','.
    let (sec, rest) = getnum(v, true).ok_or_else(|| cannot(v, "05"))?;
    if sec >= 60 {
        return Err(out_of_range("second"));
    }
    v = rest;
    let mut nsec = 0;
    if v.len() >= 2 && (v[0] == b'.' || v[0] == b',') && is_digit(v, 1) {
        let mut n = 2;
        while n < v.len() && is_digit(v, n) {
            n += 1;
        }
        nsec = parse_nanoseconds(v, n);
        v = &v[n..];
    }

    // stdISO8601ColonTZ
    let hold = v;
    let mut offset = 0;
    if v.first() == Some(&b'Z') {
        v = &v[1..];
    } else {
        if v.len() < 6 || v[3] != b':' {
            return Err(cannot(hold, "Z07:00"));
        }
        let (sign, hh, mm) = (v[0], &v[1..3], &v[4..6]);
        v = &v[6..];
        let hr = getnum(hh, true);
        let mn = hr.and_then(|_| getnum(mm, true));
        let mut bad = hr.is_none() || mn.is_none();
        let hr = hr.map_or(0, |(h, _)| h);
        let mn = mn.map_or(0, |(m, _)| m);
        // Go's range test uses > because some offsets are written as 24h or 60m.
        let mut range = None;
        if hr > 24 {
            range = Some("time zone offset hour");
        }
        if mn > 60 {
            range = Some("time zone offset minute");
        }
        offset = ((hr * 60 + mn) * 60) as i32;
        match sign {
            b'+' => {}
            b'-' => offset = -offset,
            _ => bad = true,
        }
        if let Some(what) = range {
            return Err(out_of_range(what));
        }
        if bad {
            return Err(cannot(hold, "Z07:00"));
        }
    }

    if !v.is_empty() {
        return Err(format!(
            "parsing time {}: extra text: {}",
            time_quote(value),
            time_quote(v)
        ));
    }
    if day < 1 || day > days_in(month, year) {
        return Err(out_of_range("day"));
    }
    Ok(TimeFields {
        year,
        month,
        day,
        hour,
        min,
        sec,
        nsec,
        offset,
    })
}

/// Go: the `time` package's own `quote` for error text. Non-ASCII and
/// control bytes print as `\xHH`.
fn time_quote(s: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    let mut i = 0;
    while i < s.len() {
        let (c, n) = decode_rune(&s[i..]);
        match c {
            Some(c) if (' '..'\u{80}').contains(&c) => {
                if c == '"' || c == '\\' {
                    out.push('\\');
                }
                out.push(c);
            }
            _ => {
                // An invalid byte is one RuneError of width 1; a real U+FFFD
                // and every other rune print all their bytes.
                for &b in &s[i..i + n] {
                    out.push_str("\\x");
                    out.push(HEX[usize::from(b >> 4)] as char);
                    out.push(HEX[usize::from(b & 0xf)] as char);
                }
            }
        }
        i += n;
    }
    out.push('"');
    out
}

/// Serde adapter for a Go `time.Time` field (serialization only; records
/// decode through [`decode_time`]).
pub mod time {
    use super::*;

    pub fn serialize<S: Serializer>(t: &DateTime<FixedOffset>, s: S) -> Result<S::Ok, S::Error> {
        let text = format_time(t).map_err(serde::ser::Error::custom)?;
        s.serialize_str(&text)
    }
}

/// Serde adapter for a Go `*time.Time` field: `None` is nil, and a pointer to
/// the zero time still serializes.
pub mod opt_time {
    use super::*;

    pub fn serialize<S: Serializer>(
        t: &Option<DateTime<FixedOffset>>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        match t {
            Some(t) => super::time::serialize(t, s),
            None => s.serialize_none(),
        }
    }
}

#[cfg(test)]
mod tests;
