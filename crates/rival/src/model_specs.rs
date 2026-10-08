//! What differs per model between the `command` and `run` surfaces, plus
//! the usage texts.
//!
//! The executor signatures are not uniform: grok takes a review flag, K3
//! takes no effort, and the others take effort but no flag. Each adapter
//! below absorbs that difference so both workflows can call one shape.

use std::path::Path;

use rival_core::cancel::Context;
use rival_core::config::{self, Config};
use rival_core::executor::{self, Mirror, RunResult};
use rival_core::parser::{self, ParseResult};
use rival_core::session::Session;

#[cfg(test)]
mod tests;

pub const CODEX_USAGE: &str = "Usage:
  /rival-codex 'explain the auth flow' — run any prompt with Codex
  /rival-codex -re high 'find bugs in src/main.go' — run with a different reasoning effort (default xhigh)
  /rival-codex review — bug-hunting review of the changed files (git auto-detect)
  /rival-codex review src/api/ — review specific scope
  /rival-codex -re high review src/api/ — review with high reasoning
  /rival-codex — show this usage info

Reasoning effort (-re): low, medium, high, xhigh, ultra.
Omitted uses efforts.codex from ~/.rival/config.yaml (built-in: xhigh).";

pub const CLAUDE_USAGE: &str = "Usage:
  /rival-claude 'explain the auth flow' — run any prompt with Claude
  /rival-claude -re high 'find bugs in src/main.go' — run with a higher reasoning effort
  /rival-claude review — bug-hunting review of the changed files (git auto-detect)
  /rival-claude review src/api/ — review specific scope
  /rival-claude -re high review src/api/ — review with high reasoning
  /rival-claude — show this usage info

Reasoning effort (-re): low, medium, high, xhigh.
Omitted uses efforts.claude from ~/.rival/config.yaml (built-in default: medium).";

pub const GROK_USAGE: &str = "Usage:
  /rival-grok 'explain the auth flow' — run any prompt with Grok
  /rival-grok -re high 'find bugs in src/main.go' — pick the reasoning level
  /rival-grok review — bug-hunting review of the changed files (git auto-detect)
  /rival-grok review src/api/ — review specific scope
  /rival-grok -re high review src/api/ — review with high reasoning
  /rival-grok — show this usage info

Reasoning effort (-re): low, medium, high (levels above high clamp to high).
Omitted uses efforts.grok from ~/.rival/config.yaml (built-in: high).
Review mode runs read-only sandboxed; raw prompts can edit files in the workdir.";

pub const K3_USAGE: &str = "Usage:
  echo 'explain the auth flow' | rival command k3
  echo 'review' | rival command k3
  echo 'review src/api/' | rival command k3
  rival command k3 < prompt.txt

Note: k3 runs Kimi K3 (moonshotai/kimi-k3 via opencode), a thinking-only model
pinned to max reasoning — the -re flag accepts low|medium|high|xhigh|ultra|max
and ignores the value. Needs MOONSHOT_API_KEY in the project .env (or exported).
Review mode runs read-only sandboxed (same profile as the other reviewers);
raw prompts run full auto and can edit files and run commands in the workdir.";

/// One provider run, as the workflows hand it to [`ModelSpec::run`].
pub struct RunCall<'a, 'm> {
    pub ctx: &'a Context,
    pub cfg: &'a Config,
    pub sess: &'a mut Session,
    pub prompt: &'a str,
    pub effort: &'a str,
    /// Where the provider runs.
    pub workdir: &'a str,
    /// The caller's project, where a `.env` credential is looked up. It
    /// differs from `workdir` only for a GitLab MR review in a temporary
    /// checkout.
    pub cred_workdir: &'a str,
    /// Whether the run is a review, so grok can apply its sandbox.
    pub review: bool,
    /// The live stdout mirror; `None` in command mode.
    pub out: Mirror<'m>,
}

pub type PreflightFn = Box<dyn Fn(&Config, &str) -> anyhow::Result<()>>;
pub type RunFn = Box<dyn Fn(RunCall<'_, '_>) -> anyhow::Result<RunResult>>;

/// One model's command and run surfaces. It carries only
/// what genuinely differs per model. Anything a single model needs stays an
/// explicit branch in the workflows, keyed on `command_name`.
pub struct ModelSpec {
    /// The command word: codex, claude, k3, or grok. For K3 this is NOT the
    /// display label, which is kimi-k3.
    pub command_name: &'static str,
    /// The adapter recorded on the session: codex, claude, opencode, or the
    /// grok label.
    pub cli: &'static str,
    /// The concrete model id. The display and error label is always
    /// `config::engine_label(cli, model)`.
    pub model: &'static str,
    pub usage: &'static str,
    pub parse: fn(&str) -> anyhow::Result<ParseResult>,
    /// Verifies the runtime is usable. Only K3 needs the workdir.
    pub preflight: PreflightFn,
    /// Invokes the provider.
    pub run: RunFn,
}

