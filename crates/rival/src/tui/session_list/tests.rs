use std::sync::Arc;

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, TimeDelta, TimeZone};
use ratatui::text::Line;

use rival_core::config::{CLAUDE_MODEL, GPT56_SOL_MODEL, GROK_MODEL, KIMI_MODEL};
use rival_core::session::{self, Session};

use super::*;
use crate::tui::model::group_sessions;
use crate::tui::styles::STYLES;
use crate::tui::testkit::{filter_fixture, fixed_now, fixed_zone, many_runs, run};

fn solo(s: Session) -> DisplayItem {
    DisplayItem {
        sessions: vec![Arc::new(s)],
    }
}

fn items_of(sessions: Vec<Session>) -> DisplayItem {
    DisplayItem {
        sessions: sessions.into_iter().map(Arc::new).collect(),
    }
}

fn sess(group_id: &str, cli: &str, model: &str, mode: &str) -> Session {
    Session {
        group_id: group_id.into(),
        cli: cli.into(),
        model: model.into(),
        mode: mode.into(),
        ..Session::default()
    }
}

/// Go `rows`: `rows_and_counts` without the counts, with a raw filter.
fn rows(
    items: &[DisplayItem],
    tab: StatusTab,
    filter: &str,
    now: DateTime<FixedOffset>,
) -> Vec<Row> {
    let mut hays = vec![None; items.len()];
    rows_and_counts(
        items,
        &mut hays,
        tab,
        &filter_terms(filter),
        now,
        fixed_zone,
    )
    .0
}

/// Go `rowSummary`: "#SECTION" or the first letter of the run id.
fn row_summary(items: &[DisplayItem], rows: &[Row]) -> Vec<String> {
    rows.iter()
        .map(|r| match *r {
            Row::Section(s) => format!("#{s}"),
            Row::Run(i) => items[i].primary().unwrap().id[..1].to_string(),
        })
        .collect()
}

fn pane(items: Vec<DisplayItem>, now: DateTime<FixedOffset>) -> ListPane {
    let mut l = ListPane {
        zone: fixed_zone,
        ..ListPane::default()
    };
    l.set_items(items, now);
    l
}

/// Go `manyListPane`.
fn many_list_pane(n: usize, now: DateTime<FixedOffset>) -> ListPane {
    let mut l = pane(group_sessions(&many_runs(n, now)), now);
    l.top();
    l
}

/// Go `selectedIDOf`: the first letter of the selected id.
fn selected_letter(l: &ListPane) -> String {
    l.selected()
        .map(|i| i.primary().unwrap().id[..1].to_string())
        .unwrap_or_default()
}

fn selected_id(l: &ListPane) -> String {
    l.selected()
        .map(|i| i.primary().unwrap().id.clone())
        .unwrap_or_default()
}

/// Go `pageSummary`: the current page's rows, "#SECTION" or the run id.
fn page_summary(l: &ListPane) -> Vec<String> {
    let (rows, _) = l.page_rows();
    rows.iter()
        .map(|r| match *r {
            Row::Section(s) => format!("#{s}"),
            Row::Run(i) => l.items[i].primary().unwrap().id.clone(),
        })
        .collect()
}

// --- Go session_list_test.go ------------------------------------------------

#[test]
fn group_effort_shows_mixed_defaults() {
    let effort = |e: &str| Session {
        effort: e.into(),
        ..Session::default()
    };
    let item = items_of(vec![effort("ultra"), effort("low")]);
    assert_eq!(group_effort(&item), "mixed");
    let item = items_of(vec![effort("ultra"), effort("ultra")]);
    assert_eq!(group_effort(&item), "ultra");
}

#[test]
fn paired_plan_group_shows_raw_models_and_plan_kind() {
    let created = fixed_now();
    let later = created + TimeDelta::milliseconds(1);
    // LoadAll returns newest first; grouping must restore requested order.
    let items = group_sessions(&[
        Arc::new(Session {
            id: "b".into(),
            queued_at: Some(later),
            ..sess("paired", "claude", CLAUDE_MODEL, "plan")
        }),
        Arc::new(Session {
            id: "a".into(),
            queued_at: Some(created),
            ..sess("paired", "codex", GPT56_SOL_MODEL, "plan")
        }),
    ]);
    assert_eq!(items.len(), 1);
    assert!(items[0].is_group());
    assert_eq!(kind_label(&items[0]), "plan");
    assert_eq!(
        group_model_name(&items[0]),
        format!("{GPT56_SOL_MODEL} +1"),
        "first requested model, raw id"
    );
}

#[test]
fn singleton_claude_plan_remains_logical_plan_group() {
    let items = group_sessions(&[Arc::new(Session {
        id: "claude".into(),
        status: "running".into(),
        ..sess("degraded-plan", "claude", CLAUDE_MODEL, "plan")
    })]);
    assert_eq!(items.len(), 1);
    assert!(items[0].is_group());
    assert_eq!(kind_label(&items[0]), "plan");
    assert_eq!(group_model_name(&items[0]), CLAUDE_MODEL);
}

