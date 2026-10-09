//! The check engine with a fake runner, plus one run of the real adapter
//! path against a fake `codex` script. No test runs a real reviewer CLI or
//! contacts a real proxy: the fake runner answers `/v1/models`, and the
//! real-path test has only its own script directory on `PATH`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::testutil::Fixture;

const SECRET: &str = "sk-test-not-real-a91f";

const PROXY_YAML: &str = "proxy:
  url: http://127.0.0.1:9
  claude:
    enabled: true
    model_prefix: emcd_
  codex:
    enabled: true
";

fn proxy_fixture() -> Fixture {
    Fixture::with_config_and_env(PROXY_YAML, &[("RIVAL_PROXY_KEY", SECRET)])
}

fn served() -> Vec<String> {
    [
        "emcd_/claude-opus-5-5",
        "emcd_/claude-fable-5-1",
        "emcd2_/claude-opus-5-5",
        "gpt-6-astra",
        "gpt-6.1-sol",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// What a fake live call does.
#[derive(Clone)]
enum Reply {
    /// Writes the text to the log and exits with the code.
    Out(i64, &'static str),
    /// The adapter returns this error.
    Err(&'static str),
    /// Waits until the context is done.
    Hang,
}

struct Fake {
    missing: Vec<&'static str>,
    models: Result<Vec<String>, ModelsError>,
    preflight: HashMap<&'static str, &'static str>,
    /// Preflights that wait until their context is done.
    preflight_hang: Vec<&'static str>,
    replies: HashMap<&'static str, Reply>,
    hints: HashMap<&'static str, &'static str>,
    delay: Duration,
    models_calls: AtomicUsize,
    /// `(name, log, workdir)` of each live call.
    calls: Mutex<Vec<(String, PathBuf, String)>>,
    active: AtomicUsize,
    max_active: AtomicUsize,
}

impl Fake {
    fn new() -> Fake {
        Fake {
            missing: Vec::new(),
            models: Ok(served()),
            preflight: HashMap::new(),
            preflight_hang: Vec::new(),
            replies: HashMap::new(),
            hints: HashMap::new(),
            delay: Duration::ZERO,
            models_calls: AtomicUsize::new(0),
            calls: Mutex::new(Vec::new()),
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
        }
    }

    fn called(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|c| c.0.clone())
            .collect();
        names.sort();
        names
    }
}

impl Runner for Fake {
    fn installed(&self, _: &Config, t: &Target) -> bool {
        !self.missing.contains(&t.name)
    }

    fn models(&self, _: &Route) -> Result<Vec<String>, ModelsError> {
        self.models_calls.fetch_add(1, Ordering::SeqCst);
        self.models.clone()
    }

    fn preflight(&self, ctx: &Context, _: &Config, t: &Target) -> Result<(), String> {
        if self.preflight_hang.contains(&t.name) {
            ctx.wait_timeout(Duration::from_secs(30));
            return Err("the daemon is not running".to_string());
        }
        match self.preflight.get(t.name) {
            Some(e) => Err(e.to_string()),
            None => Ok(()),
        }
    }

    fn call(
        &self,
        ctx: &Context,
        _: &Config,
        t: &Target,
        log: &Path,
        workdir: &str,
    ) -> Result<i64, String> {
        self.calls.lock().unwrap().push((
            t.name.to_string(),
            log.to_path_buf(),
            workdir.to_string(),
        ));
        let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(now, Ordering::SeqCst);
        std::thread::sleep(self.delay);
        let reply = self
            .replies
            .get(t.name)
            .cloned()
            .unwrap_or(Reply::Out(0, "ok\n"));
        let result = match reply {
            Reply::Out(code, text) => {
                fs::write(log, text).unwrap();
                Ok(code)
            }
            Reply::Err(e) => Err(e.to_string()),
            Reply::Hang => {
                ctx.wait_timeout(Duration::from_secs(30));
                Err(ctx.err().map_or("hung".to_string(), |e| e.to_string()))
            }
        };
        self.active.fetch_sub(1, Ordering::SeqCst);
        result
    }

    fn hint(&self, _: &Config, t: &Target, _: &Path) -> String {
        self.hints.get(t.name).map_or("", |h| h).to_string()
    }
}

fn check(fix: &Fixture, names: &[&str], fake: &Fake) -> Report {
    check_timeout(fix, names, fake, CHECK_TIMEOUT)
}

fn check_timeout(fix: &Fixture, names: &[&str], fake: &Fake, timeout: Duration) -> Report {
    let names: Vec<String> = names.iter().map(|s| s.to_string()).collect();
    let targets = targets(&fix.cfg, &names).unwrap();
    run_check_with(
        &Context::background(),
        &fix.cfg,
        &targets,
        fake,
        timeout,
        &|_| {},
    )
}

fn row<'a>(report: &'a Report, name: &str) -> &'a CheckRow {
    report
        .rows
        .iter()
        .find(|r| r.name == name)
        .unwrap_or_else(|| panic!("no {name} row in {report:?}"))
}

// ---- targets ----

fn names(fix: &Fixture, list: &[&str]) -> Result<Vec<&'static str>, String> {
    let list: Vec<String> = list.iter().map(|s| s.to_string()).collect();
    targets(&fix.cfg, &list).map(|ts| ts.iter().map(|t| t.name).collect())
}

