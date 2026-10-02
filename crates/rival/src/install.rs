//! `rival install`: writes the embedded skills for Claude Code and Codex.
//! Go: `cmd/install.go`.
//!
//! Skills go under the user's home (`$HOME`), never under `RIVAL_HOME`,
//! which only moves Rival's own state.

use std::ffi::OsStr;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};

use rival_core::config::Config;
use rival_core::executor::look_path;
use rival_core::paths::{self, HOME_VAR};
use rival_core::{gostd, skills};
use sha2::{Digest, Sha256};

use crate::root::{CmdEnv, CmdError};
use crate::tree::Invocation;

#[cfg(test)]
mod tests;

/// Go `retiredSkillNameHashes`: lets upgrades remove two retired
/// integration skills without retaining their obsolete public names
/// anywhere in the shipped tree. The values are SHA-256(name), not content
/// hashes.
pub const RETIRED_SKILL_NAME_HASHES: [&str; 2] = [
    "206a1c0a9997719ba41cc76d4e2e2699ff4d2000fd94fd1bad99ba5d73ddc98a",
    "75160929d947197a4444be684d0c9a67784cc4ebd84b45cd1de2234a6981056a",
];

/// Go `codexInstalled`'s system applications directory.
const SYSTEM_APPLICATIONS: &str = "/Applications";

/// `rival install [--force] [--target auto|claude|codex|all]`.
pub fn install_action(env: &mut CmdEnv<'_>, inv: &Invocation) -> Result<(), CmdError> {
    run_install(
        env,
        inv.bool("force"),
        &inv.string("target"),
        Path::new(SYSTEM_APPLICATIONS),
    )
}

/// Go `runInstall`, with the system applications directory injected.
fn run_install(
    env: &mut CmdEnv<'_>,
    force: bool,
    target: &str,
    applications: &Path,
) -> Result<(), CmdError> {
    let home = user_home_dir(env.cfg).map_err(|e| CmdError::plain(format!("get home dir: {e}")))?;
    let home = Path::new(home);
    let has_codex = detect_codex(
        home,
        applications,
        OsStr::new(env.cfg.getenv("PATH")),
        env.cfg.getenv("CODEX_HOME"),
    );
    let targets = skill_targets(home, target, has_codex).map_err(CmdError::plain)?;
    if target == "auto" && targets.len() == 1 {
        // cobra `cmd.Println` writes to stderr: nothing set this command's
        // output.
        let _ = writeln!(
            env.stderr,
            "Codex not detected; skipped (use --target codex to install explicitly)."
        );
    }
    let mut reader = env.stdin.reader();
    install_targets(&targets, force, &mut *reader, env.stdout).map_err(CmdError::plain)
}

/// Go `os.UserHomeDir`.
fn user_home_dir(cfg: &Config) -> Result<&str, &'static str> {
    match cfg.getenv(HOME_VAR) {
        "" if cfg!(windows) => Err("%userprofile% is not defined"),
        "" => Err("$HOME is not defined"),
        home => Ok(home),
    }
}

/// Go `skillTarget`: a skill host and its skills directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillTarget {
    pub host: &'static str,
    pub base: PathBuf,
}

/// Go `skillTargets`.
pub fn skill_targets(
    home: &Path,
    target: &str,
    has_codex: bool,
) -> Result<Vec<SkillTarget>, String> {
    let claude = || SkillTarget {
        host: "claude",
        base: paths::clean(&home.join(".claude").join("skills")),
    };
    let codex = || SkillTarget {
        host: "codex",
        base: paths::clean(&home.join(".agents").join("skills")),
    };
    match target {
        "auto" if has_codex => Ok(vec![claude(), codex()]),
        "auto" | "claude" => Ok(vec![claude()]),
        "codex" => Ok(vec![codex()]),
        "all" => Ok(vec![claude(), codex()]),
        _ => Err(format!(
            "unknown install target {}; use auto, claude, codex, or all",
            gostd::quote(target)
        )),
    }
}

