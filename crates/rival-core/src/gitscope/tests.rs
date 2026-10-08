//! Git scope tests, plus pins of the source behavior (env, PWD, merge
//! quirks, DiffStat).
//!
//! The tests run the installed `git` in temp repos. The child env is
//! explicit: the process `PATH` (read only), a temp `HOME`, and
//! `GIT_CONFIG_NOSYSTEM=1`. Nothing reads or changes this worktree's repo.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::*;
use crate::executor::testutil::path_str;
#[cfg(unix)]
use crate::executor::testutil::write_exe;
use crate::paths::Paths;

struct Fixture {
    home: tempfile::TempDir,
    vars: HashMap<String, String>,
}

impl Fixture {
    fn new() -> Fixture {
        let home = tempfile::tempdir().unwrap();
        let mut vars = HashMap::new();
        let path = std::env::var("PATH").unwrap_or_default();
        vars.insert("PATH".to_string(), path);
        vars.insert("HOME".to_string(), path_str(home.path()));
        // Windows reads the profile from USERPROFILE; keep it private too.
        vars.insert("USERPROFILE".to_string(), path_str(home.path()));
        // Git for Windows needs the system root; read only.
        #[cfg(windows)]
        if let Ok(root) = std::env::var("SYSTEMROOT") {
            vars.insert("SYSTEMROOT".to_string(), root);
        }
        vars.insert("GIT_CONFIG_NOSYSTEM".to_string(), "1".to_string());
        Fixture { home, vars }
    }

    fn config(&self) -> Config {
        self.config_with(Vec::new(), Some(self.home.path().to_path_buf()))
    }

    /// `extra` entries are appended to the child env after the base vars.
    fn config_with(&self, extra: Vec<OsString>, cwd: Option<PathBuf>) -> Config {
        let cfg = Config::new(Paths::from_home(self.home.path()), self.vars.clone(), cwd);
        let mut environ = cfg.environ().to_vec();
        environ.extend(extra);
        cfg.with_environ(environ)
    }

