//! `rival command plan`: review a plan/spec file with Codex, Sol, Claude
//! and/or Fable.

use std::io::{self, Write};
use std::path::Path;

use rival_core::cancel::Context;
use rival_core::config::{
    self, CLAUDE_LABEL, CLAUDE_MODEL, CODEX_LABEL, CODEX_MODEL, Config, FABLE_LABEL, FABLE_MODEL,
    OPUS_ALIAS, SOL_LABEL, SOL_MODEL, VALID_EFFORTS,
};
use rival_core::paths;
use rival_core::review::{self, PlanRunResult, ReviewBatch};

use crate::root::{CmdEnv, CmdError};
use crate::tree::Invocation;
use crate::workdir::{getwd_error, resolve_workdir_or_exit};

#[cfg(test)]
mod tests;

pub const PLAN_USAGE: &str = "Usage:
  /rival-plan path/to/plan.md — review with Codex at xhigh effort
  /rival-plan-codex path/to/plan.md — review with Codex at xhigh effort
  /rival-plan-claude path/to/plan.md — review with Claude
  /rival-plan -m opus,fable,sol path/to/plan.md — review with several models
  rival command plan --help — show native command options

Input is a single path to a markdown plan/spec file. The /rival-plan and
/rival-plan-codex skills always use xhigh. Native Codex and Sol effort defaults
to xhigh, Claude and Fable to medium, unless overridden per model in
~/.rival/config.yaml. --model accepts codex, sol, claude (or opus) and fable;
without it, plan.models from ~/.rival/config.yaml, else codex. An unavailable
model is skipped, not fatal.";

/// Runs the plan review; tests inject a fake.
pub type PlanRunner<'a> = dyn Fn(&Context, &Config, &str, &ReviewBatch<'_>, &mut dyn Write) -> anyhow::Result<PlanRunResult>
    + 'a;

