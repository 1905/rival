#[cfg(test)]
mod tests;

use std::path::Path;

use anyhow::{anyhow, bail};

use super::Mirror;
use super::oscmd;
use super::subprocess::{Request, RunResult, run_subprocess};
use crate::cancel::Context;
use crate::config::{self, Config};
use crate::session::Session;

/// Session-only settings for read-only reviews on the Antigravity CLI (`agy`).
///
/// Workspace file reads are allowed by default while non-workspace reads
/// require approval (auto-denied in headless mode). Explicit deny rules block
/// writes, terminal commands, URL access and MCP tools regardless of the user's
/// saved `settings.json`.
pub(crate) const NEON_READ_ONLY_SETTINGS: &str = r#"{"agentMode":"","toolPermission":"request-review","allowNonWorkspaceAccess":false,"permissions":{"deny":["write_file(*)","command(*)","read_url(*)","execute_url(*)","download_url(*)","mcp(*)"]}}"#;

/// Known bare and effort-suffixed Gemini 4 Neon model ids.
const NEON_BASE_MODEL: &str = "gemini-4-neon";
const EFFORT_SUFFIXES: [&str; 4] = ["-low", "-medium", "-high", "-xhigh"];

/// Whether the Antigravity (`agy`) runtime runs `model`: Gemini 4 Neon
/// (`gemini-4-neon-xhigh`, the bare `gemini-4-neon`, or an effort-suffixed slug).
pub fn is_neon_model(model: &str) -> bool {
    model == config::NEON_MODEL
        || model == NEON_BASE_MODEL
        || EFFORT_SUFFIXES
            .iter()
            .any(|suffix| model == format!("{NEON_BASE_MODEL}{suffix}"))
}

/// Checks that `agy` is installed on `PATH`. Authentication uses the CLI's own
/// cached keyring or environment credentials, which cannot be probed offline.
pub fn neon_preflight(cfg: &Config) -> anyhow::Result<()> {
    if oscmd::look_path(cfg, "agy").is_err() {
        bail!("{} runtime is not installed", config::NEON_LABEL);
    }
    Ok(())
}

/// Executes a prompt with Gemini 4 Neon's base model id.
#[allow(clippy::too_many_arguments)]
pub fn run_neon(
    ctx: &Context,
    cfg: &Config,
    sess: &mut Session,
    prompt: &str,
    effort: &str,
    workdir: &str,
    review: bool,
    log: Option<&str>,
    mirror: Mirror<'_>,
) -> anyhow::Result<RunResult> {
    run_neon_model(
        ctx,
        cfg,
        sess,
        prompt,
        effort,
        workdir,
        config::NEON_MODEL,
        review,
        log,
        mirror,
    )
}

/// Executes a prompt with an explicit Neon model through the Antigravity CLI
/// (`agy`), falling back to [`config::NEON_MODEL`] when `model` is empty.
/// `review` enforces [`NEON_READ_ONLY_SETTINGS`]; raw prompts pass
/// `--dangerously-skip-permissions`.
#[allow(clippy::too_many_arguments)]
pub fn run_neon_model(
    ctx: &Context,
    cfg: &Config,
    sess: &mut Session,
    prompt: &str,
    effort: &str,
    workdir: &str,
    model: &str,
    review: bool,
    log: Option<&str>,
    mirror: Mirror<'_>,
) -> anyhow::Result<RunResult> {
    run_neon_model_with(
        cfg,
        sess,
        prompt,
        effort,
        workdir,
        model,
        review,
        log,
        |sess, req| run_subprocess(ctx, cfg.paths(), sess, req, mirror),
    )
}

