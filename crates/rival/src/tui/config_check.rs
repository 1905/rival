//! The config window's outside world: what it reads when it opens
//! ([`ConfigSeed::load`]) and its blocking work, which the model hands to
//! the runtime as jobs: the proxy model list behind the online dot and the
//! prefix pickers ([`run_probe`]), the model check on the draft
//! ([`run_check`], one message per row) and the save ([`run_save`]).

use std::fmt;
use std::path::{Path, PathBuf};

use rival_core::cancel::Context;
use rival_core::config::write::{self, Edit};
use rival_core::config::{
    Config, PROXY_KEY_ENV, PROXY_KEY_MISSING, PROXY_SWITCH_ENV, PROXY_URL_ENV, ProxyKeySource,
    Route, UserConfig,
};
use rival_core::executor::oscmd;
use rival_core::leakguard;
use rival_core::paths;
use rival_core::proxy::ModelsError;

use super::config_form::MODELS;
use super::model::Msg;
use crate::check::{CheckRow, Report, Target};

#[cfg(test)]
mod tests;

/// The variable whose value `d` imports into the URL field.
pub const BASE_URL_ENV: &str = "ANTHROPIC_BASE_URL";

/// A proxy key typed in the window and not saved yet. `Debug` hides it, and
/// it joins the leak guard's scrub list when it is made.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(key: &str) -> Secret {
        let key = key.trim().to_string();
        leakguard::register(&key);
        Secret(key)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    /// The last 4 characters of a key of 8 or more; "" otherwise.
    pub fn tail(&self) -> String {
        key_tail(&self.0)
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<hidden>)")
    }
}

/// The last 4 characters of a key of 8 or more; "" otherwise (as
/// `ProxyKey::masked`).
pub fn key_tail(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() < 8 {
        return String::new();
    }
    chars[chars.len() - 4..].iter().collect()
}

/// The saved proxy key as the window shows it. The secret itself stays in
/// the config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyStatus {
    Missing,
    Set {
        /// The last 4 characters; "" for a short key.
        tail: String,
        /// From `RIVAL_PROXY_KEY`, which wins over the file.
        env: bool,
        /// The key file's permission bits (Unix); `None` for an env key.
        mode: Option<u32>,
    },
    /// The key file cannot be used (other users can read it, say).
    Error(String),
}

/// Everything the window reads when it opens. The runtime loads it once
/// ([`ConfigSeed::load`]); a save updates it in place. Tests build it by
/// hand, so frames never show a temp path.
#[derive(Debug, Clone)]
pub struct ConfigSeed {
    /// The config with the environment; the draft replaces its user config.
    pub cfg: Config,
    /// `config.yaml`.
    pub path: PathBuf,
    /// The path as the title shows it, with `~` for the home directory.
    pub path_shown: String,
    /// The file text; `None` when there is no file.
    pub text: Option<String>,
    /// The saved config (the default when there is no file).
    pub saved: UserConfig,
    pub key: KeyStatus,
    /// Where `config key set` writes the key.
    pub key_path: PathBuf,
    pub key_path_shown: String,
    /// Whether the runtime of each row of [`MODELS`] is on `PATH`.
    pub installed: Vec<bool>,
    /// `ANTHROPIC_BASE_URL`, for `d` on the URL field.
    pub base_url_env: String,
    /// `RIVAL_PROXY_URL`, which wins over `proxy.url`.
    pub url_env: String,
    /// `RIVAL_PROXY=off`.
    pub proxy_off_env: bool,
    /// The last probe or check number. It outlives a window, so a late
    /// answer to a closed window never matches the next one.
    pub serial: u64,
}

impl ConfigSeed {
    /// Reads the config file, the key and `PATH`. Small and local: the
    /// runtime calls it before the first frame.
    pub fn load(cfg: &Config) -> ConfigSeed {
        let path = cfg.paths().config_file();
        let home = cfg.getenv(paths::HOME_VAR).to_string();
        let text = std::fs::read_to_string(&path).ok();
        let key_path = cfg.proxy_key_file();
        ConfigSeed {
            path_shown: shorten(&path, &home),
            text,
            saved: cfg.user_config().cloned().unwrap_or_default(),
            key: key_status(cfg),
            key_path_shown: shorten(&key_path, &home),
            key_path,
            installed: MODELS
                .iter()
                .map(|m| m.binaries.iter().any(|b| oscmd::look_path(cfg, b).is_ok()))
                .collect(),
            base_url_env: cfg.getenv(BASE_URL_ENV).trim().to_string(),
            url_env: cfg.getenv(PROXY_URL_ENV).trim().to_string(),
            proxy_off_env: cfg
                .getenv(PROXY_SWITCH_ENV)
                .trim()
                .eq_ignore_ascii_case("off"),
            serial: 0,
            cfg: cfg.clone(),
            path,
        }
    }

    /// The seed after a save: the new config, its text and, when a key was
    /// written, the key's status.
    pub fn saved(&mut self, saved: &Saved) {
        if let Some(user) = &saved.user {
            self.saved = user.clone();
            self.cfg = self.cfg.clone().with_user_config(Some(user.clone()));
        }
        self.text = saved.text.clone().or(self.text.take());
        if let Some(key) = &saved.key
            && !matches!(self.key, KeyStatus::Set { env: true, .. })
        {
            self.key = KeyStatus::Set {
                tail: key.tail(),
                env: false,
                mode: Some(0o600),
            };
        }
    }
}

