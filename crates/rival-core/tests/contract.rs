//! Session record contract shared with Swift (`app/Tests`).
//! `testdata/expected.json` holds the typed values; `testdata/written/`
//! holds the writer's golden bytes. The tests here never rewrite the golden
//! files; regenerate them on purpose with
//! `cargo test -p rival-core --test contract -- --ignored regenerate_written_golden`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, TimeZone, Timelike};
use rival_core::paths::Paths;
use rival_core::session::{NewSession, Session};
use serde::Deserialize;

/// Every Session field, in the record's field order.
const FIELD_NAMES: [&str; 27] = [
    "id",
    "group_id",
    "cli",
    "mode",
    "model",
    "effort",
    "review_scope",
    "prompt",
    "prompt_preview",
    "prompt_hash",
    "status",
    "start_time",
    "queued_at",
    "queue_position",
    "end_time",
    "exit_code",
    "duration",
    "work_dir",
    "log_file",
    "output_bytes",
    "output_lines",
    "error",
    "account",
    "pid",
    "pid_start",
    "owner_pid",
    "owner_pid_start",
];

fn testdata() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata")
}

/// `[year, month, day, hour, minute, second, nanosecond, utc_offset_seconds]`.
type Time = [i64; 8];

#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fields {
    id: String,
    group_id: String,
    cli: String,
    mode: String,
    model: String,
    effort: String,
    review_scope: String,
    prompt: String,
    prompt_preview: String,
    prompt_hash: String,
    status: String,
    start_time: Option<Time>,
    queued_at: Option<Time>,
    queue_position: i64,
    end_time: Option<Time>,
    exit_code: Option<i64>,
    duration: String,
    work_dir: String,
    log_file: String,
    output_bytes: i64,
    output_lines: i64,
    error: String,
    account: String,
    pid: i64,
    pid_start: i64,
    owner_pid: i64,
    owner_pid_start: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WrittenCase {
    #[serde(default)]
    via_new_queued: bool,
    fields: Fields,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Expected {
    #[serde(rename = "_doc")]
    _doc: Vec<String>,
    sessions: BTreeMap<String, Fields>,
    written: BTreeMap<String, WrittenCase>,
}

fn read_expected<T: serde::de::DeserializeOwned>() -> T {
    let data = fs::read(testdata().join("expected.json")).expect("read expected.json");
    serde_json::from_slice(&data).expect("parse expected.json")
}

fn expected() -> Expected {
    read_expected()
}

fn time_of(t: &Time) -> DateTime<FixedOffset> {
    let part = |i: usize| u32::try_from(t[i]).expect("time part fits u32");
    let naive = NaiveDate::from_ymd_opt(i32::try_from(t[0]).expect("year"), part(1), part(2))
        .and_then(|d| d.and_hms_nano_opt(part(3), part(4), part(5), part(6)))
        .expect("valid time");
    FixedOffset::east_opt(i32::try_from(t[7]).expect("offset"))
        .expect("valid offset")
        .from_local_datetime(&naive)
        .single()
        .expect("unambiguous time")
}

fn parts_of(t: &DateTime<FixedOffset>) -> Time {
    [
        i64::from(t.year()),
        i64::from(t.month()),
        i64::from(t.day()),
        i64::from(t.hour()),
        i64::from(t.minute()),
        i64::from(t.second()),
        i64::from(t.nanosecond()),
        i64::from(t.offset().local_minus_utc()),
    ]
}

/// The session the typed values describe. No `..Default`: a new Session
/// field fails to compile here until the contract covers it.
fn session_of(f: &Fields) -> Session {
    Session {
        id: f.id.clone(),
        group_id: f.group_id.clone(),
        cli: f.cli.clone(),
        mode: f.mode.clone(),
        model: f.model.clone(),
        effort: f.effort.clone(),
        review_scope: f.review_scope.clone(),
        prompt: f.prompt.clone(),
        prompt_preview: f.prompt_preview.as_str().into(),
        prompt_hash: f.prompt_hash.clone(),
        status: f.status.clone(),
        start_time: f.start_time.as_ref().map(time_of),
        queued_at: f.queued_at.as_ref().map(time_of),
        queue_position: f.queue_position,
        end_time: f.end_time.as_ref().map(time_of),
        exit_code: f.exit_code,
        duration: f.duration.clone(),
        work_dir: f.work_dir.clone(),
        log_file: f.log_file.clone(),
        output_bytes: f.output_bytes,
        output_lines: f.output_lines,
        error_msg: f.error.clone(),
        account: f.account.clone(),
        route: String::new(),
        wire_model: String::new(),
        pid: f.pid,
        pid_start: f.pid_start,
        owner_pid: f.owner_pid,
        owner_pid_start: f.owner_pid_start,
        start_mono: Default::default(),
        ephemeral: false,
    }
}

/// The typed values of a decoded session. Times keep their offset, which
/// `DateTime` equality ignores.
fn fields_of(s: &Session) -> Fields {
    Fields {
        id: s.id.clone(),
        group_id: s.group_id.clone(),
        cli: s.cli.clone(),
        mode: s.mode.clone(),
        model: s.model.clone(),
        effort: s.effort.clone(),
        review_scope: s.review_scope.clone(),
        prompt: s.prompt.clone(),
        prompt_preview: String::from_utf8(s.prompt_preview.as_bytes().to_vec())
            .expect("a decoded prompt_preview is valid UTF-8"),
        prompt_hash: s.prompt_hash.clone(),
        status: s.status.clone(),
        start_time: s.start_time.as_ref().map(parts_of),
        queued_at: s.queued_at.as_ref().map(parts_of),
        queue_position: s.queue_position,
        end_time: s.end_time.as_ref().map(parts_of),
        exit_code: s.exit_code,
        duration: s.duration.clone(),
        work_dir: s.work_dir.clone(),
        log_file: s.log_file.clone(),
        output_bytes: s.output_bytes,
        output_lines: s.output_lines,
        error: s.error_msg.clone(),
        account: s.account.clone(),
        pid: s.pid,
        pid_start: s.pid_start,
        owner_pid: s.owner_pid,
        owner_pid_start: s.owner_pid_start,
    }
}

fn write(s: &Session) -> Vec<u8> {
    s.to_json().expect("marshal session")
}

/// The record `new_queued` writes for these inputs, with every field except
/// the generated `prompt_preview` and `prompt_hash` taken from `f`.
fn write_via_new_queued(f: &Fields) -> Vec<u8> {
    let home = tempfile::tempdir().expect("temp home");
    let paths = Paths::from_home(home.path());
    let queued = Session::new_queued(
        &paths,
        NewSession {
            cli: &f.cli,
            mode: &f.mode,
            model: &f.model,
            effort: &f.effort,
            workdir: &f.work_dir,
            prompt: &f.prompt,
            review_scope: &f.review_scope,
            group_id: &f.group_id,
        },
    )
    .expect("new_queued");
    assert_eq!(queued.prompt_hash, f.prompt_hash, "{}: prompt_hash", f.id);
    let mut s = session_of(f);
    s.prompt_preview = queued.prompt_preview;
    s.prompt_hash = queued.prompt_hash;
    write(&s)
}

fn resaved_name(name: &str) -> String {
    format!("{}.resaved.json", name.trim_end_matches(".json"))
}

/// Every golden file under `testdata/written/`, as the Rust writer produces
/// it now.
fn golden_bytes(exp: &Expected) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    for (name, f) in &exp.sessions {
        out.insert(name.clone(), write(&session_of(f)));
    }
    for (name, case) in &exp.written {
        if case.via_new_queued {
            out.insert(name.clone(), write_via_new_queued(&case.fields));
            out.insert(resaved_name(name), write(&session_of(&case.fields)));
        } else {
            out.insert(name.clone(), write(&session_of(&case.fields)));
        }
    }
    out
}

