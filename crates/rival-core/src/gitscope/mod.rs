//! Git scope helpers. Go: `internal/gitscope`.
//!
//! Task 2.3 ported `env.go` for the provider subprocess env. This module is
//! `gitscope.go`. Its git commands inherit the caller's env unfiltered (Go
//! `exec.Command` with a nil `Env`), not [`repository_env`].

mod env;
#[cfg(test)]
mod tests;

pub use env::repository_env;

use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::config::Config;
use crate::executor::oscmd::{exit_status_text, look_path};
use crate::executor::process::{self, set_exec};
use crate::executor::subprocess::{dedup_env, spawn_error_text};
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

/// Combines two newline-separated file lists, deduplicating.
///
/// As in Go, only `b` items are checked against the set, and `b` items are
/// never added to it: duplicates inside `a`, and duplicates inside `b` that
/// are not in `a`, are kept.
fn merge_file_lists(a: &str, b: &str) -> String {
    if a.is_empty() {
        return b.to_string();
    }
    if b.is_empty() {
        return a.to_string();
    }
    // Deduplicate using a set.
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();
    for f in a.split('\n') {
        let f = f.trim();
        if !f.is_empty() {
            seen.insert(f);
            result.push(f);
        }
    }
    for f in b.split('\n') {
        let f = f.trim();
        if !f.is_empty() && !seen.contains(f) {
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

/// Go `exec.Command("git", args...)` with `Dir = workdir` and `Output()`.
///
/// - `git` is looked up in `cfg`'s `$PATH`.
/// - The env is `cfg.environ()`, unfiltered, plus `PWD=<abs workdir>` when
///   `workdir` is set (Go's `Cmd.environ`; not on Windows, which has no
///   `PWD`), then Go `dedupEnv`.
/// - stdin is the null device; stderr is captured and dropped (Go keeps it in
///   the `ExitError`, which nobody reads).
///
/// stdout is decoded as lossy UTF-8. The error text is for diagnostics only;
/// every caller drops it.
fn git_cmd(cfg: &Config, workdir: &str, args: &[&str]) -> Result<String, String> {
    let path = look_path(cfg, "git").map_err(|e| e.to_string())?;
    let mut env: Vec<OsString> = cfg.environ().to_vec();
    if !workdir.is_empty() && !cfg!(windows) {
        // Go: filepath.Abs(c.Dir); its error fails Start.
        let pwd = paths::abs(cfg.cwd(), Path::new(workdir))
            .ok_or_else(|| "getwd: no such file or directory".to_string())?;
        let mut kv = OsString::from("PWD=");
        kv.push(pwd);
        env.push(kv);
    }
    let env = dedup_env(&env)?;
    let fork_error =
        |e: &std::io::Error| format!("fork/exec {}: {}", path.display(), spawn_error_text(e));

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
        return Err(exit_status_text(out.status));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
