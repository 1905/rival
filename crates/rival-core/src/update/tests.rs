//! Go `internal/update/check.go` behavior. No test touches the network: the
//! fetch is injected, and the HTTP client is only reached through
//! `fetch_latest`, which no test calls.

use super::*;

use std::cell::Cell;
use std::collections::HashMap;

use chrono::TimeZone;

fn cfg(root: &Path, vars: &[(&str, &str)]) -> Config {
    let env: HashMap<String, String> = vars
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    Config::new(
        Paths {
            root: root.to_path_buf(),
        },
        env,
        None,
    )
}

fn at(secs: i64) -> DateTime<FixedOffset> {
    FixedOffset::east_opt(0)
        .unwrap()
        .timestamp_opt(1_790_000_000 + secs, 0)
        .unwrap()
}

#[test]
fn normalize_version_pads_three_parts_with_zeros() {
    for (input, want) in [
        ("3.5.0", "003.005.000"),
        ("v3.5.0", "003.005.000"),
        ("1.10.2", "001.010.002"),
        ("1234.5.6", "1234.005.006"),
        ("1..2", "001.000.002"),
        ("a.b.c", "00a.00b.00c"),
        ("é.1.1", "00é.001.001"),
        ("dev", "dev"),
        ("vv1.2.3", "0v1.002.003"),
        ("1.2", "1.2"),
        ("1.2.3.4", "1.2.3.4"),
        ("", ""),
        ("v", ""),
    ] {
        assert_eq!(normalize_version(input), want, "{input:?}");
    }
}

#[test]
fn notice_follows_go_string_ordering() {
    let n = |c: &str, l: &str| notice(c, l);
    let text =
        |c: &str, l: &str| format!("\n  Update available: v{c} → v{l} — run 'rival update'\n\n");
    // Plain numeric releases.
    assert_eq!(n("1.2.3", "1.2.4"), Some(text("1.2.3", "1.2.4")));
    assert_eq!(n("1.2.3", "1.10.0"), Some(text("1.2.3", "1.10.0")));
    assert_eq!(n("v1.2.3", "1.3.0"), Some(text("1.2.3", "1.3.0")));
    assert_eq!(n("1.3.0", "1.2.9"), None);
    assert_eq!(n("1.2.3", "1.2.3"), None);
    assert_eq!(n("v1.2.3", "1.2.3"), None);
    assert_eq!(n("1.2.3", ""), None);
    // A dev build: "009.009.009" < "dev" byte-wise, so no notice...
    assert_eq!(n("dev", "9.9.9"), None);
    // ...but a non-semver tag that sorts after "dev" does print one.
    assert_eq!(n("dev", "zzz"), Some(text("dev", "zzz")));
    // Two-part versions compare unpadded: "1.10" < "1.9".
    assert_eq!(n("1.9", "1.10"), None);
    // The latest version prints as given, "v" and all.
    assert_eq!(n("1.0.0", "v2.0.0"), Some(text("1.0.0", "v2.0.0")));
    // An empty current version never prints.
    assert_eq!(n("", "1.0.0"), None);
}

#[test]
fn print_if_newer_writes_the_notice_only() {
    let mut out = Vec::new();
    print_if_newer("1.0.0", "0.9.0", &mut out);
    assert!(out.is_empty());
    print_if_newer("1.0.0", "1.0.1", &mut out);
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "\n  Update available: v1.0.0 → v1.0.1 — run 'rival update'\n\n"
    );
}

#[test]
fn skip_check_only_empty_zero_and_false_allow_the_check() {
    for (ci, opt_out, skip) in [
        ("", "", false),
        ("0", "false", false),
        ("1", "", true),
        ("", "1", true),
        ("true", "", true),
        ("FALSE", "", true),
        (" ", "", true),
        ("", "no", true),
    ] {
        let got = skip_check(|key| match key {
            "CI" => ci,
            "RIVAL_NO_UPDATE_CHECK" => opt_out,
            _ => panic!("unexpected key {key}"),
        });
        assert_eq!(got, skip, "CI={ci:?} RIVAL_NO_UPDATE_CHECK={opt_out:?}");
    }
}

