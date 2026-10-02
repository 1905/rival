//! `rival command plan`: review a plan/spec file with Codex and/or Claude.
//! Go: `cmd/command_plan.go`.

use std::io::{self, Write};
use std::path::Path;

use rival_core::cancel::Context;
use rival_core::config::{
    self, CLAUDE_LABEL, CLAUDE_MODEL, CODEX_LABEL, CODEX_MODEL, Config, VALID_EFFORTS,
};
use rival_core::review::{self, PlanRunResult, ReviewBatch};
use rival_core::{gostd, paths};

use crate::root::{CmdEnv, CmdError};
use crate::tree::Invocation;
use crate::workdir::{getwd_error, resolve_workdir_or_exit};

#[cfg(test)]
mod tests;

pub const PLAN_USAGE: &str = "Usage:
  /rival-plan path/to/plan.md — review with Codex at xhigh effort
  /rival-plan-codex path/to/plan.md — review with Codex at xhigh effort
  /rival-plan-claude path/to/plan.md — review with Claude
  rival command plan --help — show native command options

Input is a single path to a markdown plan/spec file. The /rival-plan and
/rival-plan-codex skills always use xhigh. Native Codex effort defaults to xhigh
and Claude to medium, unless overridden per model in ~/.rival/config.yaml.
--model accepts codex and claude. An unavailable model is skipped, not fatal.";

/// Go `review.RunPlanReview`; tests inject a fake.
pub type PlanRunner<'a> = dyn Fn(&Context, &Config, &str, &ReviewBatch<'_>, &mut dyn Write) -> anyhow::Result<PlanRunResult>
    + 'a;

/// The flags `command plan` reads.
#[derive(Debug, Clone, Default)]
pub struct PlanOptions {
    pub workdir: String,
    pub no_queue: bool,
    /// `--model` values (`GetStringSlice`).
    pub models: Vec<String>,
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
            effort: inv.string("effort"),
            effort_set: inv.changed("effort"),
        }
    }
}

/// Go `commandPlanAction` with the real plan runner.
pub fn command_plan_action(env: &mut CmdEnv<'_>, inv: &Invocation) -> Result<(), CmdError> {
    run_command_plan(
        env,
        &PlanOptions::from_invocation(inv),
        &review::run_plan_review,
    )
}

/// Go `commandPlanAction`.
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

    let clis = parse_plan_models(&opts.models).map_err(|e| fail_on_stdout(env.stdout, e))?;
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

    let abs_path = resolve_plan_path(&raw_path, &workdir, cfg.getenv("HOME"), cfg.cwd())
        .map_err(|e| fail_on_stdout(env.stdout, e))?;

    // Cancel the queue wait / child processes on SIGINT/SIGTERM.
    let (ctx, _signals) = env.signal_context()?;

    let group_id = uuid::Uuid::new_v4().to_string();
    let batch = ReviewBatch {
        effort: &effort,
        workdir: &workdir,
        group_id: &group_id,
        no_queue: opts.no_queue,
        clis: &clis,
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

/// Go `fmt.Errorf("write stdout: %w", err)` for a failed `os.Stdout` write.
pub(crate) fn write_stdout_error(e: &io::Error) -> CmdError {
    CmdError::plain(format!(
        "write stdout: write /dev/stdout: {}",
        gostd::os_error_text(e)
    ))
}

/// Go `fmt.Errorf("invalid effort %q, must be one of: %v", effort,
/// config.ValidEfforts)` for the `--effort` flag; `%v` prints the slice as
/// `[low medium …]`.
pub(crate) fn invalid_flag_effort(effort: &str) -> String {
    format!(
        "invalid effort {}, must be one of: [{}]",
        gostd::quote(effort),
        VALID_EFFORTS.join(" ")
    )
}

/// Go `parsePlanModels`: validates model-facing selectors and maps them to
/// the internal adapters the plan runner uses. It de-duplicates by concrete
/// model while preserving the user's order.
pub(crate) fn parse_plan_models(raw: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for value in raw {
        for part in value.split(',') {
            let model = gostd::to_lower(part.trim());
            let cli = match model.as_str() {
                CODEX_LABEL | CODEX_MODEL => "codex",
                CLAUDE_LABEL | CLAUDE_MODEL => "claude",
                "" => return Err("model selector cannot be empty".to_string()),
                _ => {
                    return Err(format!(
                        "unknown plan model {}; use one of: codex, claude",
                        gostd::quote(part)
                    ));
                }
            };
            if !out.iter().any(|c| c == cli) {
                out.push(cli.to_string());
            }
        }
    }
    if out.is_empty() {
        return Err("no plan models selected".to_string());
    }
    Ok(out)
}

/// Go `mergePlanEffort`: an effort from stdin wins unless the flag was set
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
            "reasoning effort conflicts: command uses {} but plan arguments request {}",
            gostd::quote(flag_effort),
            gostd::quote(input_effort)
        ));
    }
    Ok(input_effort.to_string())
}

