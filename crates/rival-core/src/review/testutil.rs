//! Shared test setup: a [`Config`] rooted in a temp home, so no test reads
//! the real `~/.rival`.

use std::collections::HashMap;
use std::path::Path;

use tempfile::TempDir;

use crate::config::Config;
use crate::paths::{self, Paths};

/// A config whose home (and `~/.rival`) is `home`, with `extra` env vars.
pub(crate) fn config_in(home: &Path, extra: &[(&str, &str)]) -> Config {
    let mut env = HashMap::from([(
        paths::HOME_VAR.to_string(),
        home.to_str().expect("utf-8 temp path").to_string(),
    )]);
    env.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    Config::new(Paths::from_home(home), env, None)
}

/// A fresh temp home with no config file, and its config.
pub(crate) fn temp_config() -> (TempDir, Config) {
    let home = tempfile::tempdir().expect("temp home");
    let cfg = config_in(home.path(), &[]);
    (home, cfg)
}