/// Every CLI shows its raw model id, and a live Claude plan run is kind
/// "plan".
#[test]
fn row_labels_for_every_cli() {
    for s in [
        sess("", "codex", GPT56_SOL_MODEL, "review"),
        sess("", "opencode", KIMI_MODEL, "review"),
        sess("", "claude", CLAUDE_MODEL, "review"),
        sess("", "grok", GROK_MODEL, "review"),
    ] {
        let (cli, model) = (s.cli.clone(), s.model.clone());
        let item = solo(s);
        assert_eq!(group_model_name(&item), model, "{cli} model cell");
        assert_eq!(kind_label(&item), "review", "{cli} kind");
    }
    let plan = solo(sess("", "claude", CLAUDE_MODEL, "plan"));
    assert_eq!(kind_label(&plan), "plan");
}

#[test]
fn model_name_shows_the_raw_id() {
    for (cli, model, want) in [
        ("codex", "gpt-6-astra", "gpt-6-astra"),
        ("claude", "claude-fable-5", "claude-fable-5"),
        ("codex", "gpt-5.5", "gpt-5.5"),
        ("codex", "", "codex"),
    ] {
        let s = sess("", cli, model, "");
        assert_eq!(model_name(&s), want, "{cli}/{model}");
    }
}

#[test]
fn group_model_name_counts_distinct_models() {
    let cases = [
        (
            "three distinct models",
            items_of(vec![
                sess("g", "", "gpt-5.5", "megareview"),
                sess("g", "", "gemini-3.1", "megareview"),
                sess("g", "", "kimi-k3", "consilium"),
            ]),
            "gpt-5.5 +2",
        ),
        (
            "judge reuses a reviewer model",
            items_of(vec![
                sess("g", "", "gpt-6-astra", "megareview"),
                sess("g", "", "gpt-6-astra", "consilium"),
            ]),
            "gpt-6-astra",
        ),
        (
            "solo",
            items_of(vec![sess("", "", "claude-opus-5-5", "")]),
            "claude-opus-5-5",
        ),
    ];
    for (name, item, want) in cases {
        assert_eq!(group_model_name(&item), want, "{name}");
    }
    assert_eq!(group_model_name(&items_of(Vec::new())), "");
}

#[test]
fn kind_label_cases() {
    let solo_of = |cli: &str, mode: &str| solo(sess("", cli, "", mode));
    let group = |modes: &[&str]| items_of(modes.iter().map(|m| sess("g", "", "", m)).collect());
    let cases = [
        ("review", solo_of("codex", "review"), "review"),
        ("plan", solo_of("codex", "plan"), "plan"),
        (
            "security",
            solo_of("opencode", session::MODE_SECURITY),
            "sec",
        ),
        ("antislop", solo_of("codex", session::MODE_ANTISLOP), "slop"),
        ("raw", solo_of("opencode", "raw"), "raw"),
        ("native", solo_of("claude", "native"), "review"),
        ("empty mode", solo_of("codex", ""), "review"),
        ("docker claude", solo_of("claude", "docker"), "review/dk"),
        (
            "docker fable (read-compat)",
            solo_of("fable", "docker"),
            "review/dk",
        ),
        (
            "megareview group",
            group(&["megareview", "megareview", "consilium"]),
            "mega",
        ),
        ("plan group", group(&["plan", "plan"]), "plan"),
        (
            "antislop group",
            group(&[session::MODE_ANTISLOP, session::MODE_ANTISLOP]),
            "slop",
        ),
        ("security group", group(&[session::MODE_SECURITY]), "sec"),
    ];
    for (name, item, want) in cases {
        assert_eq!(kind_label(&item), want, "{name}");
    }
}

