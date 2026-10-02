use std::fs;
use std::io::Write as _;
use std::thread;

use chrono::{DateTime, Duration, FixedOffset, Local, TimeZone, Timelike};

use std::time::{Duration as StdDuration, Instant};

use super::summary::{load_all_summaries, load_summary_file};
use super::*;
use crate::gojson::zero_time;

fn temp_paths() -> (tempfile::TempDir, Paths) {
    let home = tempfile::tempdir().unwrap();
    let paths = Paths::from_home(home.path());
    (home, paths)
}

fn queued(paths: &Paths, prompt: &str) -> Session {
    Session::new_queued(
        paths,
        NewSession {
            cli: "codex",
            mode: "review",
            model: "gpt-5.5",
            effort: "high",
            workdir: "/work",
            prompt,
            ..NewSession::default()
        },
    )
    .unwrap()
}

fn member(
    id: &str,
    cli: &str,
    model: &str,
    mode: &str,
    queued_at: Option<DateTime<FixedOffset>>,
) -> Session {
    Session {
        id: id.into(),
        cli: cli.into(),
        model: model.into(),
        mode: mode.into(),
        queued_at,
        ..Session::default()
    }
}

fn ids(sessions: &[Session]) -> Vec<&str> {
    sessions.iter().map(|s| s.id.as_str()).collect()
}

#[allow(clippy::too_many_arguments)]
fn at(
    offset_secs: i32,
    y: i32,
    mo: u32,
    d: u32,
    h: u32,
    mi: u32,
    s: u32,
    ns: u32,
) -> DateTime<FixedOffset> {
    FixedOffset::east_opt(offset_secs)
        .unwrap()
        .with_ymd_and_hms(y, mo, d, h, mi, s)
        .unwrap()
        .with_nanosecond(ns)
        .unwrap()
}

// ---- session_test.go ----

#[test]
fn sort_group_members_uses_creation_order_and_puts_judge_last() {
    let created = Local::now().fixed_offset();
    let second = created + Duration::milliseconds(1);
    let third = second + Duration::milliseconds(1);
    let mut sessions = vec![
        member(
            "judge",
            "codex",
            config::GPT56_SOL_MODEL,
            "consilium",
            Some(third),
        ),
        member(
            "claude",
            "claude",
            config::CLAUDE_MODEL,
            "plan",
            Some(second),
        ),
        member(
            "sol",
            "codex",
            config::GPT56_SOL_MODEL,
            "plan",
            Some(created),
        ),
    ];
    sort_group_members(&mut sessions);
    assert_eq!(ids(&sessions), ["sol", "claude", "judge"]);
}

#[test]
fn sort_group_members_uses_curated_fallback_for_legacy_sessions() {
    let mut sessions = vec![
        member("claude", "claude", config::CLAUDE_MODEL, "plan", None),
        member("k3", "opencode", config::KIMI_MODEL, "megareview", None),
        member("sol", "codex", config::GPT56_SOL_MODEL, "plan", None),
    ];
    sort_group_members(&mut sessions);
    assert_eq!(ids(&sessions), ["sol", "k3", "claude"]);
}

#[test]
fn sort_group_members_preserves_explicit_reviewer_order() {
    let created = Local::now().fixed_offset();
    let later = created + Duration::milliseconds(1);
    let mut sessions = vec![
        member(
            "sol",
            "codex",
            config::GPT56_SOL_MODEL,
            "megareview",
            Some(later),
        ),
        member(
            "k3",
            "opencode",
            config::KIMI_MODEL,
            "megareview",
            Some(created),
        ),
    ];
    sort_group_members(&mut sessions);
    assert_eq!(
        ids(&sessions),
        ["k3", "sol"],
        "explicit reviewer order was not preserved"
    );
}

#[test]
fn group_model_rank_orders_grok_after_the_existing_named_models() {
    let cases = [
        ("sol", "codex", config::GPT56_SOL_MODEL, 0),
        ("kimi-k3", "opencode", config::KIMI_MODEL, 1),
        ("claude", "claude", config::CLAUDE_MODEL, 2),
        ("grok", config::GROK_LABEL, config::GROK_MODEL, 3),
        ("unknown", "mystery", "some-retired-model", 100),
    ];
    for (name, cli, model, want) in cases {
        let s = member(name, cli, model, "", None);
        assert_eq!(group_model_rank(&s), want, "groupModelRank({name})");
    }
}

