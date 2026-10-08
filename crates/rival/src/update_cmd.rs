//! `rival update`: upgrades through Homebrew, then reinstalls the skills
//! from the upgraded binary. Go: `cmd/update.go`.

use std::ffi::OsStr;
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use rival_core::executor::{oscmd, process};
use rival_core::paths::{self, HOME_VAR};
use rival_core::update;

use crate::install::{detect_codex, install_targets, skill_targets};
use crate::root::{CmdEnv, CmdError};

#[cfg(test)]
mod tests;

/// Go `codexInstalled`'s system applications directory.
const SYSTEM_APPLICATIONS: &str = "/Applications";

/// Where child processes read and write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChildIo {
    /// Go with `os.Stdin`/`os.Stdout`/`os.Stderr`: the child shares this
    /// process's descriptors (production).
    Inherit,
    /// Go with `cmd.SetOut(&buf)`: output is copied into the command's
    /// writers, and the installer's input comes from the command's stdin
    /// source (tests).
    #[cfg_attr(not(test), allow(dead_code, reason = "test seam"))]
    Capture,
}

/// `rival update`: Go `runUpdate`.
pub fn update_action(env: &mut CmdEnv<'_>) -> Result<(), CmdError> {
    let _ = write!(env.stdout, "Checking for updates... ");
    let latest = update::fetch_latest(&update::releases_url(env.cfg))
        .map_err(|e| CmdError::plain(format!("check latest version: {e}")))?;
    update_to_version(
        env,
        ChildIo::Inherit,
        rival_core::VERSION,
        &latest,
        Path::new(SYSTEM_APPLICATIONS),
    )
}

/// Go `updateToVersion`, with the child stdio and the system applications
/// directory injected.
pub fn update_to_version(
    env: &mut CmdEnv<'_>,
    io: ChildIo,
    current: &str,
    latest: &str,
    applications: &Path,
) -> Result<(), CmdError> {
    if latest == current {
        let _ = writeln!(env.stdout, "already on latest (v{current})");
        // Go returns os.UserHomeDir's error unwrapped.
        let home = match env.cfg.getenv(HOME_VAR) {
            "" if cfg!(windows) => return Err(CmdError::plain("%userprofile% is not defined")),
            "" => return Err(CmdError::plain("$HOME is not defined")),
            home => home.to_string(),
        };
        let home = Path::new(&home);
        let has_codex = detect_codex(
            home,
            applications,
            OsStr::new(env.cfg.getenv("PATH")),
            env.cfg.getenv("CODEX_HOME"),
        );
        let targets = skill_targets(home, "auto", has_codex).map_err(CmdError::plain)?;
        let mut reader = env.stdin.reader();
        return install_targets(&targets, true, &mut *reader, env.stdout).map_err(CmdError::plain);
    }

    let _ = write!(env.stdout, "v{current} → v{latest}\n\n");

    let _ = writeln!(env.stdout, "Upgrading via Homebrew...");
    if run(env, io, Input::Null, "brew", &["upgrade", "1905/tap/rival"]).is_err() {
        // If brew upgrade fails (e.g. already latest), try reinstall.
        let _ = writeln!(env.stdout, "brew upgrade failed, trying reinstall...");
        run(
            env,
            io,
            Input::Null,
            "brew",
            &["reinstall", "1905/tap/rival"],
        )
        .map_err(|e| CmdError::plain(format!("brew reinstall: {e}")))?;
    }

    let _ = writeln!(env.stdout, "\nUpdating skills...");
    let prefix = output(env, "brew", &["--prefix", "rival"])
        .map_err(|e| CmdError::plain(format!("locate upgraded rival: {e}")))?;
    let prefix = String::from_utf8_lossy(&prefix);
    let binary = paths::clean(&Path::new(prefix.trim()).join("bin").join("rival"));
    install_updated_skills(env, io, &binary.to_string_lossy())
        .map_err(|e| CmdError::plain(format!("install skills: {e}")))?;

    let _ = write!(env.stdout, "\n✓ Updated to v{latest}\n");
    Ok(())
}

/// Go `installUpdatedSkills`: re-exec the Homebrew installation, because
/// this process still embeds the old skills.
pub fn install_updated_skills(
    env: &mut CmdEnv<'_>,
    io: ChildIo,
    binary: &str,
) -> Result<(), String> {
    run(
        env,
        io,
        Input::Command,
        binary,
        &["install", "--force", "--target", "auto"],
    )
}

/// A child's stdin.
#[derive(Clone, Copy)]
enum Input {
    /// Go's nil `Stdin`: the null device. brew gets this, so it cannot eat
    /// answers meant for the installer.
    Null,
    /// Go `cmd.InOrStdin()`.
    Command,
}

/// Go `exec.CommandContext(...).Run()` with the command's stdout/stderr.
fn run(
    env: &mut CmdEnv<'_>,
    io: ChildIo,
    input: Input,
    name: &str,
    args: &[&str],
) -> Result<(), String> {
    let (mut cmd, path) = oscmd::command(env.cfg, name, args)?;
    let stdin = match input {
        Input::Null => Stdio::null(),
        Input::Command if io == ChildIo::Inherit => Stdio::inherit(),
        Input::Command => Stdio::piped(),
    };
    cmd.stdin(stdin);
    match io {
        ChildIo::Inherit => {
            cmd.stdout(Stdio::inherit()).stderr(Stdio::inherit());
            let status = spawn(&mut cmd, &path)?
                .wait()
                .map_err(|e| format!("wait: {e}"))?;
            exit_result(status)
        }
        ChildIo::Capture => {
            cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
            let mut child = spawn(&mut cmd, &path)?;
            let child_in = child.stdin.take();
            let mut data = Vec::new();
            if child_in.is_some() {
                let _ = env.stdin.reader().read_to_end(&mut data);
            }
            let (mut out, mut err) = (child.stdout.take(), child.stderr.take());
            let (stdout, stderr) = (&mut *env.stdout, &mut *env.stderr);
            // Every worker is joined before the child is reaped.
            std::thread::scope(|s| {
                if let Some(mut child_in) = child_in {
                    // A closed pipe (the child never reads) is not an error;
                    // dropping the handle closes the child's stdin.
                    s.spawn(move || {
                        let _ = child_in.write_all(&data);
                    });
                }
                s.spawn(move || copy(out.as_mut(), stdout));
                s.spawn(move || copy(err.as_mut(), stderr));
            });
            exit_result(child.wait().map_err(|e| format!("wait: {e}"))?)
        }
    }
}

fn copy(from: Option<&mut impl Read>, to: &mut (dyn Write + Send)) {
    if let Some(from) = from {
        let _ = io::copy(from, to);
    }
}

/// Go `exec.Command(...).Output()`: stdout captured, stderr kept out of the
/// terminal (Go saves it on the `ExitError`, which nothing prints).
fn output(env: &CmdEnv<'_>, name: &str, args: &[&str]) -> Result<Vec<u8>, String> {
    let (mut cmd, path) = oscmd::command(env.cfg, name, args)?;
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = spawn(&mut cmd, &path)?;
    let mut data = Vec::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_end(&mut data);
    }
    exit_result(child.wait().map_err(|e| format!("wait: {e}"))?)?;
    Ok(data)
}

fn spawn(cmd: &mut Command, path: &Path) -> Result<std::process::Child, String> {
    process::spawn(cmd).map_err(|e| oscmd::fork_error(path, &e))
}

fn exit_result(status: std::process::ExitStatus) -> Result<(), String> {
    if status.success() {
        Ok(())
    } else {
        Err(status.to_string())
    }
}
