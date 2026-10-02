use std::io::Cursor;
use std::path::{Path, PathBuf};

use rival_core::skills;

use super::*;
use crate::testutil::{FakeStdin, Fixture, execute, no_mr, with_env};

fn reader(answers: &str) -> Cursor<Vec<u8>> {
    Cursor::new(answers.as_bytes().to_vec())
}

/// Runs [`install_skills`] and returns what it printed.
fn install(target: &SkillTarget, force: bool, answers: &str) -> String {
    let mut out = Vec::new();
    install_skills(target, force, &mut reader(answers), &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

fn version(name: &str) -> String {
    read_embedded_skill(name).unwrap().1
}

/// What the target should hold for `name`: the Claude file, or its Codex
/// rendering.
fn want_content(target: &SkillTarget, name: &str) -> Vec<u8> {
    let (content, version) = read_embedded_skill(name).unwrap();
    if target.host == "codex" {
        return skills::codex_skill(name, &version).unwrap();
    }
    content
}

fn skill_file(target: &SkillTarget, name: &str) -> PathBuf {
    target.base.join(name).join("SKILL.md")
}

fn claude_target(base: &Path) -> SkillTarget {
    SkillTarget {
        host: "claude",
        base: base.to_path_buf(),
    }
}

// Go: TestSkillTargets.
#[test]
fn skill_targets_by_host() {
    for (target, codex, want) in [
        ("auto", false, &["claude"][..]),
        ("auto", true, &["claude", "codex"]),
        ("claude", true, &["claude"]),
        ("codex", false, &["codex"]),
        ("all", false, &["claude", "codex"]),
    ] {
        let home = tempfile::tempdir().unwrap();
        let targets = skill_targets(home.path(), target, codex).unwrap();
        let mut hosts = Vec::new();
        for t in &targets {
            hosts.push(t.host);
            let folder = if t.host == "codex" {
                ".agents"
            } else {
                ".claude"
            };
            assert_eq!(
                t.base,
                home.path().join(folder).join("skills"),
                "{target}/{codex}: wrong destination"
            );
        }
        assert_eq!(hosts, want, "{target}/{codex}");
    }
    let home = tempfile::tempdir().unwrap();
    assert_eq!(
        skill_targets(home.path(), "typo", true).unwrap_err(),
        "unknown install target \"typo\"; use auto, claude, codex, or all"
    );
}

/// `filepath.Join` cleans the home path.
#[test]
fn skill_targets_clean_the_home() {
    let targets = skill_targets(Path::new("/h/./x/"), "all", false).unwrap();
    assert_eq!(targets[0].base, Path::new("/h/x/.claude/skills"));
    assert_eq!(targets[1].base, Path::new("/h/x/.agents/skills"));
}

// Go: TestDetectCodex. PATH and CODEX_HOME are passed in, not set.
#[test]
fn detect_codex_signals() {
    for signal in [
        "absent",
        "cli",
        "config",
        "custom-home",
        "user-app",
        "system-app",
    ] {
        let (home, bin, apps) = (
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
        );
        let mut codex_home = String::new();
        let dir = match signal {
            "cli" => {
                let path = bin.path().join("codex");
                fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
                }
                None
            }
            "config" => Some(home.path().join(".codex")),
            "custom-home" => {
                let dir = home.path().join("custom-codex");
                codex_home = dir.to_str().unwrap().to_string();
                Some(dir)
            }
            "user-app" => Some(home.path().join("Applications").join("Codex.app")),
            "system-app" => Some(apps.path().join("Codex.app")),
            _ => None,
        };
        if let Some(dir) = dir {
            fs::create_dir_all(dir).unwrap();
        }
        let got = detect_codex(
            home.path(),
            apps.path(),
            bin.path().as_os_str(),
            &codex_home,
        );
        assert_eq!(got, signal != "absent", "detection for {signal} = {got}");
    }
}

/// A file named `Codex.app` or `.codex` is not a signal; a non-executable
/// `codex` on PATH is not either.
#[test]
fn detect_codex_ignores_plain_files() {
    let (home, bin, apps) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    fs::write(home.path().join(".codex"), "").unwrap();
    fs::write(apps.path().join("Codex.app"), "").unwrap();
    fs::write(bin.path().join("codex"), "").unwrap();
    let codex_home = home.path().join(".codex");
    assert!(!detect_codex(
        home.path(),
        apps.path(),
        bin.path().as_os_str(),
        codex_home.to_str().unwrap()
    ));
}

// Go: TestInstallBothHostsAndPreserveUnrelatedSkills.
#[test]
fn install_both_hosts_and_preserve_unrelated_skills() {
    let home = tempfile::tempdir().unwrap();
    for target in skill_targets(home.path(), "all", false).unwrap() {
        let unrelated = target.base.join("rival-personal").join("SKILL.md");
        write_skill(unrelated.parent().unwrap(), &unrelated, b"my skill").unwrap();
        install(&target, false, "");
        for name in skills::NAMES {
            let got = fs::read(skill_file(&target, name)).unwrap();
            let want = want_content(&target, name);
            assert!(
                got == want && parse_version(&String::from_utf8_lossy(&got)) == version(name),
                "incorrect {} skill {name}",
                target.host
            );
        }
        assert_eq!(
            fs::read_to_string(&unrelated).unwrap(),
            "my skill",
            "unrelated skill changed"
        );
    }
}

// Go: TestInstallOverwriteAndBufferedAnswers.
#[test]
fn install_overwrite_and_buffered_answers() {
    let dir = tempfile::tempdir().unwrap();
    let target = SkillTarget {
        host: "codex",
        base: dir.path().to_path_buf(),
    };
    install(&target, false, "");
    let paths = [
        skill_file(&target, skills::NAMES[0]),
        skill_file(&target, skills::NAMES[1]),
    ];
    let old = b"---\nversion: old\n---\ncustom";
    for path in &paths {
        fs::write(path, old).unwrap();
    }
    install(&target, false, "n\nn\n");
    for path in &paths {
        assert_eq!(
            fs::read(path).unwrap(),
            old,
            "declined update overwrote skill"
        );
    }
    install(&target, false, "y\ny\n");
    for path in &paths {
        assert_ne!(
            fs::read(path).unwrap(),
            old,
            "buffered confirmation was lost"
        );
    }
    let data = fs::read(&paths[0]).unwrap();
    let mut custom = data.clone();
    custom.extend_from_slice(b"\nlocal customization");
    fs::write(&paths[0], &custom).unwrap();
    install(&target, false, "");
    assert_eq!(
        fs::read(&paths[0]).unwrap(),
        custom,
        "same-version customization overwritten"
    );
    install(&target, true, "");
    assert_eq!(
        fs::read(&paths[0]).unwrap(),
        data,
        "force did not refresh skill"
    );
}

// Go: TestRemoveSkillDirsByHashRemovesOnlyExactMatches.
#[test]
fn remove_skill_dirs_by_hash_removes_only_exact_matches() {
    let base = tempfile::tempdir().unwrap();
    let retired = "rival-retired-fixture";
    let kept = "rival-current-fixture";
    for name in [retired, kept] {
        fs::create_dir(base.path().join(name)).unwrap();
    }
    let sum = sha256_hex(retired.as_bytes());
    let removed = remove_skill_dirs_by_hash(base.path(), &[sum.as_str()]).unwrap();
    assert_eq!(removed, 1);
    assert!(
        !base.path().join(retired).exists(),
        "retired directory still exists"
    );
    assert!(
        base.path().join(kept).is_dir(),
        "current directory was removed"
    );
}

#[test]
fn remove_skill_dirs_by_hash_missing_base_removes_nothing() {
    let base = tempfile::tempdir().unwrap();
    let missing = base.path().join("nope");
    assert_eq!(
        remove_skill_dirs_by_hash(&missing, &RETIRED_SKILL_NAME_HASHES).unwrap(),
        0
    );
    let file = base.path().join("file");
    fs::write(&file, "").unwrap();
    assert!(remove_skill_dirs_by_hash(&file, &RETIRED_SKILL_NAME_HASHES).is_err());
}

// Go: TestRetiredSkillCleanupHashesStayConfigured.
#[test]
fn retired_skill_cleanup_hashes_stay_configured() {
    assert_eq!(RETIRED_SKILL_NAME_HASHES.len(), 2);
    assert_ne!(RETIRED_SKILL_NAME_HASHES[0], RETIRED_SKILL_NAME_HASHES[1]);
    for hash in RETIRED_SKILL_NAME_HASHES {
        assert!(
            hash.len() == 64
                && hash
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
            "{hash}"
        );
    }
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

// Go: TestInstallRemovesRetiredReviewSkill. An installed rival-review
// (megareview, removed 2026-09-26) is cleaned up on both hosts.
#[test]
fn install_removes_retired_review_skill() {
    let home = tempfile::tempdir().unwrap();
    for target in skill_targets(home.path(), "all", false).unwrap() {
        let stale = target.base.join("rival-review").join("SKILL.md");
        write_skill(
            stale.parent().unwrap(),
            &stale,
            b"---\nname: rival-review\n---\n",
        )
        .unwrap();
        let out = install(&target, false, "");
        assert!(
            !stale.parent().unwrap().exists(),
            "{}: rival-review was not removed by install",
            target.host
        );
        assert!(out.contains("  🗑 deprecated skill removed\n"), "{out}");
        assert!(out.ends_with(", 1 removed\n"), "{out}");
    }
}

/// The whole output of a fresh install.
#[test]
fn fresh_install_transcript() {
    let dir = tempfile::tempdir().unwrap();
    let target = claude_target(&dir.path().join("skills"));
    let mut want = format!("Installing claude skills to {}\n\n", target.base.display());
    for name in skills::NAMES {
        want.push_str(&format!("  ✓ {name} — installed (v{})\n", version(name)));
    }
    want.push_str("\nDone: 9 installed, 0 updated, 0 up to date, 0 removed\n");
    assert_eq!(install(&target, false, ""), want);

    // Same version again: every skill is up to date and the reader is not
    // touched.
    let mut want = format!("Installing claude skills to {}\n\n", target.base.display());
    for name in skills::NAMES {
        want.push_str(&format!(
            "  · {name} — already up to date (v{})\n",
            version(name)
        ));
    }
    want.push_str("\nDone: 0 installed, 0 updated, 9 up to date, 0 removed\n");
    let mut answers = reader("y\n");
    let mut out = Vec::new();
    install_skills(&target, false, &mut answers, &mut out).unwrap();
    assert_eq!(String::from_utf8(out).unwrap(), want);
    assert_eq!(answers.position(), 0, "an up-to-date install read stdin");
}

/// Prompt lines, answers and counters for declined, accepted, forced and
/// unknown-version updates.
#[test]
fn update_prompt_transcript() {
    let dir = tempfile::tempdir().unwrap();
    let target = claude_target(dir.path());
    install(&target, false, "");
    let [first, second, third] = [skills::NAMES[0], skills::NAMES[1], skills::NAMES[2]];
    fs::write(skill_file(&target, first), "---\nversion: 0.1\n---\n").unwrap();
    fs::write(skill_file(&target, second), "---\nversion: 0.2\n---\n").unwrap();
    // No frontmatter version: "unknown".
    fs::write(skill_file(&target, third), "no frontmatter").unwrap();

    // Answers: upper-case "YES" with spaces accepts, "no" declines, EOF
    // without a newline declines.
    let out = install(&target, false, "  YES \nno\n");
    let lines: Vec<&str> = out.lines().collect();
    let (v1, v2, v3) = (version(first), version(second), version(third));
    assert_eq!(
        lines[2],
        format!("  ? {first} — update v0.1 → v{v1}? [y/N]   ✓ {first} — updated (v0.1 → v{v1})")
    );
    assert_eq!(
        lines[3],
        format!("  ? {second} — update v0.2 → v{v2}? [y/N]     skipped")
    );
    assert_eq!(
        lines[4],
        format!("  ? {third} — update vunknown → v{v3}? [y/N]     skipped")
    );
    assert_eq!(
        out.lines().last().unwrap(),
        "Done: 0 installed, 1 updated, 8 up to date, 0 removed"
    );
    assert_eq!(
        fs::read(skill_file(&target, first)).unwrap(),
        want_content(&target, first)
    );

    // --force never prompts and rewrites even same-version skills.
    let out = install(&target, true, "");
    assert!(!out.contains("[y/N]"), "{out}");
    assert!(
        out.contains(&format!("  ✓ {first} — updated (v{v1} → v{v1})\n")),
        "{out}"
    );
    assert!(
        out.contains(&format!("  ✓ {second} — updated (v0.2 → v{v2})\n")),
        "{out}"
    );
    assert!(
        out.ends_with("Done: 0 installed, 9 updated, 0 up to date, 0 removed\n"),
        "{out}"
    );
}

/// Go keeps one `bufio.Reader` for every target: answers buffered while
/// prompting for Claude still reach the Codex prompts.
#[test]
fn one_reader_serves_every_target() {
    let home = tempfile::tempdir().unwrap();
    let targets = skill_targets(home.path(), "all", false).unwrap();
    install_targets(&targets, false, &mut reader(""), &mut Vec::new()).unwrap();
    for target in &targets {
        fs::write(
            skill_file(target, skills::NAMES[0]),
            "---\nversion: old\n---\n",
        )
        .unwrap();
    }
    let mut out = Vec::new();
    install_targets(&targets, false, &mut reader("n\ny\n"), &mut out).unwrap();
    let out = String::from_utf8(out).unwrap();
    assert_eq!(
        fs::read_to_string(skill_file(&targets[0], skills::NAMES[0])).unwrap(),
        "---\nversion: old\n---\n",
        "claude answer was not 'n'"
    );
    assert_eq!(
        fs::read(skill_file(&targets[1], skills::NAMES[0])).unwrap(),
        want_content(&targets[1], skills::NAMES[0]),
        "codex answer was lost"
    );
    assert_eq!(out.matches("Installing ").count(), 2, "{out}");
}

/// Deprecated and hash-retired skills are removed and counted; unrelated
/// skills stay.
#[test]
fn cleanup_counts_deprecated_and_retired() {
    let dir = tempfile::tempdir().unwrap();
    let target = claude_target(dir.path());
    for name in ["rival-sol", "rival-astra", "rival-hashed", "rival-personal"] {
        fs::create_dir_all(dir.path().join(name)).unwrap();
    }
    // A deprecated name that is a file is removed too.
    fs::write(dir.path().join("rival-kimi"), "").unwrap();
    let hash = sha256_hex(b"rival-hashed");
    let mut out = Vec::new();
    install_skills_with(&target, false, &mut reader(""), &mut out, &[hash.as_str()]).unwrap();
    let out = String::from_utf8(out).unwrap();
    assert_eq!(
        out.matches("  🗑 deprecated skill removed\n").count(),
        3,
        "{out}"
    );
    assert!(out.contains("  🗑 1 retired skill(s) removed\n\nDone: 9 installed, 0 updated, 0 up to date, 4 removed\n"), "{out}");
    for gone in ["rival-sol", "rival-astra", "rival-kimi", "rival-hashed"] {
        assert!(!dir.path().join(gone).exists(), "{gone}");
    }
    assert!(dir.path().join("rival-personal").is_dir());
}

#[cfg(unix)]
#[test]
fn cleanup_failure_is_reported_and_not_counted() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("skills");
    let target = claude_target(&base);
    install(&target, false, "");
    fs::create_dir_all(base.join("rival-sol").join("inner")).unwrap();
    // The entry cannot be unlinked from a read-only parent.
    fs::set_permissions(&base, fs::Permissions::from_mode(0o555)).unwrap();
    let out = install(&target, false, "");
    fs::set_permissions(&base, fs::Permissions::from_mode(0o755)).unwrap();
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        return; // root ignores the mode bits
    }
    assert!(
        out.contains(
            "  ✗ deprecated skill cleanup failed — check permissions in the skills directory\n"
        ),
        "{out}"
    );
    assert!(
        out.ends_with("Done: 0 installed, 0 updated, 9 up to date, 0 removed\n"),
        "{out}"
    );
}

