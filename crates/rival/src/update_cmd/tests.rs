//! Go `cmd/update_test.go`, plus the error branches of `updateToVersion`.
//! Every brew and rival here is a shell script in a temp dir; `PATH` holds
//! only that dir. No real Homebrew, network or install target is touched.

use super::*;

use std::path::PathBuf;

use crate::testutil::{FakeStdin, Fixture, no_mr, with_env};

/// Writes an executable script.
fn script(path: &Path, body: &str) -> PathBuf {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    path.to_path_buf()
}

struct Out {
    stdout: String,
    stderr: String,
    result: Result<(), String>,
}

/// Runs `body` until it no longer fails with Linux ETXTBSY: another test
/// thread may fork while a just-written script is still open for writing.
fn run_busy(fix: &Fixture, body: impl Fn(&mut CmdEnv<'_>) -> Result<(), String>) -> Out {
    run_busy_with(fix, "", body)
}

fn run_busy_with(
    fix: &Fixture,
    input: &str,
    body: impl Fn(&mut CmdEnv<'_>) -> Result<(), String>,
) -> Out {
    for _ in 0..20 {
        let prepare = no_mr();
        let mut stdin = FakeStdin::new(input);
        let mut result = Ok(());
        let o = with_env(fix, &mut stdin, &*prepare, |env| {
            result = body(env);
            Ok(())
        });
        let busy = result
            .as_ref()
            .err()
            .is_some_and(|e| e.contains("text file busy"));
        if !busy {
            return Out {
                stdout: o.stdout,
                stderr: o.stderr,
                result,
            };
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("script stayed busy");
}

fn update(fix: &Fixture, apps: &Path, current: &str, latest: &str) -> Out {
    run_busy(fix, |env| {
        update_to_version(env, ChildIo::Capture, current, latest, apps).map_err(|e| e.message)
    })
}

/// Go `TestUpdateInstallsFromSelectedBinary`.
#[test]
fn update_installs_from_selected_binary() {
    let fix = Fixture::new();
    let dir = tempfile::tempdir().unwrap();
    let binary = script(
        &dir.path().join("new rival"),
        "#!/bin/sh\nprintf '%s\\n' \"$@\"\nprintf 'new embedded skills\\n'\n",
    );
    let bin = binary.to_str().unwrap().to_string();
    let o = run_busy(&fix, |env| {
        install_updated_skills(env, ChildIo::Capture, &bin)
    });
    assert_eq!(o.result, Ok(()));
    assert_eq!(
        o.stdout,
        "install\n--force\n--target\nauto\nnew embedded skills\n"
    );

    script(&binary, "#!/bin/sh\necho oops >&2\nexit 7\n");
    let o = run_busy(&fix, |env| {
        install_updated_skills(env, ChildIo::Capture, &bin)
    });
    assert_eq!(o.result, Err("exit status 7".to_string()));
    assert_eq!(o.stderr, "oops\n");
}

/// A fixture whose `PATH` is only `bin`, with a brew script there and a
/// prefix holding `bin/rival`.
struct Brew {
    fix: Fixture,
    _dir: tempfile::TempDir,
    bin: PathBuf,
    prefix: PathBuf,
    apps: PathBuf,
}

fn brew(brew_body: &str, rival_body: &str) -> Brew {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    let prefix = dir.path().join("prefix");
    let apps = dir.path().join("Applications");
    std::fs::create_dir_all(&apps).unwrap();
    script(&bin.join("brew"), brew_body);
    script(&prefix.join("bin").join("rival"), rival_body);
    let fix = Fixture::with(
        &[
            ("PATH", bin.to_str().unwrap()),
            ("RIVAL_TEST_BREW_PREFIX", prefix.to_str().unwrap()),
        ],
        None,
    );
    Brew {
        fix,
        _dir: dir,
        bin,
        prefix,
        apps,
    }
}

const BREW_OK: &str = "#!/bin/sh\ncase \"$1\" in\nupgrade) exit 0;;\n--prefix) printf '%s\\n' \"$RIVAL_TEST_BREW_PREFIX\";;\n*) exit 9;;\nesac\n";
const NEW_RIVAL: &str = "#!/bin/sh\nprintf 'new release skills\\n'\n";

/// Go `TestUpdateUsesHomebrewBinaryNotOldEmbeddedSkills`, with the whole
/// transcript.
#[test]
fn update_uses_homebrew_binary_not_old_embedded_skills() {
    let b = brew(BREW_OK, NEW_RIVAL);
    let o = update(&b.fix, &b.apps, "old", "new");
    assert_eq!(o.result, Ok(()));
    assert_eq!(
        o.stdout,
        "vold → vnew\n\nUpgrading via Homebrew...\n\nUpdating skills...\nnew release skills\n\n✓ Updated to vnew\n"
    );
    assert_eq!(o.stderr, "");
}

/// Go leaves brew's Stdin nil (the null device) and gives the upgraded
/// installer `cmd.InOrStdin()`: brew must not eat the installer's input.
#[test]
fn brew_gets_no_stdin_and_the_installer_gets_the_commands() {
    let dir = tempfile::tempdir().unwrap();
    let rec = dir.path().join("rec");
    std::fs::create_dir(&rec).unwrap();
    let body = format!(
        "#!/bin/sh\n/bin/cat > '{r}/brew-'\"$1\"\ncase \"$1\" in\nupgrade) exit 1;;\nreinstall) exit 0;;\n--prefix) printf '%s\\n' \"$RIVAL_TEST_BREW_PREFIX\";;\nesac\n",
        r = rec.display()
    );
    let rival = format!("#!/bin/sh\n/bin/cat > '{}/installer'\n", rec.display());
    let b = brew(&body, &rival);
    let o = run_busy_with(&b.fix, "y\nyes\n", |env| {
        update_to_version(env, ChildIo::Capture, "1", "2", &b.apps).map_err(|e| e.message)
    });
    assert_eq!(o.result, Ok(()));
    for name in ["brew-upgrade", "brew-reinstall", "brew---prefix"] {
        assert_eq!(
            std::fs::read_to_string(rec.join(name)).unwrap(),
            "",
            "{name}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(rec.join("installer")).unwrap(),
        "y\nyes\n"
    );
}

#[test]
fn failed_upgrade_falls_back_to_reinstall() {
    let body = "#!/bin/sh\ncase \"$1\" in\nupgrade) echo 'upgrade said no' >&2; exit 1;;\nreinstall) echo reinstalled;;\n--prefix) echo 'prefix noise' >&2; printf '  %s \\n' \"$RIVAL_TEST_BREW_PREFIX\";;\n*) exit 9;;\nesac\n";
    let b = brew(body, NEW_RIVAL);
    let o = update(&b.fix, &b.apps, "1.0.0", "1.1.0");
    assert_eq!(o.result, Ok(()));
    assert_eq!(
        o.stdout,
        "v1.0.0 → v1.1.0\n\nUpgrading via Homebrew...\nbrew upgrade failed, trying reinstall...\nreinstalled\n\nUpdating skills...\nnew release skills\n\n✓ Updated to v1.1.0\n"
    );
    // brew's own stderr passes through; --prefix stderr stays hidden.
    assert_eq!(o.stderr, "upgrade said no\n");
}

#[test]
fn update_error_branches() {
    let both_fail = "#!/bin/sh\nexit 9\n";
    let b = brew(both_fail, NEW_RIVAL);
    let o = update(&b.fix, &b.apps, "1", "2");
    assert_eq!(o.result, Err("brew reinstall: exit status 9".to_string()));
    assert_eq!(
        o.stdout,
        "v1 → v2\n\nUpgrading via Homebrew...\nbrew upgrade failed, trying reinstall...\n"
    );

    let prefix_fails = "#!/bin/sh\ncase \"$1\" in\n--prefix) exit 4;;\nesac\n";
    let b = brew(prefix_fails, NEW_RIVAL);
    let o = update(&b.fix, &b.apps, "1", "2");
    assert_eq!(
        o.result,
        Err("locate upgraded rival: exit status 4".to_string())
    );
    assert!(o.stdout.ends_with("\nUpdating skills...\n"), "{}", o.stdout);

    let b = brew(BREW_OK, "#!/bin/sh\nexit 7\n");
    let o = update(&b.fix, &b.apps, "1", "2");
    assert_eq!(o.result, Err("install skills: exit status 7".to_string()));

    // A prefix without bin/rival: Go skips LookPath for a path and fails at
    // the exec.
    let b = brew(BREW_OK, NEW_RIVAL);
    std::fs::remove_file(b.prefix.join("bin").join("rival")).unwrap();
    let o = update(&b.fix, &b.apps, "1", "2");
    let want = format!(
        "install skills: fork/exec {}: no such file or directory",
        b.prefix.join("bin").join("rival").display()
    );
    assert_eq!(o.result, Err(want));

    // No brew on PATH at all.
    let b = brew(BREW_OK, NEW_RIVAL);
    std::fs::remove_file(b.bin.join("brew")).unwrap();
    let o = update(&b.fix, &b.apps, "1", "2");
    assert_eq!(
        o.result,
        Err("brew reinstall: exec: \"brew\": executable file not found in $PATH".to_string())
    );
}

/// Go `TestCurrentVersionRefreshesSkillsForNewCodexInstall`.
#[test]
fn current_version_refreshes_skills_for_new_codex_install() {
    let fix = Fixture::new();
    let home = PathBuf::from(fix.cfg.getenv("HOME"));
    std::fs::create_dir(home.join(".codex")).unwrap();
    let apps = tempfile::tempdir().unwrap();
    let o = update(&fix, apps.path(), "current", "current");
    assert_eq!(o.result, Ok(()));
    assert!(
        o.stdout
            .starts_with("already on latest (vcurrent)\nInstalling claude skills to ")
    );
    assert!(
        home.join(".agents/skills/rival-claude/SKILL.md").exists(),
        "codex skills installed"
    );
    assert!(home.join(".claude/skills/rival-claude/SKILL.md").exists());
    // Forced: a second run overwrites without reading stdin, and no
    // "Codex not detected" note is printed.
    let o = update(&fix, apps.path(), "current", "current");
    assert_eq!(o.result, Ok(()));
    assert_eq!(o.stderr, "");
}

#[test]
fn current_version_without_codex_installs_claude_only() {
    let fix = Fixture::new();
    let home = PathBuf::from(fix.cfg.getenv("HOME"));
    let apps = tempfile::tempdir().unwrap();
    let o = update(&fix, apps.path(), "dev", "dev");
    assert_eq!(o.result, Ok(()));
    assert_eq!(o.stderr, "");
    assert!(home.join(".claude/skills/rival-claude/SKILL.md").exists());
    assert!(!home.join(".agents").exists());
}

#[test]
fn current_version_needs_a_home() {
    let fix = Fixture::with(&[("HOME", "")], None);
    let apps = tempfile::tempdir().unwrap();
    let o = update(&fix, apps.path(), "dev", "dev");
    assert_eq!(o.result, Err("$HOME is not defined".to_string()));
    assert_eq!(o.stdout, "already on latest (vdev)\n");
}
