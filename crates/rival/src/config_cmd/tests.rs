//! `rival config` through the real tree and root, over a temp HOME. No test
//! reads the real `~/.rival` or uses a real key.

use std::fs;
use std::path::PathBuf;

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

#[test]
fn models_is_not_built_yet() {
    let fix = Fixture::new();
    let (code, _, err) = run(&fix, "", &["config", "models", "--json"]);
    assert_eq!(code, 1);
    assert!(err.contains("needs the proxy module"), "{err}");
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