/// Go's `os.IsNotExist` is false for ENOTDIR, so a skills path under a
/// file fails on the read, with Go's error text.
#[cfg(unix)]
#[test]
fn base_under_a_file_fails_like_go() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("skills");
    fs::write(&base, "").unwrap();
    let mut out = Vec::new();
    let err = install_skills(&claude_target(&base), false, &mut reader(""), &mut out).unwrap_err();
    let file = base.join(skills::NAMES[0]).join("SKILL.md");
    assert_eq!(
        err,
        format!("read {0}: open {0}: not a directory", file.display())
    );
}

#[cfg(unix)]
#[test]
fn write_errors_name_the_path() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("file");
    fs::write(&blocker, "").unwrap();
    let err = write_skill(&blocker, &blocker.join("SKILL.md"), b"x").unwrap_err();
    assert_eq!(
        err,
        format!("mkdir {0}: mkdir {0}: not a directory", blocker.display())
    );
    let skill_dir = dir.path().join("skill");
    let file = skill_dir.join("SKILL.md");
    fs::create_dir_all(&file).unwrap();
    let err = write_skill(&skill_dir, &file, b"x").unwrap_err();
    assert_eq!(
        err,
        format!("write {0}: open {0}: is a directory", file.display())
    );
}

/// A SKILL.md that is a directory fails Go's read, not its open.
#[cfg(unix)]
#[test]
fn skill_file_that_is_a_directory_fails_on_read() {
    let dir = tempfile::tempdir().unwrap();
    let target = claude_target(dir.path());
    let file = skill_file(&target, skills::NAMES[0]);
    fs::create_dir_all(&file).unwrap();
    let mut out = Vec::new();
    let err = install_skills(&target, false, &mut reader(""), &mut out).unwrap_err();
    assert_eq!(
        err,
        format!("read {0}: read {0}: is a directory", file.display())
    );
}

