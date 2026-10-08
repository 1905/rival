//! `--workdir` resolution shared by every command that takes it. Go:
//! `cmd/workdir.go`.

use std::io::Write;
use std::path::Path;

use rival_core::config::Config;
use rival_core::paths;

use crate::root::CmdError;

#[cfg(test)]
mod tests;

/// Turns the `--workdir` flag into an absolute, cleaned
/// path that must name an existing directory. It runs once at each command
/// entry, before preflight and session creation: the executors set the
/// child's cwd to the workdir and also pass it to the provider (codex -C,
/// opencode --dir, grok --cwd), so a relative path would be applied twice
/// ("v3/" -> v3/v3/), and sessions would record "." instead of the project
/// path. Relative paths resolve against `cfg`'s working directory.
pub fn resolve_workdir(cfg: &Config, raw: &str) -> Result<String, String> {
    // Go filepath.Abs on Windows always calls syscall.FullPath, whose
    // UTF16PtrFromString rejects a NUL with EINVAL.
    if cfg!(windows) && raw.contains('\0') {
        return Err(format!("resolve workdir {:?}: invalid argument", raw));
    }
    let Some(abs) = paths::abs(cfg.cwd(), Path::new(raw)) else {
        return Err(format!("resolve workdir {:?}: {}", raw, getwd_error()));
    };
    let shown = abs.to_string_lossy().into_owned();
    // Go os.Stat on Unix: BytePtrFromString rejects a NUL with EINVAL
    // before the syscall (std's own error has no errno).
    if shown.contains('\0') {
        return Err(format!(
            "cannot read workdir {shown}: {STAT_OP} {shown}: invalid argument"
        ));
    }
    match std::fs::metadata(&abs) {
        // ENOENT on Unix (ENOTDIR is "cannot read"); on Windows the codes
        // that std maps to NotFound.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(format!("workdir not found: {shown}"))
        }
        Err(e) => Err(format!(
            "cannot read workdir {shown}: {STAT_OP} {shown}: {e}"
        )),
        Ok(meta) if !meta.is_dir() => Err(format!("workdir is not a directory: {shown}")),
        Ok(_) => Ok(shown),
    }
}

/// The `PathError` op of a failed Go `os.Stat` that is not "not exist".
/// On Windows (`os/stat_windows.go`) such a `GetFileAttributesEx` error
/// falls through to `CreateFile`, whose error is returned; std's
/// `metadata` returns the error of its own `CreateFileW`. Go's rarer
/// `FindFirstFile`, `GetFileType` and `GetFileInformationByHandle` ops
/// are not distinguished.
const STAT_OP: &str = if cfg!(windows) { "CreateFile" } else { "stat" };

/// Go `os.Getwd`'s error, re-read: the config snapshot keeps only the
/// failure.
pub(crate) fn getwd_error() -> String {
    match std::env::current_dir() {
        Err(e) => format!("getwd: {e}"),
        Ok(_) => "getwd: no such file or directory".to_string(),
    }
}

/// [`resolve_workdir`] for a command action. A
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
