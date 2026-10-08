#[cfg(test)]
mod tests;

use std::path::Path;

use anyhow::{anyhow, bail};

use super::Mirror;
use super::oscmd::{self, Output};
use super::subprocess::{Request, RunResult, run_subprocess};
use crate::cancel::Context;
use crate::config::{self, Config};
use crate::session::Session;

/// Checks that codex is installed and authenticated,
/// naming the given model in its errors.
pub fn codex_preflight_for(cfg: &Config, model: &str) -> anyhow::Result<()> {
    let label = config::engine_label("codex", model);
    if oscmd::look_path(cfg, "codex").is_err() {
        bail!("{label} runtime is not installed");
    }

    let (out, result) = oscmd::run(cfg, "codex", &["login", "status"], Output::Combined);
    if result.is_err() {
        bail!(
            "{label} authentication is unavailable\n{}",
            String::from_utf8_lossy(&out)
        );
    }
    Ok(())
}

/// Executes a prompt with one explicit model. Review
/// pipelines use this entry point so the model recorded in the session is
/// also the model sent to the runtime. Codex is the only model it runs.
#[allow(clippy::too_many_arguments)]
pub fn run_codex_model(
    ctx: &Context,
    cfg: &Config,
    sess: &mut Session,
    prompt: &str,
    effort: &str,
    workdir: &str,
    model: &str,
    mirror: Mirror<'_>,
) -> anyhow::Result<RunResult> {
    run_codex_model_with(cfg, sess, prompt, effort, workdir, model, |sess, req| {
        run_subprocess(ctx, cfg.paths(), sess, req, mirror)
    })
}

/// [`run_codex_model`] with the spawn step injected.
pub(crate) fn run_codex_model_with(
    cfg: &Config,
    sess: &mut Session,
    prompt: &str,
    effort: &str,
    workdir: &str,
    model: &str,
    spawn: impl FnOnce(&mut Session, &Request<'_>) -> anyhow::Result<RunResult>,
) -> anyhow::Result<RunResult> {
    if model != config::CODEX_MODEL {
        bail!("unsupported codex model {:?}", model);
    }
    let args = codex_run_args(model, effort, workdir);

    let full_prompt = format!(
        "{}\n\n{}\n{prompt}",
        config::SYSTEM_PROMPT,
        cfg.build_workdir_preamble(Path::new(workdir))
    );
    let req = Request {
        binary: "codex",
        args: &args,
        env: &[],
        prompt: &full_prompt,
        drop_env: &[],
        environ: cfg.environ(),
    };
    spawn(sess, &req).map_err(|err| {
        let label = config::engine_label("codex", model);
        let message = config::replace_ordered(
            &format!("{err:#}"),
            &[
                ("Codex", label.clone()),
                ("codex", label.clone()),
                (model, label.clone()),
            ],
        );
        anyhow!("{label} runtime: {message}")
    })
}

pub(crate) fn codex_run_args(model: &str, effort: &str, workdir: &str) -> Vec<String> {
    // The codex runtime takes ultra as its own reasoning level (distinct from
    // xhigh), so preserve the requested value instead of normalizing it.
    [
        "exec",
        "-C",
        workdir,
        "-m",
        model,
        "-c",
        &format!("model_reasoning_effort={effort}"),
        "--sandbox",
        "read-only",
        "--ephemeral",
        "--skip-git-repo-check",
        "--color",
        "never",
        "-",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}
