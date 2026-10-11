//! The model check: one short live call to each model, through the same
//! adapter path a review takes ([`ModelSpec::run`]). `rival config check`,
//! the TUI config window and the app all use it.
//!
//! Each model goes through four steps, and the first failure ends its row:
//! static (the runtime binary, the proxy URL and key), the shared proxy model
//! list, the model's own preflight, then the live call. The live call sends
//! [`CHECK_PROMPT`] at low effort with review (read-only) permissions, under
//! [`CHECK_TIMEOUT`], with no queue slot and an ephemeral session that is
//! never saved. Its log is `<RIVAL_HOME>/check/<name>-<pid>-<id>.log`, one
//! per run, so concurrent checks never share a log; each check keeps the
//! newest [`KEEP_LOGS`] of a model. At most [`MAX_LIVE`] live calls run at
//! once.
//!
//! The engine runs the steps through a [`Runner`]; production uses
//! [`SpecRunner`], tests a fake.

#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use rival_core::cancel::{Context, ContextError};
use rival_core::config::{self, Config, ProxyProvider, Route, SecurityModel};
use rival_core::duration;
use rival_core::executor::{self, OpencodeRunOpts};
use rival_core::leakguard;
use rival_core::logging;
use rival_core::proxy::{self, ModelsError};
use rival_core::result;
use rival_core::session::Session;
use serde_json::{Value as Json, json};

use crate::model_specs::{
    ModelSpec, RunCall, claude_spec, codex_spec, fable_spec, grok_spec, k3_spec, session_mode,
    sol_spec,
};

/// What each model is asked.
pub const CHECK_PROMPT: &str = "Reply with exactly: ok";
/// The effort of the live call; K3 resolves it to max, grok clamps it.
pub const CHECK_EFFORT: &str = "low";
/// The budget of one live call.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(90);
/// The budget of one model's preflight (`codex login status`,
/// `docker info`).
pub const PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(30);
/// The most live calls at once.
pub const MAX_LIVE: usize = 3;
/// The check logs kept per model, the new one included.
pub const KEEP_LOGS: usize = 5;
/// The characters of the reply a row keeps.
pub const REPLY_CHARS: usize = 40;

/// The row error of a model whose runtime binary is missing.
pub const NOT_INSTALLED: &str = "not installed";

/// The `-m` name of the security reviewer Grok (OpenCode on OpenRouter).
/// `grok` is the grok CLI.
pub const SECURITY_GROK_NAME: &str = config::GROK_OPENROUTER_LABEL;

/// One model the check covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    /// The `-m` name and the row name: codex, sol, claude, fable, k3, grok
    /// or grok-4.6-openrouter.
    pub name: &'static str,
    /// The session `cli`: codex, claude, opencode or grok.
    pub runtime: &'static str,
    /// The concrete model id.
    pub model: &'static str,
}

impl Target {
    /// The proxy provider of the runtime; `None` for runtimes that never go
    /// through the proxy.
    pub fn provider(&self) -> Option<ProxyProvider> {
        match self.runtime {
            "claude" => Some(ProxyProvider::Claude),
            "codex" => Some(ProxyProvider::Codex),
            _ => None,
        }
    }

    /// The binaries that run the model; any one of them is enough (Claude
    /// runs natively or in Docker).
    pub fn binaries(&self) -> &'static [&'static str] {
        match self.runtime {
            "claude" => &["claude", "docker"],
            "codex" => &["codex"],
            "opencode" => &["opencode"],
            _ => &["grok"],
        }
    }
}

const fn target(name: &'static str, runtime: &'static str, model: &'static str) -> Target {
    Target {
        name,
        runtime,
        model,
    }
}

pub const CODEX: Target = target(config::CODEX_LABEL, "codex", config::CODEX_MODEL);
pub const SOL: Target = target(config::SOL_LABEL, "codex", config::SOL_MODEL);
pub const CLAUDE: Target = target(config::CLAUDE_LABEL, "claude", config::CLAUDE_MODEL);
pub const FABLE: Target = target(config::FABLE_LABEL, "claude", config::FABLE_MODEL);
pub const K3: Target = target(config::K3_COMMAND_NAME, "opencode", config::KIMI_MODEL);
pub const GROK: Target = target(config::GROK_LABEL, config::GROK_LABEL, config::GROK_MODEL);
pub const SECURITY_GROK: Target = target(
    SECURITY_GROK_NAME,
    "opencode",
    config::GROK_OPENROUTER_MODEL,
);