#[test]
fn cache_file_lives_in_the_rival_root() {
    let p = Paths::from_home(Path::new("/h"));
    assert_eq!(
        cache_file_path(&p),
        PathBuf::from("/h/.rival/.update-check")
    );
    // Go joins "" when the home is unknown: ".rival/.update-check".
    let p = Paths::from_vars(None, None);
    assert_eq!(cache_file_path(&p), PathBuf::from("./.rival/.update-check"));
}

#[test]
fn save_cache_writes_go_json_with_private_modes() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("deep/.rival/.update-check");
    let t = FixedOffset::east_opt(0)
        .unwrap()
        .with_ymd_and_hms(2026, 10, 3, 12, 0, 0)
        .unwrap()
        + TimeDelta::nanoseconds(123_456_000);
    save_cache(&path, "1.2.3<&>", t).unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "{\"latest\":\"1.2.3\\u003c\\u0026\\u003e\",\"checked_at\":\"2026-10-03T12:00:00.123456Z\"}"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
    }
    // Rewrites truncate.
    save_cache(&path, "1", t).unwrap();
    assert_eq!(load_cache(&path).unwrap().latest, "1");
}

#[test]
fn load_cache_decodes_like_go() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("c");
    let load = |text: &str| {
        fs::write(&path, text).unwrap();
        load_cache(&path)
    };
    assert!(load_cache(&tmp.path().join("missing")).is_err());
    let c = load(r#"{"LATEST":"2.0.0","Checked_At":"2026-10-03T12:00:00+02:00","x":1}"#).unwrap();
    assert_eq!(c.latest, "2.0.0");
    assert_eq!(c.checked_at.to_rfc3339(), "2026-10-03T12:00:00+02:00");
    // Missing fields keep Go's zero values; null keeps them too.
    let c = load(r#"{"latest":null}"#).unwrap();
    assert_eq!(c.latest, "");
    assert_eq!(c.checked_at, gojson::zero_time());
    // The last duplicate wins.
    assert_eq!(load(r#"{"latest":"1","latest":"2"}"#).unwrap().latest, "2");
    for bad in [
        "",
        "{",
        "[]",
        r#"{"latest":1}"#,
        r#"{"checked_at":"yesterday"}"#,
    ] {
        assert!(load(bad).is_err(), "{bad:?}");
    }
}

/// Runs `check` with a counting fetch. Returns (stderr, fetch calls).
fn run_check(c: &Config, now: DateTime<FixedOffset>, answer: Result<&str, &str>) -> (String, u32) {
    let calls = Cell::new(0);
    let fetch = || {
        calls.set(calls.get() + 1);
        answer.map(str::to_string).map_err(str::to_string)
    };
    let mut out = Vec::new();
    check("1.0.0", c, &|| now, &fetch, &mut out);
    (String::from_utf8(out).unwrap(), calls.get())
}

/// Go reads `time.Now()` for the cache age before the fetch and again in
/// `saveCache` after it: `checked_at` is the post-fetch time.
#[test]
fn check_reads_the_clock_before_and_after_the_fetch() {
    let tmp = tempfile::tempdir().unwrap();
    let c = cfg(tmp.path(), &[]);
    let file = tmp.path().join(".update-check");
    save_cache(&file, "0.9.0", at(0)).unwrap();
    let reads = Cell::new(0i64);
    // Each read advances 5 s; the fetch happens between the two reads.
    let clock = || {
        reads.set(reads.get() + 1);
        at(86_400 + 5 * reads.get())
    };
    let fetched_after = Cell::new(0i64);
    let fetch = || {
        fetched_after.set(reads.get());
        Ok("2.0.0".to_string())
    };
    let mut out = Vec::new();
    check("1.0.0", &c, &clock, &fetch, &mut out);
    assert_eq!(fetched_after.get(), 1, "one clock read before the fetch");
    assert_eq!(reads.get(), 2);
    assert_eq!(load_cache(&file).unwrap().checked_at, at(86_400 + 10));
    assert_eq!(String::from_utf8(out).unwrap(), NOTICE_2);
    // A failed fetch reads the clock once and saves nothing.
    reads.set(0);
    let mut out = Vec::new();
    check("1.0.0", &c, &clock, &|| Err("boom".to_string()), &mut out);
    assert_eq!(reads.get(), 1);
}

const NOTICE_2: &str = "\n  Update available: v1.0.0 → v2.0.0 — run 'rival update'\n\n";

#[test]
fn check_fetches_saves_and_prints_when_the_cache_is_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let c = cfg(tmp.path(), &[]);
    assert_eq!(run_check(&c, at(0), Ok("2.0.0")), (NOTICE_2.to_string(), 1));
    let saved = load_cache(&tmp.path().join(".update-check")).unwrap();
    assert_eq!(saved.latest, "2.0.0");
    assert_eq!(saved.checked_at, at(0));
}

#[test]
fn check_uses_a_fresh_cache_without_fetching() {
    let tmp = tempfile::tempdir().unwrap();
    let c = cfg(tmp.path(), &[]);
    let file = tmp.path().join(".update-check");
    save_cache(&file, "2.0.0", at(0)).unwrap();
    let just_fresh = at(24 * 60 * 60 - 1);
    assert_eq!(
        run_check(&c, just_fresh, Ok("9.0.0")),
        (NOTICE_2.to_string(), 0)
    );
    // A time in the future counts as fresh (negative age).
    assert_eq!(
        run_check(&c, at(-3600), Ok("9.0.0")),
        (NOTICE_2.to_string(), 0)
    );
    // A fresh cache with an older version prints nothing and fetches nothing.
    save_cache(&file, "0.9.0", at(0)).unwrap();
    assert_eq!(run_check(&c, at(1), Ok("9.0.0")), (String::new(), 0));
}

#[test]
fn check_refetches_after_the_ttl() {
    let tmp = tempfile::tempdir().unwrap();
    let c = cfg(tmp.path(), &[]);
    let file = tmp.path().join(".update-check");
    save_cache(&file, "0.9.0", at(0)).unwrap();
    let stale = at(24 * 60 * 60);
    assert_eq!(run_check(&c, stale, Ok("2.0.0")), (NOTICE_2.to_string(), 1));
    assert_eq!(load_cache(&file).unwrap().checked_at, stale);
    // An unreadable cache counts as stale.
    fs::write(&file, "{").unwrap();
    assert_eq!(run_check(&c, stale, Ok("2.0.0")), (NOTICE_2.to_string(), 1));
}

#[test]
fn check_ignores_fetch_errors_and_keeps_the_old_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let c = cfg(tmp.path(), &[]);
    let file = tmp.path().join(".update-check");
    assert_eq!(run_check(&c, at(0), Err("status 500")), (String::new(), 1));
    assert!(!file.exists());
    save_cache(&file, "2.0.0", at(0)).unwrap();
    let before = fs::read(&file).unwrap();
    assert_eq!(run_check(&c, at(90_000), Err("boom")), (String::new(), 1));
    assert_eq!(fs::read(&file).unwrap(), before);
}

#[test]
fn check_is_skipped_by_ci_or_the_opt_out() {
    let tmp = tempfile::tempdir().unwrap();
    for vars in [&[("CI", "1")][..], &[("RIVAL_NO_UPDATE_CHECK", "yes")]] {
        let c = cfg(tmp.path(), vars);
        assert_eq!(run_check(&c, at(0), Ok("2.0.0")), (String::new(), 0));
    }
    assert!(!tmp.path().join(".update-check").exists());
    let c = cfg(
        tmp.path(),
        &[("CI", "false"), ("RIVAL_NO_UPDATE_CHECK", "0")],
    );
    assert_eq!(run_check(&c, at(0), Ok("2.0.0")).1, 1);
}

#[test]
fn check_saves_an_empty_tag_and_prints_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let c = cfg(tmp.path(), &[]);
    assert_eq!(run_check(&c, at(0), Ok("")), (String::new(), 1));
    assert_eq!(
        load_cache(&tmp.path().join(".update-check"))
            .unwrap()
            .latest,
        ""
    );
}