#[test]
fn parse_version_reads_the_frontmatter_only() {
    for (content, want) in [
        ("---\nname: x\nversion: 1.2.3\n---\nbody", "1.2.3"),
        ("---\nmetadata:\n  version: 9.8.7  \n---\n", "9.8.7"),
        ("  ---  \nversion:4\n---\n", "4"),
        ("---\nname: x\n---\nversion: 2\n", "unknown"),
        ("version: 1\n---\nversion: 2\n---\n", "2"),
        ("no frontmatter", "unknown"),
        ("---\r\nversion: 5\r\n---\r\n", "5"),
        ("", "unknown"),
    ] {
        assert_eq!(parse_version(content), want, "{content:?}");
    }
}

/// `rival install` through the root, into the fixture's HOME only.
#[test]
fn install_command_writes_under_home_not_rival_home() {
    let rival_home = tempfile::tempdir().unwrap();
    let fix = Fixture::with(&[("RIVAL_HOME", rival_home.path().to_str().unwrap())], None);
    let home = PathBuf::from(fix.cfg.getenv("HOME"));
    let (code, stdout, stderr) = execute(
        &fix,
        &mut FakeStdin::new(""),
        &["install", "--target", "claude"],
    );
    assert_eq!((code, stderr.as_str()), (0, ""));
    let base = home.join(".claude").join("skills");
    assert!(
        stdout.starts_with(&format!(
            "Installing claude skills to {}\n\n",
            base.display()
        )),
        "{stdout}"
    );
    assert!(
        stdout.ends_with("Done: 9 installed, 0 updated, 0 up to date, 0 removed\n"),
        "{stdout}"
    );
    for name in skills::NAMES {
        assert!(base.join(name).join("SKILL.md").is_file(), "{name}");
    }
    assert!(!home.join(".agents").exists());
    assert_eq!(fs::read_dir(rival_home.path()).unwrap().count(), 0);

    let (code, _, stderr) = execute(
        &fix,
        &mut FakeStdin::new(""),
        &["install", "--target", "typo"],
    );
    assert_eq!(code, 1);
    assert_eq!(
        stderr,
        "unknown install target \"typo\"; use auto, claude, codex, or all\n"
    );
}

