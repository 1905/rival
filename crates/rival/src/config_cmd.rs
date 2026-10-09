//! `rival config`: show and change `~/.rival/config.yaml` and the proxy
//! key. Bare `rival config` opens the TUI (the config window comes later).

use std::collections::HashSet;
use std::io::BufRead;

use rival_core::config::write::{self, Edit};
use rival_core::config::{
    self, CLAUDE_LABEL, CODEX_LABEL, Config, FABLE_LABEL, GROK_LABEL, K3_LABEL, PROXY_KEY_ENV,
    PROXY_URL_ENV, ProxyKeySource, ProxyRoute, SECURITY_REVIEWER_K3, SOL_LABEL,
};
use serde_json::{Value as Json, json};

use crate::root::{CmdEnv, CmdError};
use crate::tree::Invocation;

#[cfg(test)]
mod tests;

/// The effort labels `show` lists, in this order.
const EFFORT_LABELS: [&str; 6] = [
    CODEX_LABEL,
    SOL_LABEL,
    CLAUDE_LABEL,
    FABLE_LABEL,
    K3_LABEL,
    GROK_LABEL,
];

/// `plan.models` when unset: the `--model` default of `command plan`.
const DEFAULT_PLAN_MODELS: [&str; 1] = [CODEX_LABEL];

/// One resolved value and where it came from: `default`, `file` or `env`.
struct Row {
    key: String,
    value: Json,
    source: &'static str,
    error: Option<String>,
}

impl Row {
    fn new(key: &str, value: Json, source: &'static str) -> Row {
        Row {
            key: key.to_string(),
            value,
            source,
            error: None,
        }
    }
}

/// `rival config show [--json]`.
pub fn show_action(env: &mut CmdEnv<'_>, inv: &Invocation) -> Result<(), CmdError> {
    let path = env.cfg.paths().config_file();
    let rows = rows(env.cfg)?;
    if inv.bool("json") {
        let values: serde_json::Map<String, Json> = rows
            .iter()
            .map(|r| {
                let mut v = json!({"value": r.value, "source": r.source});
                if let Some(e) = &r.error {
                    v["error"] = json!(e);
                }
                (r.key.clone(), v)
            })
            .collect();
        let out = json!({"config_file": path.to_string_lossy(), "values": values});
        let _ = writeln!(env.stdout, "{out}");
        return Ok(());
    }
    let shown: Vec<String> = rows.iter().map(|r| text_value(&r.value)).collect();
    let key_width = rows.iter().map(|r| r.key.len()).max().unwrap_or(0);
    let value_width = shown.iter().map(|v| v.chars().count()).max().unwrap_or(0);
    let _ = writeln!(env.stdout, "config: {}", path.display());
    for (row, value) in rows.iter().zip(&shown) {
        let _ = writeln!(
            env.stdout,
            "{:<key_width$}  {value:<value_width$}  {}",
            row.key, row.source
        );
        if let Some(e) = &row.error {
            let _ = writeln!(env.stdout, "  {e}");
        }
    }
    Ok(())
}

/// A value as `show` prints it: lists comma-separated, "" quoted.
fn text_value(value: &Json) -> String {
    match value {
        Json::String(s) if s.is_empty() => "\"\"".to_string(),
        Json::String(s) => s.clone(),
        Json::Array(items) => items.iter().map(text_value).collect::<Vec<_>>().join(","),
        other => other.to_string(),
    }
}

