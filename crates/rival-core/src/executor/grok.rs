//! Go: `internal/executor/grok.go`.

#[cfg(test)]
mod tests;

use std::io::Write;
use std::path::Path;

use anyhow::{anyhow, bail};

use super::Mirror;
use super::oscmd;
use super::process;
use super::subprocess::{Request, RunResult, io_text, run_subprocess};
use crate::cancel::Context;
use crate::config::{self, Config};
use crate::logging;
use crate::paths;
use crate::session::Session;

/// Go `GrokPreflight`: checks that grok is installed and authenticated. The
/// auth file is resolved from the real home directory rather than
/// `$GROK_HOME`: that prefix is blocked from child environments (see
/// `BLOCKED_ENV_PREFIXES`), so honoring it here would check a location the
/// run can never use.
pub fn grok_preflight(cfg: &Config) -> anyhow::Result<()> {
    if oscmd::look_path(cfg, "grok").is_err() {
        bail!("{} runtime is not installed", config::GROK_LABEL);
    }

    // Go os.UserHomeDir.
    let home = cfg.getenv(paths::HOME_VAR);
    if home.is_empty() {
        let var = if cfg!(windows) {
            "%userprofile%"
        } else {
            "$HOME"
        };
        bail!(
            "{} authentication is unavailable: {var} is not defined; run `grok login`",
            config::GROK_LABEL
        );
    }
    let auth_file = paths::clean(Path::new(&format!("{home}/.grok/auth.json")));
    if std::fs::metadata(&auth_file).is_err() {
        bail!(
            "{} authentication is unavailable ({} not found); run `grok login`",
            config::GROK_LABEL,
            auth_file.display()
        );
    }
    Ok(())
}

/// Go `RunGrok`: executes a prompt with grok's default model. It is the
/// entry point for the single-model surfaces (`rival command grok`,
/// `rival run grok`), which have no per-run model choice.
#[allow(clippy::too_many_arguments)]
pub fn run_grok(
    ctx: &Context,
    cfg: &Config,
    sess: &mut Session,
    prompt: &str,
    effort: &str,
    workdir: &str,
    review: bool,
    mirror: Mirror<'_>,
) -> anyhow::Result<RunResult> {
    run_grok_model(
        ctx,
        cfg,
        sess,
        prompt,
        effort,
        workdir,
        config::GROK_MODEL,
        review,
        mirror,
    )
}

/// Go `RunGrokModel`: executes a prompt with an explicit grok model, falling
/// back to the default when `model` is empty — the contract the review
/// pipeline relies on, where a session carries the concrete model to run or
/// judge with. Unlike codex, the grok CLI does not read the prompt from
/// stdin, so the composed prompt is handed over in a temp file, which also
/// keeps it out of the process table. `review` selects the read-only sandbox
/// used by review pipelines.
#[allow(clippy::too_many_arguments)]
pub fn run_grok_model(
    ctx: &Context,
    cfg: &Config,
    sess: &mut Session,
    prompt: &str,
    effort: &str,
    workdir: &str,
    model: &str,
    review: bool,
    mirror: Mirror<'_>,
) -> anyhow::Result<RunResult> {
    run_grok_model_with(
        cfg,
        sess,
        prompt,
        effort,
        workdir,
        model,
        review,
        |sess, req| run_subprocess(ctx, cfg.paths(), sess, req, mirror),
    )
}

