use std::fs;
use std::io::Write as _;
use std::thread;

use chrono::{DateTime, Duration, FixedOffset, Local, TimeZone, Timelike};

use std::time::{Duration as StdDuration, Instant};

use super::summary::{load_all_summaries, load_summary_file};
use super::*;
use crate::errtext::{DIR_AS_FILE, NO_SUCH_FILE, NO_SUCH_PATH};

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

// ---- group order and modes ----

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
    a.start_time = Some(t);
    let mut b = member("a", "mystery", "m", "review", None);
    b.start_time = Some(t);
    let mut c = member("c", "mystery", "m", "review", None);
    c.start_time = Some(t - Duration::seconds(1));
    let mut sessions = vec![a, b, c];
    sort_group_members(&mut sessions);
    assert_eq!(ids(&sessions), ["c", "a", "b"]);
}

#[test]
fn is_task_mode_names_only_task_modes() {
    for mode in [MODE_PLAN, MODE_SECURITY] {
        assert!(is_task_mode(mode), "{mode}");
    }
    for mode in ["review", "native", "docker", ""] {
        assert!(!is_task_mode(mode), "{mode}");
    }
}

// A session file written by the retired antislop command still loads. Its
// mode is an ordinary string, not a task mode.
#[test]
fn retired_antislop_mode_loads_as_a_plain_run() {
    let s = Session::from_json(
        br#"{"id":"old1","cli":"codex","mode":"antislop","status":"completed"}"#,
    )
    .unwrap();
    assert_eq!((s.id.as_str(), s.mode.as_str()), ("old1", "antislop"));
    assert_eq!(s.status, "completed");
    assert!(!is_task_mode(&s.mode));
}

// ---- save ----

// Concurrent writers of one
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

// A temp file another
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
fn save_without_sessions_dir_reports_open_path_error() {
    let (_home, paths) = temp_paths();
    let s = Session {
        id: "a".into(),
        ..Session::default()
    };
    let err = format!("{:#}", s.save(&paths).unwrap_err());
    // The temp name joins with the host separator; the sessions dir is
    // a missing parent (ERROR_PATH_NOT_FOUND on Windows).
    let prefix = format!(
        "create session tmp: open {}{}a.json.tmp-",
        paths.sessions_dir().display(),
        std::path::MAIN_SEPARATOR
    );
    assert!(err.starts_with(&prefix), "{err}");
    assert!(err.ends_with(&format!(": {NO_SUCH_PATH}")), "{err}");
}

// ---- writer bytes ----

/// `to_json` of [`full_session`]. `<`, `>`, `&`, U+2028 and U+2029 are
/// written as they are; the stray bytes of the split preview rune were
/// replaced by U+FFFD when the preview was cut.
const FULL: &str = concat!(
    "{\n",
    "  \"id\": \"id-1\",\n",
    "  \"group_id\": \"g\",\n",
    "  \"cli\": \"codex\",\n",
    "  \"mode\": \"review\",\n",
    "  \"model\": \"gpt-6-astra\",\n",
    "  \"effort\": \"high\",\n",
    "  \"review_scope\": \"diff\",\n",
    "  \"prompt\": \"review <a> & b\u{2028}\u{2029} \\\"q\\\" \\\\ \\n\\t\\u0001 日本語\",\n",
    "  \"prompt_preview\": \"日\u{FFFD}\u{FFFD}\",\n",
    "  \"prompt_hash\": \"abc\",\n",
    "  \"status\": \"completed\",\n",
    "  \"start_time\": \"2026-10-02T09:08:07.123400+03:00\",\n",
    "  \"queued_at\": \"2026-10-02T09:08:06Z\",\n",
    "  \"queue_position\": 2,\n",
    "  \"end_time\": \"2026-10-02T06:30:00.000000001Z\",\n",
    "  \"exit_code\": 0,\n",
    "  \"duration\": \"1m2s\",\n",
    "  \"work_dir\": \"/w\",\n",
    "  \"log_file\": \"/l.log\",\n",
    "  \"output_bytes\": 1099511627776,\n",
    "  \"output_lines\": -3,\n",
    "  \"error\": \"boom\",\n",
    "  \"account\": \"acc\",\n",
    "  \"route\": \"proxy\",\n",
    "  \"wire_model\": \"emcd_/gpt-6-astra\",\n",
    "  \"pid\": 42,\n",
    "  \"pid_start\": 7,\n",
    "  \"owner_pid\": 43,\n",
    "  \"owner_pid_start\": 8\n",
    "}"
);