#[test]
fn sort_group_members_places_grok_after_claude() {
    let created = Some(Local::now().fixed_offset());
    let mut sessions = vec![
        member(
            "grok",
            config::GROK_LABEL,
            config::GROK_MODEL,
            "review",
            created,
        ),
        member("sol", "codex", config::GPT56_SOL_MODEL, "review", created),
        member("claude", "claude", config::CLAUDE_MODEL, "review", created),
    ];
    sort_group_members(&mut sessions);
    assert_eq!(ids(&sessions), ["sol", "claude", "grok"]);
}

#[test]
fn sort_group_members_falls_back_to_start_time_then_id() {
    let t = at(0, 2026, 1, 1, 0, 0, 0, 0);
    let mut a = member("b", "mystery", "m", "review", None);
    a.start_time = t;
    let mut b = member("a", "mystery", "m", "review", None);
    b.start_time = t;
    let mut c = member("c", "mystery", "m", "review", None);
    c.start_time = t - Duration::seconds(1);
    let mut sessions = vec![a, b, c];
    sort_group_members(&mut sessions);
    assert_eq!(ids(&sessions), ["c", "a", "b"]);
}

#[test]
fn is_task_mode_names_only_task_modes() {
    for mode in [MODE_PLAN, MODE_ANTISLOP, MODE_SECURITY] {
        assert!(is_task_mode(mode), "{mode}");
    }
    for mode in ["review", "native", "docker", ""] {
        assert!(!is_task_mode(mode), "{mode}");
    }
}

// ---- save_test.go ----