#[test]
fn targets_default_all_list_and_dedupe() {
    let fix = Fixture::new();
    assert_eq!(
        names(&fix, &[]).unwrap(),
        ["codex", "sol", "claude", "fable", "k3"]
    );
    assert_eq!(
        names(&fix, &["all"]).unwrap(),
        [
            "codex",
            "sol",
            "claude",
            "fable",
            "k3",
            "grok",
            "grok-4.6-openrouter"
        ]
    );
    // opus is claude; rows keep the model order, whatever the list order.
    assert_eq!(
        names(&fix, &["fable", "opus", " Claude ", "codex"]).unwrap(),
        ["codex", "claude", "fable"]
    );
    assert_eq!(names(&fix, &["grok", "k3"]).unwrap(), ["k3", "grok"]);
    let err = names(&fix, &["gpt"]).unwrap_err();
    assert_eq!(
        err,
        format!("unknown model \"gpt\" for config check; use one of: {NAMES_HELP}")
    );
}

#[test]
fn targets_default_adds_the_configured_security_reviewer() {
    let fix = Fixture::with_config_yaml("security:\n  reviewer: grok\n");
    assert_eq!(
        names(&fix, &[]).unwrap(),
        ["codex", "sol", "claude", "fable", "grok-4.6-openrouter"]
    );
}

// ---- rows ----

#[test]
fn all_pass_direct() {
    let fix = Fixture::new();
    let fake = Fake::new();
    let report = check(&fix, &[], &fake);
    assert!(report.all_ok(), "{report:?}");
    assert_eq!(report.proxy, ProxyLine::Off);
    assert_eq!(
        fake.called(),
        ["claude", "codex", "fable", "k3", "sol"],
        "one live call each"
    );
    let codex = row(&report, "codex");
    assert_eq!(
        (
            codex.runtime.as_str(),
            codex.route.as_str(),
            codex.wire_model.as_str()
        ),
        ("codex", "direct", "gpt-6-astra")
    );
    assert_eq!(row(&report, "k3").wire_model, "moonshotai/kimi-k3");
    assert_eq!(row(&report, "k3").runtime, "opencode");
    for r in &report.rows {
        assert_eq!(
            (r.ok, r.reply.as_str(), r.error.as_str(), r.unexpected),
            (true, "ok", "", false),
            "{r:?}"
        );
        assert!(r.called);
    }
    // No proxy route: no model list.
    assert_eq!(fake.models_calls.load(Ordering::SeqCst), 0);
    let summary = report.summary_json();
    assert_eq!(summary["summary"], json!({"ok": 5, "total": 5}));
    assert_eq!(summary["proxy"], json!({"state": "off"}));
}

#[test]
fn all_pass_through_the_proxy_shares_one_model_list() {
    let fix = proxy_fixture();
    let fake = Fake::new();
    let report = check(&fix, &["codex", "sol", "claude", "fable"], &fake);
    assert!(report.all_ok(), "{report:?}");
    assert_eq!(fake.models_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        report.proxy,
        ProxyLine::Up {
            url: "http://127.0.0.1:9".to_string(),
            models: 5,
            key_tail: "a91f".to_string(),
        }
    );
    let fable = row(&report, "fable");
    assert_eq!(
        (fable.route.as_str(), fable.wire_model.as_str()),
        ("proxy", "emcd_/claude-fable-5-1")
    );
    assert_eq!(row(&report, "sol").wire_model, "gpt-6.1-sol");
}