/// An unset time is not written.
const EMPTY: &str = r#"{
  "id": "",
  "cli": "",
  "mode": "",
  "model": "",
  "effort": "",
  "status": "",
  "work_dir": "",
  "log_file": "",
  "output_bytes": 0,
  "output_lines": 0,
  "pid": 0
}"#;

const EXIT_SEVEN: &str = r#"{
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
        prompt_preview: lossy_per_byte(&"日本語".as_bytes()[..5]),
        prompt_hash: "abc".into(),
        status: "completed".into(),
        start_time: Some(at(3 * 3600, 2026, 10, 2, 9, 8, 7, 123_400_000)),
        queued_at: Some(at(0, 2026, 10, 2, 9, 8, 6, 0)),
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
        route: "proxy".into(),
        wire_model: "emcd_/gpt-6-astra".into(),
        pid: 42,
        pid_start: 7,
        owner_pid: 43,
        owner_pid_start: 8,
        start_mono: MonoStart::default(),
        ephemeral: false,
    }
}

#[test]
fn to_json_writes_pretty_json_in_record_order() {
    assert_eq!(
        String::from_utf8(full_session().to_json().unwrap()).unwrap(),
        FULL
    );
    assert_eq!(
        String::from_utf8(Session::default().to_json().unwrap()).unwrap(),
        EMPTY
    );
    let seven = Session {
        id: "x".into(),
        exit_code: Some(7),
        start_time: Some(at(-(5 * 3600 + 30 * 60), 2026, 1, 1, 0, 0, 0, 0)),
        ..Session::default()
    };
    assert_eq!(
        String::from_utf8(seven.to_json().unwrap()).unwrap(),
        EXIT_SEVEN
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
    let s = full_session();
    let back = Session::from_json(&s.to_json().unwrap()).unwrap();
    assert_eq!(back, s);
    // The offset survives, not just the instant.
    assert_eq!(
        back.start_time.unwrap().offset().local_minus_utc(),
        3 * 3600
    );
    assert_eq!(back.to_json().unwrap(), s.to_json().unwrap());
}

#[test]
fn decode_tolerates_null_missing_and_unknown_fields() {
    let s = Session::from_json(
        br#"{"id":"a","cli":null,"pid":null,"start_time":null,"queued_at":null,"exit_code":null,"new_field":{"x":1},"status":"weird-future-status"}"#,
    )
    .unwrap();
    assert_eq!(s.id, "a");
    assert_eq!(s.cli, "");
    assert_eq!(s.pid, 0);
    assert_eq!(s.start_time, None);
    assert_eq!(s.queued_at, None);
    assert_eq!(s.exit_code, None);
    assert_eq!(s.status, "weird-future-status");

    let empty = Session::from_json(b"{}").unwrap();
    assert_eq!(empty, Session::default());
    assert_eq!(empty.start_time, None);

    assert!(Session::from_json(b" null ").is_err());
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
    assert_eq!(s.queued_at, s.start_time);
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
    // each stray byte becomes U+FFFD.
    let split = format!("{}日本", "c".repeat(98));
    let s = queued(&paths, &split);
    assert_eq!(
        s.prompt_preview,
        format!("{}\u{FFFD}\u{FFFD}", "c".repeat(98))
    );
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
    s.start_time = s.start_time.map(|t| t - Duration::minutes(5));
    s.mark_running(&paths).unwrap();

    let got = Session::load(&paths, &s.id).unwrap();
    assert_eq!(got.status, "running");
    assert_eq!(got.queue_position, 0);
    assert_eq!(got.queued_at, queued_at);
    assert!(got.start_time.unwrap() >= queued_at.unwrap());
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
    s.start_time = s.start_time.map(|t| t - Duration::milliseconds(61_600));
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
fn run_duration_rounds_to_seconds_and_formats() {
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
    // A start in year 1 saturates at the i64 nanosecond range.
    assert_eq!(
        duration_text(sub_nanos(base, at(0, 1, 1, 1, 0, 0, 0, 0))),
        "2562047h47m16.854775807s"
    );
    assert_eq!(
        duration_text(sub_nanos(at(0, 1, 1, 1, 0, 0, 0, 0), base)),
        "-2562047h47m16.854775808s"
    );
}

#[test]
fn round_duration_saturates() {
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
            start_time: Some(base + Duration::seconds(secs)),
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
    String::from_utf8(line.to_vec()).expect("session files hold valid UTF-8")
}

#[test]
fn prompt_preview_writes_each_split_rune_byte_as_a_replacement_char() {
    let (_home, paths) = temp_paths();
    let cases = [
        (99, "é", "\u{FFFD}"),
        (99, "日", "\u{FFFD}"),
        (98, "日", "\u{FFFD}\u{FFFD}"),
        (99, "😀", "\u{FFFD}"),
        (98, "😀", "\u{FFFD}\u{FFFD}"),
        (97, "😀", "\u{FFFD}\u{FFFD}\u{FFFD}"),
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
        format!("  \"prompt_preview\": \"\u{FFFD}{pad}\u{FFFD}\",")
    );
}

#[test]
fn prompt_preview_is_the_same_after_save_and_load() {
    let (_home, paths) = temp_paths();
    let pad = "r".repeat(98);
    let mut s = queued(&paths, &format!("{pad}日"));
    let line = format!("  \"prompt_preview\": \"{pad}\u{FFFD}\u{FFFD}\",");
    assert_eq!(preview_line(&paths, &s.id), line);
    s.mark_running(&paths).unwrap();
    assert_eq!(preview_line(&paths, &s.id), line);

    let loaded = Session::load(&paths, &s.id).unwrap();
    assert_eq!(loaded.prompt_preview, s.prompt_preview);
    loaded.save(&paths).unwrap();
    assert_eq!(preview_line(&paths, &s.id), line);
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
    assert_eq!(Some(start.wall), s.start_time);
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
    assert_eq!(f.start_time, Some(run.wall));
    let end = after(run, Duration::hours(2), StdDuration::from_millis(61_600));
    f.fail_at(&paths, end, 1, "x").unwrap();
    assert_eq!(f.duration, "1m2s");
}

#[test]
fn loaded_or_reassigned_start_time_uses_wall_time() {
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
    s.start_time = s.start_time.map(|t| t - Duration::minutes(10));
    s.complete_at(&paths, end, 0, 0, 0).unwrap();
    assert_eq!(s.duration, "1h10m0s");
}

#[test]
fn mono_sub_is_signed() {
    let t = Instant::now();
    let later = t + StdDuration::from_secs(2);
    assert_eq!(mono_sub(later, t), 2_000_000_000);
    assert_eq!(mono_sub(t, later), -2_000_000_000);
    assert_eq!(mono_sub(t, t), 0);
}

// ---- JSON reader ----

#[test]
fn from_json_matches_keys_exactly() {
    let s = Session::from_json(br#"{"ID":"a","id":"b","Status":"running","PID":5}"#).unwrap();
    assert_eq!(s.id, "b");
    assert_eq!(s.status, "");
    assert_eq!(s.pid, 0);
}

#[test]
fn from_json_reads_null_as_the_default_and_rejects_a_duplicate_key() {
    let s = Session::from_json(
        br#"{"id":"a","cli":null,"pid":null,"start_time":null,"exit_code":null,
        "prompt_preview":null}"#,
    )
    .unwrap();
    assert_eq!(
        s,
        Session {
            id: "a".into(),
            ..Session::default()
        }
    );
    assert!(Session::from_json(br#"{"id":"a","id":"b"}"#).is_err());
}

#[test]
fn from_json_reads_the_old_unset_time_as_none() {
    let s = Session::from_json(
        br#"{"id":"a","start_time":"0001-01-01T00:00:00Z","queued_at":"0001-01-01T00:00:00Z",
        "end_time":"0001-01-01T00:00:00Z"}"#,
    )
    .unwrap();
    assert_eq!((s.start_time, s.queued_at, s.end_time), (None, None, None));
    let omitted = Session::from_json(br#"{"id":"a"}"#).unwrap();
    assert_eq!(omitted, s);
    // The next save writes the new form: no time keys at all.
    let text = String::from_utf8(s.to_json().unwrap()).unwrap();
    assert!(
        !text.contains("_time") && !text.contains("queued_at"),
        "{text}"
    );
}

#[test]
fn from_json_reads_escaped_and_raw_html_characters_alike() {
    let escaped = Session::from_json(br#"{"group_id":"a\u003cb\u0026c\u003e"}"#).unwrap();
    let raw = Session::from_json(br#"{"group_id":"a<b&c>"}"#).unwrap();
    assert_eq!(escaped.group_id, "a<b&c>");
    assert_eq!(raw, escaped);
}

#[test]
fn from_json_rejects_wrong_types_and_bad_times() {
    for doc in [
        r#"{"pid":"12"}"#,
        r#"{"output_bytes":1.5}"#,
        r#"{"pid_start":1e3}"#,
        r#"{"id":5}"#,
        r#"{"exit_code":true}"#,
        r#"{"error":[1]}"#,
        r#"{"prompt_preview":{}}"#,
        r#"{"start_time":"bad"}"#,
        r#"{"queued_at":5}"#,
        // serde_json reads -0 as a float.
        r#"{"pid":-0}"#,
        "[]",
        r#"["id"]"#,
        "{",
    ] {
        assert!(Session::from_json(doc.as_bytes()).is_err(), "{doc}");
    }
}

// ---- load errors ----

#[test]
fn load_errors_name_the_path_and_downcast_to_io() {
    let (_home, paths) = temp_paths();
    let dir = paths.sessions_dir();
    // The sessions dir does not exist yet: a missing parent.
    let open_missing =
        |name: &str, text: &str| format!("open {}: {text}", dir.join(name).display());

    let err = Session::load(&paths, "missing").unwrap_err();
    assert_eq!(err.to_string(), open_missing("missing.json", NO_SUCH_PATH));
    assert_eq!(
        err.downcast_ref::<io::Error>().unwrap().kind(),
        io::ErrorKind::NotFound
    );
    // The joined path is cleaned.
    let err = Session::load(&paths, "a/../b").unwrap_err();
    assert_eq!(err.to_string(), open_missing("b.json", NO_SUCH_PATH));

    // A directory opens but cannot be read on Unix. On Windows the open
    // fails.
    fs::create_dir_all(dir.join("d.json")).unwrap();
    let (dir_op, dir_text) = DIR_AS_FILE;
    let err = Session::load(&paths, "d").unwrap_err();
    assert_eq!(
        err.to_string(),
        format!("{dir_op} {}: {dir_text}", dir.join("d.json").display())
    );
    assert!(err.downcast_ref::<io::Error>().is_some());

    // A decode error carries no path.
    fs::write(dir.join("bad.json"), br#"{"pid":"x"}"#).unwrap();
    assert_eq!(
        Session::load(&paths, "bad").unwrap_err().to_string(),
        "invalid type: string \"x\", expected i64 at line 1 column 10"
    );

    // The summary reader keeps the path errors too.
    let missing = dir.join("gone.json");
    let err = load_summary_file(&missing, 10).unwrap_err();
    assert_eq!(err.to_string(), open_missing("gone.json", NO_SUCH_FILE));
    let err = load_summary_file(&missing, 1 << 20).unwrap_err();
    assert_eq!(err.to_string(), open_missing("gone.json", NO_SUCH_FILE));
    // The large-file path opens with os.Open, like os.ReadFile.
    let err = load_summary_file(&dir.join("d.json"), 1 << 20).unwrap_err();
    assert_eq!(
        err.to_string(),
        format!("{dir_op} {}: {dir_text}", dir.join("d.json").display())
    );
}

// ---- old files on disk ----

/// One line with every stored field of a session. Times keep their offset.
fn record_line(s: &Session) -> String {
    let t = |t: Option<DateTime<FixedOffset>>| {
        t.map_or("-".to_string(), |t| {
            t.to_rfc3339_opts(chrono::SecondsFormat::Nanos, false)
        })
    };
    format!(
        "id={} group_id={} cli={} mode={} model={} effort={} review_scope={} prompt={:?} \
         prompt_preview={:?} prompt_hash={} status={} start_time={} queued_at={} \
         queue_position={} end_time={} exit_code={:?} duration={} work_dir={} log_file={} \
         output_bytes={} output_lines={} error={:?} account={} pid={} pid_start={} \
         owner_pid={} owner_pid_start={}",
        s.id,
        s.group_id,
        s.cli,
        s.mode,
        s.model,
        s.effort,
        s.review_scope,
        s.prompt,
        s.prompt_preview.to_string(),
        s.prompt_hash,
        s.status,
        t(s.start_time),
        t(s.queued_at),
        s.queue_position,
        t(s.end_time),
        s.exit_code,
        s.duration,
        s.work_dir,
        s.log_file,
        s.output_bytes,
        s.output_lines,
        s.error_msg,
        s.account,
        s.pid,
        s.pid_start,
        s.owner_pid,
        s.owner_pid_start,
    )
}

/// Every file in `testdata/sessions/` (written by older releases or by v4)
/// loads with the field values recorded before the serde reader replaced
/// the older one.
#[test]
fn testdata_sessions_load_with_recorded_values() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/sessions");
    let mut names: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let got: Vec<String> = names
        .iter()
        .map(|name| {
            let s = Session::from_json(&fs::read(dir.join(name)).unwrap()).unwrap();
            format!("{name}: {}", record_line(&s))
        })
        .collect();
    let want: &[&str] = &[
        r##"completed-plan.json: id=a1b2c3d4-0003-4000-8000-000000000003 group_id= cli=codex mode=plan model=gpt-6-astra effort=xhigh review_scope=plans/2026-09-12-invoice-queue prompt="" prompt_preview="Review the invoice-queue plan" prompt_hash= status=completed start_time=2026-09-26T01:00:00.000000000+00:00 queued_at=- queue_position=0 end_time=2026-09-26T01:06:24.000000000+00:00 exit_code=Some(0) duration=6m24s work_dir=/Users/dev/src/acme-api log_file=/Users/dev/.rival/sessions/a1b2c3d4-0003-4000-8000-000000000003.log output_bytes=10823 output_lines=97 error="" account= pid=41000 pid_start=1790384400000000000 owner_pid=0 owner_pid_start=0"##,
        r##"failed.json: id=a1b2c3d4-0004-4000-8000-000000000004 group_id= cli=codex mode=review model=gpt-6-astra effort=high review_scope= prompt="" prompt_preview="" prompt_hash= status=failed start_time=2026-09-25T09:00:00.500000000+00:00 queued_at=- queue_position=0 end_time=2026-09-25T09:00:42.500000000+00:00 exit_code=Some(1) duration=42s work_dir=/Users/dev/src/orbit-web log_file=/Users/dev/.rival/sessions/a1b2c3d4-0004-4000-8000-000000000004.log output_bytes=512 output_lines=4 error="codex: quota exceeded (429)" account= pid=40000 pid_start=0 owner_pid=0 owner_pid_start=0"##,
        r##"mega-judge.json: id=005591bb-caed-4620-b6b2-ea97a729a103 group_id=8a138d95-3176-4afb-aad1-a59e12b879c8 cli=codex mode=consilium model=gpt-5.5 effort=xhigh review_scope=internal/core/core.billing_extension.go prompt="# Consilium Judge — Final Code Review Verdict" prompt_preview="# Consilium Judge — Final Code Review Verdict" prompt_hash=3769b9c9 status=completed start_time=2026-05-28T14:27:55.018889000+08:00 queued_at=- queue_position=0 end_time=2026-05-28T14:29:17.717611000+08:00 exit_code=Some(0) duration=1m23s work_dir=/Users/dev/src/acme-api log_file=/Users/dev/.rival/sessions/005591bb-caed-4620-b6b2-ea97a729a103.log output_bytes=3090 output_lines=1 error="" account= pid=60430 pid_start=0 owner_pid=0 owner_pid_start=0"##,
        r##"mega-reviewer-a.json: id=b0000000-0001-4000-8000-00000000000a group_id=8a138d95-3176-4afb-aad1-a59e12b879c8 cli=codex mode=megareview model=gpt-5.5 effort=xhigh review_scope=internal/core/core.billing_extension.go prompt="" prompt_preview="# Megareview" prompt_hash= status=completed start_time=2026-05-28T14:20:00.100000000+08:00 queued_at=2026-05-28T14:19:59.900000000+08:00 queue_position=0 end_time=2026-05-28T14:26:00.100000000+08:00 exit_code=Some(0) duration=6m0s work_dir=/Users/dev/src/acme-api log_file=/Users/dev/.rival/sessions/b0000000-0001-4000-8000-00000000000a.log output_bytes=20000 output_lines=300 error="" account= pid=60100 pid_start=0 owner_pid=0 owner_pid_start=0"##,
        r##"mega-reviewer-b.json: id=b0000000-0002-4000-8000-00000000000b group_id=8a138d95-3176-4afb-aad1-a59e12b879c8 cli=opencode mode=megareview model=moonshotai/kimi-k3 effort=xhigh review_scope=internal/core/core.billing_extension.go prompt="" prompt_preview="# Megareview" prompt_hash= status=completed start_time=2026-05-28T14:20:00.200000000+08:00 queued_at=2026-05-28T14:19:59.950000000+08:00 queue_position=0 end_time=2026-05-28T14:27:50.000000000+08:00 exit_code=Some(0) duration=7m50s work_dir=/Users/dev/src/acme-api log_file=/Users/dev/.rival/sessions/b0000000-0002-4000-8000-00000000000b.log output_bytes=18000 output_lines=250 error="" account= pid=60200 pid_start=0 owner_pid=0 owner_pid_start=0"##,
        r##"minimal.json: id=c0000000-0001-4000-8000-000000000001 group_id= cli=codex mode= model= effort= review_scope= prompt="" prompt_preview="" prompt_hash= status=completed start_time=2026-03-01T10:00:00.000000000+00:00 queued_at=- queue_position=0 end_time=- exit_code=None duration= work_dir= log_file= output_bytes=0 output_lines=0 error="" account= pid=0 pid_start=0 owner_pid=0 owner_pid_start=0"##,
        r##"queued.json: id=a1b2c3d4-0002-4000-8000-000000000002 group_id= cli=claude mode=review model=claude-opus-5-5 effort=high review_scope= prompt="" prompt_preview="Review the billing extension" prompt_hash= status=queued start_time=2026-09-26T11:12:00.018889000+08:00 queued_at=2026-09-26T11:12:00.018889000+08:00 queue_position=2 end_time=- exit_code=None duration= work_dir=/Users/dev/src/acme-api log_file=/Users/dev/.rival/sessions/a1b2c3d4-0002-4000-8000-000000000002.log output_bytes=0 output_lines=0 error="" account= pid=44001 pid_start=1790392320018000000 owner_pid=0 owner_pid_start=0"##,
        r##"sol-history.json: id=00080ac4-80be-4646-a2cc-44110d1acdf4 group_id= cli=codex mode=raw model=gpt-5.6-sol effort=high review_scope= prompt="" prompt_preview="full review of the ESP32 firmware source" prompt_hash= status=completed start_time=2026-08-14T19:54:36.372991000+08:00 queued_at=2026-08-14T19:54:36.371596000+08:00 queue_position=0 end_time=2026-08-14T20:09:14.007572000+08:00 exit_code=Some(0) duration=14m38s work_dir=/Users/dev/hardware/relay-sync log_file=/Users/dev/.rival/sessions/00080ac4-80be-4646-a2cc-44110d1acdf4.log output_bytes=10823 output_lines=97 error="" account= pid=43952 pid_start=1786708476373568000 owner_pid=43897 owner_pid_start=1786708475482218000"##,
        r##"solo-running.json: id=a1b2c3d4-0001-4000-8000-000000000001 group_id= cli=codex mode=review model=gpt-6-astra effort=xhigh review_scope=internal/core/ prompt="Review the fingerprint re-key in internal/core." prompt_preview="Review the fingerprint re-key in internal/core." prompt_hash=9566779c status=running start_time=2026-09-26T03:10:00.123456789+00:00 queued_at=2026-09-26T03:09:58.500000000+00:00 queue_position=0 end_time=- exit_code=None duration= work_dir=/Users/dev/src/orbit-web log_file=/Users/dev/.rival/sessions/a1b2c3d4-0001-4000-8000-000000000001.log output_bytes=0 output_lines=0 error="" account= pid=43952 pid_start=1790392198373568000 owner_pid=43897 owner_pid_start=1790392197482218000"##,
        r##"unknown-key.json: id=c0000000-0002-4000-8000-000000000002 group_id= cli=grok mode=security model=grok-4.6 effort=low review_scope= prompt="" prompt_preview="" prompt_hash= status=completed start_time=2026-09-20T08:00:00.000000000+02:00 queued_at=- queue_position=0 end_time=2026-09-20T08:03:00.000000000+02:00 exit_code=Some(0) duration=3m0s work_dir=/Users/dev/src/rival log_file=/Users/dev/.rival/sessions/c0000000-0002-4000-8000-000000000002.log output_bytes=100 output_lines=2 error="" account=work pid=1234 pid_start=0 owner_pid=0 owner_pid_start=0"##,
    ];
    assert_eq!(got, want);
}

/// Save, load, save: the two files have equal bytes, for a full record and
/// for every file in `testdata/sessions/`.
#[test]
fn save_load_save_gives_equal_bytes() {
    let (_home, paths) = temp_paths();
    fs::create_dir_all(paths.sessions_dir()).unwrap();
    let file = |id: &str| fs::read(paths.sessions_dir().join(format!("{id}.json"))).unwrap();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/sessions");
    let mut records = vec![full_session(), Session::default()];
    for e in fs::read_dir(&dir).unwrap() {
        records.push(Session::from_json(&fs::read(e.unwrap().path()).unwrap()).unwrap());
    }
    for (i, mut s) in records.into_iter().enumerate() {
        s.id = format!("rt-{i}");
        s.save(&paths).unwrap();
        let first = file(&s.id);
        let loaded = Session::load(&paths, &s.id).unwrap();
        assert_eq!(loaded, s, "{}", s.id);
        loaded.save(&paths).unwrap();
        assert_eq!(file(&s.id), first, "{}", s.id);
    }
}

/// A group that mixes legacy members (no `queued_at`) with queued ones must
/// sort without a cycle: the queued members first, in creation order.
#[test]
fn sort_group_members_mixed_legacy_and_queued_is_total() {
    let t = |s: i64| {
        chrono::DateTime::from_timestamp(s, 0)
            .unwrap()
            .fixed_offset()
    };
    let mut members: Vec<Session> = (0..12)
        .map(|i| Session {
            id: format!("id{i:02}"),
            cli: if i % 2 == 0 { "codex" } else { "claude" }.into(),
            queued_at: (i % 3 != 0).then(|| t(100 - i)),
            start_time: Some(t(50 + i)),
            ..Session::default()
        })
        .collect();
    sort_group_members(&mut members);
    let queued: Vec<bool> = members.iter().map(|m| m.queued_at.is_some()).collect();
    let first_legacy = queued.iter().position(|q| !q).unwrap();
    assert!(queued[first_legacy..].iter().all(|q| !q), "{queued:?}");
    assert!(
        members[..first_legacy]
            .windows(2)
            .all(|w| w[0].queued_at <= w[1].queued_at)
    );
}

/// `route` and `wire_model` are optional: a record without them loads, and
/// a direct run (both empty) writes neither.
#[test]
fn route_fields_are_optional() {
    let old = Session::from_json(EXIT_SEVEN.as_bytes()).unwrap();
    assert_eq!((old.route.as_str(), old.wire_model.as_str()), ("", ""));
    assert_eq!(
        String::from_utf8(old.to_json().unwrap()).unwrap(),
        EXIT_SEVEN
    );
    let full = Session::from_json(FULL.as_bytes()).unwrap();
    assert_eq!(full.route, "proxy");
    assert_eq!(full.wire_model, "emcd_/gpt-6-astra");
}

/// The leak guard: a registered secret never reaches the record.
#[test]
fn to_json_scrubs_registered_secrets() {
    crate::leakguard::register("test-proxy-key-session-0000");
    let sess = Session {
        error_msg: "claude said test-proxy-key-session-0000".into(),
        ..Session::default()
    };
    let text = String::from_utf8(sess.to_json().unwrap()).unwrap();
    assert!(!text.contains("test-proxy-key-session-0000"), "{text}");
    assert!(
        text.contains("\"error\": \"claude said <redacted>\""),
        "{text}"
    );
}

#[test]
fn ephemeral_session_save_writes_nothing() {
    let home = tempfile::tempdir().unwrap();
    let paths = Paths::from_home(home.path());
    fs::create_dir_all(paths.sessions_dir()).unwrap();
    let sess = Session {
        id: "check-codex".to_string(),
        ephemeral: true,
        ..Session::default()
    };
    sess.save(&paths).unwrap();
    assert_eq!(fs::read_dir(paths.sessions_dir()).unwrap().count(), 0);
    // The flag is not part of the record.
    let json = String::from_utf8(sess.to_json().unwrap()).unwrap();
    assert!(!json.contains("ephemeral"), "{json}");
}