/// Every model, in row order.
pub const ALL: [Target; 7] = [CODEX, SOL, CLAUDE, FABLE, K3, GROK, SECURITY_GROK];

/// The accepted `-m` words, for the error text.
pub const NAMES_HELP: &str =
    "codex, sol, claude (or opus), fable, k3, grok, grok-4.6-openrouter, all";

/// The models to check. No names: codex, sol, claude, fable and the
/// configured security reviewer. `all`: every model. `opus` is `claude`;
/// a model named twice is checked once. Rows follow [`ALL`] order.
pub fn targets(cfg: &Config, names: &[String]) -> Result<Vec<Target>, String> {
    let names: Vec<String> = names
        .iter()
        .map(|n| n.trim().to_lowercase())
        .filter(|n| !n.is_empty())
        .collect();
    let mut chosen: HashSet<&'static str> = HashSet::new();
    if names.is_empty() {
        chosen.extend([CODEX.name, SOL.name, CLAUDE.name, FABLE.name]);
        chosen.insert(security_target(cfg).name);
    }
    for name in &names {
        if name == "all" {
            chosen.extend(ALL.iter().map(|t| t.name));
            continue;
        }
        let word = if name == config::OPUS_ALIAS {
            config::CLAUDE_LABEL
        } else {
            name.as_str()
        };
        let Some(t) = ALL.iter().find(|t| t.name == word) else {
            return Err(format!(
                "unknown model {name:?} for config check; use one of: {NAMES_HELP}"
            ));
        };
        chosen.insert(t.name);
    }
    Ok(ALL
        .iter()
        .filter(|t| chosen.contains(t.name))
        .copied()
        .collect())
}

/// The configured security reviewer: K3, or Grok on OpenRouter. An invalid
/// value is caught when the config loads; K3 is the fallback.
fn security_target(cfg: &Config) -> Target {
    match cfg.resolve_security_model() {
        Ok(m) if m.name == config::SECURITY_REVIEWER_GROK => SECURITY_GROK,
        _ => K3,
    }
}

/// One checked model.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckRow {
    pub name: String,
    /// The session `cli`: codex, claude, opencode or grok.
    pub runtime: String,
    /// `proxy` or `direct`.
    pub route: String,
    /// The model id on the wire: with the prefix on the proxy route.
    pub wire_model: String,
    pub ok: bool,
    /// The live call's latency; 0 when no call ran.
    pub ms: u64,
    /// The first [`REPLY_CHARS`] characters of the reply.
    pub reply: String,
    /// The first error line; "" on a pass.
    pub error: String,
    /// What to do about the error: the 429 hint or the auth hint.
    pub hint: String,
    /// The account is at its limit (a 429): the route works, the account
    /// is empty.
    pub limit: bool,
    /// A pass whose reply is not `ok`: the model answered, shown in yellow.
    pub unexpected: bool,
    /// Whether the live call ran.
    pub called: bool,
}

impl CheckRow {
    /// The `--json` line object.
    pub fn to_json(&self) -> Json {
        json!({
            "name": self.name,
            "runtime": self.runtime,
            "route": self.route,
            "wire_model": self.wire_model,
            "ok": self.ok,
            "ms": self.ms,
            "reply": self.reply,
            "error": self.error,
            "hint": self.hint,
            "limit": self.limit,
            "unexpected": self.unexpected,
        })
    }

    fn fail(mut self, error: &str) -> CheckRow {
        self.ok = false;
        self.error = scrubbed(first_line(error));
        self
    }
}

/// The proxy status line above the rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyLine {
    /// No route is enabled, or `RIVAL_PROXY=off`.
    Off,
    /// `/v1/models` answered 200 with the key.
    Up {
        url: String,
        models: usize,
        /// The last 4 characters of the key; "" for a short key.
        key_tail: String,
    },
    /// The route is on but unusable: no URL or key, or the call failed.
    Down { url: String, error: String },
}