#[test]
fn project_name_is_the_last_path_element() {
    // Go filepath.Base of "/" is the host separator.
    let root = if cfg!(windows) { r"\" } else { "/" };
    for (path, want) in [
        ("/a/b/orbit-web", "orbit-web"),
        ("/a/b/orbit-web/", "orbit-web"),
        ("", "-"),
        ("/", root),
        ("plain", "plain"),
    ] {
        assert_eq!(project_name(path), want, "{path:?}");
    }
}

/// Both host rules on every platform: Windows splits at either separator and
/// drops a drive or UNC volume; Unix keeps a backslash in the name.
#[test]
fn base_name_follows_each_platform_rule() {
    for (path, want) in [
        (r"C:\work\orbit-web", "orbit-web"),
        (r"C:\work\orbit-web\", "orbit-web"),
        ("C:/work/orbit-web", "orbit-web"),
        (r"\\host\share\orbit-web", "orbit-web"),
        (r"C:\", r"\"),
        ("C:orbit-web", "orbit-web"),
        (r"a\b/c", "c"),
    ] {
        assert_eq!(windows_base_name(path), want, "Windows {path:?}");
    }
    for (path, want) in [
        (r"C:\work\orbit-web", r"C:\work\orbit-web"),
        (r"a\b/c", "c"),
        (r"/x/a\b", r"a\b"),
    ] {
        assert_eq!(base_name(path), want, "Unix {path:?}");
    }
    if cfg!(windows) {
        assert_eq!(project_name(r"D:\src\app"), "app");
    } else {
        assert_eq!(project_name(r"/src/a\b"), r"a\b");
    }
}

#[test]
fn section_for_buckets_by_calendar_day() {
    let tz = FixedOffset::east_opt(2 * 3600).unwrap();
    let at = |d, h, m, s| tz.with_ymd_and_hms(2026, 9, d, h, m, s).unwrap();
    let now = at(26, 11, 40, 0);
    for (t, want) in [
        (now - TimeDelta::hours(1), "TODAY"),
        (at(26, 0, 0, 1), "TODAY"),
        (at(26, 0, 0, 0), "TODAY"),
        (at(25, 23, 59, 0), "YESTERDAY"),
        (at(25, 0, 0, 0), "YESTERDAY"),
        (now - TimeDelta::days(3), "THIS WEEK"),
        (at(20, 0, 0, 0), "THIS WEEK"),
        (at(19, 23, 59, 59), "OLDER"),
        (now - TimeDelta::days(30), "OLDER"),
        (rival_core::gojson::zero_time(), "OLDER"),
        // A future start (clock skew) is today.
        (now + TimeDelta::days(2), "TODAY"),
    ] {
        assert_eq!(section_for(t, now, fixed_zone), want, "{t}");
    }
    // The same instant written in another offset buckets by the zone's day.
    let utc = (at(26, 0, 30, 0)).with_timezone(&FixedOffset::east_opt(0).unwrap());
    assert_eq!(section_for(utc.fixed_offset(), now, fixed_zone), "TODAY");
}

fn hours(h: i32) -> FixedOffset {
    FixedOffset::east_opt(h * 3600).unwrap()
}

fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(y, mo, d)
        .unwrap()
        .and_hms_opt(h, mi, 0)
        .unwrap()
}

/// Europe/Berlin in 2026: CET (+1), CEST (+2) from 29 Mar 01:00 UTC until
/// 25 Oct 01:00 UTC. The switches happen at 02:00/03:00 local time.
fn berlin(t: NaiveDateTime) -> FixedOffset {
    let summer = utc(2026, 3, 29, 1, 0) <= t && t < utc(2026, 10, 25, 1, 0);
    hours(if summer { 2 } else { 1 })
}

/// A zone that switches at local midnight, like America/Havana: -5, then
/// -4 from 8 Mar 2026 00:00 (skipped to 01:00) until 1 Nov 2026 01:00
/// (back to 00:00, so that midnight happens twice).
fn havana(t: NaiveDateTime) -> FixedOffset {
    let summer = utc(2026, 3, 8, 5, 0) <= t && t < utc(2026, 11, 1, 5, 0);
    hours(if summer { -4 } else { -5 })
}

/// `y-mo-d h:mi` local time written with the given offset.
fn wall(off: i32, y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<FixedOffset> {
    hours(off).with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
}

/// Go `newDayBounds` builds each boundary with `time.Date` in the local
/// zone. A DST switch between `now` and a boundary must not shift that
/// boundary by the offset change. The spring "edge" cases, the fall cases
/// without "edge" and "utc now" land in the wrong section when every
/// midnight reuses `now`'s offset; the others pin the right side.
#[test]
fn section_for_follows_dst_per_boundary() {
    let cases = [
        // Spring forward, Sun 29 Mar. Noon is +2, but midnight was +1.
        (
            "spring today",
            wall(2, 2026, 3, 29, 12, 0),
            wall(1, 2026, 3, 29, 0, 0),
            "TODAY",
        ),
        (
            "spring today edge",
            wall(2, 2026, 3, 29, 12, 0),
            wall(1, 2026, 3, 28, 23, 30),
            "YESTERDAY",
        ),
        (
            "spring yesterday",
            wall(2, 2026, 3, 30, 9, 0),
            wall(1, 2026, 3, 29, 0, 10),
            "YESTERDAY",
        ),
        (
            "spring yesterday edge",
            wall(2, 2026, 3, 30, 9, 0),
            wall(1, 2026, 3, 28, 23, 30),
            "THIS WEEK",
        ),
        (
            "spring week",
            wall(2, 2026, 4, 2, 12, 0),
            wall(1, 2026, 3, 27, 0, 0),
            "THIS WEEK",
        ),
        (
            "spring week edge",
            wall(2, 2026, 4, 2, 12, 0),
            wall(1, 2026, 3, 26, 23, 30),
            "OLDER",
        ),
        // Fall back, Sun 25 Oct. Noon is +1, but midnight was +2.
        (
            "fall today",
            wall(1, 2026, 10, 25, 12, 0),
            wall(2, 2026, 10, 25, 0, 30),
            "TODAY",
        ),
        (
            "fall today edge",
            wall(1, 2026, 10, 25, 12, 0),
            wall(2, 2026, 10, 24, 23, 59),
            "YESTERDAY",
        ),
        (
            "fall yesterday",
            wall(1, 2026, 10, 26, 9, 0),
            wall(2, 2026, 10, 25, 0, 30),
            "YESTERDAY",
        ),
        (
            "fall yesterday edge",
            wall(1, 2026, 10, 26, 9, 0),
            wall(2, 2026, 10, 24, 23, 59),
            "THIS WEEK",
        ),
        (
            "fall week",
            wall(1, 2026, 10, 29, 12, 0),
            wall(2, 2026, 10, 23, 0, 30),
            "THIS WEEK",
        ),
        (
            "fall week edge",
            wall(1, 2026, 10, 29, 12, 0),
            wall(2, 2026, 10, 22, 23, 59),
            "OLDER",
        ),
        // "now" written in UTC still takes today's date in the zone:
        // 28 Mar 23:30 UTC is already 29 Mar 00:30 in Berlin.
        (
            "utc now",
            wall(0, 2026, 3, 28, 23, 30),
            wall(1, 2026, 3, 29, 0, 10),
            "TODAY",
        ),
        (
            "utc now edge",
            wall(0, 2026, 3, 28, 23, 30),
            wall(1, 2026, 3, 28, 23, 50),
            "YESTERDAY",
        ),
    ];
    let wrong: Vec<String> = cases
        .iter()
        .filter_map(|&(name, now, t, want)| {
            let got = section_for(t, now, berlin);
            (got != want).then(|| format!("{name}: {t} at {now} is {got}, want {want}"))
        })
        .collect();
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// Go `time.Date` on a midnight a switch skips or repeats: it guesses the
/// offset at the wall time read as UTC, then rechecks it at the result.
#[test]
fn midnight_resolves_skipped_and_repeated_midnights_as_go() {
    let cases: [(&str, Zone, Option<NaiveDate>, DateTime<FixedOffset>); 5] = [
        // No switch that day: the plain local midnight.
        (
            "plain",
            havana,
            NaiveDate::from_ymd_opt(2026, 6, 1),
            wall(-4, 2026, 6, 1, 0, 0),
        ),
        (
            "berlin spring",
            berlin,
            NaiveDate::from_ymd_opt(2026, 3, 29),
            wall(1, 2026, 3, 29, 0, 0),
        ),
        (
            "berlin fall",
            berlin,
            NaiveDate::from_ymd_opt(2026, 10, 25),
            wall(2, 2026, 10, 25, 0, 0),
        ),
        // 00:00 is skipped. Go's guess (-5) lands on 05:00 UTC, past the
        // switch, so it retries with -4: 04:00 UTC, 23:00 the day before.
        (
            "skipped",
            havana,
            NaiveDate::from_ymd_opt(2026, 3, 8),
            wall(-5, 2026, 3, 7, 23, 0),
        ),
        // 00:00 happens twice. Go's guess (-4) holds: the first one.
        (
            "repeated",
            havana,
            NaiveDate::from_ymd_opt(2026, 11, 1),
            wall(-4, 2026, 11, 1, 0, 0),
        ),
    ];
    for (name, zone, day, want) in cases {
        let got = midnight(zone, day.unwrap());
        assert_eq!(got, want, "{name}: {got}");
        assert_eq!(*got.offset(), zone(got.naive_utc()), "{name}: shown offset");
    }
    // The skipped midnight, seen through the sections: the hour before it
    // already counts as the new day, as in Go.
    let now = wall(-4, 2026, 3, 8, 12, 0);
    assert_eq!(
        section_for(wall(-5, 2026, 3, 7, 23, 30), now, havana),
        "TODAY"
    );
    assert_eq!(
        section_for(wall(-5, 2026, 3, 7, 22, 59), now, havana),
        "YESTERDAY"
    );
}

/// `local_zone` reads the system zone per instant, not once. A child
/// process gets a switching POSIX `TZ` rule, so neither this process's zone
/// nor any parallel test changes. Unix only: chrono reads `TZ` there.
#[cfg(unix)]
#[test]
fn local_zone_switches_offset_at_dst() {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const CHILD: &str = "RIVAL_TEST_TZ_CHILD";
    const DONE: &str = "local-zone-child-done";
    if std::env::var_os(CHILD).is_some() {
        assert_eq!(local_zone(utc(2026, 3, 29, 0, 59)), hours(1));
        assert_eq!(local_zone(utc(2026, 3, 29, 1, 0)), hours(2));
        assert_eq!(local_zone(utc(2026, 10, 25, 0, 59)), hours(2));
        assert_eq!(local_zone(utc(2026, 10, 25, 1, 0)), hours(1));
        let now = wall(2, 2026, 3, 30, 9, 0);
        assert_eq!(
            section_for(wall(1, 2026, 3, 29, 0, 10), now, local_zone),
            "YESTERDAY"
        );
        assert_eq!(
            section_for(wall(1, 2026, 3, 28, 23, 30), now, local_zone),
            "THIS WEEK"
        );
        println!("{DONE}");
        return;
    }
    let name = format!(
        "{}::local_zone_switches_offset_at_dst",
        module_path!().split_once("::").unwrap().1
    );
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args([name.as_str(), "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        // Europe/Berlin as a POSIX rule, so no tz database is needed.
        .env("TZ", "CET-1CEST,M3.5.0,M10.5.0/3")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = rival_core::executor::process::spawn(&mut cmd).unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut timed_out = false;
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            // Kill only this child; wait_with_output below reaps it.
            let _ = child.kill();
            timed_out = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let out = child.wait_with_output().unwrap();
    let full = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // At most 4 KiB of the child's output goes into a failure message.
    let cut = (0..=full.len().min(4096))
        .rev()
        .find(|&i| full.is_char_boundary(i))
        .unwrap_or(0);
    let text = &full[..cut];
    assert!(
        !timed_out,
        "the TZ child test did not finish in 60s:\n{text}"
    );
    assert!(out.status.success(), "{}:\n{text}", out.status);
    // A filter that matched nothing would also exit 0.
    assert!(full.contains(DONE), "{text}");
}

#[test]
fn matches_filter_cases() {
    let items = filter_fixture(fixed_now());
    let cases: [(&[&str], [bool; 4]); 10] = [
        (&[], [true, true, true, true]),
        (&["orbit"], [true, false, true, false]),
        (&["orbit", "failed"], [false, false, true, false]),
        (&["opus"], [false, true, false, false]),
        (&["fingerprint"], [true, false, false, false]),
        (&["service-identity"], [false, true, false, false]),
        (&["cccccccc"], [false, false, true, false]),
        (&["plan"], [false, true, false, false]),
        (&["xhigh"], [true, false, false, false]),
        (&["nothing-matches"], [false, false, false, false]),
    ];
    for (terms, want) in cases {
        let terms: Vec<String> = terms.iter().map(|t| t.to_string()).collect();
        for (i, item) in items.iter().enumerate() {
            assert_eq!(
                matches_filter(item, &mut None, &terms),
                want[i],
                "item {i}, {terms:?}"
            );
        }
    }
}

#[test]
fn filter_haystack_never_spans_two_fields() {
    let items = filter_fixture(fixed_now());
    // "medium" + "ledger" are adjacent fields; the separator keeps them apart.
    let terms = filter_terms("mediumledger");
    assert!(!matches_filter(&items[1], &mut None, &terms));
    // The short id is the first 8 bytes only.
    assert!(!matches_filter(
        &items[1],
        &mut None,
        &filter_terms("bbbbbbbb-2")
    ));
    // A cached haystack is reused, not rebuilt.
    let mut hay = Some("cached".to_string());
    assert!(matches_filter(&items[0], &mut hay, &filter_terms("CACHED")));
}

#[test]
fn build_rows() {
    let now = fixed_now();
    let items = filter_fixture(now);
    assert_eq!(
        section_for(now - TimeDelta::days(1), now, fixed_zone),
        "YESTERDAY"
    );
    let cases: [(&str, StatusTab, &str, &[&str]); 7] = [
        (
            "all, no filter",
            StatusTab::All,
            "",
            &["#TODAY", "a", "b", "#YESTERDAY", "c", "#OLDER", "d"],
        ),
        (
            "filter case-insensitive",
            StatusTab::All,
            "ORBIT",
            &["#TODAY", "a", "#YESTERDAY", "c"],
        ),
        ("running tab", StatusTab::Running, "", &["#TODAY", "a"]),
        ("failed tab", StatusTab::Failed, "", &["#YESTERDAY", "c"]),
        (
            "done tab",
            StatusTab::Done,
            "",
            &["#TODAY", "b", "#OLDER", "d"],
        ),
        (
            "tab composes with filter",
            StatusTab::Done,
            "gpt-5.5",
            &["#OLDER", "d"],
        ),
        ("no match", StatusTab::All, "zzz", &[]),
    ];
    for (name, tab, filter, want) in cases {
        let got = row_summary(&items, &rows(&items, tab, filter, now));
        assert_eq!(got, want, "{name}");
    }
}

/// Out-of-order input still yields one header per section, in order.
#[test]
fn build_rows_groups_out_of_order_items() {
    let now = fixed_now();
    let base = filter_fixture(now);
    let shuffled = vec![
        base[3].clone(),
        base[0].clone(),
        base[2].clone(),
        base[1].clone(),
    ];
    let got = row_summary(&shuffled, &rows(&shuffled, StatusTab::All, "", now));
    assert_eq!(got, ["#TODAY", "a", "b", "#YESTERDAY", "c", "#OLDER", "d"]);
}

#[test]
fn counts_follow_the_filter_not_the_tab() {
    let now = fixed_now();
    let items = filter_fixture(now);
    let mut hays = vec![None; items.len()];
    let (_, counts) = rows_and_counts(&items, &mut hays, StatusTab::Done, &[], now, fixed_zone);
    assert_eq!(counts, [4, 1, 1, 2]);
    let (_, counts) = rows_and_counts(
        &items,
        &mut hays,
        StatusTab::Done,
        &filter_terms("orbit"),
        now,
        fixed_zone,
    );
    assert_eq!(counts, [2, 1, 1, 0]);
}

#[test]
fn layout_columns_drops_effort_then_project() {
    let c = layout_columns(120);
    assert!(c.effort > 0 && c.project > 0, "{c:?}");
    let c = layout_columns(68);
    assert!(c.effort > 0 && c.project > 0, "{c:?}");
    let c = layout_columns(67);
    assert!(c.effort == 0 && c.project > 0, "{c:?}");
    let c = layout_columns(59);
    assert!(c.effort == 0 && c.project == 0, "{c:?}");
    for w in [60, 70, 89, 90, 120, 200] {
        let c = layout_columns(w);
        let sum: usize = 1 + c
            .widths()
            .iter()
            .filter(|&&x| x > 0)
            .map(|x| x + 1)
            .sum::<usize>();
        // PROJECT absorbs the spare width, so the row is exactly the pane
        // width plus its trailing gap.
        assert_eq!(sum - 1, w, "{c:?}");
    }
}

#[test]
fn fit_cell_go_cases() {
    for (s, w, want) in [
        ("abc", 5, "abc  "),
        ("abcdef", 4, "abc…"),
        ("日本語", 4, "日… "),
        ("✓", 3, "✓  "),
        ("x", 0, ""),
    ] {
        let got = fit_cell(s, w);
        assert_eq!(got, want, "fit_cell({s:?}, {w})");
        assert_eq!(width(&got), w);
    }
}

#[test]
fn status_glyph_cases() {
    for (status, want) in [
        ("running", "⠋"),
        ("queued", "◌"),
        ("completed", "✓"),
        ("failed", "✗"),
        ("killed", "·"),
        ("", "·"),
    ] {
        assert_eq!(status_glyph(status, "⠋"), want, "{status:?}");
    }
}

/// Regression for misaligned rows: every line is exactly the pane width,
/// whatever multi-byte, wide or joined text sits in its cells.
#[test]
fn rendered_rows_fill_exactly_the_pane_width() {
    let now = fixed_now();
    let queued_at = now - TimeDelta::minutes(1);
    let items = vec![
        solo(Session {
            prompt_preview: "長い prompt ".repeat(30).into(),
            ..run(
                "a1",
                "claude",
                "claude-opus-4-6[1m]",
                "docker",
                "max",
                "running",
                now - TimeDelta::minutes(1),
                "/src/日本語のプロジェクト",
            )
        }),
        solo(Session {
            duration: "6m24s".into(),
            prompt_preview: "p".repeat(300).into(),
            ..run(
                "b2",
                "codex",
                "gpt-6-astra",
                "review",
                "xhigh",
                "completed",
                now - TimeDelta::hours(1),
                "/src/disk-watcher",
            )
        }),
        solo(Session {
            queue_position: 3,
            queued_at: Some(queued_at),
            ..run(
                "c3",
                "codex",
                "gpt-5.5",
                "review",
                "",
                "queued",
                rival_core::gojson::zero_time(),
                "/x",
            )
        }),
        items_of(vec![
            Session {
                group_id: "g".into(),
                ..run(
                    "d4",
                    "codex",
                    "gpt-5.5",
                    "megareview",
                    "high",
                    "failed",
                    now - TimeDelta::days(1),
                    "/src/mathquest",
                )
            },
            Session {
                group_id: "g".into(),
                ..run(
                    "d5",
                    "opencode",
                    "kimi-k3",
                    "megareview",
                    "high",
                    "completed",
                    now - TimeDelta::days(1),
                    "",
                )
            },
        ]),
        // A joined emoji and a flag in the cells never split.
        solo(run(
            "e6",
            "codex",
            "👩\u{200D}💻-model-with-a-long-name",
            "review",
            "🇺🇦",
            "completed",
            now - TimeDelta::hours(3),
            "/src/👩\u{200D}💻👩\u{200D}💻👩\u{200D}💻👩\u{200D}💻👩\u{200D}💻👩\u{200D}💻👩\u{200D}💻👩\u{200D}💻👩\u{200D}💻",
        )),
    ];
    for w in [60, 67, 68, 90, 120, 200] {
        let mut l = pane(items.clone(), now);
        for selected in [0isize, 4] {
            l.top();
            l.move_by(selected);
            let lines = l.view_lines(w, 20, "⠋", now, &STYLES);
            assert_eq!(lines.len(), 20, "width {w}");
            for (i, line) in lines.iter().enumerate() {
                assert_eq!(line_width(line), w, "width {w}, line {i}: {line}");
            }
        }
    }
}

// --- Go list_model_test.go (pane level) ---------------------------------------

#[test]
fn list_move_skips_section_rows() {
    let now = fixed_now();
    // #TODAY a b #YESTERDAY c #OLDER d
    let mut l = pane(filter_fixture(now), now);
    l.top();
    assert!(l.rows[l.cursor].is_run());
    assert_eq!(selected_letter(&l), "a");
    l.move_by(1);
    l.move_by(1); // b -> across #YESTERDAY -> c
    assert_eq!(selected_letter(&l), "c");
    l.move_by(-1);
    assert_eq!(selected_letter(&l), "b");
    l.bottom();
    assert_eq!(selected_letter(&l), "d");
    l.move_by(5); // stops at the end
    assert_eq!(selected_letter(&l), "d");
    l.top();
    l.move_by(-3); // stops at the start, never on the header
    assert_eq!(selected_letter(&l), "a");
    // A multi-step move skips headers too.
    l.move_by(3);
    assert_eq!(selected_letter(&l), "d");
}

/// A group is anchored by its group id, a solo run by its own id, and a
/// cursor that falls on a header moves to the run below it.
#[test]
fn rebuild_anchors_by_item_key_and_skips_headers() {
    let now = fixed_now();
    let grouped = |id: &str, start| {
        Arc::new(Session {
            group_id: "g1".into(),
            ..run(
                id,
                "codex",
                "gpt-5.5",
                "megareview",
                "high",
                "completed",
                start,
                "/p",
            )
        })
    };
    let solo_run = |id: &str, start| {
        Arc::new(run(
            id,
            "codex",
            "gpt-5.5",
            "review",
            "high",
            "completed",
            start,
            "/p",
        ))
    };
    let old = now - TimeDelta::days(40);
    let mut l = pane(
        group_sessions(&[solo_run("s1", now), grouped("m1", old), grouped("m2", old)]),
        now,
    );
    l.bottom();
    assert_eq!(l.selected_key(), "group:g1");
    // The group's members change and it moves to TODAY: still selected.
    l.set_items(
        group_sessions(&[
            grouped("m3", now),
            solo_run("s1", now - TimeDelta::minutes(1)),
        ]),
        now,
    );
    assert_eq!(l.selected_key(), "group:g1");
    assert_eq!(l.rows[l.cursor - 1], Row::Section("TODAY"));
    // The group vanishes: the cursor clamps onto a run, never a header.
    l.set_items(group_sessions(&[solo_run("s1", old)]), now);
    assert_eq!(l.rows, [Row::Section("OLDER"), Row::Run(0)]);
    assert_eq!(selected_id(&l), "s1");
    // Everything vanishes.
    l.set_items(Vec::new(), now);
    assert_eq!((l.cursor, l.selected().is_none()), (0, true));

    // The old index now holds a header: the run below it wins.
    let mut l = pane(
        group_sessions(&[solo_run("a", now), solo_run("b", now)]),
        now,
    );
    l.bottom();
    l.set_items(
        group_sessions(&[solo_run("a", now), solo_run("c", old)]),
        now,
    );
    assert_eq!(l.rows[2], Row::Section("OLDER"));
    assert_eq!(selected_id(&l), "c");
}

// --- Go pagination_test.go (pane level) ---------------------------------------

#[test]
fn page_slicing_with_sections() {
    let now = fixed_now();
    let mut l = many_list_pane(120, now);
    assert_eq!(l.page_count(), 3);
    let pages: [(&str, &str, &[&str], usize); 3] = [
        ("r000", "r049", &["#TODAY", "#YESTERDAY"], 50),
        // Page 2 starts mid-YESTERDAY: it gets its own copy of the header.
        ("r050", "r099", &["#YESTERDAY", "#OLDER"], 50),
        ("r100", "r119", &["#OLDER"], 20),
    ];
    for (p, (first, last, headers, runs)) in pages.into_iter().enumerate() {
        assert_eq!(l.page(), p);
        let rows = page_summary(&l);
        assert!(
            rows[0].starts_with('#'),
            "page {} starts with {:?}",
            p + 1,
            rows[0]
        );
        let (got_headers, got_runs): (Vec<&String>, Vec<&String>) =
            rows.iter().partition(|r| r.starts_with('#'));
        assert_eq!(got_headers, headers, "page {}", p + 1);
        assert_eq!(got_runs.len(), runs, "page {}", p + 1);
        assert_eq!(got_runs[0], first);
        assert_eq!(got_runs[runs - 1], last);
        assert_eq!(
            selected_id(&l),
            first,
            "page {}: cursor on the first run",
            p + 1
        );
        l.turn_page(1);
    }
    for (n, pages) in [(0, 1), (1, 1), (50, 1), (51, 2), (100, 2), (101, 3)] {
        assert_eq!(many_list_pane(n, now).page_count(), pages, "{n} runs");
    }
}

/// A page that starts right after a header borrows it; the cursor index
/// inside the page counts the header.
#[test]
fn page_rows_cursor_counts_the_copied_header() {
    let now = fixed_now();
    let mut l = many_list_pane(120, now);
    let (rows, cursor) = l.page_rows();
    assert_eq!((rows.len(), cursor), (52, 1), "#TODAY 30 #YESTERDAY 20");
    l.turn_page(1);
    let (rows, cursor) = l.page_rows();
    assert!(matches!(rows, Cow::Owned(_)), "page 2 copies #YESTERDAY");
    assert_eq!((rows.len(), cursor), (52, 1));
    l.turn_page(1);
    let (rows, cursor) = l.page_rows();
    assert_eq!((rows.len(), cursor), (21, 1));
    assert_eq!(rows[0], Row::Section("OLDER"));
}

#[test]
fn clamp_offset_keeps_the_cursor_and_its_header_in_view() {
    let now = fixed_now();
    let mut l = many_list_pane(120, now);
    l.clamp_offset(10);
    assert_eq!(l.offset, 0);
    l.move_by(20);
    l.clamp_offset(10);
    let (_, cursor) = l.page_rows();
    assert_eq!(cursor, 21);
    assert_eq!(l.offset, 12, "cursor on the last visible row");
    // r030 is the first YESTERDAY run: its header stays in view.
    l.move_by(10);
    l.clamp_offset(10);
    let (rows, cursor) = l.page_rows();
    assert_eq!(rows[cursor - 1], Row::Section("YESTERDAY"));
    assert_eq!(l.offset, cursor + 1 - 10);
    l.move_by(-9); // up to r021: scroll back
    l.clamp_offset(10);
    assert_eq!(l.offset, 22);
    // Moving to the top of a section scrolls its header in.
    l.offset = 40;
    l.move_by(9); // r030 again
    l.clamp_offset(10);
    let (_, cursor) = l.page_rows();
    assert_eq!(l.offset, cursor - 1);
    // Never past the end of the page, never with a zero window.
    l.bottom();
    l.offset = 999;
    l.clamp_offset(0);
    let (rows, _) = l.page_rows();
    assert_eq!(l.offset, rows.len() - 1);
}

#[test]
fn page_footer_text() {
    let now = fixed_now();
    let mut l = many_list_pane(120, now);
    assert_eq!(l.footer_text(), "‹ prev  page 1/3  next ›  · 120 runs");
    l.turn_page(1);
    assert_eq!(l.footer_text(), "‹ prev  page 2/3  next ›  · 120 runs");
    assert_eq!(many_list_pane(4, now).footer_text(), "4 runs");
    assert_eq!(many_list_pane(0, now).footer_text(), "0 runs");
    assert_eq!(many_list_pane(1, now).footer_text(), "1 run");

    // prev is dim on page 1, next is dim on the last page.
    let span_style = |line: &Line<'_>, text: &str| {
        line.spans
            .iter()
            .find(|s| s.content == text)
            .map(|s| s.style)
            .unwrap_or_else(|| panic!("{text:?} missing from {line}"))
    };
    let mut first = many_list_pane(120, now);
    let footer = first.footer(80, &STYLES);
    assert_eq!(span_style(&footer, "‹ prev"), STYLES.dim);
    assert_eq!(span_style(&footer, "next ›"), STYLES.text);
    first.turn_page(5);
    let footer = first.footer(80, &STYLES);
    assert_eq!(span_style(&footer, "‹ prev"), STYLES.text);
    assert_eq!(span_style(&footer, "next ›"), STYLES.dim);
    assert_eq!(first.page(), 2, "turn_page stops at the last page");

    // It always fills the width exactly, even when it has to be cut.
    for w in [10, 30, 66, 200] {
        assert_eq!(line_width(&l.footer(w, &STYLES)), w, "footer at {w}");
    }
    // Too narrow for the styled form: one dim, cut line.
    let cut = l.footer(10, &STYLES);
    assert_eq!(cut.to_string(), " ‹ prev  …");
    assert_eq!(cut.style, STYLES.dim);
}

#[test]
fn run_count_plural() {
    for (n, want) in [
        (0, "0 runs"),
        (1, "1 run"),
        (2, "2 runs"),
        (120, "120 runs"),
    ] {
        assert_eq!(run_count(n), want);
    }
}

// --- times ------------------------------------------------------------------

#[test]
fn time_cell_uses_the_injected_clock() {
    let now = fixed_now();
    let running = solo(run(
        "a",
        "codex",
        "m",
        "review",
        "",
        "running",
        now - TimeDelta::seconds(83),
        "/p",
    ));
    assert_eq!(row_time(&running, now), "1m23s");
    let queued = solo(Session {
        queue_position: 2,
        queued_at: Some(now - TimeDelta::milliseconds(4500)),
        ..run(
            "b",
            "codex",
            "m",
            "review",
            "",
            "queued",
            rival_core::gojson::zero_time(),
            "/p",
        )
    });
    assert_eq!(
        row_time(&queued, now),
        "#2 5s",
        "rounds half away from zero"
    );
    let finished = solo(Session {
        duration: "6m24s".into(),
        ..run("c", "codex", "m", "review", "", "completed", now, "/p")
    });
    assert_eq!(row_time(&finished, now), "6m24s");
    let unknown = solo(run("d", "codex", "m", "review", "", "killed", now, "/p"));
    assert_eq!(row_time(&unknown, now), "-");
    // A queued run sorts by its queue time until it starts.
    assert_eq!(item_time(&queued), now - TimeDelta::milliseconds(4500));
    assert_eq!(item_time(&finished), now);
}
