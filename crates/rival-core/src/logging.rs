//! JSON-lines logger with zerolog's field names and order.
//!
//! A line is
//! `level`, `app`, the per-call fields in call order, `time`, then `message`.
//! `time` is RFC3339 at seconds precision in local time; an empty message is
//! omitted, and duplicate keys are kept as written.

use std::fmt::Display;
use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::{DateTime, FixedOffset, Local, SecondsFormat};

const APP: &str = "rival";

/// Where lines go; `None` means stderr.
static SINK: Mutex<Option<Box<dyn Write + Send>>> = Mutex::new(None);
static ENABLED: AtomicBool = AtomicBool::new(true);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    fn as_str(self) -> &'static str {
        match self {
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Value {
    Str(String),
    Int(i64),
    Bool(bool),
    /// A JSON number already in its final text form.
    Number(String),
}

/// One log line under construction. Finish it with [`Event::msg`].
#[derive(Debug, Clone)]
#[must_use = "a log event is written only by msg()"]
pub struct Event {
    level: Level,
    fields: Vec<(String, Value)>,
}

pub fn debug() -> Event {
    Event::new(Level::Debug)
}

pub fn info() -> Event {
    Event::new(Level::Info)
}

pub fn warn() -> Event {
    Event::new(Level::Warn)
}

pub fn error() -> Event {
    Event::new(Level::Error)
}

impl Event {
    pub fn new(level: Level) -> Self {
        Event {
            level,
            fields: Vec::new(),
        }
    }

    pub fn str(mut self, key: &str, value: impl Into<String>) -> Self {
        self.fields
            .push((key.to_string(), Value::Str(value.into())));
        self
    }

    pub fn int(mut self, key: &str, value: impl Into<i64>) -> Self {
        self.fields
            .push((key.to_string(), Value::Int(value.into())));
        self
    }

    pub fn bool(mut self, key: &str, value: bool) -> Self {
        self.fields.push((key.to_string(), Value::Bool(value)));
        self
    }

    /// zerolog `Dur`: milliseconds as a float (`DurationFieldUnit` =
    /// ms, `DurationFieldInteger` = false), so 5s prints `5000` and 1.5ms
    /// prints `1.5`. Rust's shortest float text equals zerolog's
    /// `strconv.FormatFloat(v, 'f', -1, 64)` only below 1e21 ms; zerolog
    /// prints 'e' from there. A signed 64-bit nanosecond duration (at most
    /// ~9.2e15 ms) never gets there; a larger `Duration` would print wrong.
    pub fn dur(mut self, key: &str, d: Duration) -> Self {
        let ms = d.as_nanos() as f64 / 1e6;
        self.fields
            .push((key.to_string(), Value::Number(format!("{ms}"))));
        self
    }

    /// Adds `"error": <err>`. Uses `{:#}` so an anyhow chain prints as
    /// "outer: inner".
    pub fn err(self, err: impl Display) -> Self {
        self.str("error", format!("{err:#}"))
    }

    /// Formats the line (no trailing newline) with an explicit timestamp.
    pub fn format(&self, msg: &str, time: DateTime<FixedOffset>) -> String {
        let mut line = String::from("{");
        push_pair(&mut line, "level", &Value::Str(self.level.as_str().into()));
        push_pair(&mut line, "app", &Value::Str(APP.into()));
        for (key, value) in &self.fields {
            push_pair(&mut line, key, value);
        }
        let time = time.to_rfc3339_opts(SecondsFormat::Secs, true);
        push_pair(&mut line, "time", &Value::Str(time));
        if !msg.is_empty() {
            push_pair(&mut line, "message", &Value::Str(msg.into()));
        }
        line.push('}');
        line
    }

    /// Writes the line plus a newline to `w` in one call.
    pub fn write_to(
        &self,
        w: &mut dyn Write,
        msg: &str,
        time: DateTime<FixedOffset>,
    ) -> std::io::Result<()> {
        let mut line = self.format(msg, time);
        line.push('\n');
        w.write_all(line.as_bytes())?;
        w.flush()
    }

    /// Writes the line to the global sink, unless logging is disabled.
    pub fn msg(self, msg: &str) {
        if !ENABLED.load(Ordering::Relaxed) {
            return;
        }
        let now = Local::now().fixed_offset();
        let mut sink = SINK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let result = match sink.as_mut() {
            Some(w) => self.write_to(w.as_mut(), msg, now),
            None => self.write_to(&mut std::io::stderr().lock(), msg, now),
        };
        if let Err(err) = result {
            eprintln!("zerolog: could not write event: {err}");
        }
    }
}

fn push_pair(line: &mut String, key: &str, value: &Value) {
    if line.len() > 1 {
        line.push(',');
    }
    line.push_str(&json_string(key));
    line.push(':');
    match value {
        Value::Str(s) => line.push_str(&json_string(s)),
        Value::Int(n) => line.push_str(&n.to_string()),
        Value::Bool(b) => line.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => line.push_str(n),
    }
}

fn json_string(s: &str) -> String {
    serde_json::to_string(s).expect("a str always serializes")
}

/// Startup: logs go to stderr and are enabled.
pub fn init() {
    *SINK.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    ENABLED.store(true, Ordering::Relaxed);
}

/// Sends lines to `w` instead of stderr.
pub fn set_writer(w: Box<dyn Write + Send>) {
    *SINK.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(w);
}

/// Turns all logging on or off. The TUI turns it off.
pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Disables logging until dropped, then restores the previous state.
#[must_use = "logging is re-enabled when the guard drops"]
pub struct Suppressed {
    previous: bool,
}

pub fn suppress() -> Suppressed {
    Suppressed {
        previous: ENABLED.swap(false, Ordering::Relaxed),
    }
}

impl Drop for Suppressed {
    fn drop(&mut self) {
        ENABLED.store(self.previous, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use chrono::TimeZone;
    use serde_json::{Map, Value as Json};

    /// Serializes the tests that touch the global sink or switch.
    static GLOBAL: Mutex<()> = Mutex::new(());

    fn plus3(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<FixedOffset> {
        FixedOffset::east_opt(3 * 3600)
            .unwrap()
            .with_ymd_and_hms(y, mo, d, h, mi, s)
            .unwrap()
    }

    fn parse(line: &str) -> Map<String, Json> {
        serde_json::from_str(line).unwrap()
    }

    /// Object keys in document order, duplicates kept (serde_json's Map sorts and dedups).
    struct Keys(Vec<String>);

    impl<'de> serde::Deserialize<'de> for Keys {
        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            struct V;
            impl<'de> serde::de::Visitor<'de> for V {
                type Value = Keys;
                fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    f.write_str("a JSON object")
                }
                fn visit_map<A: serde::de::MapAccess<'de>>(
                    self,
                    mut map: A,
                ) -> Result<Keys, A::Error> {
                    let mut keys = Vec::new();
                    while let Some((key, _)) = map.next_entry::<String, serde::de::IgnoredAny>()? {
                        keys.push(key);
                    }
                    Ok(Keys(keys))
                }
            }
            d.deserialize_map(V)
        }
    }

    fn keys(line: &str) -> Vec<String> {
        serde_json::from_str::<Keys>(line).unwrap().0
    }

    // Golden lines from zerolog v1.33.0 with
    // zerolog.New(w).With().Timestamp().Str("app", "rival") and a fixed
    // TimestampFunc of 2026-10-02T14:05:09.123456789+03:00.
    const WANT_INFO: &str = r#"{"level":"info","app":"rival","session":"abc123","pid":4242,"detach":true,"error":"boom: \"quoted\"\n<tag>&","time":"2026-10-02T14:05:09+03:00","message":"reaping orphaned session"}"#;
    const WANT_DEBUG_EMPTY: &str =
        r#"{"level":"debug","app":"rival","time":"2026-10-02T14:05:09+03:00"}"#;
    const WANT_WARN_DUP: &str = r#"{"level":"warn","app":"rival","k":"v","k":"dup","neg":-7,"f":false,"time":"2026-10-02T14:05:09+03:00","message":"tab\there é"}"#;
    const WANT_ERROR: &str =
        r#"{"level":"error","app":"rival","time":"2026-10-02T14:05:09+03:00","message":"nil err"}"#;
    const WANT_UTC: &str =
        r#"{"level":"info","app":"rival","time":"2026-01-02T03:04:05Z","message":"utc"}"#;

    #[test]
    fn matches_zerolog_golden_lines() {
        let t = plus3(2026, 10, 2, 14, 5, 9);
        let utc = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 1, 2, 3, 4, 5)
            .unwrap();
        let cases = [
            (
                "info with every field type",
                info()
                    .str("session", "abc123")
                    .int("pid", 4242)
                    .bool("detach", true)
                    .err("boom: \"quoted\"\n<tag>&")
                    .format("reaping orphaned session", t),
                WANT_INFO,
            ),
            (
                "debug, empty message omitted",
                debug().format("", t),
                WANT_DEBUG_EMPTY,
            ),
            (
                "warn, duplicate keys and escapes",
                warn()
                    .str("k", "v")
                    .str("k", "dup")
                    .int("neg", -7)
                    .bool("f", false)
                    .format("tab\there é", t),
                WANT_WARN_DUP,
            ),
            ("error level", error().format("nil err", t), WANT_ERROR),
            ("utc offset prints Z", info().format("utc", utc), WANT_UTC),
        ];
        for (name, rust, go) in cases {
            assert_eq!(parse(&rust), parse(go), "{name}: maps differ");
            assert_eq!(keys(&rust), keys(go), "{name}: key order differs");
            assert_eq!(rust, go, "{name}: bytes differ");
        }
    }

    // Expected text derived from the zerolog v1.33.0 source (not a live run):
    // Dur → AppendDuration(float64(d)/float64(time.Millisecond)) →
    // appendFloat(v, 64, -1) = strconv 'f' -1, as 1ns = 1e-6ms is not below
    // the 1e-6 'e' cutoff. Calls: Str("session","s1"), Dur("grace",5s),
    // Dur("d",1500µs), Dur("z",0), Dur("n",1ns).
    const WANT_DUR: &str = r#"{"level":"warn","app":"rival","session":"s1","grace":5000,"d":1.5,"z":0,"n":0.000001,"time":"2026-10-02T14:05:09+03:00","message":"m"}"#;

    #[test]
    fn dur_matches_zerolog_float_milliseconds() {
        let line = warn()
            .str("session", "s1")
            .dur("grace", Duration::from_secs(5))
            .dur("d", Duration::from_micros(1500))
            .dur("z", Duration::ZERO)
            .dur("n", Duration::from_nanos(1))
            .format("m", plus3(2026, 10, 2, 14, 5, 9));
        assert_eq!(line, WANT_DUR);
    }

    #[test]
    fn err_prints_anyhow_chain_as_outer_colon_inner() {
        let err = anyhow::anyhow!("inner").context("outer");
        let line = warn().err(err).format("x", plus3(2026, 1, 1, 0, 0, 0));
        assert_eq!(parse(&line)["error"], "outer: inner");
    }

    #[test]
    fn write_to_appends_one_newline() {
        let mut buf = Vec::new();
        info()
            .str("a", "b")
            .write_to(&mut buf, "hi", plus3(2026, 1, 1, 0, 0, 0))
            .unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(text.ends_with("}\n"));
        assert_eq!(text.matches('\n').count(), 1);
    }

    #[derive(Clone, Default)]
    struct Shared(Arc<Mutex<Vec<u8>>>);

    impl Write for Shared {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Shared {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    #[test]
    fn global_sink_writes_complete_lines_and_suppress_restores() {
        let _serial = GLOBAL.lock().unwrap_or_else(|p| p.into_inner());
        let buf = Shared::default();
        init();
        set_writer(Box::new(buf.clone()));

        let threads: Vec<_> = (0..8)
            .map(|i| {
                std::thread::spawn(move || {
                    for j in 0..50 {
                        info().int("thread", i).int("n", j).msg("line");
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        // Other tests may log into the shared sink meanwhile; every line must
        // still parse, and ours must all be there.
        let maps: Vec<_> = buf.text().lines().map(parse).collect();
        let ours: Vec<_> = maps
            .iter()
            .filter(|m| m.get("message") == Some(&Json::from("line")))
            .collect();
        assert_eq!(ours.len(), 400);
        for map in ours {
            assert_eq!(map["level"], "info");
            assert_eq!(map["app"], "rival");
            DateTime::parse_from_rfc3339(map["time"].as_str().unwrap()).unwrap();
        }

        {
            let _quiet = suppress();
            assert!(!is_enabled());
            error().msg("hidden");
        }
        assert!(is_enabled());
        set_enabled(false);
        warn().msg("hidden too");
        set_enabled(true);
        assert!(!buf.text().contains("hidden"));

        init();
    }
}