impl ProxyLine {
    pub fn text(&self) -> String {
        match self {
            ProxyLine::Off => "proxy  off".to_string(),
            ProxyLine::Up {
                url,
                models,
                key_tail,
            } => {
                let key = if key_tail.is_empty() {
                    "key set".to_string()
                } else {
                    format!("key …{key_tail}")
                };
                format!("proxy  {url}  ✓ 200  {models} models  {key}")
            }
            ProxyLine::Down { url, error } if url.is_empty() => format!("proxy  ✗ {error}"),
            ProxyLine::Down { url, error } => format!("proxy  {url}  ✗ {error}"),
        }
    }

    pub fn to_json(&self) -> Json {
        match self {
            ProxyLine::Off => json!({"state": "off"}),
            ProxyLine::Up {
                url,
                models,
                key_tail,
            } => json!({"state": "up", "url": url, "models": models, "key_tail": key_tail}),
            ProxyLine::Down { url, error } => json!({"state": "down", "url": url, "error": error}),
        }
    }
}

/// A finished check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub proxy: ProxyLine,
    /// In target order.
    pub rows: Vec<CheckRow>,
}

impl Report {
    pub fn passed(&self) -> usize {
        self.rows.iter().filter(|r| r.ok).count()
    }

    pub fn all_ok(&self) -> bool {
        self.passed() == self.rows.len()
    }

    /// The `--json` summary line object, with the proxy line.
    pub fn summary_json(&self) -> Json {
        json!({
            "summary": {"ok": self.passed(), "total": self.rows.len()},
            "proxy": self.proxy.to_json(),
        })
    }

    /// The text output: the proxy line, the rows, then `N of M ok`.
    pub fn text(&self) -> String {
        let name_w = width(self.rows.iter().map(|r| r.name.as_str()));
        let route_w = width(self.rows.iter().map(|r| r.route.as_str()));
        let wire_w = width(self.rows.iter().map(|r| r.wire_model.as_str()));
        let times: Vec<String> = self.rows.iter().map(row_time).collect();
        let time_w = width(times.iter().map(String::as_str));
        let mut out = format!("{}\n\n", self.proxy.text());
        for (row, time) in self.rows.iter().zip(&times) {
            let mark = if row.ok { "✓" } else { "✗" };
            let message = if row.ok && row.unexpected {
                format!("{}  (not ok)", row.reply)
            } else if row.ok {
                row.reply.clone()
            } else if row.limit {
                format!("limit  {}", row.error)
            } else {
                row.error.clone()
            };
            let line = format!(
                "  {mark} {:<name_w$}  {:<route_w$}  {:<wire_w$}  {}  {message}",
                row.name,
                row.route,
                row.wire_model,
                pad(time, time_w),
            );
            out.push_str(line.trim_end());
            out.push('\n');
            if !row.ok && !row.hint.is_empty() {
                out.push_str(&format!("      {}\n", row.hint));
            }
        }
        out.push_str(&format!("\n{} of {} ok\n", self.passed(), self.rows.len()));
        out
    }
}

/// The time column: `2.1s`, or `—` when no call ran.
fn row_time(row: &CheckRow) -> String {
    if row.called {
        format!("{:.1}s", row.ms as f64 / 1000.0)
    } else {
        "—".to_string()
    }
}

fn width<'a>(items: impl Iterator<Item = &'a str>) -> usize {
    items.map(|s| s.chars().count()).max().unwrap_or(0)
}

/// Left-aligned in `w` characters (`format!` pads by chars too, but `—` is
/// one char of three bytes, so count explicitly).
fn pad(s: &str, w: usize) -> String {
    let n = s.chars().count();
    format!("{s}{}", " ".repeat(w.saturating_sub(n)))
}

/// The steps of one check. Production is [`SpecRunner`]; tests fake it.
pub trait Runner: Sync {
    /// Whether one of `t`'s binaries is on `PATH`.
    fn installed(&self, cfg: &Config, t: &Target) -> bool;
    /// The proxy's model list (`GET /v1/models`).
    fn models(&self, route: &Route) -> Result<Vec<String>, ModelsError>;
    /// The model's own preflight. It ends (killing any command it runs)
    /// once `ctx` is done.
    fn preflight(&self, ctx: &Context, cfg: &Config, t: &Target) -> Result<(), String>;
    /// The live call: [`CHECK_PROMPT`] with review permissions, its output
    /// in `log`, in `workdir`. `Ok` is the exit code.
    fn call(
        &self,
        ctx: &Context,
        cfg: &Config,
        t: &Target,
        log: &Path,
        workdir: &str,
    ) -> Result<i64, String>;
    /// The provider's hint for a failed call (its `auth_hint`); "" for none.
    fn hint(&self, cfg: &Config, t: &Target, log: &Path) -> String;
}