impl ModelSpec {
    /// The public name used in every message and log field.
    pub fn label(&self) -> String {
        config::engine_label(self.cli, self.model)
    }

    /// K3 is pinned to the only level its provider
    /// supports; grok clamps the shared ladder onto its own shorter menu.
    pub fn resolve_effort(&self, cfg: &Config, requested: &str) -> Result<String, String> {
        if self.command_name == config::K3_COMMAND_NAME {
            return Ok("max".to_string());
        }
        // Claude and Codex resolve their own configured defaults rather than
        // the shared review one: a non-empty fallback here short-circuits
        // the built-in model effort and would silently override Codex's xhigh.
        let fallback = if self.command_name == config::CLAUDE_LABEL
            || self.command_name == config::CODEX_LABEL
        {
            ""
        } else {
            config::DEFAULT_REVIEW_EFFORT
        };
        let effort = cfg
            .resolve_effort(self.model, requested, fallback)
            .map_err(|e| e.to_string())?;
        if self.command_name == config::GROK_LABEL {
            return executor::grok_effort(&effort).map_err(|e| format!("{e:#}"));
        }
        Ok(effort)
    }

    /// A provider-specific hint for a failed run, or "" when
    /// the provider has none. Only Claude distinguishes auth failures.
    pub fn auth_hint(&self, cfg: &Config, log_file: &str) -> String {
        if self.command_name != config::CLAUDE_LABEL {
            return String::new();
        }
        executor::claude_auth_hint(cfg, Path::new(log_file))
    }
}

/// Names the run for the dashboards.
pub fn session_mode(is_review: bool) -> &'static str {
    if is_review { "review" } else { "raw" }
}

pub fn codex_spec() -> ModelSpec {
    ModelSpec {
        command_name: config::CODEX_LABEL,
        cli: "codex",
        model: config::CODEX_MODEL,
        usage: CODEX_USAGE,
        parse: parser::parse_codex_args,
        preflight: Box::new(|cfg, _| executor::codex_preflight_for(cfg, config::CODEX_MODEL)),
        run: Box::new(|c| {
            executor::run_codex_model(
                c.ctx,
                c.cfg,
                c.sess,
                c.prompt,
                c.effort,
                c.workdir,
                config::CODEX_MODEL,
                c.out,
            )
        }),
    }
}

pub fn claude_spec() -> ModelSpec {
    ModelSpec {
        command_name: config::CLAUDE_LABEL,
        cli: "claude",
        model: config::CLAUDE_MODEL,
        usage: CLAUDE_USAGE,
        parse: parser::parse_claude_args,
        preflight: Box::new(|cfg, _| executor::claude_preflight(cfg)),
        run: Box::new(|c| {
            executor::run_claude(
                c.ctx, c.cfg, c.sess, c.prompt, c.effort, c.workdir, c.review, c.out,
            )
        }),
    }
}

pub fn k3_spec() -> ModelSpec {
    ModelSpec {
        command_name: config::K3_COMMAND_NAME,
        cli: "opencode",
        model: config::KIMI_MODEL,
        usage: K3_USAGE,
        parse: parser::parse_kimi_args,
        preflight: Box::new(executor::kimi_preflight),
        // K3 takes no effort: its provider exposes only max reasoning. It is
        // the only adapter whose credential lives in the project .env.
        run: Box::new(|c| {
            executor::run_kimi(
                c.ctx,
                c.cfg,
                c.sess,
                c.prompt,
                c.workdir,
                c.cred_workdir,
                c.out,
            )
        }),
    }
}

pub fn grok_spec() -> ModelSpec {
    ModelSpec {
        command_name: config::GROK_LABEL,
        cli: config::GROK_LABEL,
        model: config::GROK_MODEL,
        usage: GROK_USAGE,
        parse: parser::parse_grok_args,
        preflight: Box::new(|cfg, _| executor::grok_preflight(cfg)),
        // Grok sandboxes reviews and only reviews.
        run: Box::new(|c| {
            executor::run_grok(
                c.ctx, c.cfg, c.sess, c.prompt, c.effort, c.workdir, c.review, c.out,
            )
        }),
    }
}