#[test]
fn one_fail_names_the_exit_code_and_the_last_log_line() {
    let fix = Fixture::new();
    let mut fake = Fake::new();
    fake.replies.insert(
        "sol",
        Reply::Out(1, "Reconnecting... 1/5\nError: stream disconnected\n\n"),
    );
    fake.hints.insert("sol", "rival: check the key");
    let report = check(&fix, &["codex", "sol"], &fake);
    assert!(!report.all_ok());
    assert_eq!(report.passed(), 1);
    let sol = row(&report, "sol");
    assert!(!sol.ok && sol.called && !sol.limit);
    assert_eq!(sol.error, "exited with code 1: Error: stream disconnected");
    assert_eq!(sol.hint, "check the key", "the rival: prefix is dropped");
    assert!(
        report.text().ends_with("\n1 of 2 ok\n"),
        "{}",
        report.text()
    );
}

#[test]
fn adapter_error_fails_with_its_first_line() {
    let fix = Fixture::new();
    let mut fake = Fake::new();
    fake.replies.insert(
        "codex",
        Reply::Err("codex runtime: start codex: denied\nmore"),
    );
    let report = check(&fix, &["codex"], &fake);
    assert_eq!(
        row(&report, "codex").error,
        "codex runtime: start codex: denied"
    );
}

#[test]
fn binary_missing_fails_without_a_call() {
    let fix = Fixture::new();
    let mut fake = Fake::new();
    fake.missing = vec!["claude"];
    let report = check(&fix, &["claude", "codex"], &fake);
    let claude = row(&report, "claude");
    assert_eq!(
        (claude.ok, claude.error.as_str(), claude.called, claude.ms),
        (false, NOT_INSTALLED, false, 0)
    );
    assert_eq!(fake.called(), ["codex"]);
    assert!(row(&report, "codex").ok);
}

#[test]
fn preflight_error_fails_with_its_first_line_and_no_call() {
    let fix = Fixture::new();
    let mut fake = Fake::new();
    fake.preflight.insert(
        "k3",
        "model kimi-k3 requires MOONSHOT_API_KEY — add it to the project .env or export it\nsecond",
    );
    let report = check(&fix, &["k3"], &fake);
    assert_eq!(
        row(&report, "k3").error,
        "model kimi-k3 requires MOONSHOT_API_KEY — add it to the project .env or export it"
    );
    assert!(fake.called().is_empty());
}

#[test]
fn proxy_down_fails_the_proxied_rows_only() {
    let fix = proxy_fixture();
    let mut fake = Fake::new();
    let down = "proxy unreachable at http://127.0.0.1:9: connection refused";
    fake.models = Err(ModelsError::Unreachable(down.to_string()));
    let report = check(&fix, &["codex", "claude", "k3"], &fake);
    assert_eq!(row(&report, "codex").error, down);
    assert_eq!(row(&report, "claude").error, down);
    assert!(row(&report, "k3").ok, "K3 does not use the proxy");
    assert_eq!(fake.called(), ["k3"]);
    // The failed list is asked once for the header and every row.
    assert_eq!(fake.models_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        report.proxy.text(),
        format!("proxy  http://127.0.0.1:9  ✗ {down}")
    );
}

#[test]
fn key_rejected() {
    let fix = proxy_fixture();
    let mut fake = Fake::new();
    fake.models = Err(ModelsError::Rejected(401));
    let report = check(&fix, &["fable"], &fake);
    assert_eq!(
        row(&report, "fable").error,
        "proxy rejected the key (401) — run rival config key set"
    );
    assert!(fake.called().is_empty());
}

#[test]
fn key_missing_is_a_static_failure() {
    let fix = Fixture::with_config_yaml(PROXY_YAML);
    let fake = Fake::new();
    let report = check(&fix, &["codex", "claude"], &fake);
    for r in &report.rows {
        assert_eq!(
            (r.route.as_str(), r.error.as_str()),
            ("proxy", config::PROXY_KEY_MISSING)
        );
    }
    assert_eq!(fake.models_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        report.proxy.text(),
        format!("proxy  http://127.0.0.1:9  ✗ {}", config::PROXY_KEY_MISSING)
    );
}

