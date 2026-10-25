//! What differs per model between the `command` and `run` surfaces, plus
//! the usage texts.
//!
//! The executor signatures are not uniform: grok takes a review flag, and
//! the others take effort but no flag. Each adapter
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

pub const SOL_USAGE: &str = "Usage:
  /rival-sol 'explain the auth flow' — run any prompt with Sol
  /rival-sol -re high 'find bugs in src/main.go' — run with a different reasoning effort (default xhigh)
  /rival-sol review — bug-hunting review of the changed files (git auto-detect)
  /rival-sol review src/api/ — review specific scope
  /rival-sol -re high review src/api/ — review with high reasoning
  /rival-sol — show this usage info

Reasoning effort (-re): low, medium, high, xhigh, ultra.
Omitted uses efforts.sol from ~/.rival/config.yaml (built-in: xhigh).";

pub const FABLE_USAGE: &str = "Usage:
  /rival-fable 'explain the auth flow' — run any prompt with Fable
  /rival-fable -re high 'find bugs in src/main.go' — run with a higher reasoning effort
  /rival-fable review — bug-hunting review of the changed files (git auto-detect)
  /rival-fable review src/api/ — review specific scope
  /rival-fable -re high review src/api/ — review with high reasoning
  /rival-fable — show this usage info

Reasoning effort (-re): low, medium, high, xhigh.
Omitted uses efforts.fable from ~/.rival/config.yaml (built-in default: medium).";

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

pub const CHINA_USAGE: &str = "Usage:
  /china 'explain the auth flow' — run any prompt with the China model
  /china -re high 'find bugs in src/main.rs' — run with a lower reasoning effort
  /china review — bug-hunting review of the changed files (git auto-detect)
  /china review src/api/ — review specific scope
  /china — show this usage info

The model is china.model in ~/.rival/config.yaml, an opencode provider/model
(default moonshot/kimi-k3.1-pro). Change it with:
  rival config set china.model <provider/model>
It runs through opencode v2 with opencode's own provider auth.
Reasoning effort (-re): low, medium, high, xhigh, ultra; Moonshot takes
low, high or max (medium -> high, xhigh/ultra -> max). Omitted: max.
Review mode runs read-only; raw prompts can edit files in the workdir.";

/// The usage of the `/rival` skill, for both hosts. It names no model: the
/// model comes from `skill.<host>`.
pub const RIVAL_USAGE: &str = "Usage:
  /rival 'explain the auth flow' — run any prompt with the configured model
  /rival -re high 'find bugs in src/main.go' — run with a different reasoning effort
  /rival review — bug-hunting review of the changed files (git auto-detect)
  /rival review src/api/ — review specific scope
  /rival -re high review src/api/ — review with high reasoning
  /rival security — security review of the changed files (git auto-detect)
  /rival security src/api/ — security review of a specific scope
  /rival — show this usage info

The model comes from skill.claude (in Claude Code) or skill.codex (in Codex)
in ~/.rival/config.yaml. Pick it in rival config (Models), or run:
  rival config set skill.claude <model>
Reasoning effort (-re): low, medium, high, xhigh, ultra.
Omitted uses efforts.<model> from ~/.rival/config.yaml.
Security reviews run on china.model, the /china model.";

/// One provider run, as the workflows hand it to [`ModelSpec::run`].
pub struct RunCall<'a, 'm> {
    pub ctx: &'a Context,
    pub cfg: &'a Config,
    pub sess: &'a mut Session,
    pub prompt: &'a str,
    pub effort: &'a str,
    /// Where the provider runs.
    pub workdir: &'a str,
    /// Whether the run is a review, so grok can apply its sandbox.
    pub review: bool,
    /// The file that gets the provider output; `None` is the session log.
    pub log: Option<&'a str>,
    /// The live stdout mirror; `None` in command mode.
    pub out: Mirror<'m>,
}

pub type PreflightFn = Box<dyn Fn(&Config) -> anyhow::Result<()>>;
pub type RunFn = Box<dyn Fn(RunCall<'_, '_>) -> anyhow::Result<RunResult>>;

/// One model's command and run surfaces. It carries only
/// what genuinely differs per model. Anything a single model needs stays an
/// explicit branch in the workflows, keyed on `command_name`.
pub struct ModelSpec {
    /// The command word: codex, sol, claude, fable, china, or grok.
    pub command_name: &'static str,
    /// The adapter recorded on the session: codex, claude, or the china or
    /// grok label.
    pub cli: &'static str,
    /// The concrete model id. The display and error label is always
    /// `config::engine_label(cli, model)`.
    pub model: &'static str,
    pub usage: &'static str,
    pub parse: fn(&str) -> anyhow::Result<ParseResult>,
    /// Verifies the runtime is usable.
    pub preflight: PreflightFn,
    /// Invokes the provider.
    pub run: RunFn,
}