/// Go `detectCodex`: a `codex` on `path_env`, or a directory at
/// `codex_home` (`$CODEX_HOME`), `~/.codex`, `~/Applications/Codex.app` or
/// `<applications>/Codex.app`.
pub fn detect_codex(home: &Path, applications: &Path, path_env: &OsStr, codex_home: &str) -> bool {
    if look_path("codex", Some(path_env)).is_ok() {
        return true;
    }
    [
        PathBuf::from(codex_home),
        home.join(".codex"),
        home.join("Applications").join("Codex.app"),
        applications.join("Codex.app"),
    ]
    .iter()
    .any(|path| fs::metadata(path).is_ok_and(|m| m.is_dir()))
}

/// Go `installTargets`: one reader for every target, so answers already
/// buffered for a later target are not lost.
pub fn install_targets(
    targets: &[SkillTarget],
    force: bool,
    reader: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<(), String> {
    for target in targets {
        install_skills(target, force, reader, out)?;
    }
    Ok(())
}

/// Go `installSkills`.
pub fn install_skills(
    target: &SkillTarget,
    force: bool,
    reader: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<(), String> {
    install_skills_with(target, force, reader, out, &RETIRED_SKILL_NAME_HASHES)
}

/// [`install_skills`] with the retired-name hashes injected for tests.
fn install_skills_with(
    target: &SkillTarget,
    force: bool,
    reader: &mut dyn BufRead,
    out: &mut dyn Write,
    retired_hashes: &[&str],
) -> Result<(), String> {
    let target_base = &target.base;
    let _ = write!(
        out,
        "Installing {} skills to {}\n\n",
        target.host,
        target_base.display()
    );
    let (mut installed, mut updated, mut skipped) = (0, 0, 0);

    for name in skills::NAMES {
        let (mut src_content, src_version) =
            read_embedded_skill(name).map_err(|e| format!("read embedded skill {name}: {e}"))?;
        if target.host == "codex" {
            src_content = skills::codex_skill(name, &src_version)?;
        }

        let target_dir = target_base.join(name);
        let target_file = target_dir.join("SKILL.md");

        if fs::metadata(&target_file).is_err_and(|e| e.kind() == io::ErrorKind::NotFound) {
            // New install
            write_skill(&target_dir, &target_file, &src_content)?;
            let _ = writeln!(out, "  ✓ {name} — installed (v{src_version})");
            installed += 1;
            continue;
        }

        // Existing — compare versions
        let existing =
            read_file(&target_file).map_err(|e| format!("read {}: {e}", target_file.display()))?;
        let dst_version = parse_version(&String::from_utf8_lossy(&existing));

        if src_version == dst_version && !force {
            let _ = writeln!(out, "  · {name} — already up to date (v{src_version})");
            skipped += 1;
            continue;
        }

        if !force {
            let _ = write!(
                out,
                "  ? {name} — update v{dst_version} → v{src_version}? [y/N] "
            );
            // Go `ReadString('\n')`: the error is ignored, the bytes read
            // so far are the answer.
            let mut line = Vec::new();
            let _ = reader.read_until(b'\n', &mut line);
            let answer = gostd::to_lower(&String::from_utf8_lossy(&line));
            let answer = answer.trim();
            if answer != "y" && answer != "yes" {
                let _ = writeln!(out, "    skipped");
                skipped += 1;
                continue;
            }
        }

        write_skill(&target_dir, &target_file, &src_content)?;
        let _ = writeln!(
            out,
            "  ✓ {name} — updated (v{dst_version} → v{src_version})"
        );
        updated += 1;
    }

    // Clean up deprecated skills.
    let mut removed = 0;
    for name in skills::DEPRECATED {
        let target_dir = target_base.join(name);
        if fs::metadata(&target_dir).is_ok() {
            if remove_all(&target_dir).is_err() {
                let _ = writeln!(
                    out,
                    "  ✗ deprecated skill cleanup failed — check permissions in the skills directory"
                );
            } else {
                let _ = writeln!(out, "  🗑 deprecated skill removed");
                removed += 1;
            }
        }
    }
    match remove_skill_dirs_by_hash(target_base, retired_hashes) {
        Err(_) => {
            let _ = writeln!(
                out,
                "  ✗ retired skill cleanup failed — check permissions in the skills directory"
            );
        }
        Ok(hash_removed) if hash_removed > 0 => {
            let _ = writeln!(out, "  🗑 {hash_removed} retired skill(s) removed");
            removed += hash_removed;
        }
        Ok(_) => {}
    }

    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "Done: {installed} installed, {updated} updated, {skipped} up to date, {removed} removed"
    );
    Ok(())
}

/// Go `removeSkillDirsByHash`: removes every entry of `target_base` whose
/// name hashes to one of `hashes`, in name order. A missing directory
/// removes nothing.
fn remove_skill_dirs_by_hash(target_base: &Path, hashes: &[&str]) -> io::Result<usize> {
    let mut entries = match fs::read_dir(target_base) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
        Ok(dir) => dir.collect::<io::Result<Vec<_>>>()?,
    };
    entries.sort_by_key(|entry| entry.file_name());

    let mut removed = 0;
    for entry in entries {
        let name = entry.file_name();
        if !hashes.contains(&sha256_hex(name.as_encoded_bytes()).as_str()) {
            continue;
        }
        remove_all(&target_base.join(&name))?;
        removed += 1;
    }
    Ok(removed)
}

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Go `os.RemoveAll` for a path that exists: a symlink is removed, never
/// followed.
fn remove_all(path: &Path) -> io::Result<()> {
    let removed = match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(e) => Err(e),
    };
    match removed {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Go `readEmbeddedSkill`: the skill's content and frontmatter version.
fn read_embedded_skill(name: &str) -> Result<(Vec<u8>, String), String> {
    let content = skills::read_file(&format!("{name}/SKILL.md"))?;
    let version = parse_version(&String::from_utf8_lossy(content));
    Ok((content.to_vec(), version))
}

/// Go `os.ReadFile`, with its `*PathError` text.
fn read_file(path: &Path) -> Result<Vec<u8>, String> {
    let mut file = File::open(path).map_err(|e| path_error("open", path, &e))?;
    let mut data = Vec::new();
    file.read_to_end(&mut data)
        .map_err(|e| path_error("read", path, &e))?;
    Ok(data)
}

/// Go `writeSkill`.
fn write_skill(dir: &Path, file: &Path, content: &[u8]) -> Result<(), String> {
    mkdir_all(dir)
        .map_err(|e| format!("mkdir {}: {}", dir.display(), path_error("mkdir", dir, &e)))?;
    write_file(file, content).map_err(|e| format!("write {}: {e}", file.display()))
}

/// Go `os.MkdirAll(dir, 0o755)`. Go's error may name the parent that
/// failed; this one always names `dir`.
fn mkdir_all(dir: &Path) -> io::Result<()> {
    match fs::metadata(dir) {
        Ok(meta) if meta.is_dir() => return Ok(()),
        Ok(_) => return Err(io::ErrorKind::NotADirectory.into()),
        Err(_) => {}
    }
    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o755);
    builder.create(dir)
}

/// Go `os.WriteFile(file, content, 0o644)`, with its `*PathError` text.
fn write_file(path: &Path, content: &[u8]) -> Result<(), String> {
    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o644);
    let mut file = opts.open(path).map_err(|e| path_error("open", path, &e))?;
    file.write_all(content)
        .map_err(|e| path_error("write", path, &e))
}

/// Go `*fs.PathError` text: `<op> <path>: <errno text>`.
fn path_error(op: &str, path: &Path, err: &io::Error) -> String {
    format!("{op} {}: {}", path.display(), gostd::os_error_text(err))
}

/// Go `parseVersion`: the `version:` field of the YAML frontmatter, which
/// sits between the first and second `---` lines; `unknown` when absent.
pub fn parse_version(content: &str) -> String {
    let mut in_frontmatter = false;
    for line in content.split('\n') {
        let trimmed = line.trim();
        if trimmed == "---" {
            if !in_frontmatter {
                in_frontmatter = true;
                continue;
            }
            break; // end of frontmatter
        }
        if in_frontmatter && let Some(rest) = trimmed.strip_prefix("version:") {
            return rest.trim().to_string();
        }
    }
    "unknown".to_string()
}
