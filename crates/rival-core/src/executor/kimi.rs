#[cfg(test)]
mod tests;

use std::path::Path;

use anyhow::bail;

use super::Mirror;
use super::opencode::{OPENCODE_FULL_AUTO_PERMISSION, OpencodeRunOpts, run_opencode_model_with};
use super::oscmd;
use super::subprocess::{Request, RunResult, run_subprocess};
use crate::cancel::Context;
use crate::config::{self, Config};
use crate::session::Session;

/// Checks that the opencode CLI is installed and a
/// Moonshot API key is available (env / `.env` walk-up from `workdir` — see
/// [`Config::kimi_api_key_from`]). Kimi K3 runs through OpenCode's built-in
/// provider with the key injected per run via `OPENCODE_CONFIG_CONTENT`;
/// without it the run fails mid-flight with an opaque provider auth error,
/// so preflight turns that into one clear hint.
pub fn kimi_preflight(cfg: &Config, workdir: &str) -> anyhow::Result<()> {
    if oscmd::look_path(cfg, "opencode").is_err() {
        bail!("opencode CLI not installed. Install: curl -fsSL https://opencode.ai/install | bash");
    }
    if cfg.kimi_api_key_from(Path::new(workdir)).is_empty() {
        bail!("MOONSHOT_API_KEY is not set — add it to the project .env or export it");
    }
    Ok(())
}

/// Credential vars stripped from the kimi child in its
/// full-auto (non-review) mode, where every tool call is allowed — a
/// prompt-injected repo could otherwise read any inherited secret via `env`
/// and exfiltrate it. The opencode child needs none of these; its auth
/// arrives via `OPENCODE_CONFIG_CONTENT`. Not applied in review mode, which
/// runs under the read-only profile where bash is denied. This shrinks the
/// blast radius but is NOT a sandbox in full-auto mode: the agent can still
/// run arbitrary commands as the user.
pub(crate) const KIMI_DROP_ENV: [&str; 9] = [
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "GOOGLE_",
    "RIVAL_CLAUDE_TOKEN",
    // Trailing underscore = prefix drop: catches SESSION_TOKEN, PROFILE,
    // WEB_IDENTITY_TOKEN_FILE, container credential URIs — the whole family.
    "AWS_",
    "GITHUB_TOKEN",
    "GH_TOKEN",
    "GITLAB_TOKEN",
];

/// Executes a prompt with Kimi K3 through the opencode CLI
/// (moonshotai/kimi-k3, the built-in Moonshot AI provider, 1M context). The
/// reasoning variant is pinned to max by the registry entry — K3 is a
/// thinking-only model whose API accepts no other level, so the requested
/// rival effort is ignored. Permissions follow the session mode: review
/// runs under the same mechanical read-only `OPENCODE_PERMISSION` profile as
/// the megareview reviewers; every other mode runs full-auto (every tool
/// allowed) with known credential env vars stripped as blast-radius
/// reduction.
///
/// `cred_workdir` is where the Moonshot key is looked up. It differs from
/// `workdir` only for a GitLab MR review, which runs in a temporary checkout
/// that has no project .env; the key still comes from the caller's project.
pub fn run_kimi(
    ctx: &Context,
    cfg: &Config,
    sess: &mut Session,
    prompt: &str,
    workdir: &str,
    cred_workdir: &str,
    mirror: Mirror<'_>,
) -> anyhow::Result<RunResult> {
    run_kimi_with(cfg, sess, prompt, workdir, cred_workdir, |sess, req| {
        run_subprocess(ctx, cfg.paths(), sess, req, mirror)
    })
}

/// [`run_kimi`] with the spawn step injected.
pub(crate) fn run_kimi_with(
    cfg: &Config,
    sess: &mut Session,
    prompt: &str,
    workdir: &str,
    cred_workdir: &str,
    spawn: impl FnOnce(&mut Session, &Request<'_>) -> anyhow::Result<RunResult>,
) -> anyhow::Result<RunResult> {
    let opts = kimi_run_opts(cfg, &sess.mode, cred_workdir);
    run_opencode_model_with(
        cfg,
        sess,
        prompt,
        "max",
        workdir,
        config::KIMI_MODEL,
        &opts,
        spawn,
    )
}

/// Selects the permission profile and env hardening for
/// one run by session mode. Review keeps the zero-value read-only reviewer
/// defaults; only the API key differs (Moonshot, read from `cred_workdir`).
/// Every mode other than "review" — raw, and also the task modes plan
/// and security — gets the full-auto profile.
pub(crate) fn kimi_run_opts(cfg: &Config, mode: &str, cred_workdir: &str) -> OpencodeRunOpts {
    let mut opts = OpencodeRunOpts {
        api_key: cfg.kimi_api_key_from(Path::new(cred_workdir)),
        ..OpencodeRunOpts::default()
    };
    if mode != "review" {
        opts.permission = OPENCODE_FULL_AUTO_PERMISSION.to_string();
        opts.drop_env = KIMI_DROP_ENV.iter().map(|s| s.to_string()).collect();
    }
    opts
}