#[test]
fn model_not_listed() {
    let fix = proxy_fixture();
    let mut fake = Fake::new();
    fake.models = Ok(vec![
        "emcd_/claude-opus-5-5".to_string(),
        "emcd2_/claude-fable-5-1".to_string(),
        "gpt-6-astra".to_string(),
    ]);
    let report = check(&fix, &["claude", "fable", "sol"], &fake);
    assert!(row(&report, "claude").ok);
    assert_eq!(
        row(&report, "fable").error,
        "proxy does not serve emcd_/claude-fable-5-1; it serves claude-fable-5-1 as: emcd2_/claude-fable-5-1 — set proxy.claude.model_prefix"
    );
    assert!(
        row(&report, "sol")
            .error
            .starts_with("proxy does not serve gpt-6.1-sol;")
    );
    assert_eq!(fake.called(), ["claude"]);
}

#[test]
fn limit_429_sets_limit_and_names_the_other_prefixes() {
    let fix = proxy_fixture();
    let mut fake = Fake::new();
    fake.replies.insert(
        "claude",
        Reply::Out(
            1,
            "API Error: 429 All credentials for model emcd_/claude-opus-5-5 are cooling down (rate_limit_error: monthly spend limit)\n",
        ),
    );
    fake.hints
        .insert("claude", "rival: the auth hint loses to the 429");
    let report = check(&fix, &["claude"], &fake);
    let claude = row(&report, "claude");
    assert!(!claude.ok && claude.limit, "{claude:?}");
    assert_eq!(
        claude.hint,
        "proxy account emcd_ is at its limit (429); other prefixes that serve claude-opus-5-5: emcd2_ — set proxy.claude.model_prefix"
    );
    let text = report.text();
    assert!(
        text.contains("  limit  exited with code 1: API Error: 429"),
        "{text}"
    );
    assert!(
        text.contains("\n      proxy account emcd_ is at its limit"),
        "{text}"
    );
}

#[test]
fn quota_on_the_direct_route_sets_limit() {
    let fix = Fixture::new();
    let mut fake = Fake::new();
    fake.replies
        .insert("codex", Reply::Out(1, "ERROR: insufficient_quota\n"));
    let report = check(&fix, &["codex"], &fake);
    assert!(row(&report, "codex").limit);
}

#[test]
fn timeout_ends_the_call() {
    let fix = Fixture::new();
    let mut fake = Fake::new();
    fake.replies.insert("codex", Reply::Hang);
    let started = Instant::now();
    let report = check_timeout(&fix, &["codex", "sol"], &fake, Duration::from_millis(50));
    assert!(started.elapsed() < Duration::from_secs(10));
    let codex = row(&report, "codex");
    assert_eq!(codex.error, "no reply in 50ms (timeout)");
    assert!(codex.called);
    assert!(row(&report, "sol").ok);
}

/// A preflight that never answers (`docker info` on a stuck daemon) ends
/// at the check's budget; the other models go on.
#[test]
fn timeout_ends_a_hung_preflight() {
    let fix = Fixture::new();
    let mut fake = Fake::new();
    fake.preflight_hang.push("codex");
    let started = Instant::now();
    let report = check_timeout(&fix, &["codex", "sol"], &fake, Duration::from_millis(50));
    assert!(started.elapsed() < Duration::from_secs(10));
    let codex = row(&report, "codex");
    assert_eq!(codex.error, "preflight: no answer in 50ms (timeout)");
    assert!(!codex.called);
    assert!(row(&report, "sol").ok);
}

/// Cancelling the check (`x`, closing the window) ends a hung preflight.
#[test]
fn cancel_ends_a_hung_preflight() {
    let fix = Fixture::new();
    let mut fake = Fake::new();
    fake.preflight_hang.push("codex");
    let targets = targets(&fix.cfg, &["codex".to_string()]).unwrap();
    let (ctx, cancel) = Context::background().with_cancel();
    let started = Instant::now();
    let report = std::thread::scope(|s| {
        s.spawn(|| {
            std::thread::sleep(Duration::from_millis(100));
            cancel.cancel();
        });
        run_check_with(&ctx, &fix.cfg, &targets, &fake, CHECK_TIMEOUT, &|_| {})
    });
    assert!(started.elapsed() < Duration::from_secs(10));
    let codex = row(&report, "codex");
    assert_eq!(codex.error, "context canceled");
    assert!(!codex.called);
}