/// Checks `targets` with the real adapters. `on_row` gets each row as it
/// finishes (from a worker thread); the report has them in target order.
/// Cancelling `ctx` ends the live calls.
pub fn run_check(
    ctx: &Context,
    cfg: &Config,
    targets: &[Target],
    on_row: &(dyn Fn(&CheckRow) + Sync),
) -> Report {
    run_check_with(ctx, cfg, targets, &SpecRunner, CHECK_TIMEOUT, on_row)
}

/// [`run_check`] with the runner and the call budget injected.
pub fn run_check_with(
    ctx: &Context,
    cfg: &Config,
    targets: &[Target],
    runner: &dyn Runner,
    timeout: Duration,
    on_row: &(dyn Fn(&CheckRow) + Sync),
) -> Report {
    // The adapters log at info (auth mode, transport); only warn and up
    // may land between the rows on stderr.
    let _quiet = logging::min_level(logging::Level::Warn);
    let lists = ModelLists::default();
    let proxy = proxy_line(cfg, runner, &lists);
    let dir = cfg.paths().check_dir();
    let dir_err = make_private_dir(&dir).err();
    let engine = Engine {
        ctx,
        cfg,
        runner,
        timeout,
        lists: &lists,
        dir: &dir,
        dir_err: dir_err.as_deref(),
        live: Slots::new(MAX_LIVE),
    };
    let rows: Vec<CheckRow> = std::thread::scope(|s| {
        let handles: Vec<_> = targets
            .iter()
            .map(|t| {
                let engine = &engine;
                s.spawn(move || {
                    let row = engine.check(t);
                    on_row(&row);
                    row
                })
            })
            .collect();
        handles
            .into_iter()
            .zip(targets)
            .map(|(h, t)| {
                h.join()
                    .unwrap_or_else(|_| base_row(cfg, t).fail("the check panicked"))
            })
            .collect()
    });
    Report { proxy, rows }
}

/// The proxy line: the first enabled route (Claude, then Codex); both share
/// the URL and the key.
fn proxy_line(cfg: &Config, runner: &dyn Runner, lists: &ModelLists) -> ProxyLine {
    let url = cfg.proxy_url().unwrap_or_default();
    for provider in [ProxyProvider::Claude, ProxyProvider::Codex] {
        match cfg.proxy_route(provider) {
            Ok(None) => continue,
            Err(e) => {
                return ProxyLine::Down {
                    url,
                    error: scrubbed(&e.to_string()),
                };
            }
            Ok(Some(route)) => {
                return match lists.get(runner, &route) {
                    Ok(ids) => ProxyLine::Up {
                        url: route.url.clone(),
                        models: ids.len(),
                        key_tail: key_tail(route.key()),
                    },
                    Err(e) => ProxyLine::Down {
                        url: route.url.clone(),
                        error: scrubbed(&e.to_string()),
                    },
                };
            }
        }
    }
    ProxyLine::Off
}

/// The last 4 characters of a key of 8 or more; "" otherwise (as
/// `ProxyKey::masked`).
fn key_tail(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() < 8 {
        return String::new();
    }
    chars[chars.len() - 4..].iter().collect()
}

/// The model lists of this check, one call per URL and key; an error is
/// kept too, so every row of a down proxy shows the same error without a
/// second 5 s wait.
#[derive(Default)]
struct ModelLists {
    seen: Mutex<Vec<ListedModels>>,
}

/// `(url, key, list)`.
type ListedModels = (String, String, Result<Vec<String>, ModelsError>);

impl ModelLists {
    fn get(&self, runner: &dyn Runner, route: &Route) -> Result<Vec<String>, ModelsError> {
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, _, r)) = seen
            .iter()
            .find(|(u, k, _)| *u == route.url && k == route.key())
        {
            return r.clone();
        }
        let r = runner.models(route);
        seen.push((route.url.clone(), route.key().to_string(), r.clone()));
        r
    }
}

/// A counting semaphore for the live calls.
struct Slots {
    free: Mutex<usize>,
    cond: Condvar,
}