/// Go `parsePlanInput`: extracts an optional skill-facing `-re`/`--effort`
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
                "unknown plan option {}; use -re/--effort or -- before a path beginning with '-'",
                gostd::quote(option)
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
            "invalid effort {}, must be one of: {}",
            gostd::quote(effort),
            VALID_EFFORTS.join(", ")
        ));
    }
    let path = rest.trim();
    if path.is_empty() {
        return Err(format!("plan path is required after {name} {effort}"));
    }
    Ok((path.to_string(), effort.to_string()))
}

/// The separators of Go's `popPlanToken` (`TrimLeft`/`IndexAny`).
const TOKEN_SPACE: [char; 4] = [' ', '\t', '\r', '\n'];

/// Go `popPlanToken`.
fn pop_plan_token(s: &str) -> (&str, &str) {
    let s = s.trim_start_matches(TOKEN_SPACE);
    match s.find(TOKEN_SPACE) {
        Some(i) => (&s[..i], s[i..].trim_start_matches(TOKEN_SPACE)),
        None => (s, ""),
    }
}

/// Go `splitPlanOption`: `name=value` → `(name, Some(value))`.
fn split_plan_option(token: &str) -> (&str, Option<&str>) {
    match token.split_once('=') {
        Some((name, value)) => (name, Some(value)),
        None => (token, None),
    }
}

/// Go `filepath.Join(a, b)` on Unix: empty elements are dropped, the rest
/// joined with `/` and cleaned. Unlike [`Path::join`], an absolute `b` does
/// not replace `a`.
fn go_join(a: &str, b: &str) -> String {
    let parts: Vec<&str> = [a, b].into_iter().filter(|e| !e.is_empty()).collect();
    if parts.is_empty() {
        return String::new();
    }
    paths::clean(Path::new(&parts.join("/")))
        .to_string_lossy()
        .into_owned()
}

/// Go `resolvePlanPath`: turns the raw user-supplied path into a validated
/// absolute path to an existing regular file. Relative paths are resolved
/// against `workdir`. Any file name is accepted; `.md` is not required.
/// `home` is Go's `os.UserHomeDir()` (`$HOME`; empty = unavailable) and
/// `cwd` the `os.Getwd()` snapshot `filepath.Abs` would use.
pub(crate) fn resolve_plan_path(
    raw_path: &str,
    workdir: &str,
    home: &str,
    cwd: Option<&Path>,
) -> Result<String, String> {
    let mut p = raw_path.trim().to_string();
    // Expand a leading ~ to the home directory.
    if (p == "~" || p.starts_with("~/")) && !home.is_empty() {
        p = go_join(home, &p[1..]);
    }
    if !Path::new(&p).is_absolute() {
        p = go_join(workdir, &p);
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
            "resolve plan path {}: {}",
            gostd::quote(raw_path),
            getwd_error()
        ));
    };
    let abs = abs.to_string_lossy().into_owned();

    let meta = match std::fs::metadata(&abs) {
        // Go os.IsNotExist: ENOENT only.
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(format!("plan file not found: {abs}"));
        }
        Err(e) => {
            return Err(format!(
                "cannot read plan file {abs}: stat {abs}: {}",
                gostd::os_error_text(&e)
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
        return Err(format!(
            "cannot read plan file {abs}: open {abs}: {}",
            gostd::os_error_text(&e)
        ));
    }
    Ok(abs)
}