#[test]
fn unexpected_reply_passes_flagged_and_is_cut_to_40_chars() {
    let fix = Fixture::new();
    let mut fake = Fake::new();
    fake.replies.insert(
        "codex",
        Reply::Out(
            0,
            "Sure! Here is the word you asked for, as requested: ok\n",
        ),
    );
    fake.replies.insert("sol", Reply::Out(0, "OK.\n"));
    fake.replies.insert("claude", Reply::Out(0, "  \n"));
    let report = check(&fix, &["codex", "sol", "claude"], &fake);
    let codex = row(&report, "codex");
    assert!(codex.ok && codex.unexpected);
    assert_eq!(codex.reply, "Sure! Here is the word you asked for, as");
    assert_eq!(codex.reply.chars().count(), REPLY_CHARS);
    let sol = row(&report, "sol");
    assert!(sol.ok && !sol.unexpected);
    assert_eq!(sol.reply, "OK.");
    let claude = row(&report, "claude");
    assert_eq!((claude.ok, claude.error.as_str()), (false, "empty reply"));
}

#[test]
fn reply_drops_the_codex_banner() {
    let log = "OpenAI Codex v0.161.0\n--------\nmodel: gpt-6-astra\n--------\nuser\nReply with exactly: ok\n\ncodex\nok\ntokens used\n1,234\nok\n";
    assert_eq!(reply_text(log), "ok");
    assert!(is_ok_reply("Ok!"));
    assert!(!is_ok_reply("okay"));
}

/// The run logs of `name` in `dir`, oldest first by name order of the
/// calls (`<name>-<pid>-<id>.log`).
fn run_logs(dir: &Path, name: &str) -> Vec<PathBuf> {
    let mut logs: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let file = p.file_name().unwrap().to_str().unwrap();
            file.strip_prefix(&format!("{name}-{}-", std::process::id()))
                .and_then(|rest| rest.strip_suffix(".log"))
                .is_some_and(|id| id.len() == 32 && id.chars().all(|c| c.is_ascii_hexdigit()))
        })
        .collect();
    logs.sort();
    logs
}

#[test]
fn logs_go_to_the_check_dir_and_no_session_is_saved() {
    let fix = Fixture::new();
    let fake = Fake::new();
    let report = check(&fix, &["codex", "fable"], &fake);
    assert!(report.all_ok());
    let dir = fix.cfg.paths().check_dir();
    for (name, log, workdir) in fake.calls.lock().unwrap().iter() {
        assert_eq!(run_logs(&dir, name), vec![log.clone()]);
        assert_eq!(workdir, dir.to_str().unwrap());
    }
    assert_eq!(
        fs::read_to_string(&run_logs(&dir, "codex")[0]).unwrap(),
        "ok\n"
    );
    assert!(fix.sessions().is_empty());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }
    // A second check writes its own log and leaves the first one alone.
    let report = check(&fix, &["codex"], &fake);
    assert!(report.all_ok());
    let logs = run_logs(&dir, "codex");
    assert_eq!(logs.len(), 2);
    for log in &logs {
        assert_eq!(fs::read_to_string(log).unwrap(), "ok\n");
    }
}

/// Two checks at once (two `rival config check` runs, or the TUI and the
/// CLI) each get their own log, so neither deletes or reads the other's.
#[test]
fn concurrent_checks_get_their_own_logs() {
    let fix = Fixture::new();
    let mut fake = Fake::new();
    fake.delay = Duration::from_millis(100);
    std::thread::scope(|s| {
        for _ in 0..2 {
            s.spawn(|| assert!(check(&fix, &["codex"], &fake).all_ok()));
        }
    });
    let calls = fake.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_ne!(calls[0].1, calls[1].1);
    let dir = fix.cfg.paths().check_dir();
    assert_eq!(run_logs(&dir, "codex").len(), 2);
}