/// [`run_grok_model`] with the spawn step injected (Go's `grokSubprocess`
/// test seam). The prompt file exists for the whole spawn and is removed
/// on every return path after it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_grok_model_with(
    cfg: &Config,
    sess: &mut Session,
    prompt: &str,
    effort: &str,
    workdir: &str,
    model: &str,
    review: bool,
    spawn: impl FnOnce(&mut Session, &Request<'_>) -> anyhow::Result<RunResult>,
) -> anyhow::Result<RunResult> {
    let label = config::GROK_LABEL;
    let (mut file, name) = oscmd::create_temp(cfg, "rival-grok-*.md")
        .map_err(|e| anyhow!("{label} runtime: create prompt file: {e}"))?;
    let _cleanup = PromptFile(&name);

    if let Err(e) = file.write_all(grok_full_prompt(cfg, prompt, workdir).as_bytes()) {
        let _ = process::close_file(file);
        bail!(
            "{label} runtime: write prompt file: write {name}: {}",
            io_text(&e)
        );
    }
    if let Err(e) = process::close_file(file) {
        bail!(
            "{label} runtime: close prompt file: close {name}: {}",
            io_text(&e)
        );
    }

    let args = grok_run_args(model, &name, effort, workdir, review)
        .map_err(|e| anyhow!("{label} runtime: {e}"))?;

    // The prompt is already in the file; stdin carries nothing.
    let req = Request {
        binary: "grok",
        args: &args,
        env: &[],
        prompt: "",
        drop_env: &[],
        environ: cfg.environ(),
    };
    spawn(sess, &req).map_err(|err| anyhow!("{label} runtime: {err:#}"))
}

/// Removes the prompt file when the run returns. Go's deferred `os.Remove`:
/// a failure other than "not found" is logged, never returned.
struct PromptFile<'a>(&'a str);

impl Drop for PromptFile<'_> {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_file(self.0)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            logging::warn()
                .err(format!("remove {}: {}", self.0, io_text(&e)))
                .str("file", self.0)
                .msg("failed to remove grok prompt file");
        }
    }
}

/// Go `grokFullPrompt`: composes the prompt exactly as the other executors
/// do, so a grok run sees the same system prompt and workdir preamble as
/// the others.
pub(crate) fn grok_full_prompt(cfg: &Config, prompt: &str, workdir: &str) -> String {
    format!(
        "{}\n\n{}\n{prompt}",
        config::SYSTEM_PROMPT,
        cfg.build_workdir_preamble(Path::new(workdir))
    )
}

/// Go `grokModelOrDefault`: resolves an optional model to grok's default,
/// so a session that never recorded one still produces a valid `-m`
/// instead of a bare flag.
pub(crate) fn grok_model_or_default(model: &str) -> &str {
    if model.trim().is_empty() {
        return config::GROK_MODEL;
    }
    model
}

/// Go `grokRunArgs`: grok's argv. An empty model falls back to the default
/// here rather than at the call site, so every entry point inherits the
/// fallback.
pub(crate) fn grok_run_args(
    model: &str,
    prompt_file: &str,
    effort: &str,
    workdir: &str,
    review: bool,
) -> anyhow::Result<Vec<String>> {
    let mapped = grok_effort(effort)?;

    let mut args: Vec<String> = [
        "--prompt-file",
        prompt_file,
        "-m",
        grok_model_or_default(model),
        "--effort",
        &mapped,
        "--output-format",
        "plain",
        "--no-auto-update",
        "--yolo",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if !workdir.is_empty() {
        args.extend(["--cwd".to_string(), workdir.to_string()]);
    }
    if review {
        // Review pipelines run grok in its read-only sandbox; raw prompts
        // get none.
        args.extend(["--sandbox".to_string(), "read-only".to_string()]);
    }
    Ok(args)
}

/// Go `GrokEffort`: maps rival's effort menu onto grok-4.6's own
/// low/medium/high. Levels above high clamp to high and levels below low
/// clamp to low rather than failing a run over a level the model simply
/// does not expose; an unrecognized value is still an error so typos do not
/// silently downgrade. Public so callers can record the clamped value on
/// the session instead of the level the user asked for.
pub fn grok_effort(effort: &str) -> anyhow::Result<String> {
    match effort {
        "low" | "medium" | "high" => Ok(effort.to_string()),
        "xhigh" | "ultra" | "max" => Ok("high".to_string()),
        "minimal" | "none" => Ok("low".to_string()),
        "" => bail!("effort is required for {}", config::GROK_LABEL),
        _ => bail!(
            "unsupported {} effort {:?}; use one of: low, medium, high",
            config::GROK_LABEL,
            effort
        ),
    }
}
