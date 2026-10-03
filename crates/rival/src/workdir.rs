//! `--workdir` resolution shared by every command that takes it. Go:
//! `cmd/workdir.go`.

use std::io::Write;
use std::path::Path;

use rival_core::config::Config;
use rival_core::{gostd, paths};

use crate::root::CmdError;

#[cfg(test)]
mod tests;

/// Go `resolveWorkdir`: turns the `--workdir` flag into an absolute, cleaned
/// path that must name an existing directory. It runs once at each command
/// entry, before preflight and session creation: the executors set the
/// child's cwd to the workdir and also pass it to the provider (codex -C,
/// opencode --dir, grok --cwd), so a relative path would be applied twice
/// ("v3/" -> v3/v3/), and sessions would record "." instead of the project
/// path. Relative paths resolve against `cfg`'s working directory.
pub fn resolve_workdir(cfg: &Config, raw: &str) -> Result<String, String> {
    let Some(abs) = paths::abs(cfg.cwd(), Path::new(raw)) else {
        return Err(format!(
            "resolve workdir {}: {}",
            gostd::quote(raw),
            getwd_error()
        ));
    };
    let shown = abs.to_string_lossy().into_owned();
    match std::fs::metadata(&abs) {
        // Go os.IsNotExist: ENOENT on Unix (ENOTDIR is "cannot read"); on
        // Windows its own three codes, narrower than std's NotFound.
        Err(e) if gostd::is_not_exist(&e) => Err(format!("workdir not found: {shown}")),
        Err(e) => Err(format!(
            "cannot read workdir {shown}: stat {shown}: {}",
            gostd::os_error_text(&e)
        )),
        Ok(meta) if !meta.is_dir() => Err(format!("workdir is not a directory: {shown}")),
        Ok(_) => Ok(shown),
    }
}

/// Go `os.Getwd`'s error, re-read: the config snapshot keeps only the
/// failure.
pub(crate) fn getwd_error() -> String {
    match std::env::current_dir() {
        Err(e) => format!("getwd: {}", gostd::os_error_text(&e)),
        Ok(_) => "getwd: no such file or directory".to_string(),
    }
}

/// Go `resolveWorkdirOrExit`: [`resolve_workdir`] for a command action. A
/// bad workdir is printed to `stdout`, where the calling skill captures it,
/// and ends the command with exit code 1 — the same contract as an
/// invalid-argument error.
pub fn resolve_workdir_or_exit(
    cfg: &Config,
    raw: &str,
    stdout: &mut dyn Write,
) -> Result<String, CmdError> {
    resolve_workdir(cfg, raw).map_err(|err| {
        let _ = writeln!(stdout, "{err}");
        CmdError::exit(1, err)
    })
}