#[test]
fn parse_release_reads_the_first_value_like_go_decoder() {
    assert_eq!(parse_release(br#"{"tag_name":"v1.2.3"}"#).unwrap(), "1.2.3");
    assert_eq!(parse_release(br#"{"tag_name":"1.2.3"}"#).unwrap(), "1.2.3");
    assert_eq!(parse_release(br#"{"tag_name":"vv1"}"#).unwrap(), "v1");
    assert_eq!(parse_release(br#"{"TAG_NAME":"vzzz"}"#).unwrap(), "zzz");
    assert_eq!(parse_release(br#"{"name":"x"}"#).unwrap(), "");
    assert_eq!(parse_release(br#"{"tag_name":null}"#).unwrap(), "");
    assert_eq!(parse_release(b"null").unwrap(), "");
    // Data after the first value is never read.
    assert_eq!(
        parse_release(br#"{"tag_name":"v2"} trailing {"#).unwrap(),
        "2"
    );
    assert_eq!(parse_release(b"").unwrap_err(), "EOF");
    assert_eq!(parse_release(b"  \n").unwrap_err(), "EOF");
    assert_eq!(
        parse_release(br#"{"tag_name":"v"#).unwrap_err(),
        "unexpected EOF"
    );
    assert_eq!(
        parse_release(b"[]").unwrap_err(),
        "json: cannot unmarshal array into Go value of type struct { TagName string \"json:\\\"tag_name\\\"\" }"
    );
    assert_eq!(
        parse_release(br#"{"tag_name":1}"#).unwrap_err(),
        "json: cannot unmarshal number into Go struct field .tag_name of type string"
    );
}

/// A reader that serves `data` once, then fails the test if read again.
struct OneShot<'a> {
    data: Option<&'a [u8]>,
}

impl Read for OneShot<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let data = self.data.take().expect("read past the first JSON value");
        buf[..data.len()].copy_from_slice(data);
        Ok(data.len())
    }
}

/// Go's decoder returns after the first value; it never waits for EOF or
/// trailing data. A body that stalls after the object must not block.
#[test]
fn parse_release_stops_at_the_end_of_the_first_value() {
    let body = OneShot {
        data: Some(br#"{"tag_name":"v3.0.0","assets":[{"x":1}]}"#),
    };
    assert_eq!(parse_release_from(body).unwrap(), "3.0.0");
}

#[test]
fn releases_url_defaults_to_github() {
    assert_eq!(
        releases_url_for("", true),
        "https://api.github.com/repos/1905/rival/releases/latest"
    );
    assert_eq!(
        releases_url_for("http://127.0.0.1:9", true),
        "http://127.0.0.1:9/repos/1905/rival/releases/latest"
    );
    assert_eq!(
        releases_url_for("http://127.0.0.1:9/", true),
        "http://127.0.0.1:9/repos/1905/rival/releases/latest"
    );
    // A build that does not honour the override always asks GitHub.
    assert_eq!(
        releases_url_for("http://127.0.0.1:9", false),
        "https://api.github.com/repos/1905/rival/releases/latest"
    );
}

/// The override is compiled in for debug builds only. Run this test with
/// `cargo test --release` to check the release side.
#[test]
fn release_builds_ignore_the_api_override() {
    assert_eq!(HONOURS_API_OVERRIDE, cfg!(debug_assertions));
    let tmp = tempfile::tempdir().unwrap();
    let c = cfg(tmp.path(), &[(API_OVERRIDE_VAR, "http://127.0.0.1:9")]);
    let want = if cfg!(debug_assertions) {
        "http://127.0.0.1:9/repos/1905/rival/releases/latest"
    } else {
        "https://api.github.com/repos/1905/rival/releases/latest"
    };
    assert_eq!(releases_url(&c), want);
}

#[test]
fn update_constants_match_go() {
    assert_eq!(CACHE_TTL, Duration::from_secs(86_400));
    assert_eq!(CACHE_TTL_DELTA.num_seconds(), 86_400);
    assert_eq!(HTTP_TIMEOUT, Duration::from_secs(2));
}