/// [`run_neon_model`] with the spawn step injected (a test seam).
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_neon_model_with(
    cfg: &Config,
    sess: &mut Session,
    prompt: &str,
    effort: &str,
    workdir: &str,
    model: &str,
    review: bool,
    log: Option<&str>,
    spawn: impl FnOnce(&mut Session, &Request<'_>) -> anyhow::Result<RunResult>,
) -> anyhow::Result<RunResult> {
    let label = config::NEON_LABEL;
    let model = neon_model_or_default(model);
    let full_prompt = neon_full_prompt(cfg, prompt, workdir);
    let args = neon_run_args(model, &full_prompt, effort, review)
        .map_err(|e| anyhow!("{label} runtime: {e}"))?;

    // The prompt travels via `-p`; stdin carries nothing.
    let req = Request {
        binary: "agy",
        args: &args,
        env: &[],
        prompt: "",
        drop_env: &[],
        environ: cfg.environ(),
        log,
    };
    spawn(sess, &req).map_err(|err| {
        anyhow!(
            "{label} runtime: {}",
            config::public_runtime_error(label, model, &format!("{err:#}"))
        )
    })
}

/// Composes the prompt identically to the other executors, so a Neon run sees
/// the same system prompt and workdir preamble.
pub(crate) fn neon_full_prompt(cfg: &Config, prompt: &str, workdir: &str) -> String {
    format!(
        "{}\n\n{}\n{prompt}",
        config::SYSTEM_PROMPT,
        cfg.build_workdir_preamble(Path::new(workdir))
    )
}

/// Resolves an optional model to [`config::NEON_MODEL`].
pub(crate) fn neon_model_or_default(model: &str) -> &str {
    if model.trim().is_empty() {
        return config::NEON_MODEL;
    }
    model
}

/// The `agy` model slug for `model`: always `gemini-4-neon-xhigh` because Neon
/// has a single effort level (`xhigh`). `effort` is still validated so typos
/// fail instead of running silently.
pub fn neon_model_slug(model: &str, effort: &str) -> anyhow::Result<String> {
    let mapped = neon_effort(effort)?;
    let base = neon_model_or_default(model);
    let stem = EFFORT_SUFFIXES
        .iter()
        .find_map(|suffix| base.strip_suffix(suffix))
        .unwrap_or(base);
    Ok(format!("{stem}-{mapped}"))
}

/// Builds `agy`'s argv. `-p` is placed last with the composed prompt; review
/// runs apply [`NEON_READ_ONLY_SETTINGS`] while raw runs pass
/// `--dangerously-skip-permissions`.
pub(crate) fn neon_run_args(
    model: &str,
    full_prompt: &str,
    effort: &str,
    review: bool,
) -> anyhow::Result<Vec<String>> {
    let slug = neon_model_slug(model, effort)?;
    let mut args = vec![
        "--model".to_string(),
        slug,
        "--disable-slash-commands".to_string(),
        "--output-format".to_string(),
        "text".to_string(),
    ];
    if review {
        args.extend([
            "--settings".to_string(),
            NEON_READ_ONLY_SETTINGS.to_string(),
        ]);
    } else {
        args.push("--dangerously-skip-permissions".to_string());
    }
    args.extend(["-p".to_string(), full_prompt.to_string()]);
    Ok(args)
}

/// Maps rival's effort menu onto Gemini 4 Neon's single effort level (`xhigh`).
/// Any known effort (or empty) resolves to `xhigh`; unrecognized values fail so
/// typos never run silently.
pub fn neon_effort(effort: &str) -> anyhow::Result<String> {
    match effort {
        "" | "low" | "medium" | "high" | "xhigh" | "ultra" | "max" | "minimal" | "none" => {
            Ok("xhigh".to_string())
        }
        _ => bail!(
            "unsupported {} effort {:?}; {} only supports: xhigh",
            config::NEON_LABEL,
            effort,
            config::NEON_LABEL
        ),
    }
}

/// CLI output fragments that indicate an unauthenticated `agy` session.
const NEON_AUTH_MARKERS: [&str; 2] = ["authentication required", "not authenticated"];

/// Inspects a failed `agy` log for authentication errors and returns an
/// actionable hint ("" if none).
pub fn neon_auth_hint(log_file: &Path) -> String {
    let Ok(data) = std::fs::read(log_file) else {
        return String::new();
    };
    let lower = String::from_utf8_lossy(&data).to_lowercase();
    if NEON_AUTH_MARKERS.iter().any(|m| lower.contains(m)) {
        return "rival: neon authentication failed — run `agy` once to sign in".to_string();
    }
    String::new()
}