/// `path` with the home directory shown as `~`.
pub fn shorten(path: &Path, home: &str) -> String {
    let shown = path.to_string_lossy().into_owned();
    if home.is_empty() {
        return shown;
    }
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.to_string_lossy()),
        Err(_) => shown,
    }
}

fn key_status(cfg: &Config) -> KeyStatus {
    match cfg.proxy_key() {
        Ok(None) => KeyStatus::Missing,
        Err(e) => KeyStatus::Error(e.to_string()),
        Ok(Some(key)) => {
            let env = key.source == ProxyKeySource::Env;
            KeyStatus::Set {
                tail: key_tail(key.secret()),
                env,
                mode: match &key.source {
                    ProxyKeySource::File(path) => file_mode(path),
                    ProxyKeySource::Env => None,
                },
            }
        }
    }
}

#[cfg(unix)]
fn file_mode(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path)
        .ok()
        .map(|m| m.permissions().mode() & 0o777)
}

#[cfg(not(unix))]
fn file_mode(_path: &Path) -> Option<u32> {
    None
}

/// `GET /v1/models`. Production is `proxy::models`; tests fake it.
pub type ModelsFn = fn(&Route) -> Result<Vec<String>, ModelsError>;

/// The model check. Production is `check::run_check`; tests fake it.
pub type CheckFn = fn(&Context, &Config, &[Target], &(dyn Fn(&CheckRow) + Sync)) -> Report;

/// The proxy's model list for the draft URL and key.
#[derive(Debug, Clone)]
pub struct ProbeRequest {
    pub seq: u64,
    /// The draft config: its URL and key are asked.
    pub cfg: Config,
}

/// Requests are equal when they are the same probe: the pool merges by it.
impl PartialEq for ProbeRequest {
    fn eq(&self, other: &Self) -> bool {
        self.seq == other.seq
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeResult {
    pub seq: u64,
    /// The ids, or the error line.
    pub result: Result<Vec<String>, String>,
}

/// Runs a probe. No URL or no key is an error without a call.
pub fn run_probe(req: ProbeRequest, models: ModelsFn) -> ProbeResult {
    let call = || -> Result<Vec<String>, String> {
        let url = req.cfg.proxy_url().map_err(|e| e.to_string())?;
        if url.is_empty() {
            return Err("no URL".to_string());
        }
        let key = req
            .cfg
            .proxy_key()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| PROXY_KEY_MISSING.to_string())?;
        models(&Route::new(url, "", key.secret())).map_err(|e| e.to_string())
    };
    ProbeResult {
        seq: req.seq,
        result: call(),
    }
}

/// One model check on the draft.
#[derive(Debug, Clone)]
pub struct CheckRequest {
    pub run: u64,
    pub cfg: Config,
    pub targets: Vec<Target>,
    /// Cancelled by `x`, by closing the window and by the exit.
    pub ctx: Context,
}

impl PartialEq for CheckRequest {
    fn eq(&self, other: &Self) -> bool {
        self.run == other.run
    }
}

/// Runs a check: each row goes to `emit` as it finishes, and the report is
/// the returned message.
pub fn run_check(req: CheckRequest, check: CheckFn, emit: &(dyn Fn(Msg) + Sync)) -> Msg {
    let run = req.run;
    let on_row = |row: &CheckRow| {
        emit(Msg::CheckRow {
            run,
            row: Box::new(row.clone()),
        })
    };
    let report = check(&req.ctx, &req.cfg, &req.targets, &on_row);
    Msg::CheckDone {
        run,
        report: Box::new(report),
    }
}

/// `s`: the draft's edits through the config writer and a new key through
/// the key file writer, as `rival config set --json` and `rival config key
/// set` do.
#[derive(Debug, Clone, PartialEq)]
pub struct SaveRequest {
    pub path: PathBuf,
    pub edits: Vec<Edit>,
    pub key: Option<Secret>,
    pub key_path: PathBuf,
}

/// A finished save.
#[derive(Debug, Clone, PartialEq)]
pub struct Saved {
    /// The new config; `None` when only the key changed.
    pub user: Option<UserConfig>,
    /// The new file text.
    pub text: Option<String>,
    /// The key that was written.
    pub key: Option<Secret>,
}

pub type SaveResult = Result<Saved, String>;

/// Writes the config, then the key. A key error after a good config write
/// says that the config was saved.
pub fn run_save(req: SaveRequest) -> SaveResult {
    let user = if req.edits.is_empty() {
        None
    } else {
        Some(write::apply(&req.path, &req.edits).map_err(|e| e.to_string())?)
    };
    if let Some(key) = &req.key {
        write::write_key_file(&req.key_path, key.expose()).map_err(|e| {
            if user.is_some() {
                format!("config saved, but the key was not: {e}")
            } else {
                e.to_string()
            }
        })?;
    }
    Ok(Saved {
        text: user
            .as_ref()
            .and_then(|_| std::fs::read_to_string(&req.path).ok()),
        user,
        key: req.key,
    })
}

/// Whether `RIVAL_PROXY_KEY` is set, so a key typed here would not be used.
pub fn env_key_wins(cfg: &Config) -> bool {
    !cfg.getenv(PROXY_KEY_ENV).trim().is_empty()
}