impl Slots {
    fn new(n: usize) -> Slots {
        Slots {
            free: Mutex::new(n),
            cond: Condvar::new(),
        }
    }

    fn acquire(&self) -> SlotGuard<'_> {
        let mut free = self.free.lock().unwrap_or_else(|e| e.into_inner());
        while *free == 0 {
            free = self.cond.wait(free).unwrap_or_else(|e| e.into_inner());
        }
        *free -= 1;
        SlotGuard(self)
    }
}

struct SlotGuard<'a>(&'a Slots);

impl Drop for SlotGuard<'_> {
    fn drop(&mut self) {
        *self.0.free.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        self.0.cond.notify_one();
    }
}

struct Engine<'a> {
    ctx: &'a Context,
    cfg: &'a Config,
    runner: &'a dyn Runner,
    timeout: Duration,
    lists: &'a ModelLists,
    dir: &'a Path,
    dir_err: Option<&'a str>,
    live: Slots,
}

/// A row with the name, the runtime, the route and the wire id filled in.
/// An enabled route that fails (no URL or key) still shows as `proxy`.
fn base_row(cfg: &Config, t: &Target) -> CheckRow {
    let route = t.provider().map(|p| cfg.proxy_route(p));
    let (route_name, wire_model) = match &route {
        Some(Ok(Some(r))) => ("proxy", proxy::wire_id(&r.prefix, t.model)),
        Some(Err(_)) => ("proxy", t.model.to_string()),
        _ => ("direct", t.model.to_string()),
    };
    CheckRow {
        name: t.name.to_string(),
        runtime: t.runtime.to_string(),
        route: route_name.to_string(),
        wire_model,
        ..CheckRow::default()
    }
}

impl Engine<'_> {
    fn check(&self, t: &Target) -> CheckRow {
        let row = base_row(self.cfg, t);
        // 1. Static: the route's URL and key, then the binary.
        let route = match t.provider().map(|p| self.cfg.proxy_route(p)) {
            Some(Err(e)) => return row.fail(&e.to_string()),
            Some(Ok(route)) => route,
            None => None,
        };
        if !self.runner.installed(self.cfg, t) {
            return row.fail(NOT_INSTALLED);
        }
        // 2. Proxy: the shared list serves the wire id.
        let mut served: Vec<String> = Vec::new();
        if let (Some(route), Some(provider)) = (&route, t.provider()) {
            match self.lists.get(self.runner, route) {
                Ok(ids) => served = ids,
                Err(e) => return row.fail(&e.to_string()),
            }
            if let Err(e) = proxy::check_served(&served, route, provider, t.model) {
                return row.fail(&e);
            }
        }
        // 3. The model's own preflight, under the check's context and
        // budget: `docker info` on a stuck daemon must not hang the check.
        if let Err(e) = self.preflight(t) {
            return row.fail(&e);
        }
        if let Some(e) = self.dir_err {
            return row.fail(e);
        }
        // 4. Live.
        self.live(t, row, route.as_ref(), &served)
    }

    /// [`Runner::preflight`] under [`PREFLIGHT_TIMEOUT`] (or the call
    /// budget when shorter); a cancelled check or the timeout is the error.
    fn preflight(&self, t: &Target) -> Result<(), String> {
        let budget = self.timeout.min(PREFLIGHT_TIMEOUT);
        let (ctx, cancel) = self.ctx.with_timeout(budget);
        let result = self.runner.preflight(&ctx, self.cfg, t);
        let done = ctx.err();
        cancel.cancel();
        let Err(e) = result else { return Ok(()) };
        if let Some(parent) = self.ctx.err() {
            return Err(parent.to_string());
        }
        if done == Some(ContextError::DeadlineExceeded) {
            let budget = i64::try_from(budget.as_nanos()).unwrap_or(i64::MAX);
            return Err(format!(
                "preflight: no answer in {} (timeout)",
                duration::format(budget)
            ));
        }
        Err(e)
    }

    fn live(
        &self,
        t: &Target,
        mut row: CheckRow,
        route: Option<&Route>,
        served: &[String],
    ) -> CheckRow {
        // A new log for each run: the adapters append, and another check
        // may be running the same model.
        prune_logs(self.dir, t.name, KEEP_LOGS - 1);
        let log = log_path(self.dir, t.name);
        let workdir = self.dir.to_string_lossy().into_owned();
        let _slot = self.live.acquire();
        if let Some(e) = self.ctx.err() {
            return row.fail(&e.to_string());
        }
        let (ctx, cancel) = self.ctx.with_timeout(self.timeout);
        let start = Instant::now();
        let result = self.runner.call(&ctx, self.cfg, t, &log, &workdir);
        row.ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        row.called = true;
        let timed_out = ctx.err() == Some(ContextError::DeadlineExceeded);
        cancel.cancel();
        let output = fs::read(&log)
            .map(|d| String::from_utf8_lossy(&d).into_owned())
            .unwrap_or_default();
        if timed_out {
            let budget = i64::try_from(self.timeout.as_nanos()).unwrap_or(i64::MAX);
            return row.fail(&format!(
                "no reply in {} (timeout)",
                duration::format(budget)
            ));
        }
        let exit_code = match result {
            Ok(code) => code,
            Err(e) => {
                row.hint = self.hint(t, route, served, &output, &log, &mut row.limit);
                return row.fail(&e);
            }
        };
        let reply = reply_text(&output);
        if exit_code == 0 && !reply.is_empty() {
            row.ok = true;
            row.unexpected = !is_ok_reply(&reply);
            row.reply = scrubbed(&truncate_chars(&reply, REPLY_CHARS));
            return row;
        }
        row.hint = self.hint(t, route, served, &output, &log, &mut row.limit);
        let error = if exit_code == 0 {
            "empty reply".to_string()
        } else {
            match last_line(&output) {
                "" => format!("exited with code {exit_code}"),
                line => format!("exited with code {exit_code}: {line}"),
            }
        };
        row.fail(&error)
    }

    /// The 429 hint (and `limit`), else the provider's auth hint.
    fn hint(
        &self,
        t: &Target,
        route: Option<&Route>,
        served: &[String],
        output: &str,
        log: &Path,
        limit: &mut bool,
    ) -> String {
        if let (Some(route), Some(provider)) = (route, t.provider())
            && let Some(hint) = proxy::classify_limit(output, route, provider, t.model, served)
        {
            *limit = true;
            return scrubbed(&hint);
        }
        if route.is_none() && executor::is_quota_exhausted(output) {
            *limit = true;
        }
        let hint = self.runner.hint(self.cfg, t, log);
        scrubbed(hint.strip_prefix("rival: ").unwrap_or(&hint))
    }
}