/// The flags `command plan` reads.
#[derive(Debug, Clone, Default)]
pub struct PlanOptions {
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

impl PlanOptions {
    pub fn from_invocation(inv: &Invocation) -> Self {
        PlanOptions {
            workdir: inv.string("workdir"),
            no_queue: inv.bool("no-queue"),
            models: inv.strings("model"),
            models_set: inv.changed("model"),
            effort: inv.string("effort"),
            effort_set: inv.changed("effort"),
        }
    }
}

/// `rival plan` with the real plan runner.
pub fn command_plan_action(env: &mut CmdEnv<'_>, inv: &Invocation) -> Result<(), CmdError> {
    run_command_plan(
        env,
        &PlanOptions::from_invocation(inv),
        &review::run_plan_review,
    )
}

pub fn run_command_plan(
    env: &mut CmdEnv<'_>,
    opts: &PlanOptions,
    run: &PlanRunner<'_>,
) -> Result<(), CmdError> {
    let cfg = env.cfg;
    let workdir = resolve_workdir_or_exit(cfg, &opts.workdir, env.stdout)?;
    // Cobra retains the historical flag default for help/API compatibility.
    // Keep an omitted invocation empty so each selected model can resolve
    // its own configured or surface-specific default.
    let effort = if opts.effort_set {
        opts.effort.as_str()
    } else {
        ""
    };

    if !effort.is_empty() && !config::is_valid_effort(effort) {
        return Err(fail_on_stdout(env.stdout, invalid_flag_effort(effort)));
    }

    // No --model: plan.models from the config, else the flag default (codex).
    let selected = if !opts.models_set && !cfg.plan_models().is_empty() {
        cfg.plan_models()
    } else {
        opts.models.as_slice()
    };
    let models = parse_plan_models(selected).map_err(|e| fail_on_stdout(env.stdout, e))?;
    // A terminal stdin means no piped path: show usage instead of hanging.
    if env.stdin.is_char_device() {
        let _ = writeln!(env.stdout, "{PLAN_USAGE}");
        return Ok(());
    }

    let raw = env
        .stdin
        .read_all()
        .map_err(|e| CmdError::plain(format!("read stdin: {e}")))?;
    let raw = String::from_utf8_lossy(&raw);

    let (raw_path, input_effort) =
        parse_plan_input(&raw).map_err(|e| fail_on_stdout(env.stdout, e))?;
    let effort = merge_plan_effort(effort, opts.effort_set, &input_effort)
        .map_err(|e| fail_on_stdout(env.stdout, e))?;
    if raw_path.is_empty() {
        let _ = writeln!(env.stdout, "{PLAN_USAGE}");
        return Ok(());
    }

    let abs_path = resolve_plan_path(&raw_path, &workdir, cfg.getenv(paths::HOME_VAR), cfg.cwd())
        .map_err(|e| fail_on_stdout(env.stdout, e))?;

    // Cancel the queue wait / child processes on SIGINT/SIGTERM.
    let (ctx, _signals) = env.signal_context()?;

    let group_id = uuid::Uuid::new_v4().to_string();
    let batch = ReviewBatch {
        effort: &effort,
        workdir: &workdir,
        group_id: &group_id,
        no_queue: opts.no_queue,
        models: &models,
    };
    let result = run(&ctx, cfg, &abs_path, &batch, env.stderr)
        .map_err(|e| CmdError::plain(format!("{e:#}")))?;

    let out = review::format_plan_result(Some(&result), &abs_path);
    env.stdout
        .write_all(out.as_bytes())
        .map_err(|e| write_stdout_error(&e))
}

/// Prints `msg` on stdout, where the calling skill captures it, and fails
/// the command with exit code 1. The root prints it again on stderr.
pub(crate) fn fail_on_stdout(stdout: &mut dyn Write, msg: String) -> CmdError {
    let _ = writeln!(stdout, "{msg}");
    CmdError::exit(1, msg)
}

/// The error for a failed stdout write, prefixed `write stdout: `. A missing
/// Windows stdout fails with the bare invalid-argument error, without the
/// op and path.
pub(crate) fn write_stdout_error(e: &io::Error) -> CmdError {
    if crate::root::is_nil_file(e) {
        return CmdError::plain(format!("write stdout: {e}"));
    }
    CmdError::plain(format!("write stdout: write /dev/stdout: {}", e))
}

/// The error for a bad `--effort` flag. The valid efforts print as
/// `[low medium …]`.
pub(crate) fn invalid_flag_effort(effort: &str) -> String {
    format!(
        "invalid effort {:?}, must be one of: [{}]",
        effort,
        VALID_EFFORTS.join(" ")
    )
}

/// Validates model-facing selectors and maps them to the model ids the
/// plan runner uses. It de-duplicates by model id (`opus` and `claude` are
/// one model) while preserving the user's order.
pub(crate) fn parse_plan_models(raw: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for value in raw {
        for part in value.split(',') {
            let name = part.trim().to_lowercase();
            let model = match name.as_str() {
                CODEX_LABEL | CODEX_MODEL => CODEX_MODEL,
                SOL_LABEL | SOL_MODEL => SOL_MODEL,
                CLAUDE_LABEL | OPUS_ALIAS | CLAUDE_MODEL => CLAUDE_MODEL,
                FABLE_LABEL | FABLE_MODEL => FABLE_MODEL,
                "" => return Err("model selector cannot be empty".to_string()),
                _ => {
                    return Err(format!(
                        "unknown plan model {:?}; use one of: codex, sol, claude, opus, fable",
                        part
                    ));
                }
            };
            if !out.iter().any(|m| m == model) {
                out.push(model.to_string());
            }
        }
    }
    if out.is_empty() {
        return Err("no plan models selected".to_string());
    }
    Ok(out)
}

/// An effort from stdin wins unless the flag was set
/// to a different value.
pub(crate) fn merge_plan_effort(
    flag_effort: &str,
    flag_set: bool,
    input_effort: &str,
) -> Result<String, String> {
    if input_effort.is_empty() {
        return Ok(flag_effort.to_string());
    }
    if flag_set && input_effort != flag_effort {
        return Err(format!(
            "reasoning effort conflicts: command uses {:?} but plan arguments request {:?}",
            flag_effort, input_effort
        ));
    }
    Ok(input_effort.to_string())
}

/// Extracts an optional skill-facing `-re`/`--effort`
/// prefix while leaving the rest of the input intact as the path (spaces
/// included). A leading `-- ` escapes a path beginning with a dash.
/// Returns `(path, effort)`.
pub(crate) fn parse_plan_input(raw: &str) -> Result<(String, String), String> {
    let s = raw.trim();
    if s.is_empty() {
        return Ok((String::new(), String::new()));
    }
    if s.starts_with("-- ") {
        return Ok((s["--".len()..].trim().to_string(), String::new()));
    }

    let (option, rest) = pop_plan_token(s);
    let (name, inline) = split_plan_option(option);
    if name != "-re" && name != "--effort" {
        if option.starts_with('-') {
            return Err(format!(
                "unknown plan option {:?}; use -re/--effort or -- before a path beginning with '-'",
                option
            ));
        }
        return Ok((s.to_string(), String::new()));
    }

    let (effort, rest) = match inline {
        Some(value) => (value.trim(), rest),
        None => {
            if rest.trim().is_empty() {
                return Err(format!("option {name} requires a value"));
            }
            pop_plan_token(rest)
        }
    };
    if effort.is_empty() || effort.starts_with('-') {
        return Err(format!("option {name} requires a value"));
    }
    if !config::is_valid_effort(effort) {
        return Err(format!(
            "invalid effort {:?}, must be one of: {}",
            effort,
            VALID_EFFORTS.join(", ")
        ));
    }
    let path = rest.trim();
    if path.is_empty() {
        return Err(format!("plan path is required after {name} {effort}"));
    }
    Ok((path.to_string(), effort.to_string()))
}

/// The separators [`pop_plan_token`] splits on.
const TOKEN_SPACE: [char; 4] = [' ', '\t', '\r', '\n'];

fn pop_plan_token(s: &str) -> (&str, &str) {
    let s = s.trim_start_matches(TOKEN_SPACE);
    match s.find(TOKEN_SPACE) {
        Some(i) => (&s[..i], s[i..].trim_start_matches(TOKEN_SPACE)),
        None => (s, ""),
    }
}

/// `name=value` → `(name, Some(value))`.
fn split_plan_option(token: &str) -> (&str, Option<&str>) {
    match token.split_once('=') {
        Some((name, value)) => (name, Some(value)),
        None => (token, None),
    }
}

/// A lexical join for the host ([`paths::join`]): empty elements
/// are dropped, the rest joined and cleaned; Windows keeps drive and UNC
/// volumes. Unlike [`Path::join`], an absolute `b` does not replace `a`.
fn lexical_join(a: &str, b: &str) -> String {
    paths::join(Path::new(a), Path::new(b))
        .to_string_lossy()
        .into_owned()
}

/// The op in a failed stat's error: `GetFileAttributesEx` is Windows' first
/// stat call.
const STAT_OP: &str = if cfg!(windows) {
    "GetFileAttributesEx"
} else {
    "stat"
};

/// Turns the raw user-supplied path into a validated
/// absolute path to an existing regular file. Relative paths are resolved
/// against `workdir`. Any file name is accepted; `.md` is not required.
/// `home` is the home directory (`$HOME`, `%USERPROFILE%` on Windows;
/// empty = unavailable) and `cwd` the current directory snapshot used to
/// make paths absolute. The absolute check, join and absolutize follow the
/// host's rules.
pub(crate) fn resolve_plan_path(
    raw_path: &str,
    workdir: &str,
    home: &str,
    cwd: Option<&Path>,
) -> Result<String, String> {
    let mut p = raw_path.trim().to_string();
    // Expand a leading ~ to the home directory.
    if (p == "~" || p.starts_with("~/")) && !home.is_empty() {
        p = lexical_join(home, &p[1..]);
    }
    if !paths::is_abs(Path::new(&p)) {
        p = lexical_join(workdir, &p);
    }
    // Reject control characters (e.g. a newline in the file name): the path
    // is interpolated into the model prompt, so a control char could inject
    // prompt text. Real plan files never have these in their path.
    if let Some((i, _)) = p
        .char_indices()
        .find(|&(_, c)| (c as u32) < 0x20 || c == '\x7f')
    {
        return Err(format!(
            "plan path contains a control character at position {i} — refusing"
        ));
    }

    let Some(abs) = paths::abs(cwd, Path::new(&p)) else {
        return Err(format!(
            "resolve plan path {:?}: {}",
            raw_path,
            getwd_error()
        ));
    };
    let abs = abs.to_string_lossy().into_owned();

    let meta = match std::fs::metadata(&abs) {
        // Not found: ENOENT only.
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(format!("plan file not found: {abs}"));
        }
        Err(e) => {
            return Err(format!(
                "cannot read plan file {abs}: {STAT_OP} {abs}: {}",
                e
            ));
        }
        Ok(meta) => meta,
    };
    if meta.is_dir() {
        return Err(format!("plan path is a directory, not a file: {abs}"));
    }
    if !meta.is_file() {
        return Err(format!("plan path is not a regular file: {abs}"));
    }
    // Confirm the file is readable now, so an unreadable file fails here
    // with a clear message rather than later inside a model runner.
    if let Err(e) = std::fs::File::open(&abs) {
        return Err(format!("cannot read plan file {abs}: open {abs}: {}", e));
    }
    Ok(abs)
}