/// Runs [`run_install`] over `fix` with an empty system applications dir.
fn run(
    fix: &Fixture,
    force: bool,
    target: &str,
    answers: &str,
) -> (Result<(), CmdError>, String, String) {
    let apps = tempfile::tempdir().unwrap();
    let mut stdin = FakeStdin::new(answers);
    let prepare = no_mr();
    let outcome = with_env(fix, &mut stdin, &*prepare, |env| {
        run_install(env, force, target, apps.path())
    });
    (outcome.result, outcome.stdout, outcome.stderr)
}

#[test]
fn auto_without_codex_installs_claude_and_says_so_on_stderr() {
    let bin = tempfile::tempdir().unwrap();
    let fix = Fixture::with(&[("PATH", bin.path().to_str().unwrap())], None);
    let home = PathBuf::from(fix.cfg.getenv("HOME"));
    let (result, stdout, stderr) = run(&fix, false, "auto", "");
    result.unwrap();
    assert_eq!(
        stderr,
        "Codex not detected; skipped (use --target codex to install explicitly).\n"
    );
    assert_eq!(stdout.matches("Installing ").count(), 1, "{stdout}");
    assert!(home.join(".claude/skills/rival-codex/SKILL.md").is_file());
    assert!(!home.join(".agents").exists());
}