fn read_golden(name: &str) -> Vec<u8> {
    fs::read(testdata().join("written").join(name)).unwrap_or_else(|e| {
        panic!(
            "read testdata/written/{name}: {e}; regenerate with \
             cargo test -p rival-core --test contract -- --ignored regenerate_written_golden"
        )
    })
}

fn assert_bytes(what: &str, got: &[u8], want: &[u8]) {
    assert!(
        got == want,
        "{what}: bytes differ\n--- got\n{}\n--- want\n{}",
        got.escape_ascii(),
        want.escape_ascii()
    );
}

fn decode(what: &str, data: &[u8]) -> Session {
    Session::from_json(data).unwrap_or_else(|e| panic!("{what}: decode: {e}"))
}

fn names_in(dir: &Path) -> BTreeSet<String> {
    fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .map(|e| {
            e.expect("dir entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

/// The keys of a JSON object in document order (serde_json's `Map` sorts).
struct Keys(Vec<String>);

impl<'de> Deserialize<'de> for Keys {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Keys, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = Keys;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("an object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Keys, A::Error> {
                let mut keys = Vec::new();
                while let Some((k, _)) = map.next_entry::<String, serde::de::IgnoredAny>()? {
                    keys.push(k);
                }
                Ok(Keys(keys))
            }
        }
        d.deserialize_map(V)
    }
}

/// expected.json with each fields object reduced to its keys.
#[derive(Deserialize)]
struct KeyOrder {
    sessions: BTreeMap<String, Keys>,
    written: BTreeMap<String, CaseKeys>,
}

#[derive(Deserialize)]
struct CaseKeys {
    fields: Keys,
}

#[test]
fn contract_expected_lists_every_field_in_schema_order() {
    let order: KeyOrder = read_expected();
    let objects = order
        .sessions
        .into_iter()
        .chain(order.written.into_iter().map(|(n, c)| (n, c.fields)));
    for (name, keys) in objects {
        assert_eq!(
            keys.0, FIELD_NAMES,
            "{name}: fields must list every Session field in schema order"
        );
    }
}

#[test]
fn contract_corpus_files_match_expected() {
    let exp = expected();
    let sessions: BTreeSet<String> = exp.sessions.keys().cloned().collect();
    assert_eq!(names_in(&testdata().join("sessions")), sessions);
    assert!(sessions.len() == 10, "the 10 app fixtures");
    let golden: BTreeSet<String> = golden_bytes(&exp).into_keys().collect();
    assert_eq!(names_in(&testdata().join("written")), golden);
}

#[test]
fn contract_fixtures_decode_to_expected() {
    for (name, want) in expected().sessions {
        let data = fs::read(testdata().join("sessions").join(&name)).expect("read fixture");
        assert_eq!(fields_of(&decode(&name, &data)), want, "{name}");
    }
}

#[test]
fn contract_writer_matches_golden_bytes() {
    for (name, got) in golden_bytes(&expected()) {
        assert_bytes(&name, &got, &read_golden(&name));
    }
}

#[test]
fn contract_fixture_load_save_matches_golden() {
    for name in expected().sessions.keys() {
        let data = fs::read(testdata().join("sessions").join(name)).expect("read fixture");
        assert_bytes(name, &write(&decode(name, &data)), &read_golden(name));
    }
}

/// Load a golden record, save it, load that again: the same fields and the
/// same bytes.
#[test]
fn contract_golden_load_save_load() {
    let exp = expected();
    let cases = exp.sessions.iter().map(|(n, f)| (n, f, false)).chain(
        exp.written
            .iter()
            .map(|(n, c)| (n, &c.fields, c.via_new_queued)),
    );
    for (name, want, via_new_queued) in cases {
        let golden = read_golden(name);
        let loaded = decode(name, &golden);
        assert_eq!(fields_of(&loaded), *want, "{name}: decoded");
        let resaved_file = if via_new_queued {
            resaved_name(name)
        } else {
            name.clone()
        };
        let resaved = write(&loaded);
        assert_bytes(
            &format!("{name} resaved"),
            &resaved,
            &read_golden(&resaved_file),
        );
        assert_eq!(
            fields_of(&decode(&resaved_file, &resaved)),
            *want,
            "{resaved_file}"
        );
        assert_bytes(
            &format!("{resaved_file} resaved"),
            &write(&decode(&resaved_file, &resaved)),
            &resaved,
        );
    }
}

#[test]
fn contract_split_preview_bytes_are_literal_replacement_chars() {
    let line = |data: &[u8]| -> Vec<u8> {
        data.split(|&b| b == b'\n')
            .find(|l| l.starts_with(b"  \"prompt_preview\""))
            .expect("prompt_preview line")
            .to_vec()
    };
    let first = line(&read_golden("preview-split-4byte.json"));
    assert!(
        first.ends_with("x\u{FFFD}\u{FFFD}\",".as_bytes()),
        "each stray byte is one U+FFFD: {}",
        first.escape_ascii()
    );
    assert!(
        first.starts_with("  \"prompt_preview\": \"Literal \u{FFFD} kept.".as_bytes()),
        "a literal U+FFFD in the prompt stays literal: {}",
        first.escape_ascii()
    );
    let resaved = line(&read_golden("preview-split-4byte.resaved.json"));
    assert_eq!(resaved, first, "a load and save keeps the bytes");
}

/// A record from an older release: HTML characters as `\u003c`-style
/// escapes and the zero time for "unset". It loads with the same values as
/// the new form, and the next save writes the new form.
#[test]
fn contract_old_escapes_and_zero_times_load_like_the_new_form() {
    let old = br#"{
  "id": "d0000000-0002-4000-8000-000000000002",
  "group_id": "a\u003cb\u0026c\u003e",
  "cli": "codex",
  "status": "running",
  "start_time": "0001-01-01T00:00:00Z",
  "queued_at": "0001-01-01T00:00:00Z",
  "end_time": "0001-01-01T00:00:00Z",
  "pid": 1
}"#;
    let s = decode("old", old);
    assert_eq!(s.group_id, "a<b&c>");
    assert_eq!((s.start_time, s.queued_at, s.end_time), (None, None, None));
    let new = write(&s);
    let text = String::from_utf8(new.clone()).expect("UTF-8");
    assert!(text.contains(r#""group_id": "a<b&c>""#), "{text}");
    assert!(
        !text.contains("_time") && !text.contains("queued_at"),
        "{text}"
    );
    assert_eq!(decode("new", &new), s);
}

/// Rewrites `testdata/written/` from the typed values. Run on purpose only.
#[test]
#[ignore = "rewrites the committed golden files"]
fn regenerate_written_golden() {
    let dir = testdata().join("written");
    fs::create_dir_all(&dir).expect("create testdata/written");
    for (name, bytes) in golden_bytes(&expected()) {
        fs::write(dir.join(&name), bytes).expect("write golden");
    }
}