/// `<dir>/<name>-<pid>-<id>.log`: a new path for each run.
pub fn log_path(dir: &Path, name: &str) -> PathBuf {
    let id = uuid::Uuid::new_v4().simple();
    dir.join(format!("{name}-{}-{id}.log", std::process::id()))
}

/// The model name of a [`log_path`] file name; `None` for other files.
fn log_owner(file: &str) -> Option<&str> {
    let mut parts = file.strip_suffix(".log")?.rsplitn(3, '-');
    let id = parts.next()?;
    let pid = parts.next()?;
    let name = parts.next()?;
    let hex = id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit());
    let digits = !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit());
    (hex && digits).then_some(name)
}

/// Removes all but the newest `keep` run logs of `name` in `dir`. Errors
/// are ignored: a log that stays is harmless.
fn prune_logs(dir: &Path, name: &str, keep: usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut logs: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|e| e.file_name().to_str().and_then(log_owner) == Some(name))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    logs.sort_by(|a, b| b.cmp(a));
    for (_, path) in logs.into_iter().skip(keep) {
        let _ = fs::remove_file(path);
    }
}

fn make_private_dir(dir: &Path) -> Result<(), String> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder
        .create(dir)
        .map_err(|e| format!("create {}: {e}", dir.display()))
}

/// The reply in a provider log: the final answer (no Codex banner, footer
/// or hook lines), its lines joined by spaces.
pub fn reply_text(log: &str) -> String {
    result::final_answer(log)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// `ok` in any case, with trailing punctuation.
pub fn is_ok_reply(reply: &str) -> bool {
    reply
        .trim()
        .trim_end_matches(|c: char| c.is_ascii_punctuation())
        .trim()
        .eq_ignore_ascii_case("ok")
}

fn truncate_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn first_line(s: &str) -> &str {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
}

fn last_line(s: &str) -> &str {
    s.lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
}

fn scrubbed(s: &str) -> String {
    leakguard::scrub(s).into_owned()
}

/// The real adapters, through each model's [`ModelSpec`].
pub struct SpecRunner;

/// The spec of a target. Specs are not `Sync`, so each thread builds its
/// own.
pub fn spec_for(t: &Target) -> ModelSpec {
    match t.name {
        "codex" => codex_spec(),
        "sol" => sol_spec(),
        "claude" => claude_spec(),
        "fable" => fable_spec(),
        "k3" => k3_spec(),
        "grok" => grok_spec(),
        _ => security_grok_spec(),
    }
}

/// The security reviewer Grok (OpenCode on OpenRouter) as a spec, so the
/// check calls it through [`ModelSpec::run`] like the others.
fn security_grok_spec() -> ModelSpec {
    let mut spec = k3_spec();
    spec.command_name = SECURITY_GROK_NAME;
    spec.model = config::GROK_OPENROUTER_MODEL;
    spec.preflight = Box::new(|cfg, workdir| {
        executor::opencode_preflight_entry(cfg, &security_grok_entry(), workdir)
    });
    spec.run = Box::new(|c| {
        executor::run_opencode_entry(
            c.ctx,
            c.cfg,
            c.sess,
            c.prompt,
            c.effort,
            c.workdir,
            &security_grok_entry(),
            &OpencodeRunOpts::default(),
            c.log,
            c.out,
        )
    });
    spec
}

fn security_grok_entry() -> SecurityModel {
    config::open_code_entry_for(config::GROK_OPENROUTER_MODEL)
        .expect("the security registry has the OpenRouter Grok")
}

/// Where a `.env` credential is looked up: the caller's directory.
fn cred_workdir(cfg: &Config) -> String {
    cfg.cwd()
        .map_or_else(|| ".".to_string(), |p| p.to_string_lossy().into_owned())
}

impl Runner for SpecRunner {
    fn installed(&self, cfg: &Config, t: &Target) -> bool {
        t.binaries()
            .iter()
            .any(|b| executor::oscmd::look_path(cfg, b).is_ok())
    }

    fn models(&self, route: &Route) -> Result<Vec<String>, ModelsError> {
        proxy::models(route)
    }

    fn preflight(&self, ctx: &Context, cfg: &Config, t: &Target) -> Result<(), String> {
        executor::oscmd::with_context(ctx, || (spec_for(t).preflight)(cfg, &cred_workdir(cfg)))
            .map_err(|e| format!("{e:#}"))
    }

    fn call(
        &self,
        ctx: &Context,
        cfg: &Config,
        t: &Target,
        log: &Path,
        workdir: &str,
    ) -> Result<i64, String> {
        let spec = spec_for(t);
        let effort = if t.name == SECURITY_GROK_NAME {
            security_grok_entry().variant.to_string()
        } else {
            spec.resolve_effort(cfg, CHECK_EFFORT)?
        };
        let log = log.to_string_lossy().into_owned();
        let mut sess = check_session(&spec, &effort, workdir, &log);
        if spec.cli == "claude" {
            sess.account = cfg.claude_subscription().to_string();
        }
        (spec.run)(RunCall {
            ctx,
            cfg,
            sess: &mut sess,
            prompt: CHECK_PROMPT,
            effort: &effort,
            workdir,
            cred_workdir: &cred_workdir(cfg),
            review: true,
            log: Some(&log),
            out: None,
        })
        .map(|r| r.exit_code)
        .map_err(|e| format!("{e:#}"))
    }

    fn hint(&self, cfg: &Config, t: &Target, log: &Path) -> String {
        spec_for(t).auth_hint(cfg, &log.to_string_lossy())
    }
}

/// The session of a live call: running, ephemeral (never saved), its log
/// the check log.
pub fn check_session(spec: &ModelSpec, effort: &str, workdir: &str, log: &str) -> Session {
    Session {
        id: uuid::Uuid::new_v4().to_string(),
        cli: spec.cli.to_string(),
        mode: session_mode(true).to_string(),
        model: spec.model.to_string(),
        effort: effort.to_string(),
        prompt: CHECK_PROMPT.to_string(),
        status: "running".to_string(),
        work_dir: workdir.to_string(),
        log_file: log.to_string(),
        ephemeral: true,
        ..Session::default()
    }
}
