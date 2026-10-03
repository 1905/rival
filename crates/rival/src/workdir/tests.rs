//! Go: `cmd/workdir_test.go`, `cmd/workdir_session_test.go`.

use super::*;

use crate::testutil::{Fixture, fake_run, no_mr, run_command_with};

/// A root with `v3/` and `plain.txt`; `cwd` is the root as a canonical
/// path (Go's `os.Getwd` after `t.Chdir`, with macOS /var resolved).
fn layout() -> (tempfile::TempDir, std::path::PathBuf) {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("v3")).unwrap();
    std::fs::write(root.path().join("plain.txt"), "x").unwrap();
    let cwd = root.path().canonicalize().unwrap();
    (root, cwd)
}

// ---- Go TestResolveWorkdir ----

#[test]
fn resolve_workdir_cases() {
    let (root, cwd) = layout();
    let fix = Fixture::with(&[], Some(cwd.clone()));
    let s = |p: std::path::PathBuf| p.to_str().unwrap().to_string();
    let abs_v3 = s(root.path().join("v3"));
    let unclean_abs = format!("{}/v3//.", root.path().display());
    let ok = [
        (
            "relative subdir with trailing slash",
            "v3/",
            s(cwd.join("v3")),
        ),
        ("dot", ".", s(cwd.clone())),
        ("unclean relative", "./v3/../v3", s(cwd.join("v3"))),
        ("absolute", abs_v3.as_str(), abs_v3.clone()),
        // pflag accepts an empty value; Go's filepath.Abs("") is the cwd.
        ("empty", "", s(cwd.clone())),
        ("unclean absolute", unclean_abs.as_str(), abs_v3.clone()),
    ];
    for (name, raw, want) in ok {
        assert_eq!(resolve_workdir(&fix.cfg, raw), Ok(want), "{name}");
    }
    let errs = [
        (
            "missing",
            "nope",
            format!("workdir not found: {}", cwd.join("nope").display()),
        ),
        (
            "file",
            "plain.txt",
            format!(
                "workdir is not a directory: {}",
                cwd.join("plain.txt").display()
            ),
        ),
        // ENOTDIR is not "not exist" in Go: the stat error is kept. Windows
        // reports ERROR_PATH_NOT_FOUND, which is.
        (
            "through a file",
            "plain.txt/x",
            if cfg!(windows) {
                format!(
                    "workdir not found: {}",
                    cwd.join("plain.txt").join("x").display()
                )
            } else {
                format!(
                    "cannot read workdir {p}: stat {p}: not a directory",
                    p = cwd.join("plain.txt/x").display()
                )
            },
        ),
    ];
    for (name, raw, want) in errs {
        assert_eq!(resolve_workdir(&fix.cfg, raw), Err(want), "{name}");
    }
}

/// A NUL is EINVAL: Unix os.Stat reports it as a stat error; Windows
/// filepath.Abs (syscall.FullPath) fails first.
#[test]
fn resolve_workdir_nul_is_invalid_argument() {
    let (_root, cwd) = layout();
    let fix = Fixture::with(&[], Some(cwd.clone()));
    let want = if cfg!(windows) {
        r#"resolve workdir "v3\x00x": invalid argument"#.to_string()
    } else {
        format!(
            "cannot read workdir {p}: stat {p}: invalid argument",
            p = cwd.join("v3\0x").display()
        )
    };
    assert_eq!(resolve_workdir(&fix.cfg, "v3\0x"), Err(want));
}

/// `|` is a legal Unix name, so the path is just missing. On Windows both
/// GetFileAttributesEx and the CreateFile fallback fail with
/// ERROR_INVALID_NAME, which is not "not exist": Go reports CreateFile.
#[test]
fn resolve_workdir_invalid_windows_name() {
    let (_root, cwd) = layout();
    let fix = Fixture::with(&[], Some(cwd.clone()));
    let p = cwd.join("a|b");
    let want = if cfg!(windows) {
        format!(
            "cannot read workdir {p}: CreateFile {p}: \
             The filename, directory name, or volume label syntax is incorrect.",
            p = p.display()
        )
    } else {
        format!("workdir not found: {}", p.display())
    };
    assert_eq!(resolve_workdir(&fix.cfg, "a|b"), Err(want));
}

#[test]
fn relative_workdir_without_a_cwd_fails_but_absolute_works() {
    let (root, _) = layout();
    let fix = Fixture::with(&[], None);
    let err = resolve_workdir(&fix.cfg, "v3").unwrap_err();
    assert!(err.starts_with("resolve workdir \"v3\": getwd: "), "{err}");
    let abs = root.path().join("v3");
    assert_eq!(
        resolve_workdir(&fix.cfg, abs.to_str().unwrap()),
        Ok(abs.to_str().unwrap().to_string())
    );
}

#[test]
fn or_exit_prints_to_stdout_and_exits_one() {
    let (_root, cwd) = layout();
    let fix = Fixture::with(&[], Some(cwd.clone()));
    let mut out = Vec::new();
    let err = resolve_workdir_or_exit(&fix.cfg, "nope", &mut out).unwrap_err();
    let want = format!("workdir not found: {}", cwd.join("nope").display());
    assert_eq!(err, CmdError::exit(1, want.clone()));
    assert_eq!(String::from_utf8(out).unwrap(), format!("{want}\n"));
}

// ---- Go TestCommandRelativeWorkdirStoresAbsolutePath ----

/// A relative --workdir is resolved once at the command entry: the provider
/// gets an absolute dir (so codex -C / opencode --dir / grok --cwd cannot
/// re-apply it on top of the child cwd) and the session records the
/// absolute path, not ".".
#[test]
fn command_relative_workdir_stores_absolute_path() {
    let (_root, cwd) = layout();
    let fix = Fixture::with(&[], Some(cwd.clone()));
    let want = cwd.join("v3").to_str().unwrap().to_string();
    let f = fake_run("answer\n");
    let out = run_command_with(&fix, &f, "explain the auth flow", "v3/", &*no_mr());
    out.result.unwrap();
    assert_eq!(f.borrow().workdir, want, "provider workdir");
    let sessions = fix.sessions();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].work_dir, want, "session work_dir");
}
