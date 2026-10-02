//! Go `cmd/queue.go` and `cmd/sessions.go` output. Go has no tests for these
//! commands; the expectations come from the format strings.

use super::*;

use chrono::{TimeDelta, TimeZone};
use rival_core::queue::Ticket;

use crate::testutil::{FakeStdin, Fixture, execute};

fn t0() -> DateTime<FixedOffset> {
    FixedOffset::east_opt(0)
        .unwrap()
        .with_ymd_and_hms(2026, 10, 3, 12, 0, 0)
        .unwrap()
}

fn ticket(
    state: &str,
    mode: &str,
    pid: i64,
    created: i64,
    started: Option<i64>,
    wd: &str,
) -> Ticket {
    let mut t = Ticket::default();
    t.id = "id".into();
    t.mode = mode.into();
    t.pid = pid;
    t.state = state.into();
    t.created_at = t0() + TimeDelta::milliseconds(created);
    t.started_at = started.map(|s| t0() + TimeDelta::milliseconds(s));
    t.work_dir = wd.into();
    t
}

fn table(entries: &[Entry], now_ms: i64) -> String {
    let mut out = Vec::new();
    write_queue(&mut out, entries, t0() + TimeDelta::milliseconds(now_ms));
    String::from_utf8(out).unwrap()
}

#[test]
fn empty_queue_says_so() {
    assert_eq!(table(&[], 0), "Queue is empty.\n");
}

#[test]
fn queue_table_matches_go_format() {
    let entries = [
        Entry {
            ticket: ticket("running", "review", 4242, 0, Some(60_000), "/w/a"),
            position: 0,
        },
        Entry {
            ticket: ticket("waiting", "plan", 7, 30_000, None, "/w/b"),
            position: 1,
        },
        Entry {
            // Waiting longer than 30 minutes.
            ticket: ticket(
                "waiting",
                "antislop-long-mode",
                12345678,
                -(31 * 60_000),
                None,
                "",
            ),
            position: 12,
        },
    ];
    // now = t0 + 90.5 s: running since 60 s -> 30.5 s rounds up to 31s.
    assert_eq!(
        table(&entries, 90_500),
        "POS   STATE     MODE          PID      WAIT       WORKDIR\n\
         -     running   review        4242     31s        /w/a\n\
         #1    waiting   plan          7        1m1s       /w/b\n\
         #12   stale?    antislop-long-mode  12345678  32m31s     \n"
    );
}

#[test]
fn running_without_start_uses_creation_and_future_times_go_negative() {
    let entries = [Entry {
        ticket: ticket("running", "review", 1, 10_000, None, "/w"),
        position: 0,
    }];
    assert!(
        table(&entries, 0).contains("  -10s       /w\n"),
        "{}",
        table(&entries, 0)
    );
    // Exactly 30 minutes is not stale: Go uses >.
    let entries = [Entry {
        ticket: ticket("waiting", "review", 1, 0, None, "/w"),
        position: 1,
    }];
    assert!(table(&entries, 30 * 60_000).contains("waiting "));
    assert!(table(&entries, 30 * 60_000 + 1).contains("stale? "));
}

#[test]
fn round_seconds_matches_go_duration_round() {
    let s = NANOS_PER_SECOND;
    for (d, want) in [
        (0, 0),
        (s / 2 - 1, 0),
        (s / 2, s),
        (-s / 2, -s),
        (-s / 2 + 1, 0),
        (3 * s + 1, 3 * s),
        (i64::MAX, i64::MAX),
        (i64::MIN, i64::MIN),
    ] {
        assert_eq!(round_seconds(d), want, "{d}");
    }
}

#[test]
fn clear_message_plurals() {
    assert_eq!(clear_message(0, false), "Removed 0 dead tickets.\n");
    assert_eq!(clear_message(1, false), "Removed 1 dead ticket.\n");
    assert_eq!(clear_message(2, false), "Removed 2 dead tickets.\n");
    assert_eq!(clear_message(0, true), "Removed 0 tickets.\n");
    assert_eq!(clear_message(1, true), "Removed 1 ticket.\n");
    assert_eq!(clear_message(5, true), "Removed 5 tickets.\n");
}

fn session(id: &str, cli: &str, model: &str, status: &str, effort: &str, dur: &str) -> Session {
    Session {
        id: id.into(),
        cli: cli.into(),
        model: model.into(),
        status: status.into(),
        effort: effort.into(),
        duration: dur.into(),
        ..Session::default()
    }
}

