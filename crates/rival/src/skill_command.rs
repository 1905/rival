//! `rival command skill --host claude|codex`: the one `/rival` skill. It
//! runs the model set in `skill.<host>` through the `rival command <model>`
//! workflow, or the security review when the input starts with `security`.

use std::io::BufRead;

use rival_core::config::{SKILL_MODEL_NAMES, SkillHost};

use crate::command_security::{SecurityExecutor, SecurityOptions, run_command_security};
use crate::model_command::run_model_command;
use crate::model_specs::{ModelSpec, RIVAL_USAGE, spec_for_name};
use crate::root::{CmdEnv, CmdError, StdinSource};
use crate::tree::Invocation;

#[cfg(test)]
mod tests;

/// The first word that sends the input to the security review.
const SECURITY_WORD: &str = "security";

/// What a `/rival` input can run; tests inject fakes.
pub struct SkillRoutes<'a> {
    /// The spec of a `rival command` model name.
    pub spec: &'a dyn Fn(&str) -> Option<ModelSpec>,
    pub security: &'a SecurityExecutor<'a>,
}

/// [`run_skill_command`] with the real models and the real security
/// executor.
pub fn skill_command_action(env: &mut CmdEnv<'_>, inv: &Invocation) -> Result<(), CmdError> {
    let raw_host = inv.string("host");
    let host = SkillHost::parse(&raw_host).ok_or_else(|| host_error(&raw_host))?;
    let security = SecurityExecutor::production();
    run_skill_command(
        env,
        host,
        &inv.string("workdir"),
        inv.bool("no-queue"),
        &SkillRoutes {
            spec: &spec_for_name,
            security: &security,
        },
    )
}

/// The usage error for a missing or unknown `--host`.
fn host_error(raw: &str) -> CmdError {
    if raw.trim().is_empty() {
        return CmdError::plain("rival command skill: --host is required: claude or codex");
    }
    CmdError::plain(format!(
        "rival command skill: invalid --host {raw:?}; use claude or codex"
    ))
}

/// Reads the skill args from stdin once, then runs them: `security
/// [scope]` as the security review, anything else with the model of
/// `skill.<host>`. A terminal stdin or empty args print [`RIVAL_USAGE`].
pub fn run_skill_command(
    env: &mut CmdEnv<'_>,
    host: SkillHost,
    workdir: &str,
    no_queue: bool,
    routes: &SkillRoutes<'_>,
) -> Result<(), CmdError> {
    // A terminal stdin means no piped args, so show usage instead of hanging.
    if env.stdin.is_char_device() {
        let _ = writeln!(env.stdout, "{RIVAL_USAGE}");
        return Ok(());
    }
    let raw = env
        .stdin
        .read_all()
        .map_err(|e| CmdError::plain(format!("read stdin: {e}")))?;
    let raw = String::from_utf8_lossy(&raw).into_owned();

    if let Some(scope) = security_scope(&raw) {
        let opts = SecurityOptions {
            workdir: workdir.to_string(),
            no_queue,
            which: false,
        };
        let mut fed = FedStdin(scope.as_bytes().to_vec());
        return with_stdin(env, &mut fed, |env| {
            run_command_security(env, &opts, routes.security)
        });
    }

    let name = env.cfg.skill_model(host);
    let mut spec = (routes.spec)(name)
        .ok_or_else(|| CmdError::plain(format!("{}: no model named {name:?}", host.key())))?;
    // The resolved model's usage names its removed skill.
    spec.usage = RIVAL_USAGE;
    add_preflight_hint(&mut spec, host);
    let mut fed = FedStdin(raw.into_bytes());
    with_stdin(env, &mut fed, |env| {
        run_model_command(env, &spec, workdir, no_queue)
    })
}

/// The scope of a `security [scope]` input, trimmed; `None` when the first
/// word is not `security`.
fn security_scope(raw: &str) -> Option<&str> {
    let rest = raw.trim_start().strip_prefix(SECURITY_WORD)?;
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    Some(rest.trim())
}

/// Appends how to pick another model to a failed preflight: the model of
/// `skill.<host>` may have no runtime on this machine.
fn add_preflight_hint(spec: &mut ModelSpec, host: SkillHost) {
    let hint = format!(
        "set another model: rival config set {} <model> ({})",
        host.key(),
        SKILL_MODEL_NAMES.join(", ")
    );
    let preflight = std::mem::replace(&mut spec.preflight, Box::new(|_| Ok(())));
    spec.preflight =
        Box::new(move |cfg| preflight(cfg).map_err(|e| anyhow::anyhow!("{e:#}\n{hint}")));
}

/// Runs `body` with `stdin` in place of the env's stdin.
fn with_stdin<T>(
    env: &mut CmdEnv<'_>,
    stdin: &mut dyn StdinSource,
    body: impl FnOnce(&mut CmdEnv<'_>) -> T,
) -> T {
    let mut inner = CmdEnv {
        cfg: env.cfg,
        stdin,
        stdout: &mut *env.stdout,
        stderr: &mut *env.stderr,
        live_stdout: env.live_stdout,
        prepare_mr: env.prepare_mr,
        signals: env.signals,
    };
    body(&mut inner)
}

/// Stdin that was already read: it holds the args, so it is not a terminal
/// and its stat does not fail.
struct FedStdin(Vec<u8>);

impl StdinSource for FedStdin {
    fn is_char_device(&self) -> bool {
        false
    }

    fn read_all(&mut self) -> Result<Vec<u8>, String> {
        Ok(self.0.clone())
    }

    fn reader(&mut self) -> Box<dyn BufRead + '_> {
        Box::new(std::io::Cursor::new(self.0.clone()))
    }
}