    /// Init, `checkout -b main`, one commit of a.go.
    fn init_repo(&self) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        self.git(dir.path(), &["init"]);
        self.git(dir.path(), &["checkout", "-b", "main"]);
        std::fs::write(dir.path().join("a.go"), "package a").unwrap();
        self.git(dir.path(), &["add", "."]);
        self.git(dir.path(), &["commit", "-m", "init"]);
        dir
    }

    fn git(&self, dir: &Path, args: &[&str]) {
        let mut cmd = Command::new("git");
        cmd.args(args)
            .current_dir(dir)
            .env_clear()
            .envs(&self.vars)
            .envs([
                ("GIT_AUTHOR_NAME", "test"),
                ("GIT_AUTHOR_EMAIL", "test@test"),
                ("GIT_COMMITTER_NAME", "test"),
                ("GIT_COMMITTER_EMAIL", "test@test"),
            ])
            // `output()`'s defaults, spawned under the fork lock.
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let out = process::spawn(&mut cmd)
            .and_then(std::process::Child::wait_with_output)
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

fn s(p: &Path) -> String {
    path_str(p)
}

#[test]
fn resolve_dirty_files() {
    let fx = Fixture::new();
    let dir = fx.init_repo();
    std::fs::write(dir.path().join("b.go"), "package b").unwrap();
    fx.git(dir.path(), &["add", "b.go"]);
    assert_eq!(resolve(&fx.config(), &s(dir.path())), "b.go");
}

#[test]
fn resolve_last_commit() {
    let fx = Fixture::new();
    let dir = fx.init_repo();
    std::fs::write(dir.path().join("c.go"), "package c").unwrap();
    fx.git(dir.path(), &["add", "."]);
    fx.git(dir.path(), &["commit", "-m", "add c"]);
    assert_eq!(resolve(&fx.config(), &s(dir.path())), "c.go");
}

#[test]
fn resolve_untracked_files() {
    let fx = Fixture::new();
    let dir = fx.init_repo();
    // Create a new file without staging — should still be detected.
    std::fs::write(dir.path().join("new.go"), "package new").unwrap();
    assert_eq!(resolve(&fx.config(), &s(dir.path())), "new.go");
}

#[test]
fn resolve_modified_unstaged() {
    let fx = Fixture::new();
    let dir = fx.init_repo();
    // Modify an existing tracked file without staging.
    std::fs::write(dir.path().join("a.go"), "package a // modified").unwrap();
    assert_eq!(resolve(&fx.config(), &s(dir.path())), "a.go");
}

#[test]
fn resolve_single_commit_clean() {
    // Repo with exactly one commit, clean working tree → "" (HEAD~1 missing).
    let fx = Fixture::new();
    let dir = fx.init_repo();
    assert_eq!(resolve(&fx.config(), &s(dir.path())), "");
}

#[test]
fn resolve_not_git_repo() {
    let fx = Fixture::new();
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(resolve(&fx.config(), &s(dir.path())), "");
}

// Rust-only pins.

#[test]
fn resolve_merges_tracked_then_untracked() {
    let fx = Fixture::new();
    let dir = fx.init_repo();
    std::fs::write(dir.path().join("a.go"), "package a // modified").unwrap();
    std::fs::write(dir.path().join("z.go"), "package z").unwrap();
    std::fs::write(dir.path().join("b.go"), "package b").unwrap();
    assert_eq!(resolve(&fx.config(), &s(dir.path())), "a.go\nb.go\nz.go");
}

#[test]
fn merge_file_lists_dedups_and_trims() {
    assert_eq!(merge_file_lists("", "x\ny"), "x\ny");
    assert_eq!(merge_file_lists("x\ny", ""), "x\ny");
    // Items are trimmed and blank lines dropped.
    assert_eq!(merge_file_lists(" a \n\nb", "c\n \nb"), "a\nb\nc");
    // Duplicates inside a, inside b, and across both are removed.
    assert_eq!(merge_file_lists("a\na", "b\nb\na"), "a\nb");
    // A single list gets the same trim and dedup.
    assert_eq!(merge_file_lists("", "x\n\nx "), "x");
    assert_eq!(merge_file_lists("y\ny", ""), "y");
}

#[test]
fn diff_stat_follows_resolve_modes() {
    let fx = Fixture::new();
    let cfg = fx.config();

    // Not a repo, and a clean one-commit repo: "".
    let plain = tempfile::tempdir().unwrap();
    assert_eq!(diff_stat(&cfg, &s(plain.path())), "");
    let dir = fx.init_repo();
    let wd = s(dir.path());
    assert_eq!(diff_stat(&cfg, &wd), "");

    // Untracked-only changes are dirty, but `git diff --stat HEAD` is empty.
    std::fs::write(dir.path().join("new.go"), "package new").unwrap();
    assert_eq!(resolve(&cfg, &wd), "new.go");
    assert_eq!(diff_stat(&cfg, &wd), "");

    // Tracked change: the stat of the working tree against HEAD.
    std::fs::write(dir.path().join("a.go"), "package a\n// modified\n").unwrap();
    let stat = diff_stat(&cfg, &wd);
    assert!(stat.starts_with("a.go | "), "{stat:?}");
    assert!(
        stat.ends_with("1 file changed, 2 insertions(+), 1 deletion(-)"),
        "{stat:?}"
    );

    // Clean after a second commit: the last commit's stat.
    std::fs::remove_file(dir.path().join("new.go")).unwrap();
    fx.git(dir.path(), &["commit", "-am", "two"]);
    let stat = diff_stat(&cfg, &wd);
    assert!(stat.starts_with("a.go | "), "{stat:?}");
}

/// An inherited `GIT_DIR` (as in a git hook) does not point scope
/// detection at another repository.
#[test]
fn inherited_git_dir_does_not_change_scope() {
    let fx = Fixture::new();
    let a = fx.init_repo();
    let b = tempfile::tempdir().unwrap();
    fx.git(b.path(), &["init"]);
    std::fs::write(b.path().join("b.go"), "package b").unwrap();
    fx.git(b.path(), &["add", "."]);
    fx.git(b.path(), &["commit", "-m", "b"]);
    std::fs::write(a.path().join("a2.go"), "package a").unwrap();

    assert_eq!(resolve(&fx.config(), &s(a.path())), "a2.go");
    let extra = vec![
        OsString::from(format!("GIT_DIR={}", s(&b.path().join(".git")))),
        OsString::from(format!("GIT_WORK_TREE={}", s(b.path()))),
        OsString::from(format!(
            "GIT_INDEX_FILE={}",
            s(&b.path().join(".git/index"))
        )),
    ];
    let cfg = fx.config_with(extra, None);
    assert_eq!(resolve(&cfg, &s(a.path())), "a2.go");
    assert_eq!(diff_stat(&cfg, &s(a.path())), "");
}

#[test]
fn missing_git_resolves_to_empty() {
    let mut fx = Fixture::new();
    let empty = tempfile::tempdir().unwrap();
    fx.vars.insert("PATH".to_string(), s(empty.path()));
    let cfg = fx.config();
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(resolve(&cfg, &s(dir.path())), "");
    assert_eq!(diff_stat(&cfg, &s(dir.path())), "");
    let path_var = if cfg!(windows) { "%PATH%" } else { "$PATH" };
    assert_eq!(
        git_cmd(&cfg, &s(dir.path()), &["status"]).unwrap_err(),
        format!(r#"exec: "git": executable file not found in {path_var}"#)
    );
}

/// No `PWD` is added on Windows: an inherited one passes
/// through unchanged, and none appears otherwise. The fake git is a `.cmd`
/// found through `PATHEXT`; in a batch file an unset `%PWD%` is empty.
#[cfg(windows)]
#[test]
fn git_cmd_adds_no_pwd_on_windows() {
    let mut fx = Fixture::new();
    let bin = tempfile::tempdir().unwrap();
    std::fs::write(
        bin.path().join("git.cmd"),
        "@echo off\r\necho [%PWD%]^|%*\r\n",
    )
    .unwrap();
    fx.vars.insert("PATH".to_string(), s(bin.path()));
    let work = tempfile::tempdir().unwrap();

    let cfg = fx.config_with(Vec::new(), Some(work.path().to_path_buf()));
    let got = git_cmd(&cfg, &s(work.path()), &["diff", "--stat"]).unwrap();
    assert_eq!(got.trim_end(), "[]|diff --stat");

    let cfg = fx.config_with(vec![OsString::from(r"PWD=C:\stale")], None);
    let got = git_cmd(&cfg, &s(work.path()), &["x"]).unwrap();
    assert_eq!(got.trim_end(), r"[C:\stale]|x");
}

/// With a workdir set and no explicit env, `PWD=<Abs(Dir)>` is appended,
/// so it wins over an inherited PWD. The fake git sees argv0 `git`, the
/// args, and the env without repository overrides such as `GIT_DIR`.
#[cfg(unix)]
#[test]
fn git_cmd_env_argv_and_pwd() {
    let mut fx = Fixture::new();
    let bin = tempfile::tempdir().unwrap();
    write_exe(
        &bin.path().join("git"),
        "#!/bin/sh\nprintf '%s|%s|%s|%s\\n' \"$0\" \"$*\" \"$PWD\" \"$GIT_DIR\"\n",
    );
    fx.vars.insert("PATH".to_string(), s(bin.path()));

    // The workdir is a symlink, so sh keeps the given PWD instead of getcwd.
    let real = tempfile::tempdir().unwrap();
    let links = tempfile::tempdir().unwrap();
    let link = links.path().join("wd");
    std::os::unix::fs::symlink(real.path(), &link).unwrap();
    let extra = vec![
        OsString::from("PWD=/stale"),
        OsString::from("GIT_DIR=/elsewhere"),
    ];
    let cfg = fx.config_with(extra, None);

    let got = retry_busy(|| git_cmd(&cfg, &s(&link), &["diff", "--stat", "HEAD"]));
    let want = format!(
        "{}|diff --stat HEAD|{}|\n",
        s(&bin.path().join("git")),
        s(&link)
    );
    assert_eq!(got.unwrap(), want);

    // A relative workdir is made absolute against cfg.cwd; without a cwd it
    // fails before the spawn with the getwd error.
    assert_eq!(
        git_cmd(&cfg, ".", &["status"]).unwrap_err(),
        "getwd: no such file or directory"
    );
    let here = std::env::current_dir().unwrap();
    let here_link = links.path().join("here");
    std::os::unix::fs::symlink(&here, &here_link).unwrap();
    let cfg = fx.config_with(Vec::new(), Some(here_link.clone()));
    let got = retry_busy(|| git_cmd(&cfg, ".", &["x"])).unwrap();
    assert!(got.contains(&format!("|x|{}|", s(&here_link))), "{got:?}");

    // A failing git is an error with the exit status text.
    write_exe(&bin.path().join("git"), "#!/bin/sh\necho out\nexit 3\n");
    let got = retry_busy(|| git_cmd(&cfg, "", &["x"]));
    assert_eq!(got.unwrap_err(), "exit status: 3");
}

/// Linux ETXTBSY: another test thread may fork while a fake is open for
/// writing (see `executor::testutil::retry_busy`).
#[cfg(unix)]
fn retry_busy(mut f: impl FnMut() -> Result<String, String>) -> Result<String, String> {
    crate::executor::testutil::retry_busy(&mut f, |r| match r {
        Err(e) => e.clone(),
        Ok(_) => String::new(),
    })
}