fn list(all: Vec<Session>, active: bool, recent: i64) -> String {
    let mut out = Vec::new();
    write_sessions(&mut out, all, active, recent);
    String::from_utf8(out).unwrap()
}

fn sample() -> Vec<Session> {
    vec![
        session(
            "0b7c6a43-1111-2222-3333-444455556666",
            "codex",
            rival_core::config::CODEX_MODEL,
            "running",
            "high",
            "",
        ),
        session(
            "short",
            "claude",
            rival_core::config::CLAUDE_MODEL,
            "completed",
            "medium",
            "1m2s",
        ),
        session("abcdefghij", "mystery", "old", "failed", "", ""),
    ]
}

#[test]
fn sessions_list_matches_go_format() {
    let codex = config::engine_label("codex", rival_core::config::CODEX_MODEL);
    let claude = config::engine_label("claude", rival_core::config::CLAUDE_MODEL);
    let retired = config::engine_label("mystery", "old");
    assert_eq!(
        list(sample(), false, 0),
        format!(
            "0b7c6a43  {codex:<20}  running     high    running...\n\
             short     {claude:<20}  completed   medium  1m2s\n\
             abcdefgh  {retired:<20}  failed              \n"
        )
    );
}

#[test]
fn sessions_filters_and_recent() {
    assert_eq!(list(Vec::new(), false, 0), "No sessions found.\n");
    let active = list(sample(), true, 0);
    assert_eq!(active.lines().count(), 1);
    assert!(active.starts_with("0b7c6a43"));
    assert_eq!(list(sample(), false, 2).lines().count(), 2);
    // recent >= len and recent <= 0 keep everything.
    assert_eq!(list(sample(), false, 3).lines().count(), 3);
    assert_eq!(list(sample(), false, 99).lines().count(), 3);
    assert_eq!(list(sample(), false, -1).lines().count(), 3);
    // --active with no running session.
    assert_eq!(
        list(sample()[1..].to_vec(), true, 1),
        "No sessions found.\n"
    );
}

#[test]
fn queue_and_sessions_commands_through_the_root() {
    let fix = Fixture::new();
    let mut stdin = FakeStdin::new("");
    assert_eq!(
        execute(&fix, &mut stdin, &["queue"]),
        (0, "Queue is empty.\n".into(), String::new())
    );
    assert!(fix.cfg.paths().queue_dir().join(".lock").exists());
    assert_eq!(
        execute(&fix, &mut stdin, &["queue", "clear"]),
        (0, "Removed 0 dead tickets.\n".into(), String::new())
    );
    assert_eq!(
        execute(&fix, &mut stdin, &["queue", "clear", "--force"]),
        (0, "Removed 0 tickets.\n".into(), String::new())
    );
    assert_eq!(
        execute(&fix, &mut stdin, &["sessions", "--active", "--recent", "2"]),
        (0, "No sessions found.\n".into(), String::new())
    );
    let dir = fix.cfg.paths().sessions_dir();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("a.json"),
        r#"{"id":"aaaaaaaa-1","cli":"codex","model":"gpt-x","status":"completed","effort":"low","duration":"2s","start_time":"2026-01-01T00:00:00Z"}"#,
    )
    .unwrap();
    let (code, out, err) = execute(&fix, &mut stdin, &["sessions"]);
    assert_eq!((code, err.as_str()), (0, ""));
    assert_eq!(
        out,
        format!(
            "aaaaaaaa  {:<20}  completed   low     2s\n",
            config::engine_label("codex", "gpt-x")
        )
    );
}

#[test]
fn queue_errors_are_wrapped() {
    let fix = Fixture::new();
    // A regular file where the queue dir should be.
    std::fs::create_dir_all(&fix.cfg.paths().root).unwrap();
    std::fs::write(fix.cfg.paths().queue_dir(), "x").unwrap();
    let mut stdin = FakeStdin::new("");
    let (code, out, err) = execute(&fix, &mut stdin, &["queue"]);
    assert_eq!((code, out.as_str()), (1, ""));
    assert!(
        err.starts_with("read queue: create queue dir: mkdir "),
        "{err}"
    );
    let (code, _, err) = execute(&fix, &mut stdin, &["queue", "clear"]);
    assert_eq!(code, 1);
    assert!(
        err.starts_with("clear queue: create queue dir: mkdir "),
        "{err}"
    );
}
