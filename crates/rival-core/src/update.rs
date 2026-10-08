//! Release check: queries GitHub for the latest release, caches the answer
//! for a day, and prints a notice when a newer version exists.
//!
//! Go: `internal/update/check.go`. Versions compare as padded strings, not
//! semver: `normalize_version` zero-pads three dot-separated parts to three
//! runes, and anything else compares as written. So `9.9.9` sorts before
//! `dev`, while a tag such as `vzzz` sorts after it.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, FixedOffset, Local, TimeDelta};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use crate::config::Config;
use crate::paths::Paths;
use crate::{gostd, json};

#[cfg(test)]
mod tests;

/// Go `releasesURL`, split into the API base and the release path.
pub const GITHUB_API: &str = "https://api.github.com";
pub const RELEASES_PATH: &str = "/repos/1905/rival/releases/latest";
/// Go `cacheTTL`.
pub const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// Go `httpTimeout`: the whole request, like `http.Client.Timeout`.
pub const HTTP_TIMEOUT: Duration = Duration::from_secs(2);
/// The test-only API base override. Rust-port addition: only debug builds
/// read it, so a release build always asks GitHub.
pub const API_OVERRIDE_VAR: &str = "RIVAL_UPDATE_API";
/// Whether this build honours [`API_OVERRIDE_VAR`].
pub const HONOURS_API_OVERRIDE: bool = cfg!(debug_assertions);

/// Go `cache`: the last answer and when it was fetched. A cache without
/// `checked_at` is stale.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cache {
    #[serde(deserialize_with = "json::nullable")]
    pub latest: String,
    #[serde(skip_serializing_if = "Option::is_none", with = "json::opt_time")]
    pub checked_at: Option<DateTime<FixedOffset>>,
}

/// Go `FetchLatest` or a test stand-in: the latest tag without its `v`.
pub type Fetch<'a> = &'a dyn Fn() -> Result<String, String>;

/// Go `time.Now` or a test clock.
pub type Clock<'a> = &'a dyn Fn() -> DateTime<FixedOffset>;

/// Go `Check(currentVersion)`, with every input explicit. All errors are
/// ignored. `out` is Go's `os.Stderr`. The clock is read twice, as in Go:
/// for the cache age before the fetch, and for `checked_at` after it.
pub fn check(current: &str, cfg: &Config, now: Clock<'_>, fetch: Fetch<'_>, out: &mut dyn Write) {
    if skip_check(|key| cfg.getenv(key)) {
        return;
    }
    let cache_file = cache_file_path(cfg.paths());
    if let Ok(c) = load_cache(&cache_file)
        && let Some(checked_at) = c.checked_at
        && since(now(), checked_at) < CACHE_TTL_DELTA
    {
        print_if_newer(current, &c.latest, out);
        return;
    }
    let Ok(latest) = fetch() else {
        return;
    };
    let _ = save_cache(&cache_file, &latest, now());
    print_if_newer(current, &latest, out);
}

/// The production check: the real clock and GitHub. `out` is the process
/// stderr, except under the TUI, whose root buffers the notice until the
/// terminal is restored.
pub fn check_production(current: &str, cfg: &Config, out: &mut dyn Write) {
    let url = releases_url(cfg);
    check(
        current,
        cfg,
        &|| Local::now().fixed_offset(),
        &|| fetch_latest(&url),
        out,
    );
}

const CACHE_TTL_DELTA: TimeDelta = TimeDelta::seconds(24 * 60 * 60);

/// Go `time.Since(t)` for a parsed time (no monotonic reading), saturating.
fn since(now: DateTime<FixedOffset>, t: DateTime<FixedOffset>) -> TimeDelta {
    now.signed_duration_since(t)
}

/// Go `skipCheck`: `CI` or `RIVAL_NO_UPDATE_CHECK` set to anything but
/// empty, `0` or `false`.
pub fn skip_check<'a>(getenv: impl Fn(&str) -> &'a str) -> bool {
    ["CI", "RIVAL_NO_UPDATE_CHECK"]
        .iter()
        .any(|key| !matches!(getenv(key), "" | "0" | "false"))
}

/// Go `cacheFilePath`: `<home>/.rival/.update-check`. The port puts it in the
/// rival root, which is that path unless `RIVAL_HOME` moves the root.
pub fn cache_file_path(paths: &Paths) -> PathBuf {
    paths.root.join(".update-check")
}

/// Go `loadCache`: reads and decodes the cache file.
pub fn load_cache(path: &Path) -> Result<Cache, String> {
    let data = fs::read(path).map_err(|e| e.to_string())?;
    json::decode(&data).map_err(|e| e.to_string())
}