/// Every resolved value, in the order `show` prints them.
fn rows(cfg: &Config) -> Result<Vec<Row>, CmdError> {
    let user = cfg.user_config().cloned().unwrap_or_default();
    let in_file: HashSet<String> = if cfg.user_config().is_some() {
        write::keys_in_file(&cfg.paths().config_file())
            .map_err(|e| CmdError::plain(e.to_string()))?
    } else {
        HashSet::new()
    };
    let from_file = |key: &str| {
        if in_file.contains(key) {
            "file"
        } else {
            "default"
        }
    };
    let mut rows = Vec::new();

    let url = if cfg.getenv(PROXY_URL_ENV).trim().is_empty() {
        Row::new("proxy.url", json!(user.proxy.url), from_file("proxy.url"))
    } else {
        match cfg.proxy_url() {
            Ok(url) => Row::new("proxy.url", json!(url), "env"),
            Err(e) => Row {
                error: Some(e.to_string()),
                ..Row::new("proxy.url", json!(cfg.getenv(PROXY_URL_ENV)), "env")
            },
        }
    };
    rows.push(url);
    rows.push(Row::new(
        "proxy.key_file",
        json!(cfg.proxy_key_file().to_string_lossy()),
        from_file("proxy.key_file"),
    ));
    rows.push(match cfg.proxy_key() {
        Ok(Some(key)) => {
            let source = match key.source {
                ProxyKeySource::Env => "env",
                ProxyKeySource::File(_) => "file",
            };
            Row::new("proxy.key", json!(key.masked()), source)
        }
        Ok(None) => Row::new("proxy.key", json!("missing"), "default"),
        Err(e) => Row {
            error: Some(e.to_string()),
            ..Row::new("proxy.key", json!("error"), "file")
        },
    });
    for (name, route) in [("claude", &user.proxy.claude), ("codex", &user.proxy.codex)] {
        rows.extend(route_rows(cfg, name, route, &from_file));
    }

    let models = cfg.plan_models();
    rows.push(if models.is_empty() {
        Row::new("plan.models", json!(DEFAULT_PLAN_MODELS), "default")
    } else {
        Row::new("plan.models", json!(models), "file")
    });
    for label in EFFORT_LABELS {
        let key = format!("efforts.{label}");
        rows.push(match user.efforts.get(label) {
            Some(effort) => Row::new(&key, json!(effort), "file"),
            None => Row::new(&key, json!(config::builtin_effort(label)), "default"),
        });
    }
    let reviewer = cfg.configured_security_reviewer();
    rows.push(if reviewer.is_empty() {
        Row::new("security.reviewer", json!(SECURITY_REVIEWER_K3), "default")
    } else {
        Row::new("security.reviewer", json!(reviewer), "file")
    });
    rows.push(Row::new(
        "auto_fix_critical_high",
        json!(user.auto_fix_critical_high),
        from_file("auto_fix_critical_high"),
    ));
    rows.push(Row::new(
        "claude.subscription",
        json!(user.claude.subscription),
        from_file("claude.subscription"),
    ));
    Ok(rows)
}

/// `proxy.<name>.enabled` and `.model_prefix`. `RIVAL_PROXY=off` shows the
/// route as off, from the environment.
fn route_rows(
    cfg: &Config,
    name: &str,
    route: &ProxyRoute,
    from_file: &dyn Fn(&str) -> &'static str,
) -> [Row; 2] {
    let enabled = format!("proxy.{name}.enabled");
    let prefix = format!("proxy.{name}.model_prefix");
    let enabled_row = if cfg.proxy_off() {
        Row::new(&enabled, json!(false), "env")
    } else {
        Row::new(&enabled, json!(route.enabled), from_file(&enabled))
    };
    [
        enabled_row,
        Row::new(&prefix, json!(route.model_prefix), from_file(&prefix)),
    ]
}