impl ModelSpec {
    /// The public name used in every message and log field.
    pub fn label(&self) -> String {
        config::engine_label(self.cli, self.model)
    }

    /// china maps the ladder onto its provider's menu; grok clamps it onto
    /// its own shorter menu.
    pub fn resolve_effort(&self, cfg: &Config, requested: &str) -> Result<String, String> {
        if self.command_name == config::CHINA_LABEL {
            return executor::china_effort(requested)
                .map(str::to_string)
                .map_err(|e| format!("{e:#}"));
        }
        // The Claude and codex runtimes (Opus, Fable, Codex, Sol) resolve
        // their own configured defaults rather than the shared review one: a
        // non-empty fallback here short-circuits the built-in model effort
        // and would silently override Codex's xhigh.
        let fallback = if self.cli == "claude" || self.cli == "codex" {
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
    /// the provider has none. The Claude runtime distinguishes auth
    /// failures; the codex runtime has a hint on the proxy route only.
    pub fn auth_hint(&self, cfg: &Config, log_file: &str) -> String {
        match self.cli {
            "claude" => executor::claude_auth_hint(cfg, self.model, Path::new(log_file)),
            "codex" => executor::codex_auth_hint(cfg, self.model, Path::new(log_file)),
            _ => String::new(),
        }
    }
}

/// Names the run for the dashboards.
pub fn session_mode(is_review: bool) -> &'static str {
    if is_review { "review" } else { "raw" }
}

pub fn codex_spec() -> ModelSpec {
    codex_runtime_spec(config::CODEX_LABEL, config::CODEX_MODEL, CODEX_USAGE)
}

/// Sol 6.1 on the codex runtime.
pub fn sol_spec() -> ModelSpec {
    codex_runtime_spec(config::SOL_LABEL, config::SOL_MODEL, SOL_USAGE)
}

fn codex_runtime_spec(
    command_name: &'static str,
    model: &'static str,
    usage: &'static str,
) -> ModelSpec {
    ModelSpec {
        command_name,
        cli: "codex",
        model,
        usage,
        parse: parser::parse_codex_args,
        preflight: Box::new(move |cfg| executor::codex_preflight_for(cfg, model)),
        run: Box::new(move |c| {
            executor::run_codex_model(
                c.ctx, c.cfg, c.sess, c.prompt, c.effort, c.workdir, model, c.review, c.log, c.out,
            )
        }),
    }
}

pub fn claude_spec() -> ModelSpec {
    claude_runtime_spec(config::CLAUDE_LABEL, config::CLAUDE_MODEL, CLAUDE_USAGE)
}

/// Fable 5.1 on the Claude runtime.
pub fn fable_spec() -> ModelSpec {
    claude_runtime_spec(config::FABLE_LABEL, config::FABLE_MODEL, FABLE_USAGE)
}

fn claude_runtime_spec(
    command_name: &'static str,
    model: &'static str,
    usage: &'static str,
) -> ModelSpec {
    ModelSpec {
        command_name,
        cli: "claude",
        model,
        usage,
        parse: parser::parse_claude_args,
        preflight: Box::new(move |cfg| executor::claude_preflight_model(cfg, model)),
        run: Box::new(move |c| {
            executor::run_claude(
                c.ctx, c.cfg, c.sess, c.prompt, c.effort, c.workdir, model, c.review, c.log, c.out,
            )
        }),
    }
}

/// The `china.model` on opencode v2. The model comes from the config, so
/// this spec is built per run; the selector is leaked once to fit the
/// `&'static` field.
pub fn china_spec(cfg: &Config) -> ModelSpec {
    ModelSpec {
        command_name: config::CHINA_LABEL,
        cli: config::CHINA_LABEL,
        model: Box::leak(cfg.china_model().to_string().into_boxed_str()),
        usage: CHINA_USAGE,
        parse: parser::parse_codex_args,
        preflight: Box::new(executor::china_preflight),
        run: Box::new(|c| {
            executor::run_china(
                c.ctx, c.cfg, c.sess, c.prompt, c.effort, c.workdir, c.log, c.out,
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
        preflight: Box::new(executor::grok_preflight),
        // Grok sandboxes reviews and only reviews.
        run: Box::new(|c| {
            executor::run_grok(
                c.ctx, c.cfg, c.sess, c.prompt, c.effort, c.workdir, c.review, c.log, c.out,
            )
        }),
    }
}

/// The spec of a `rival command` model name (a [`config::SKILL_MODEL_NAMES`]
/// entry), or `None` for any other name.
pub fn spec_for_name(name: &str) -> Option<ModelSpec> {
    let spec = match name {
        config::CODEX_LABEL => codex_spec(),
        config::SOL_LABEL => sol_spec(),
        config::CLAUDE_LABEL => claude_spec(),
        config::FABLE_LABEL => fable_spec(),
        config::GROK_LABEL => grok_spec(),
        _ => return None,
    };
    Some(spec)
}
