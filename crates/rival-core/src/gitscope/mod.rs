//! Git scope helpers.
//!
//! The `env` submodule drops git's repository overrides from an env. The git
//! commands in this module run with [`repository_env`] of the caller's env,
//! so an inherited `GIT_DIR` (as in a git hook) cannot change the scope.

mod env;
#[cfg(test)]
mod tests;

pub use env::repository_env;

use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::config::Config;
use crate::executor::oscmd::{fork_error, look_path};
use crate::executor::process::{self, set_exec};
use crate::executor::subprocess::dedup_env;
use crate::paths;

/// Detects which files to review based on git state.
/// Priority: dirty files (staged+unstaged+untracked) → last commit → empty
/// string. Returns a newline-separated file list, or "" if not a git repo or
/// no changes found.
pub fn resolve(cfg: &Config, workdir: &str) -> String {
    // Try dirty files first: tracked changes (staged + unstaged modifications).
    let tracked = git_cmd(cfg, workdir, &["diff", "--name-only", "HEAD"]).unwrap_or_default();
    // Also catch untracked new files (not yet git-added).
    let untracked = git_cmd(
        cfg,
        workdir,
        &["ls-files", "--others", "--exclude-standard"],
    )
    .unwrap_or_default();

    let files = merge_file_lists(tracked.trim(), untracked.trim());
    if !files.is_empty() {
        return files;
    }

    // Working tree is clean — try last commit.
    if let Ok(out) = git_cmd(cfg, workdir, &["diff", "--name-only", "HEAD~1", "HEAD"]) {
        let files = out.trim();
        if !files.is_empty() {
            return files.to_string();
        }
    }

    String::new()
}

/// Combines two newline-separated file lists: items are trimmed, blank
/// lines dropped, and every duplicate removed (inside one list and across
/// both). The first occurrence keeps its place.
fn merge_file_lists(a: &str, b: &str) -> String {
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();
    for f in a.split('\n').chain(b.split('\n')) {
        let f = f.trim();
        if !f.is_empty() && seen.insert(f) {
            result.push(f);
        }
    }
    result.join("\n")
}

/// Returns `git diff --stat` output, mirroring [`resolve`]'s mode detection.
/// Returns "" if not a git repo or no changes.
pub fn diff_stat(cfg: &Config, workdir: &str) -> String {
    let tracked = git_cmd(cfg, workdir, &["diff", "--name-only", "HEAD"]).unwrap_or_default();
    let untracked = git_cmd(
        cfg,
        workdir,
        &["ls-files", "--others", "--exclude-standard"],
    )
    .unwrap_or_default();
    let has_dirty = !tracked.trim().is_empty() || !untracked.trim().is_empty();

    if has_dirty {
        let stat = git_cmd(cfg, workdir, &["diff", "--stat", "HEAD"]).unwrap_or_default();
        return stat.trim().to_string();
    }

    match git_cmd(cfg, workdir, &["diff", "--stat", "HEAD~1", "HEAD"]) {
        Ok(stat) => stat.trim().to_string(),
        Err(_) => String::new(),
    }
}

/// Runs `git` with `args` in `workdir` and returns its stdout.
///
/// - `git` is looked up in `cfg`'s `$PATH`.
/// - The env is `cfg.environ()` without git's repository overrides
///   ([`repository_env`]), plus `PWD=<abs workdir>` when
///   `workdir` is set (not on Windows, which has no `PWD`), then
///   duplicate names are removed.
/// - stdin is the null device; stderr is captured and dropped (the error text never
///   includes it).
///
/// stdout is decoded as lossy UTF-8. The error text is for diagnostics only;
/// every caller drops it.
fn git_cmd(cfg: &Config, workdir: &str, args: &[&str]) -> Result<String, String> {
    let path = look_path(cfg, "git").map_err(|e| e.to_string())?;
    let mut env: Vec<OsString> = repository_env(cfg.environ());
    if !workdir.is_empty() && !cfg!(windows) {
        // Make the workdir absolute; a failure fails the spawn.
        let pwd = paths::abs(cfg.cwd(), Path::new(workdir))
            .ok_or_else(|| "getwd: no such file or directory".to_string())?;
        let mut kv = OsString::from("PWD=");
        kv.push(pwd);
        env.push(kv);
    }
    let env = dedup_env(&env)?;
    let fork_error = |e: &std::io::Error| fork_error(&path, e);

    let program = process::program_in_dir(&path, Path::new(workdir)).map_err(|e| fork_error(&e))?;
    let mut cmd = Command::new(program);
    set_exec(&mut cmd, &path, "git", args, &env).map_err(|e| fork_error(&e))?;
    if !workdir.is_empty() {
        cmd.current_dir(workdir);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // `output()` with the fork lock released before the reads and the wait.
    let out = process::spawn(&mut cmd)
        .and_then(std::process::Child::wait_with_output)
        .map_err(|e| fork_error(&e))?;
    if !out.status.success() {
        return Err(out.status.to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