// Go: TestSaveConcurrentWritersNeverShareATempFile. Concurrent writers of one
// session (owner, TUI stop, reaper, the Mac app) must never share a temp file.
#[test]
fn save_concurrent_writers_never_share_a_temp_file() {
    let (_home, paths) = temp_paths();
    let s = queued(&paths, "prompt");

    let errs: Vec<String> = thread::scope(|scope| {
        let workers: Vec<_> = (0..20)
            .map(|_| {
                let c = s.clone(); // each writer saves its own copy, like separate processes
                let paths = &paths;
                scope.spawn(move || {
                    (0..20)
                        .filter_map(|_| c.save(paths).err().map(|e| format!("{e:#}")))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|w| w.join().unwrap())
            .collect()
    });
    assert!(errs.is_empty(), "concurrent save failed: {errs:?}");

    Session::load(&paths, &s.id).expect("record unreadable after concurrent saves");
    for entry in fs::read_dir(paths.sessions_dir()).unwrap() {
        let name = entry.unwrap().file_name();
        assert_eq!(
            name.to_string_lossy(),
            format!("{}.json", s.id),
            "leftover file"
        );
    }
}

// Go: TestSaveLeavesForeignTempFilesAndReadersSkipThem. A temp file another
// writer holds open is never reused or renamed away, and no reader treats
// either temp form as a session.
#[test]
fn save_leaves_foreign_temp_files_and_readers_skip_them() {
    let (_home, paths) = temp_paths();
    let s = queued(&paths, "prompt");
    let dir = paths.sessions_dir();
    let foreign = dir.join(format!("{}.json.tmp", s.id));
    fs::write(&foreign, br#"{"id":"tmp-legacy","status":"running"}"#).unwrap();
    let unique = dir.join("other.json.tmp-123456");
    fs::write(&unique, br#"{"id":"tmp-unique","status":"running"}"#).unwrap();

    s.save(&paths).unwrap();

    let got = fs::read_to_string(&foreign).unwrap();
    assert!(
        got.contains("tmp-legacy"),
        "foreign temp file clobbered: {got:?}"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(dir.join(format!("{}.json", s.id)))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    for got in Session::load_all(&paths) {
        assert_eq!(got.id, s.id, "load_all read temp file as session");
    }
    for got in load_all_summaries(&paths) {
        assert_eq!(got.id, s.id, "load_all_summaries read temp file as session");
    }
    for (name, want) in [
        ("a.json", true),
        ("a.json.tmp", false),
        ("a.json.tmp-123", false),
        ("a.log", false),
    ] {
        assert_eq!(is_session_file(name), want, "is_session_file({name:?})");
    }
}

#[test]
fn save_rejects_an_id_with_a_path_separator() {
    let (_home, paths) = temp_paths();
    fs::create_dir_all(paths.sessions_dir()).unwrap();
    let s = Session {
        id: "../x".into(),
        ..Session::default()
    };
    let err = format!("{:#}", s.save(&paths).unwrap_err());
    assert_eq!(
        err,
        "create session tmp: createtemp ../x.json.tmp-*: pattern contains path separator"
    );
}

#[test]
fn save_without_sessions_dir_reports_go_style_error() {
    let (_home, paths) = temp_paths();
    let s = Session {
        id: "a".into(),
        ..Session::default()
    };
    let err = format!("{:#}", s.save(&paths).unwrap_err());
    let prefix = format!(
        "create session tmp: open {}/a.json.tmp-",
        paths.sessions_dir().display()
    );
    assert!(err.starts_with(&prefix), "{err}");
    assert!(err.ends_with(": no such file or directory"), "{err}");
}

// ---- writer: Go MarshalIndent bytes ----

/// `json.MarshalIndent` of the same values. Go 1.25 (the release toolchain,
/// legacy `encode.go`) writes each stray byte of the split preview rune as
/// the escape `\ufffd`.
const GO_FULL: &str = r#"{
  "id": "id-1",
  "group_id": "g",
  "cli": "codex",
  "mode": "review",
  "model": "gpt-6-astra",
  "effort": "high",
  "review_scope": "diff",
  "prompt": "review \u003ca\u003e \u0026 b\u2028\u2029 \"q\" \\ \n\t\u0001 日本語",
  "prompt_preview": "日\ufffd\ufffd",
  "prompt_hash": "abc",
  "status": "completed",
  "start_time": "2026-10-02T09:08:07.1234+03:00",
  "queued_at": "0001-01-01T00:00:00Z",
  "queue_position": 2,
  "end_time": "2026-10-02T06:30:00.000000001Z",
  "exit_code": 0,
  "duration": "1m2s",
  "work_dir": "/w",
  "log_file": "/l.log",
  "output_bytes": 1099511627776,
  "output_lines": -3,
  "error": "boom",
  "account": "acc",
  "pid": 42,
  "pid_start": 7,
  "owner_pid": 43,
  "owner_pid_start": 8
}"#;

const GO_EMPTY: &str = r#"{
  "id": "",
  "cli": "",
  "mode": "",
  "model": "",
  "effort": "",
  "status": "",
  "start_time": "0001-01-01T00:00:00Z",
  "work_dir": "",
  "log_file": "",
  "output_bytes": 0,
  "output_lines": 0,
  "pid": 0
}"#;

const GO_EXIT_SEVEN: &str = r#"{
  "id": "x",
  "cli": "",
  "mode": "",
  "model": "",
  "effort": "",
  "status": "",
  "start_time": "2026-01-01T00:00:00-05:30",
  "exit_code": 7,
  "work_dir": "",
  "log_file": "",
  "output_bytes": 0,
  "output_lines": 0,
  "pid": 0
}"#;

fn full_session() -> Session {
    Session {
        id: "id-1".into(),
        group_id: "g".into(),
        cli: "codex".into(),
        mode: "review".into(),
        model: "gpt-6-astra".into(),
        effort: "high".into(),
        review_scope: "diff".into(),
        prompt: "review <a> & b\u{2028}\u{2029} \"q\" \\ \n\t\u{1} 日本語".into(),
        prompt_preview: GoString::from_bytes(&"日本語".as_bytes()[..5]),
        prompt_hash: "abc".into(),
        status: "completed".into(),
        start_time: at(3 * 3600, 2026, 10, 2, 9, 8, 7, 123_400_000),
        queued_at: Some(zero_time()),
        queue_position: 2,
        end_time: Some(at(0, 2026, 10, 2, 6, 30, 0, 1)),
        exit_code: Some(0),
        duration: "1m2s".into(),
        work_dir: "/w".into(),
        log_file: "/l.log".into(),
        output_bytes: 1 << 40,
        output_lines: -3,
        error_msg: "boom".into(),
        account: "acc".into(),
        pid: 42,
        pid_start: 7,
        owner_pid: 43,
        owner_pid_start: 8,
        start_mono: MonoStart::default(),
    }
}

#[test]
fn to_json_matches_go_marshal_indent_bytes() {
    assert_eq!(
        String::from_utf8(full_session().to_json().unwrap()).unwrap(),
        GO_FULL
    );
    assert_eq!(
        String::from_utf8(Session::default().to_json().unwrap()).unwrap(),
        GO_EMPTY
    );
    let seven = Session {
        id: "x".into(),
        exit_code: Some(7),
        start_time: at(-(5 * 3600 + 30 * 60), 2026, 1, 1, 0, 0, 0, 0),
        ..Session::default()
    };
    assert_eq!(
        String::from_utf8(seven.to_json().unwrap()).unwrap(),
        GO_EXIT_SEVEN
    );
}

#[test]
fn exit_code_unset_zero_and_nonzero() {
    let json = |code: Option<i64>| {
        let s = Session {
            exit_code: code,
            ..Session::default()
        };
        String::from_utf8(s.to_json().unwrap()).unwrap()
    };
    assert!(
        !json(None).contains("exit_code"),
        "unset exit_code must be absent"
    );
    assert!(json(Some(0)).contains("\n  \"exit_code\": 0,\n"));
    assert!(json(Some(-1)).contains("\n  \"exit_code\": -1,\n"));
    assert!(json(Some(137)).contains("\n  \"exit_code\": 137,\n"));

    for code in [None, Some(0), Some(137)] {
        let s = Session {
            exit_code: code,
            ..Session::default()
        };
        let back = Session::from_json(&s.to_json().unwrap()).unwrap();
        assert_eq!(back.exit_code, code);
    }
}

#[test]
fn json_round_trips_every_field() {
    let mut s = full_session();
    s.prompt_preview = "日本".into();
    let back = Session::from_json(&s.to_json().unwrap()).unwrap();
    assert_eq!(back, s);
    // The offset survives, not just the instant.
    assert_eq!(back.start_time.offset().local_minus_utc(), 3 * 3600);
    assert_eq!(back.to_json().unwrap(), s.to_json().unwrap());
}

#[test]
fn decode_tolerates_null_missing_and_unknown_fields_like_go() {
    let s = Session::from_json(
        br#"{"id":"a","cli":null,"pid":null,"start_time":null,"queued_at":null,"exit_code":null,"new_field":{"x":1},"status":"weird-future-status"}"#,
    )
    .unwrap();
    assert_eq!(s.id, "a");
    assert_eq!(s.cli, "");
    assert_eq!(s.pid, 0);
    assert_eq!(s.start_time, zero_time());
    assert_eq!(s.queued_at, None);
    assert_eq!(s.exit_code, None);
    assert_eq!(s.status, "weird-future-status");

    let empty = Session::from_json(b"{}").unwrap();
    assert_eq!(empty, Session::default());
    assert_eq!(
        gojson::format_time(&empty.start_time).unwrap(),
        "0001-01-01T00:00:00Z"
    );
    // Go leaves the struct unchanged for a top-level null.
    assert_eq!(Session::from_json(b" null ").unwrap(), Session::default());

    assert!(Session::from_json(br#"{"pid":"12"}"#).is_err());
    assert!(Session::from_json(br#"{"start_time":"nope"}"#).is_err());
}

// ---- create / lifecycle ----

#[test]
fn new_queued_writes_a_queued_record() {
    let (_home, paths) = temp_paths();
    let s = Session::new_queued(
        &paths,
        NewSession {
            cli: "codex",
            mode: "review",
            model: "gpt-5.5",
            effort: "high",
            workdir: "/work",
            prompt: "hello",
            review_scope: "diff",
            group_id: "grp",
        },
    )
    .unwrap();

    assert_eq!(s.status, "queued");
    assert_eq!(s.queued_at, Some(s.start_time));
    assert_eq!(s.prompt_preview, "hello");
    assert_eq!(
        s.prompt_hash,
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    );
    assert_eq!(
        (s.review_scope.as_str(), s.group_id.as_str()),
        ("diff", "grp")
    );
    let pid = i64::from(std::process::id());
    assert_eq!((s.pid, s.owner_pid), (pid, pid));
    assert_eq!(s.pid_start, s.owner_pid_start);
    assert_eq!(
        s.log_file,
        paths
            .sessions_dir()
            .join(format!("{}.log", s.id))
            .to_string_lossy()
    );
    assert!(uuid::Uuid::parse_str(&s.id).is_ok());
    assert_eq!(Session::load(&paths, &s.id).unwrap(), s);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(paths.sessions_dir())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
    }
}

#[test]
fn prompt_preview_is_first_100_bytes() {
    let (_home, paths) = temp_paths();
    let ascii = "a".repeat(150);
    assert_eq!(queued(&paths, &ascii).prompt_preview, "a".repeat(100));
    assert_eq!(
        queued(&paths, &"b".repeat(100)).prompt_preview,
        "b".repeat(100)
    );

    // 98 ASCII bytes, then a 3-byte rune split after its first two bytes:
    // the stray bytes stay as they are until Go's encoder escapes them.
    let split = format!("{}日本", "c".repeat(98));
    let s = queued(&paths, &split);
    assert_eq!(s.prompt_preview.as_bytes(), &split.as_bytes()[..100]);
    assert_eq!(s.prompt, split);
    // The rune ends exactly at byte 100: kept whole.
    let whole = format!("{}日x", "d".repeat(97));
    assert_eq!(
        queued(&paths, &whole).prompt_preview,
        format!("{}日", "d".repeat(97))
    );
}

#[test]
fn mark_running_resets_start_and_keeps_queued_at() {
    let (_home, paths) = temp_paths();
    let mut s = queued(&paths, "p");
    s.queue_position = 3;
    let queued_at = s.queued_at;
    s.start_time -= Duration::minutes(5);
    s.mark_running(&paths).unwrap();

    let got = Session::load(&paths, &s.id).unwrap();
    assert_eq!(got.status, "running");
    assert_eq!(got.queue_position, 0);
    assert_eq!(got.queued_at, queued_at);
    assert!(got.start_time >= queued_at.unwrap());
}

#[test]
fn set_queue_position_writes_only_on_change() {
    let (_home, paths) = temp_paths();
    let mut s = queued(&paths, "p");
    let file = paths.sessions_dir().join(format!("{}.json", s.id));

    s.set_queue_position(&paths, 2).unwrap();
    assert_eq!(Session::load(&paths, &s.id).unwrap().queue_position, 2);

    // Unchanged: no write, so a removed record stays removed.
    fs::remove_file(&file).unwrap();
    s.set_queue_position(&paths, 2).unwrap();
    assert!(!file.exists());

    s.set_queue_position(&paths, 1).unwrap();
    assert_eq!(Session::load(&paths, &s.id).unwrap().queue_position, 1);
}

#[test]
fn complete_and_fail_record_the_outcome() {
    let (_home, paths) = temp_paths();
    let mut s = queued(&paths, "p");
    s.start_time -= Duration::milliseconds(61_600);
    s.complete(&paths, 0, 1234, 7).unwrap();
    let got = Session::load(&paths, &s.id).unwrap();
    assert_eq!(got.status, "completed");
    assert_eq!(got.exit_code, Some(0));
    assert_eq!((got.output_bytes, got.output_lines), (1234, 7));
    assert!(got.end_time.is_some());
    assert!(
        got.duration == "1m2s" || got.duration == "1m3s",
        "{}",
        got.duration
    );

    let mut f = queued(&paths, "p");
    f.fail(&paths, 2, "boom").unwrap();
    let got = Session::load(&paths, &f.id).unwrap();
    assert_eq!(got.status, "failed");
    assert_eq!(got.exit_code, Some(2));
    assert_eq!(got.error_msg, "boom");
    assert_eq!(got.duration, "0s");
}

#[test]
fn run_duration_mirrors_go_round_and_string() {
    let base = at(0, 2026, 1, 1, 0, 0, 0, 0);
    let cases = [
        (0, "0s"),
        (499_999_999, "0s"),
        (500_000_000, "1s"),
        (1_500_000_000, "2s"),
        (2_500_000_000, "3s"),
        (62_400_000_000, "1m2s"),
        (3_600_000_000_000, "1h0m0s"),
        (3_725_000_000_000, "1h2m5s"),
        (-1_500_000_000, "-2s"),
        (-400_000_000, "0s"),
    ];
    for (nanos, want) in cases {
        let end = base + Duration::nanoseconds(nanos);
        assert_eq!(duration_text(sub_nanos(end, base)), want, "{nanos}ns");
    }
    // A zero start time saturates like Go's time.Sub.
    assert_eq!(
        duration_text(sub_nanos(base, zero_time())),
        "2562047h47m16.854775807s"
    );
    assert_eq!(
        duration_text(sub_nanos(zero_time(), base)),
        "-2562047h47m16.854775808s"
    );
}

#[test]
fn round_duration_saturates_like_go() {
    let s = 1_000_000_000;
    assert_eq!(round_duration(i64::MAX, s), i64::MAX);
    assert_eq!(round_duration(i64::MIN, s), i64::MIN);
    assert_eq!(round_duration(7, 0), 7);
}

// ---- load ----

#[test]
fn load_all_sorts_newest_first_and_skips_bad_files() {
    let (_home, paths) = temp_paths();
    let dir = paths.sessions_dir();
    fs::create_dir_all(&dir).unwrap();
    let base = at(0, 2026, 1, 1, 0, 0, 0, 0);
    for (id, secs) in [("old", 0), ("new", 20), ("mid", 10)] {
        let s = Session {
            id: id.into(),
            start_time: base + Duration::seconds(secs),
            ..Session::default()
        };
        s.save(&paths).unwrap();
    }
    fs::write(dir.join("broken.json"), b"{").unwrap();
    fs::write(dir.join("x.json.tmp-1"), br#"{"id":"tmp"}"#).unwrap();
    fs::write(dir.join("notes.txt"), br#"{"id":"txt"}"#).unwrap();

    assert_eq!(ids(&Session::load_all(&paths)), ["new", "mid", "old"]);
    assert_eq!(ids(&load_all_summaries(&paths)), ["new", "mid", "old"]);
}

#[test]
fn load_all_without_dir_is_empty() {
    let (_home, paths) = temp_paths();
    assert!(Session::load_all(&paths).is_empty());
    assert!(load_all_summaries(&paths).is_empty());
    assert!(Session::load(&paths, "missing").is_err());
}

#[test]
fn open_log_appends_with_mode_0600() {
    let (_home, paths) = temp_paths();
    let s = queued(&paths, "p");
    s.open_log().unwrap().write_all(b"one\n").unwrap();
    s.open_log().unwrap().write_all(b"two\n").unwrap();
    assert_eq!(fs::read_to_string(&s.log_file).unwrap(), "one\ntwo\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&s.log_file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

// ---- prompt_preview bytes ----

/// The `prompt_preview` line of the saved record, as raw bytes.
fn preview_line(paths: &Paths, id: &str) -> String {
    let data = fs::read(paths.sessions_dir().join(format!("{id}.json"))).unwrap();
    let line = data
        .split(|&b| b == b'\n')
        .find(|l| l.starts_with(b"  \"prompt_preview\": "))
        .expect("prompt_preview line");
    String::from_utf8(line.to_vec()).expect("Go writes valid UTF-8")
}

#[test]
fn prompt_preview_writes_split_rune_bytes_as_go_escapes() {
    let (_home, paths) = temp_paths();
    let cases = [
        (99, "é", r"\ufffd"),
        (99, "日", r"\ufffd"),
        (98, "日", r"\ufffd\ufffd"),
        (99, "😀", r"\ufffd"),
        (98, "😀", r"\ufffd\ufffd"),
        (97, "😀", r"\ufffd\ufffd\ufffd"),
        (96, "😀", "😀"),
    ];
    for (n, rune, tail) in cases {
        let pad = "p".repeat(n);
        let s = queued(&paths, &format!("{pad}{rune}tail"));
        assert_eq!(
            preview_line(&paths, &s.id),
            format!("  \"prompt_preview\": \"{pad}{tail}\","),
            "{n} bytes + {rune}"
        );
    }
}

#[test]
fn prompt_preview_keeps_a_literal_replacement_char_literal() {
    let (_home, paths) = temp_paths();
    // A real U+FFFD (3 bytes) + 96 bytes, then 日 split after its first byte.
    let pad = "q".repeat(96);
    let s = queued(&paths, &format!("\u{FFFD}{pad}日"));
    assert_eq!(
        preview_line(&paths, &s.id),
        format!("  \"prompt_preview\": \"\u{FFFD}{pad}\\ufffd\",")
    );
}

#[test]
fn prompt_preview_escapes_survive_resaves_until_a_load() {
    let (_home, paths) = temp_paths();
    let pad = "r".repeat(98);
    let mut s = queued(&paths, &format!("{pad}日"));
    let escaped = format!("  \"prompt_preview\": \"{pad}\\ufffd\\ufffd\",");
    assert_eq!(preview_line(&paths, &s.id), escaped);

    // The same in-memory session keeps writing the escapes.
    s.save(&paths).unwrap();
    assert_eq!(preview_line(&paths, &s.id), escaped);
    s.mark_running(&paths).unwrap();
    assert_eq!(preview_line(&paths, &s.id), escaped);

    // Go decodes each escape to a real U+FFFD and then writes it literally.
    let loaded = Session::load(&paths, &s.id).unwrap();
    assert_eq!(loaded.prompt_preview, format!("{pad}\u{FFFD}\u{FFFD}"));
    assert_ne!(loaded.prompt_preview, s.prompt_preview);
    loaded.save(&paths).unwrap();
    assert_eq!(
        preview_line(&paths, &s.id),
        format!("  \"prompt_preview\": \"{pad}\u{FFFD}\u{FFFD}\",")
    );
}

// ---- monotonic durations ----

fn after(start: Now, wall: Duration, mono: StdDuration) -> Now {
    Now {
        wall: start.wall + wall,
        mono: start.mono + mono,
    }
}

#[test]
fn complete_and_fail_use_the_monotonic_clock_in_process() {
    let (_home, paths) = temp_paths();
    let mut s = queued(&paths, "p");
    let start = s
        .start_mono
        .0
        .expect("new_queued takes a monotonic reading");
    assert_eq!(start.wall, s.start_time);
    // The wall clock stepped back an hour while 5s passed.
    let end = after(start, -Duration::hours(1), StdDuration::from_secs(5));
    s.complete_at(&paths, end, 0, 0, 0).unwrap();
    assert_eq!(s.duration, "5s");
    assert_eq!(s.end_time, Some(end.wall));

    // mark_running takes a fresh reading.
    let mut f = queued(&paths, "p");
    let run = Now {
        wall: at(0, 2026, 1, 1, 0, 0, 0, 0),
        mono: Instant::now(),
    };
    f.mark_running_at(&paths, run).unwrap();
    assert_eq!(f.start_time, run.wall);
    let end = after(run, Duration::hours(2), StdDuration::from_millis(61_600));
    f.fail_at(&paths, end, 1, "x").unwrap();
    assert_eq!(f.duration, "1m2s");
}

#[test]
fn loaded_or_reassigned_start_time_uses_wall_time_like_go() {
    let (_home, paths) = temp_paths();
    let mut s = queued(&paths, "p");
    let start = s.start_mono.0.unwrap();
    let end = after(start, Duration::hours(1), StdDuration::from_secs(5));

    let mut loaded = Session::load(&paths, &s.id).unwrap();
    assert!(loaded.start_mono.0.is_none());
    assert_eq!(loaded, s, "the monotonic reading does not affect equality");
    loaded.complete_at(&paths, end, 0, 0, 0).unwrap();
    assert_eq!(loaded.duration, "1h0m0s");

    // A start_time the caller sets no longer matches the reading.
    s.start_time -= Duration::minutes(10);
    s.complete_at(&paths, end, 0, 0, 0).unwrap();
    assert_eq!(s.duration, "1h10m0s");
}

#[test]
fn mono_sub_is_signed_like_go_sub() {
    let t = Instant::now();
    let later = t + StdDuration::from_secs(2);
    assert_eq!(mono_sub(later, t), 2_000_000_000);
    assert_eq!(mono_sub(t, later), -2_000_000_000);
    assert_eq!(mono_sub(t, t), 0);
}

// ---- JSON reader ----

#[test]
fn from_json_matches_keys_case_insensitively_like_go() {
    let doc = "{\"ID\":\"a\",\"Status\":\"running\",\"PID\":5,\
        \"Start_Time\":\"2026-01-02T03:04:05Z\",\"EXIT_CODE\":3,\"Error\":\"e\",\
        \"wor\u{212A}_dir\":\"/w\",\"\u{17F}tatus\":\"done\",\"\u{130}d\":\"no\",\
        \"Prompt_Preview\":\"pp\",\"QUEUED_at\":null}";
    let s = Session::from_json(doc.as_bytes()).unwrap();
    assert_eq!(s.id, "a");
    assert_eq!(s.status, "done");
    assert_eq!(s.pid, 5);
    assert_eq!(s.start_time, at(0, 2026, 1, 2, 3, 4, 5, 0));
    assert_eq!(s.exit_code, Some(3));
    assert_eq!(s.error_msg, "e");
    assert_eq!(s.work_dir, "/w");
    assert_eq!(s.prompt_preview, "pp");
    assert_eq!(s.queued_at, None);
}

#[test]
fn from_json_assigns_duplicate_keys_in_order_like_go() {
    let s = Session::from_json(
        br#"{"id":"a","id":"b","Id":"c",
        "cli":"x","cli":null,
        "pid":5,"pid":null,
        "start_time":"2026-01-02T03:04:05Z","start_time":null,
        "queued_at":"2026-01-02T03:04:05Z","queued_at":null,
        "end_time":null,"end_time":"2026-01-02T03:04:05Z",
        "exit_code":1,"exit_code":null,
        "prompt_preview":"p","prompt_preview":null}"#,
    )
    .unwrap();
    let t = at(0, 2026, 1, 2, 3, 4, 5, 0);
    assert_eq!(s.id, "c");
    // null leaves a value field alone...
    assert_eq!(s.cli, "x");
    assert_eq!(s.pid, 5);
    assert_eq!(s.start_time, t);
    assert_eq!(s.prompt_preview, "p");
    // ...and clears a pointer field.
    assert_eq!(s.queued_at, None);
    assert_eq!(s.exit_code, None);
    assert_eq!(s.end_time, Some(t));

    // Exact-match priority picks the field; the last assignment still wins.
    let id = |doc: &[u8]| Session::from_json(doc).unwrap().id;
    assert_eq!(id(br#"{"Id":"fold","id":"exact"}"#), "exact");
    assert_eq!(id(br#"{"id":"exact","Id":"fold"}"#), "fold");
}

#[test]
fn from_json_replaces_invalid_utf8_in_strings_like_go() {
    let doc = b"{\"id\":\"a\xffb\",\"prompt_preview\":\"\xe6\x97\",\"error\":\"\\ud800!\",\
        \"x\xff\":{\"y\":\"\xfe\"},\"status\":\"ok\"}";
    let s = Session::from_json(doc).unwrap();
    assert_eq!(s.id, "a\u{FFFD}b");
    assert_eq!(s.prompt_preview, "\u{FFFD}\u{FFFD}");
    assert_eq!(s.error_msg, "\u{FFFD}!");
    assert_eq!(s.status, "ok");
}

#[test]
fn from_json_reports_go_type_errors() {
    let field = |value: &str, name: &str, ty: &str| {
        format!("json: cannot unmarshal {value} into Go struct field Session.{name} of type {ty}")
    };
    let cases = [
        (r#"{"pid":"12"}"#, field("string", "pid", "int")),
        (
            r#"{"output_bytes":1.5}"#,
            field("number 1.5", "output_bytes", "int64"),
        ),
        (
            r#"{"pid_start":1e3}"#,
            field("number 1e3", "pid_start", "int64"),
        ),
        (r#"{"id":5}"#, field("number", "id", "string")),
        (r#"{"exit_code":true}"#, field("bool", "exit_code", "int")),
        (r#"{"Error":[1]}"#, field("array", "error", "string")),
        (
            r#"{"prompt_preview":{}}"#,
            field("object", "prompt_preview", "string"),
        ),
        // The first mismatch is kept while decoding goes on...
        (r#"{"pid":"x","id":5}"#, field("string", "pid", "int")),
        // ...but a time error stops decoding at once and wins.
        (
            r#"{"pid":"x","start_time":"bad"}"#,
            r#"parsing time "bad" as "2006-01-02T15:04:05Z07:00": cannot parse "bad" as "2006""#
                .to_string(),
        ),
        (
            r#"{"queued_at":5}"#,
            "Time.UnmarshalJSON: input is not a JSON string".to_string(),
        ),
        (
            "[]",
            "json: cannot unmarshal array into Go value of type session.Session".to_string(),
        ),
    ];
    for (doc, want) in cases {
        let err = Session::from_json(doc.as_bytes()).unwrap_err();
        assert_eq!(err.to_string(), want, "{doc}");
    }
    assert_eq!(Session::from_json(br#"{"pid":-0}"#).unwrap().pid, 0);
}

// ---- load errors ----

#[test]
fn load_errors_read_like_go_and_downcast_to_io() {
    let (_home, paths) = temp_paths();
    let dir = paths.sessions_dir();
    let open_missing = |name: &str| {
        format!(
            "open {}: no such file or directory",
            dir.join(name).display()
        )
    };

    let err = Session::load(&paths, "missing").unwrap_err();
    assert_eq!(err.to_string(), open_missing("missing.json"));
    assert_eq!(
        err.downcast_ref::<io::Error>().unwrap().kind(),
        io::ErrorKind::NotFound
    );
    // filepath.Join cleans the path.
    let err = Session::load(&paths, "a/../b").unwrap_err();
    assert_eq!(err.to_string(), open_missing("b.json"));

    // A directory opens but cannot be read.
    fs::create_dir_all(dir.join("d.json")).unwrap();
    let err = Session::load(&paths, "d").unwrap_err();
    assert_eq!(
        err.to_string(),
        format!("read {}: is a directory", dir.join("d.json").display())
    );
    assert!(err.downcast_ref::<io::Error>().is_some());

    // A decode error carries no path, as in Go.
    fs::write(dir.join("bad.json"), br#"{"pid":"x"}"#).unwrap();
    assert_eq!(
        Session::load(&paths, "bad").unwrap_err().to_string(),
        "json: cannot unmarshal string into Go struct field Session.pid of type int"
    );

    // The summary reader keeps Go's path errors too.
    let missing = dir.join("gone.json");
    let err = load_summary_file(&missing, 10).unwrap_err();
    assert_eq!(err.to_string(), open_missing("gone.json"));
    let err = load_summary_file(&missing, 1 << 20).unwrap_err();
    assert_eq!(err.to_string(), open_missing("gone.json"));
}
