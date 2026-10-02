//! `rival command antislop`: quality-only slop and over-engineering review.
//! Go: `cmd/command_antislop.go`.

use std::io::Write;

use rival_core::cancel::Context;
use rival_core::config::{self, Config, DEFAULT_ANTISLOP_EFFORT};
use rival_core::parser::parse_review_args;
use rival_core::review::{self, DocReview, PlanRunResult, ReviewBatch};
use rival_core::session::MODE_ANTISLOP;

use crate::command_plan::{
    fail_on_stdout, invalid_flag_effort, merge_plan_effort, parse_plan_models, write_stdout_error,
};
use crate::gitscope_helper::{antislop_code_prompt, build_review_prompt};
use crate::merge_request::reject_unresolved_mr;
use crate::root::{CmdEnv, CmdError};
use crate::tree::Invocation;
use crate::workdir::resolve_workdir_or_exit;

#[cfg(test)]
mod tests;

pub const ANTISLOP_USAGE: &str = "Usage:
  /rival-antislop — quality-only review of the changed files (git auto-detect)
  /rival-antislop src/api/ — review a specific scope
  /rival-antislop -m claude -re high src/ — pick model and reasoning effort
  rival command antislop --help — show native command options

Antislop hunts slop and over-engineering — reuse/DRY, simplification,
efficiency, altitude, backward-compat hoarding, library reinvention, comment
and wrapper slop — and returns a leanness rating (1-10) plus a cut list. It
never reports bugs; use the code review commands for that.

Input is a code-review scope. \"--\" ends option parsing and takes the rest
verbatim, so a scope beginning with a dash is still reviewable. Default models
are codex and claude, each printing its own block; -m accepts codex and claude
(comma-separated), so -m claude runs Claude alone. Default reasoning effort is
high; override with -re/--effort or per model in ~/.rival/config.yaml.";

/// Go `review.RunDocReview`; tests inject a fake.
pub type DocRunner<'a> = dyn Fn(
        &Context,
        &Config,
        &DocReview<'_>,
        &ReviewBatch<'_>,
        &mut dyn Write,
    ) -> anyhow::Result<PlanRunResult>
    + 'a;

/// The flags `command antislop` reads.
#[derive(Debug, Clone, Default)]
pub struct AntislopOptions {
    pub workdir: String,
    pub no_queue: bool,
    /// `--model` values (`GetStringSlice`).
    pub models: Vec<String>,
    /// `Flags().Changed("model")`.
    pub models_set: bool,
    pub effort: String,
    /// `Flags().Changed("effort")`.
    pub effort_set: bool,
}

impl AntislopOptions {
    pub fn from_invocation(inv: &Invocation) -> Self {
        AntislopOptions {
            workdir: inv.string("workdir"),
            no_queue: inv.bool("no-queue"),
            models: inv.strings("model"),
            models_set: inv.changed("model"),
            effort: inv.string("effort"),
            effort_set: inv.changed("effort"),
        }
    }
}

/// Go `commandAntislopAction` with the real doc runner.
pub fn command_antislop_action(env: &mut CmdEnv<'_>, inv: &Invocation) -> Result<(), CmdError> {
    run_command_antislop(
        env,
        &AntislopOptions::from_invocation(inv),
        &review::run_doc_review,
    )
}

/// Go `commandAntislopAction`.
pub fn run_command_antislop(
    env: &mut CmdEnv<'_>,
    opts: &AntislopOptions,
    run: &DocRunner<'_>,
) -> Result<(), CmdError> {
    let cfg = env.cfg;
    let workdir = resolve_workdir_or_exit(cfg, &opts.workdir, env.stdout)?;

    if opts.effort_set && !config::is_valid_effort(&opts.effort) {
        return Err(fail_on_stdout(
            env.stdout,
            invalid_flag_effort(&opts.effort),
        ));
    }

    // A terminal stdin means no piped scope: show usage instead of hanging.
    if env.stdin.is_char_device() {
        let _ = writeln!(env.stdout, "{ANTISLOP_USAGE}");
        return Ok(());
    }

    let raw = env
        .stdin
        .read_all()
        .map_err(|e| CmdError::plain(format!("read stdin: {e}")))?;
    let raw = String::from_utf8_lossy(&raw);

    let parsed =
        parse_review_args(&raw).map_err(|e| fail_on_stdout(env.stdout, format!("{e:#}")))?;
    if parsed.is_empty {
        let _ = writeln!(env.stdout, "{ANTISLOP_USAGE}");
        return Ok(());
    }
    reject_unresolved_mr(&raw).map_err(CmdError::plain)?;

    if opts.models_set && !parsed.models.is_empty() {
        return Err(fail_on_stdout(
            env.stdout,
            "model selection was provided both as --model command flags and in arguments; use one form"
                .to_string(),
        ));
    }
    let selectors = if parsed.models.is_empty() {
        &opts.models
    } else {
        &parsed.models
    };
    let clis = parse_plan_models(selectors).map_err(|e| fail_on_stdout(env.stdout, e))?;

    let effort = merge_plan_effort(&opts.effort, opts.effort_set, &parsed.effort)
        .map_err(|e| fail_on_stdout(env.stdout, e))?;

    let (prompt, target, display) = build_review_prompt(
        antislop_code_prompt,
        &parsed.review_scope,
        parsed.auto_scope,
        cfg,
        &workdir,
    );

    // Cancel the queue wait / child processes on SIGINT/SIGTERM.
    let (ctx, _signals) = env.signal_context()?;

    let group_id = uuid::Uuid::new_v4().to_string();
    let doc = DocReview {
        mode: MODE_ANTISLOP,
        prompt: &prompt,
        target: &target,
        fallback_effort: DEFAULT_ANTISLOP_EFFORT,
    };
    let batch = ReviewBatch {
        effort: &effort,
        workdir: &workdir,
        group_id: &group_id,
        no_queue: opts.no_queue,
        clis: &clis,
    };
    let result =
        run(&ctx, cfg, &doc, &batch, env.stderr).map_err(|e| CmdError::plain(format!("{e:#}")))?;

    let out = review::format_antislop_result(Some(&result), &display);
    env.stdout
        .write_all(out.as_bytes())
        .map_err(|e| write_stdout_error(&e))
}