/// `rival config set KEY VALUE` and `rival config set --json` (a patch on
/// stdin, applied in one write).
pub fn set_action(env: &mut CmdEnv<'_>, inv: &Invocation) -> Result<(), CmdError> {
    let path = env.cfg.paths().config_file();
    let plain = |e: config::ConfigError| CmdError::plain(e.to_string());
    if inv.bool("json") {
        if !inv.args.is_empty() {
            return Err(CmdError::plain(
                "rival config set --json reads the patch from stdin; it takes no arguments",
            ));
        }
        let data = env.stdin.read_all().map_err(CmdError::plain)?;
        let patch: Json = serde_json::from_slice(&data)
            .map_err(|e| CmdError::plain(format!("parse the JSON patch: {e}")))?;
        let edits = write::edits_from_json(&patch).map_err(plain)?;
        write::apply(&path, &edits).map_err(plain)?;
        let keys: Vec<&str> = edits.iter().map(|e| e.key.as_str()).collect();
        let out = json!({"saved": path.to_string_lossy(), "keys": keys});
        let _ = writeln!(env.stdout, "{out}");
        return Ok(());
    }
    let edit: Edit = match inv.args.as_slice() {
        [key, value] => write::parse_setting(key, value).map_err(plain)?,
        other => {
            // Name the key error first: `set proxy.key` must point to
            // `key set`, not to the usage.
            if let Some(key) = other.first() {
                write::parse_setting(key, "").map_err(plain)?;
            }
            return Err(CmdError::plain(
                "usage: rival config set KEY VALUE, or rival config set --json with a JSON patch on stdin",
            ));
        }
    };
    write::apply(&path, std::slice::from_ref(&edit)).map_err(plain)?;
    let _ = writeln!(env.stdout, "{} = {}", inv.args[0], inv.args[1]);
    Ok(())
}

/// `rival config key set`: reads the key from stdin, never from an
/// argument, and writes the key file with mode 0600.
pub fn key_set_action(env: &mut CmdEnv<'_>, inv: &Invocation) -> Result<(), CmdError> {
    if !inv.args.is_empty() {
        // The argument is not shown: it may be the key.
        return Err(CmdError::plain(
            "rival config key set reads the key from stdin; do not pass it as an argument",
        ));
    }
    let data = if env.stdin.is_char_device() {
        read_hidden(env)?
    } else {
        env.stdin.read_all().map_err(CmdError::plain)?
    };
    let key = String::from_utf8(data).map_err(|_| CmdError::plain("the key is not UTF-8"))?;
    let path = env.cfg.proxy_key_file();
    write::write_key_file(&path, &key).map_err(|e| CmdError::plain(e.to_string()))?;
    let _ = writeln!(env.stdout, "saved the proxy key to {}", path.display());
    if !env.cfg.getenv(PROXY_KEY_ENV).trim().is_empty() {
        let _ = writeln!(
            env.stderr,
            "note: {PROXY_KEY_ENV} is set and wins over the key file"
        );
    }
    Ok(())
}

/// Reads one line from a terminal with echo off. Falls back to a plain
/// read when the terminal cannot go raw (`/dev/null`, for one).
fn read_hidden(env: &mut CmdEnv<'_>) -> Result<Vec<u8>, CmdError> {
    if crossterm::terminal::enable_raw_mode().is_err() {
        return env.stdin.read_all().map_err(CmdError::plain);
    }
    let _ = write!(env.stderr, "proxy key (hidden, Enter to end): ");
    let result = read_raw_line(&mut *env.stdin.reader());
    let _ = crossterm::terminal::disable_raw_mode();
    let _ = write!(env.stderr, "\r\n");
    result
}

/// One raw-mode line: ends at Enter or EOF; Backspace deletes; Ctrl-C
/// cancels.
fn read_raw_line(reader: &mut dyn BufRead) -> Result<Vec<u8>, CmdError> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match reader.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => match byte[0] {
                b'\r' | b'\n' => break,
                0x03 => return Err(CmdError::plain("cancelled")),
                0x7f | 0x08 => {
                    line.pop();
                }
                b => line.push(b),
            },
            Err(e) => return Err(CmdError::plain(format!("read /dev/stdin: {e}"))),
        }
    }
    Ok(line)
}

/// `rival config key clear`: removes the key file.
pub fn key_clear_action(env: &mut CmdEnv<'_>) -> Result<(), CmdError> {
    let path = env.cfg.proxy_key_file();
    let removed = write::clear_key_file(&path).map_err(|e| CmdError::plain(e.to_string()))?;
    if removed {
        let _ = writeln!(env.stdout, "removed {}", path.display());
    } else {
        let _ = writeln!(env.stdout, "no proxy key file at {}", path.display());
    }
    Ok(())
}

/// `rival config models [--json]`: not built yet.
pub fn models_action() -> Result<(), CmdError> {
    Err(CmdError::plain(
        "rival config models needs the proxy module (not built yet)",
    ))
}