/// Each check keeps the newest [`KEEP_LOGS`] logs of a model and removes
/// older ones; other models' logs and other files stay.
#[test]
fn old_check_logs_are_pruned() {
    let fix = Fixture::new();
    let fake = Fake::new();
    let dir = fix.cfg.paths().check_dir();
    fs::create_dir_all(&dir).unwrap();
    let pid = std::process::id();
    let old = |name: &str, i: usize| dir.join(format!("{name}-{pid}-{:032x}.log", i));
    let mut aged = Vec::new();
    for i in 0..8 {
        for name in ["codex", "grok", "grok-4.6-openrouter"] {
            let path = old(name, i);
            fs::write(&path, "old\n").unwrap();
            let when = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000 + i as u64);
            fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(when)
                .unwrap();
            aged.push(path);
        }
    }
    fs::write(dir.join("codex.log"), "legacy\n").unwrap();
    fs::write(dir.join("notes.txt"), "mine\n").unwrap();
    assert!(check(&fix, &["codex"], &fake).all_ok());
    let codex = run_logs(&dir, "codex");
    assert_eq!(codex.len(), KEEP_LOGS, "{codex:?}");
    // The newest old ones stay, with the new run's log.
    for i in 8 - (KEEP_LOGS - 1)..8 {
        assert!(old("codex", i).exists(), "codex {i}");
    }
    let new = &fake.calls.lock().unwrap()[0].1;
    assert!(codex.contains(new));
    for name in ["grok", "grok-4.6-openrouter"] {
        assert_eq!(run_logs(&dir, name).len(), 8, "{name}");
    }
    assert!(dir.join("codex.log").exists());
    assert!(dir.join("notes.txt").exists());
}

#[test]
fn at_most_three_live_calls_at_once() {
    let fix = Fixture::new();
    let mut fake = Fake::new();
    fake.delay = Duration::from_millis(150);
    let report = check(&fix, &["all"], &fake);
    assert_eq!(report.rows.len(), 7);
    assert_eq!(fake.called().len(), 7);
    assert_eq!(fake.max_active.load(Ordering::SeqCst), MAX_LIVE);
}

#[test]
fn rows_reach_the_callback_as_they_finish() {
    let fix = Fixture::new();
    let mut fake = Fake::new();
    fake.missing = vec!["sol"];
    fake.delay = Duration::from_millis(100);
    let seen: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let targets = targets(&fix.cfg, &["codex".to_string(), "sol".to_string()]).unwrap();
    let report = run_check_with(
        &Context::background(),
        &fix.cfg,
        &targets,
        &fake,
        CHECK_TIMEOUT,
        &|r| seen.lock().unwrap().push(r.name.clone()),
    );
    // sol fails at once; codex answers after its call.
    assert_eq!(*seen.lock().unwrap(), ["sol", "codex"]);
    let order: Vec<&str> = report.rows.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(order, ["codex", "sol"]);
}

#[test]
fn text_output_matches_the_spec_layout() {
    let report = Report {
        proxy: ProxyLine::Up {
            url: "http://127.0.0.1:8317".to_string(),
            models: 38,
            key_tail: "a91f".to_string(),
        },
        rows: vec![
            CheckRow {
                name: "claude".into(),
                runtime: "claude".into(),
                route: "proxy".into(),
                wire_model: "emcd_/claude-opus-5-5".into(),
                ok: true,
                ms: 2100,
                reply: "ok".into(),
                called: true,
                ..CheckRow::default()
            },
            CheckRow {
                name: "sol".into(),
                runtime: "codex".into(),
                route: "proxy".into(),
                wire_model: "gpt-6.1-sol".into(),
                error: "proxy does not serve gpt-6.1-sol".into(),
                ..CheckRow::default()
            },
            CheckRow {
                name: "k3".into(),
                runtime: "opencode".into(),
                route: "direct".into(),
                wire_model: "moonshotai/kimi-k3".into(),
                ok: true,
                ms: 4000,
                reply: "Sure".into(),
                unexpected: true,
                called: true,
                ..CheckRow::default()
            },
        ],
    };
    assert_eq!(
        report.text(),
        "proxy  http://127.0.0.1:8317  ✓ 200  38 models  key …a91f

  ✓ claude  proxy   emcd_/claude-opus-5-5  2.1s  ok
  ✗ sol     proxy   gpt-6.1-sol            —     proxy does not serve gpt-6.1-sol
  ✓ k3      direct  moonshotai/kimi-k3     4.0s  Sure  (not ok)

2 of 3 ok
"
    );
    assert_eq!(ProxyLine::Off.text(), "proxy  off");
    assert_eq!(
        report.rows[1].to_json().to_string(),
        r#"{"error":"proxy does not serve gpt-6.1-sol","hint":"","limit":false,"ms":0,"name":"sol","ok":false,"reply":"","route":"proxy","runtime":"codex","unexpected":false,"wire_model":"gpt-6.1-sol"}"#
    );
}

