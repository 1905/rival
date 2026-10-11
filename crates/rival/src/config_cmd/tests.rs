//! `rival config` through the real tree and root, over a temp HOME. No test
//! reads the real `~/.rival` or uses a real key.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rival_core::config::write::HEADER;

use crate::testutil::{FakeStdin, Fixture, execute};

const SECRET: &str = "sk-test-not-real-1234";

fn config_file(fix: &Fixture) -> PathBuf {
    fix.cfg.paths().config_file()
}

fn key_file(fix: &Fixture) -> PathBuf {
    fix.cfg.paths().root.join("proxy.key")
}

fn run(fix: &Fixture, input: &str, args: &[&str]) -> (i32, String, String) {
    execute(fix, &mut FakeStdin::new(input), args)
}

/// The output line for `key`, with its runs of spaces squeezed.
fn line(out: &str, key: &str) -> String {
    out.lines()
        .find(|l| l.split_whitespace().next() == Some(key))
        .unwrap_or_else(|| panic!("no {key} line in:\n{out}"))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

const PROXY_YAML: &str = "proxy:
  url: http://127.0.0.1:8317/v1
  claude:
    enabled: true
    model_prefix: emcd_
efforts:
  fable: low
";

#[test]
fn show_lists_each_value_with_its_source() {
    let fix = Fixture::with_config_and_env(PROXY_YAML, &[("RIVAL_PROXY_KEY", SECRET)]);
    let (code, out, err) = run(&fix, "", &["config", "show"]);
    assert_eq!((code, err.as_str()), (0, ""), "{out}");
    assert!(!out.contains(SECRET), "{out}");
    assert_eq!(
        line(&out, "proxy.url"),
        "proxy.url http://127.0.0.1:8317 file"
    );
    assert_eq!(line(&out, "proxy.key"), "proxy.key set (…1234) env");
    assert_eq!(
        line(&out, "proxy.claude.enabled"),
        "proxy.claude.enabled true file"
    );
    assert_eq!(
        line(&out, "proxy.claude.model_prefix"),
        "proxy.claude.model_prefix emcd_ file"
    );
    assert_eq!(
        line(&out, "proxy.codex.enabled"),
        "proxy.codex.enabled false default"
    );
    assert_eq!(
        line(&out, "proxy.codex.model_prefix"),
        "proxy.codex.model_prefix \"\" default"
    );
    assert_eq!(line(&out, "plan.models"), "plan.models codex default");
    assert_eq!(line(&out, "efforts.fable"), "efforts.fable low file");
    assert_eq!(line(&out, "efforts.codex"), "efforts.codex xhigh default");
    assert_eq!(line(&out, "efforts.sol"), "efforts.sol xhigh default");
    assert_eq!(
        line(&out, "security.reviewer"),
        "security.reviewer k3 default"
    );
    assert_eq!(
        line(&out, "auto_fix_critical_high"),
        "auto_fix_critical_high false default"
    );
}

#[test]
fn show_reads_the_key_file_and_env_switches() {
    let fix = Fixture::with_config_and_env(
        PROXY_YAML,
        &[
            ("RIVAL_PROXY", "off"),
            ("RIVAL_PROXY_URL", "https://proxy.example/"),
        ],
    );
    let (_, out, _) = run(&fix, "", &["config", "show"]);
    assert_eq!(line(&out, "proxy.key"), "proxy.key missing default");
    assert_eq!(
        line(&out, "proxy.url"),
        "proxy.url https://proxy.example env"
    );
    assert_eq!(
        line(&out, "proxy.claude.enabled"),
        "proxy.claude.enabled false env"
    );
    // The key loads once per process (one `Config`), so a new fixture.
    let fix = Fixture::with_config_yaml(PROXY_YAML);
    let (code, _, err) = run(&fix, &format!("{SECRET}\n"), &["config", "key", "set"]);
    assert_eq!((code, err.as_str()), (0, ""));
    let (_, out, _) = run(&fix, "", &["config", "show"]);
    assert_eq!(line(&out, "proxy.key"), "proxy.key set (…1234) file");
    assert!(!out.contains(SECRET));
}

#[test]
fn show_json_is_one_object_without_the_key() {
    let fix = Fixture::with_config_and_env(PROXY_YAML, &[("RIVAL_PROXY_KEY", SECRET)]);
    let (code, out, _) = run(&fix, "", &["config", "show", "--json"]);
    assert_eq!(code, 0);
    assert!(!out.contains(SECRET), "{out}");
    assert_eq!(out.matches('\n').count(), 1, "{out}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let values = &v["values"];
    assert_eq!(values["proxy.key"]["value"], "set (…1234)");
    assert_eq!(values["proxy.key"]["source"], "env");
    assert_eq!(values["proxy.claude.enabled"]["value"], true);
    assert_eq!(values["proxy.claude.enabled"]["source"], "file");
    assert_eq!(values["plan.models"]["value"], serde_json::json!(["codex"]));
    assert_eq!(values["efforts.fable"]["value"], "low");
    assert_eq!(
        v["config_file"],
        config_file(&fix).to_str().unwrap(),
        "{out}"
    );
}

#[test]
fn set_writes_one_key() {
    let fix = Fixture::new();
    let (code, out, err) = run(
        &fix,
        "",
        &["config", "set", "proxy.claude.model_prefix", "emcd2_"],
    );
    assert_eq!((code, err.as_str()), (0, ""), "{out}");
    assert_eq!(out, "proxy.claude.model_prefix = emcd2_\n");
    assert_eq!(
        fs::read_to_string(config_file(&fix)).unwrap(),
        format!("{HEADER}\nproxy:\n  claude:\n    model_prefix: emcd2_\n")
    );
    let (code, _, err) = run(&fix, "", &["config", "set", "plan.models", "sol,opus"]);
    assert_eq!((code, err.as_str()), (0, ""));
    assert!(
        fs::read_to_string(config_file(&fix))
            .unwrap()
            .ends_with("plan:\n  models:\n  - sol\n  - opus\n")
    );
}

#[test]
fn set_rejects_the_key_bad_values_and_bad_usage() {
    let fix = Fixture::new();
    let (code, out, err) = run(&fix, "", &["config", "set", "proxy.key", SECRET]);
    assert_eq!(code, 1);
    assert!(err.contains("rival config key set"), "{err}");
    assert!(!err.contains(SECRET) && !out.contains(SECRET), "{err}");
    let (code, _, err) = run(&fix, "", &["config", "set", "efforts.codex", "huge"]);
    assert_eq!(code, 1);
    assert!(err.contains("invalid effort \"huge\" for codex"), "{err}");
    for args in [
        &["config", "set"][..],
        &["config", "set", "proxy.url"],
        &["config", "set", "proxy.url", "b", "c"],
    ] {
        let (code, _, err) = run(&fix, "", args);
        assert_eq!(code, 1, "{args:?}");
        assert!(
            err.contains("usage: rival config set KEY VALUE"),
            "{args:?}: {err}"
        );
    }
    assert!(!config_file(&fix).exists());
}

#[test]
fn set_json_applies_a_patch_in_one_write() {
    let fix = Fixture::new();
    let patch = r#"{"proxy": {"url": "http://127.0.0.1:8317", "codex": {"enabled": true}}, "efforts.sol": "ultra"}"#;
    let (code, out, err) = run(&fix, patch, &["config", "set", "--json"]);
    assert_eq!((code, err.as_str()), (0, ""), "{out}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["saved"], config_file(&fix).to_str().unwrap());
    let text = fs::read_to_string(config_file(&fix)).unwrap();
    assert!(text.contains("  url: http://127.0.0.1:8317\n"), "{text}");
    assert!(text.contains("    enabled: true\n"), "{text}");
    assert!(text.contains("efforts:\n  sol: ultra\n"), "{text}");
    // A bad patch writes nothing.
    for (bad, want) in [
        ("{", "parse the JSON patch"),
        (r#"{"proxy.url": "x"}"#, "invalid proxy.url"),
        (r#"{"proxy": {"key": "abc"}}"#, "rival config key set"),
    ] {
        let (code, _, err) = run(&fix, bad, &["config", "set", "--json"]);
        assert_eq!(code, 1, "{bad}");
        assert!(err.contains(want), "{bad}: {err}");
    }
    assert_eq!(fs::read_to_string(config_file(&fix)).unwrap(), text);
    let (code, _, err) = run(&fix, "{}", &["config", "set", "--json", "proxy.url"]);
    assert_eq!(code, 1);
    assert!(err.contains("takes no arguments"), "{err}");
}

#[test]
fn set_repairs_an_invalid_config_that_blocks_other_commands() {
    let fix = Fixture::with_config_yaml("efforts:\n  codex: huge\n");
    let (code, _, err) = run(&fix, "", &["config", "show"]);
    assert_eq!(code, 1);
    assert!(err.contains("invalid effort"), "{err}");
    let (code, _, err) = run(&fix, "", &["config", "set", "efforts.codex", "high"]);
    assert_eq!((code, err.as_str()), (0, ""));
}

#[test]
fn key_set_reads_stdin_only() {
    let fix = Fixture::new();
    let (code, out, err) = run(&fix, &format!("  {SECRET}\n"), &["config", "key", "set"]);
    assert_eq!((code, err.as_str()), (0, ""));
    assert!(!out.contains(SECRET), "{out}");
    assert!(out.contains(key_file(&fix).to_str().unwrap()), "{out}");
    assert_eq!(
        fs::read_to_string(key_file(&fix)).unwrap(),
        format!("{SECRET}\n")
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = fs::metadata(key_file(&fix)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    // A key as an argument is an error that does not echo it.
    let mut stdin = FakeStdin::new("");
    stdin.forbid_read = true;
    let (code, out, err) = execute(&fix, &mut stdin, &["config", "key", "set", "sk-arg-9999"]);
    assert_eq!(code, 1);
    assert!(err.contains("reads the key from stdin"), "{err}");
    assert!(!err.contains("sk-arg") && !out.contains("sk-arg"), "{err}");
    let (code, _, err) = run(&fix, "\n", &["config", "key", "set"]);
    assert_eq!(code, 1);
    assert!(err.contains("the key is empty"), "{err}");
    assert_eq!(
        fs::read_to_string(key_file(&fix)).unwrap(),
        format!("{SECRET}\n")
    );
}

#[test]
fn key_clear_removes_the_file() {
    let fix = Fixture::new();
    run(&fix, SECRET, &["config", "key", "set"]);
    let (code, out, _) = run(&fix, "", &["config", "key", "clear"]);
    assert_eq!(code, 0);
    assert!(out.starts_with("removed "), "{out}");
    assert!(!key_file(&fix).exists());
    let (code, out, _) = run(&fix, "", &["config", "key", "clear"]);
    assert_eq!(code, 0);
    assert!(out.starts_with("no proxy key file at "), "{out}");
}

/// A one-route HTTP fake on `127.0.0.1:0`: every request gets `status`
/// and `body`. Returns the base URL and the Authorization headers seen.
fn fake_proxy(status: u16, body: &'static str) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut auth = String::new();
            loop {
                let mut h = String::new();
                if reader.read_line(&mut h).unwrap() == 0 || h == "\r\n" {
                    break;
                }
                if let Some((name, v)) = h.split_once(':')
                    && name.eq_ignore_ascii_case("authorization")
                {
                    auth = v.trim().to_string();
                }
            }
            log.lock().unwrap().push(auth);
            let _ = write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    (url, seen)
}

const MODEL_LIST: &str = r#"{"data":[{"id":"emcd_/claude-opus-5-5"},{"id":"gpt-6-astra"},{"id":"emcd2_/claude-opus-5-5"},{"id":"emcd_/claude-fable-5-1"}]}"#;

#[test]
fn models_lists_the_proxy_models_by_prefix() {
    let (url, seen) = fake_proxy(200, MODEL_LIST);
    let fix = Fixture::with_config_and_env(
        "",
        &[
            ("RIVAL_PROXY_URL", url.as_str()),
            ("RIVAL_PROXY_KEY", SECRET),
        ],
    );
    let (code, out, err) = run(&fix, "", &["config", "models"]);
    assert_eq!((code, err.as_str()), (0, ""), "{out}");
    assert_eq!(
        out,
        format!(
            "proxy  {url}  4 models\n\n(no prefix)\n  gpt-6-astra\n\nemcd2_\n  claude-opus-5-5\n\nemcd_\n  claude-opus-5-5\n  claude-fable-5-1\n"
        )
    );
    let (code, out, _) = run(&fix, "", &["config", "models", "--json"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(out.lines().count(), 1, "{out}");
    assert_eq!(v["url"], serde_json::json!(url));
    assert_eq!(v["count"], 4);
    assert_eq!(v["models"][0], "emcd_/claude-opus-5-5");
    assert_eq!(
        v["prefixes"]["emcd_"],
        serde_json::json!(["claude-opus-5-5", "claude-fable-5-1"])
    );
    assert_eq!(v["prefixes"][""], serde_json::json!(["gpt-6-astra"]));
    // One call per process: the second command reused the list.
    assert_eq!(*seen.lock().unwrap(), vec![format!("Bearer {SECRET}")]);
}

#[test]
fn models_names_the_missing_url_key_and_a_rejected_key() {
    let fix = Fixture::new();
    let (code, _, err) = run(&fix, "", &["config", "models"]);
    assert_eq!(
        (code, err.as_str()),
        (
            1,
            "proxy.url is not set — run rival config set proxy.url <url>\n"
        )
    );
    let fix = Fixture::with_config_and_env("proxy:\n  url: http://127.0.0.1:9/\n", &[]);
    let (code, _, err) = run(&fix, "", &["config", "models"]);
    assert_eq!(
        (code, err.as_str()),
        (1, "proxy key missing — run rival config key set\n")
    );
    let (url, _) = fake_proxy(401, "{}");
    let fix = Fixture::with_config_and_env(
        "",
        &[
            ("RIVAL_PROXY_URL", url.as_str()),
            ("RIVAL_PROXY_KEY", SECRET),
        ],
    );
    let (code, _, err) = run(&fix, "", &["config", "models", "--json"]);
    assert_eq!(
        (code, err.as_str()),
        (
            1,
            "proxy rejected the key (401) — run rival config key set\n"
        )
    );
}

#[test]
fn bare_config_opens_the_tui_and_key_prints_help() {
    let fix = Fixture::new();
    let (code, _, err) = run(&fix, "", &["config"]);
    assert_eq!(
        (code, err.as_str()),
        (1, "tui: the TUI does not run in tests\n")
    );
    let (code, out, _) = run(&fix, "", &["config", "key"]);
    assert_eq!(code, 0);
    assert!(out.contains("Store or remove the proxy key"), "{out}");
    let (code, _, err) = run(&fix, "", &["config", "bogus"]);
    assert_eq!(code, 1);
    assert_eq!(err, "unknown command \"bogus\" for \"rival config\"\n");
}

// ---- config check ----

/// A temp `PATH` dir with a fake `codex` that replies `reply` to `exec` and
/// records `RIVAL_PROXY_KEY` (present or not) in `seen-key`. Only this dir
/// is on `PATH`, so no real CLI (and no docker) can answer.
#[cfg(unix)]
fn fake_codex_bin(reply: &str) -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("codex");
    fs::write(
        &script,
        format!(
            "#!/bin/sh\nwhile IFS= read -r l; do :; done\nprintf '%s' \"${{RIVAL_PROXY_KEY:+set}}\" > '{}/seen-key'\nprintf '{reply}\\n'\n",
            dir.path().display()
        ),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    dir
}

const CODEX_PROXY_YAML: &str = "proxy:\n  codex:\n    enabled: true\n";

#[cfg(unix)]
#[test]
fn check_json_prints_one_line_per_model_then_the_summary() {
    let (url, seen) = fake_proxy(200, MODEL_LIST);
    let bin = fake_codex_bin("ok");
    let fix = Fixture::with_config_and_env(
        CODEX_PROXY_YAML,
        &[
            ("PATH", bin.path().to_str().unwrap()),
            ("RIVAL_PROXY_URL", url.as_str()),
            ("RIVAL_PROXY_KEY", SECRET),
        ],
    );
    let (code, out, err) = run(&fix, "", &["config", "check", "-m", "codex", "--json"]);
    assert_eq!((code, err.as_str()), (0, ""), "{out}");
    assert!(!out.contains(SECRET), "{out}");
    let lines: Vec<serde_json::Value> = out
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 2, "{out}");
    let row = &lines[0];
    assert_eq!(
        (
            &row["name"],
            &row["runtime"],
            &row["route"],
            &row["wire_model"],
            &row["ok"],
            &row["reply"],
            &row["unexpected"],
        ),
        (
            &serde_json::json!("codex"),
            &serde_json::json!("codex"),
            &serde_json::json!("proxy"),
            &serde_json::json!("gpt-6-astra"),
            &serde_json::json!(true),
            &serde_json::json!("ok"),
            &serde_json::json!(false),
        )
    );
    assert_eq!(
        lines[1]["summary"],
        serde_json::json!({"ok": 1, "total": 1})
    );
    assert_eq!(lines[1]["proxy"]["state"], "up");
    assert_eq!(*seen.lock().unwrap(), vec![format!("Bearer {SECRET}")]);
    assert_eq!(
        fs::read_to_string(bin.path().join("seen-key")).unwrap(),
        "set"
    );
    assert!(fix.sessions().is_empty());
}

#[cfg(unix)]
#[test]
fn check_text_and_exit_1_on_a_failed_model() {
    let bin = fake_codex_bin("Sure, ok");
    let fix = Fixture::with(&[("PATH", bin.path().to_str().unwrap())], None);
    let (code, out, err) = run(&fix, "", &["config", "check", "-m", "codex,opus,k3"]);
    assert_eq!(
        (code, err.as_str()),
        (1, "config check: 2 of 3 models failed\n"),
        "{out}"
    );
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "proxy  off");
    assert_eq!(lines[1], "");
    assert!(
        lines[2].starts_with("  ✓ codex   direct  gpt-6-astra         ")
            && lines[2].ends_with("s  Sure, ok  (not ok)"),
        "{out}"
    );
    assert_eq!(
        lines[3],
        "  ✗ claude  direct  claude-opus-5-5     —     not installed"
    );
    assert_eq!(
        lines[4],
        "  ✗ k3      direct  moonshotai/kimi-k3  —     not installed"
    );
    assert_eq!(&lines[5..], ["", "1 of 3 ok"]);
}

#[test]
fn check_rejects_an_unknown_model() {
    let fix = Fixture::new();
    let (code, _, err) = run(&fix, "", &["config", "check", "-m", "gpt"]);
    assert_eq!(code, 1);
    assert!(
        err.starts_with("unknown model \"gpt\" for config check"),
        "{err}"
    );
}

#[cfg(unix)]
#[test]
fn check_config_stdin_uses_the_draft_not_the_saved_file() {
    let (url, _) = fake_proxy(200, MODEL_LIST);
    let bin = fake_codex_bin("ok");
    // The saved file is invalid: only the draft is read.
    let fix = Fixture::with_config_and_env(
        "efforts:\n  codex: bogus\n",
        &[
            ("PATH", bin.path().to_str().unwrap()),
            ("RIVAL_PROXY_KEY", SECRET),
        ],
    );
    let draft = format!("proxy:\n  url: {url}/v1\n  codex:\n    enabled: true\n");
    let (code, out, err) = run(
        &fix,
        &draft,
        &["config", "check", "-m", "codex", "--json", "--config-stdin"],
    );
    assert_eq!((code, err.as_str()), (0, ""), "{out}");
    let row: serde_json::Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
    assert_eq!(
        (&row["route"], &row["ok"]),
        (&serde_json::json!("proxy"), &serde_json::json!(true))
    );
    assert_eq!(
        fs::read_to_string(bin.path().join("seen-key")).unwrap(),
        "set"
    );
    // The saved file is unchanged, and without the draft it still fails.
    assert_eq!(
        fs::read_to_string(config_file(&fix)).unwrap(),
        "efforts:\n  codex: bogus\n"
    );
    let (code, _, err) = run(&fix, "", &["config", "check", "-m", "codex"]);
    assert_eq!(code, 1);
    assert!(err.contains("invalid effort \"bogus\" for codex"), "{err}");
}

#[test]
fn check_config_stdin_validates_the_draft() {
    let fix = Fixture::new();
    let (code, out, err) = run(
        &fix,
        "proxy:\n  key: sk-in-yaml\n",
        &["config", "check", "--config-stdin"],
    );
    assert_eq!((code, out.as_str()), (1, ""));
    assert_eq!(
        err,
        "proxy.key is not allowed in <stdin>; remove it and store the key with: rival config key set\n"
    );
}