#[test]
fn auto_with_codex_installs_both_hosts() {
    let bin = tempfile::tempdir().unwrap();
    let codex_home = tempfile::tempdir().unwrap();
    let fix = Fixture::with(
        &[
            ("PATH", bin.path().to_str().unwrap()),
            ("CODEX_HOME", codex_home.path().to_str().unwrap()),
        ],
        None,
    );
    let home = PathBuf::from(fix.cfg.getenv("HOME"));
    let (result, stdout, stderr) = run(&fix, false, "auto", "");
    result.unwrap();
    assert_eq!(stderr, "");
    assert!(stdout.contains(&format!(
        "Installing codex skills to {}\n",
        home.join(".agents/skills").display()
    )));
    for name in skills::NAMES {
        let codex = fs::read(home.join(".agents/skills").join(name).join("SKILL.md")).unwrap();
        assert_eq!(codex, skills::codex_skill(name, &version(name)).unwrap());
    }
}

/// An explicit target never prints the detection note.
#[test]
fn explicit_targets_skip_the_note() {
    let bin = tempfile::tempdir().unwrap();
    let fix = Fixture::with(&[("PATH", bin.path().to_str().unwrap())], None);
    for target in ["claude", "codex", "all"] {
        let (result, _, stderr) = run(&fix, true, target, "");
        result.unwrap();
        assert_eq!(stderr, "", "{target}");
    }
}

#[test]
fn empty_home_fails_before_any_target() {
    let fix = Fixture::with(&[("HOME", "")], None);
    let (result, stdout, stderr) = run(&fix, false, "typo", "");
    let want = if cfg!(windows) {
        "get home dir: %userprofile% is not defined"
    } else {
        "get home dir: $HOME is not defined"
    };
    assert_eq!(result.unwrap_err(), CmdError::plain(want));
    assert_eq!((stdout.as_str(), stderr.as_str()), ("", ""));
}