// ---- the real adapter path ----

/// A fake `codex` that records its argv and replies `ok`. `PATH` holds only
/// its directory, so no real CLI can answer.
#[cfg(unix)]
fn fake_codex(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt as _;
    let argv = dir.join("argv");
    let script = dir.join("codex");
    fs::write(
        &script,
        format!(
            "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done >> '{}'\nwhile IFS= read -r l; do :; done\nprintf 'ok\\n'\n",
            argv.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    argv
}

/// The real preflight path: a `codex login status` that hangs is killed
/// when the check is cancelled, and the check returns.
#[cfg(unix)]
#[test]
fn real_preflight_is_killed_on_cancel() {
    use std::os::unix::fs::PermissionsExt as _;
    let bin = tempfile::tempdir().unwrap();
    let pidfile = bin.path().join("pid");
    let script = bin.path().join("codex");
    fs::write(
        &script,
        format!(
            "#!/bin/sh\necho $$ > '{}'\nexec /bin/sleep 30\n",
            pidfile.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let fix = Fixture::with(&[("PATH", bin.path().to_str().unwrap())], None);
    let targets = targets(&fix.cfg, &["codex".to_string()]).unwrap();
    let (ctx, cancel) = Context::background().with_cancel();
    let started = Instant::now();
    let report = std::thread::scope(|s| {
        s.spawn(|| {
            while !pidfile.exists() && started.elapsed() < Duration::from_secs(10) {
                std::thread::sleep(Duration::from_millis(20));
            }
            cancel.cancel();
        });
        run_check(&ctx, &fix.cfg, &targets, &|_| {})
    });
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(
        row(&report, "codex").error,
        "context canceled",
        "{report:?}"
    );
    let pid: i32 = fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // SAFETY: signal 0 only checks that the PID exists.
    assert_ne!(unsafe { libc::kill(pid, 0) }, 0, "codex still runs");
}

#[cfg(unix)]
#[test]
fn real_adapter_path_saves_no_session() {
    let bin = tempfile::tempdir().unwrap();
    let argv = fake_codex(bin.path());
    let fix = Fixture::with(&[("PATH", bin.path().to_str().unwrap())], None);
    // The directory exists, so a save would succeed.
    let sessions = fix.cfg.paths().sessions_dir();
    fs::create_dir_all(&sessions).unwrap();
    let targets = targets(&fix.cfg, &["codex".to_string(), "claude".to_string()]).unwrap();
    let report = run_check(&Context::background(), &fix.cfg, &targets, &|_| {});
    let codex = row(&report, "codex");
    assert!(codex.ok, "{report:?}");
    assert_eq!(codex.reply, "ok");
    // Neither claude nor docker is on PATH.
    assert_eq!(row(&report, "claude").error, NOT_INSTALLED);
    let args = fs::read_to_string(&argv).unwrap();
    // `codex login status` (the preflight), then the read-only exec at low.
    assert!(args.starts_with("login\nstatus\nexec\n"), "{args}");
    assert!(args.contains("\nmodel_reasoning_effort=low\n"), "{args}");
    assert!(args.contains("\n--sandbox\nread-only\n"), "{args}");
    let dir = fix.cfg.paths().check_dir();
    assert!(
        args.contains(&format!("\n-C\n{}\n", dir.display())),
        "{args}"
    );
    let logs = run_logs(&dir, "codex");
    assert_eq!(logs.len(), 1, "{logs:?}");
    assert_eq!(fs::read_to_string(&logs[0]).unwrap(), "ok\n");
    let saved: Vec<_> = fs::read_dir(&sessions)
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert!(saved.is_empty(), "sessions/ holds {saved:?}");
}