/// Go `saveCache`: `MkdirAll(dir, 0700)` (error ignored), then
/// `WriteFile(path, json, 0600)`.
pub fn save_cache(path: &Path, latest: &str, now: DateTime<FixedOffset>) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        let mut b = DirBuilder::new();
        b.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
        let _ = b.create(dir);
    }
    let data = serde_json::to_vec(&Cache {
        latest: latest.to_string(),
        checked_at: Some(now),
    })
    .unwrap_or_default();
    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    opts.open(path)?.write_all(&data)
}

/// The release endpoint. Debug builds let a non-empty
/// [`API_OVERRIDE_VAR`] replace the GitHub API base; release builds never
/// read it.
pub fn releases_url(cfg: &Config) -> String {
    releases_url_for(cfg.getenv(API_OVERRIDE_VAR), HONOURS_API_OVERRIDE)
}

/// Pure form of [`releases_url`].
pub fn releases_url_for(override_base: &str, honour_override: bool) -> String {
    let base = if honour_override && !override_base.is_empty() {
        override_base.trim_end_matches('/')
    } else {
        GITHUB_API
    };
    format!("{base}{RELEASES_PATH}")
}

/// Go `FetchLatest`: GET with a 2 s budget for the whole request.
pub fn fetch_latest(url: &str) -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(HTTP_TIMEOUT))
        .http_status_as_error(false)
        .build()
        .into();
    let mut resp = agent
        .get(url)
        .call()
        .map_err(|e| format!("Get {}: {e}", gostd::quote(url)))?;
    let status = resp.status().as_u16();
    if status != 200 {
        return Err(format!("status {status}"));
    }
    // Unlimited, like Go's body; the global timeout bounds it.
    parse_release_from(resp.body_mut().as_reader())
}

/// [`parse_release_from`] over bytes.
pub fn parse_release(body: &[u8]) -> Result<String, String> {
    parse_release_from(body)
}

/// Go `json.NewDecoder(body).Decode(&release)` plus `TrimPrefix(tag, "v")`.
/// Only the first JSON value is read: the decoder stops at its end and never
/// waits for EOF. Errors carry serde_json's text.
pub fn parse_release_from(body: impl Read) -> Result<String, String> {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Release {
        #[serde(deserialize_with = "json::nullable")]
        tag_name: String,
    }
    let first = serde_json::Deserializer::from_reader(BufReader::new(body))
        .into_iter::<Box<RawValue>>()
        .next();
    let raw = match first {
        None => return Err("EOF".to_string()),
        Some(Err(e)) if e.is_eof() => return Err("unexpected EOF".to_string()),
        Some(Err(e)) => return Err(e.to_string()),
        Some(Ok(raw)) => raw,
    };
    // A `null` document has no tag.
    let release: Option<json::Object<Release>> =
        serde_json::from_str(raw.get()).map_err(|e| e.to_string())?;
    let tag = release.unwrap_or_default().0.tag_name;
    Ok(tag.strip_prefix('v').unwrap_or(&tag).to_string())
}

/// Go `printIfNewer`.
pub fn print_if_newer(current: &str, latest: &str, out: &mut dyn Write) {
    if let Some(text) = notice(current, latest) {
        let _ = out.write_all(text.as_bytes());
    }
}

/// The notice `printIfNewer` would print, if any.
pub fn notice(current: &str, latest: &str) -> Option<String> {
    if latest.is_empty() {
        return None;
    }
    let cv = normalize_version(current);
    let lv = normalize_version(latest);
    if !lv.is_empty() && !cv.is_empty() && lv != cv && lv > cv {
        let current_display = current.strip_prefix('v').unwrap_or(current);
        return Some(format!(
            "\n  Update available: v{current_display} → v{latest} — run 'rival update'\n\n"
        ));
    }
    None
}

/// Go `normalizeVersion`: `fmt.Sprintf("%03s.%03s.%03s", ...)` for exactly
/// three parts. Go zero-pads strings on the left, counting runes.
pub fn normalize_version(v: &str) -> String {
    let v = v.strip_prefix('v').unwrap_or(v);
    let parts: Vec<&str> = v.split('.').collect();
    if parts.len() != 3 {
        return v.to_string();
    }
    let pad = |s: &str| {
        let n = s.chars().count();
        format!("{}{s}", "0".repeat(3usize.saturating_sub(n)))
    };
    format!("{}.{}.{}", pad(parts[0]), pad(parts[1]), pad(parts[2]))
}
