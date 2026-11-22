//! `rival command consilium`: 3-judge rated code review (normal or heavy mode)
//! with detailed subsection rating breakdowns and actionable findings for any
//! goal (bugs by default, or design, architecture, tests, etc.).
//!
//! Normal mode runs Claude (Opus 5.5), Sol (Sol 6.2) and Neon (xhigh).
//! Heavy mode runs Fable (Fable 5.2), Codex (Astra 6.2) and Neon (xhigh).

use std::io::Write;

use rival_core::cancel::Context;
use rival_core::config::{self, Config, VALID_EFFORTS, WHOLE_PROJECT};
use rival_core::review::{
    self, ConsiliumMode, ConsiliumRequest, ConsiliumRunResult, build_consilium_prompt,
};

use crate::command_plan::{fail_on_stdout, invalid_flag_effort, write_stdout_error};
use crate::gitscope_helper::build_review_prompt;
use crate::merge_request::prepare_review_target_with;
use crate::model_command::print_notice;
use crate::root::{CmdEnv, CmdError};
use crate::tree::Invocation;
use crate::workdir::resolve_workdir_or_exit;

#[cfg(test)]
mod tests;

pub const CONSILIUM_USAGE: &str = "Usage:
  /consilium — 3-judge rated review of changed files (normal: claude, sol, neon)
  /consilium src/api/ — 3-judge rated review of a specific scope (normal mode)
  /consilium heavy src/api/ — 3-judge rated review in heavy mode (fable, codex, neon)
  /consilium --goal 'design fixes' src/api/ — rated review for a custom goal
  rival command consilium [--heavy] [--goal <goal>] [--effort <level>] [--workdir <dir>] [--no-queue]

Normal mode runs Claude (Opus 5.5), Sol (Sol 6.2) and Neon (xhigh).
Heavy mode runs Fable (Fable 5.2), Codex (Astra 6.2) and Neon (xhigh).
Each judge returns a 1-10 rating, a subsection rating breakdown, and findings
against the goal (concrete bugs by default). Reasoning effort (-re/--effort)
overrides Claude/Codex judges; Neon always runs at xhigh. If Neon (or another
judge) fails, Rival notifies you and skips it.";

/// Runs the 3-judge consilium review; tests inject a fake.
pub type ConsiliumRunner<'a> = dyn Fn(
        &Context,
        &Config,
        &ConsiliumRequest<'_>,
        &mut dyn Write,
    ) -> anyhow::Result<ConsiliumRunResult>
    + 'a;

/// The flags `command consilium` reads.
#[derive(Debug, Clone, Default)]
pub struct ConsiliumOptions {
    pub workdir: String,
    pub no_queue: bool,
    pub heavy: bool,
    pub goal: String,
    pub goal_set: bool,
    pub effort: String,
    pub effort_set: bool,
}

impl ConsiliumOptions {
    pub fn from_invocation(inv: &Invocation) -> Self {
        ConsiliumOptions {
            workdir: inv.string("workdir"),
            no_queue: inv.bool("no-queue"),
            heavy: inv.bool("heavy"),
            goal: inv.string("goal"),
            goal_set: inv.changed("goal"),
            effort: inv.string("effort"),
            effort_set: inv.changed("effort"),
        }
    }
}

/// `rival command consilium` with the real consilium runner.
pub fn command_consilium_action(env: &mut CmdEnv<'_>, inv: &Invocation) -> Result<(), CmdError> {
    run_command_consilium(
        env,
        &ConsiliumOptions::from_invocation(inv),
        &review::run_consilium_review,
    )
}

/// Parsed stdin arguments for `rival command consilium`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedConsiliumInput {
    pub mode: Option<ConsiliumMode>,
    pub effort: String,
    pub goal: String,
    pub scope: String,
    pub auto_scope: bool,
}

pub fn run_command_consilium(
    env: &mut CmdEnv<'_>,
    opts: &ConsiliumOptions,
    run: &ConsiliumRunner<'_>,
) -> Result<(), CmdError> {
    let cfg = env.cfg;
    let workdir = resolve_workdir_or_exit(cfg, &opts.workdir, env.stdout)?;
    let flag_effort = if opts.effort_set {
        opts.effort.as_str()
    } else {
        ""
    };
    if !flag_effort.is_empty() && !config::is_valid_effort(flag_effort) {
        return Err(fail_on_stdout(env.stdout, invalid_flag_effort(flag_effort)));
    }

    // A terminal stdin means no piped args: show usage instead of hanging.
    if env.stdin.is_char_device() {
        let _ = writeln!(env.stdout, "{CONSILIUM_USAGE}");
        return Ok(());
    }

    let mut raw = String::new();
    if !env.stdin.stat_failed() {
        let bytes = env
            .stdin
            .read_all()
            .map_err(|e| CmdError::plain(format!("read stdin: {e}")))?;
        raw = String::from_utf8_lossy(&bytes).into_owned();
    }

    let parsed = parse_consilium_input(&raw).map_err(|e| fail_on_stdout(env.stdout, e))?;
    let effort = merge_consilium_effort(flag_effort, opts.effort_set, &parsed.effort)
        .map_err(|e| fail_on_stdout(env.stdout, e))?;
    let mode =
        merge_consilium_mode(opts.heavy, parsed.mode).map_err(|e| fail_on_stdout(env.stdout, e))?;
    let flag_goal = if opts.goal_set { opts.goal.trim() } else { "" };
    let goal = merge_consilium_goal(flag_goal, opts.goal_set, &parsed.goal)
        .map_err(|e| fail_on_stdout(env.stdout, e))?;

    let (ctx, _signals) = env.signal_context()?;
    let prepare = env.prepare_mr;
    let target = prepare_review_target_with(
        &ctx,
        cfg,
        &parsed.scope,
        &workdir,
        env.stdout,
        |c, cfg, s, w| prepare(c, cfg, s, w),
    )
    .map_err(CmdError::plain)?;
    let (prompt, recorded, _) = build_review_prompt(
        |scope| build_consilium_prompt(cfg, scope, &goal),
        &target.scope,
        parsed.auto_scope,
        cfg,
        &target.workdir,
    );
    let display_scope = if parsed.auto_scope {
        recorded
    } else {
        target.display.clone()
    };
    let run_workdir = target.workdir.clone();

    let group_id = uuid::Uuid::new_v4().to_string();
    let req = ConsiliumRequest {
        mode,
        prompt: &prompt,
        scope: &display_scope,
        goal: &goal,
        effort: &effort,
        workdir: &run_workdir,
        group_id: &group_id,
        no_queue: opts.no_queue,
    };
    let result = run(&ctx, cfg, &req, env.stderr).map_err(|e| CmdError::plain(format!("{e:#}")))?;

    let out = review::format_consilium_result(&result, &display_scope, &goal);
    env.stdout
        .write_all(out.as_bytes())
        .map_err(|e| write_stdout_error(&e))?;
    for notice in &result.notices {
        print_notice(env, notice);
    }
    Ok(())
}

fn merge_consilium_mode(
    flag_heavy: bool,
    input_mode: Option<ConsiliumMode>,
) -> Result<ConsiliumMode, String> {
    match (flag_heavy, input_mode) {
        (true, Some(ConsiliumMode::Normal)) => Err(
            "consilium mode conflicts: --heavy flag requested heavy mode, but arguments requested normal"
                .to_string(),
        ),
        (true, _) => Ok(ConsiliumMode::Heavy),
        (false, mode) => Ok(mode.unwrap_or(ConsiliumMode::Normal)),
    }
}

fn merge_consilium_effort(
    flag_effort: &str,
    flag_set: bool,
    input_effort: &str,
) -> Result<String, String> {
    if input_effort.is_empty() {
        return Ok(flag_effort.to_string());
    }
    if flag_set && input_effort != flag_effort {
        return Err(format!(
            "reasoning effort conflicts: command uses {:?} but consilium arguments request {:?}",
            flag_effort, input_effort
        ));
    }
    Ok(input_effort.to_string())
}

fn merge_consilium_goal(
    flag_goal: &str,
    flag_set: bool,
    input_goal: &str,
) -> Result<String, String> {
    let input_goal = input_goal.trim();
    if input_goal.is_empty() {
        return Ok(flag_goal.to_string());
    }
    if flag_set && !flag_goal.is_empty() && input_goal != flag_goal {
        return Err(format!(
            "consilium goal conflicts: --goal uses {:?} but arguments request {:?}",
            flag_goal, input_goal
        ));
    }
    Ok(input_goal.to_string())
}

/// One parsed shell-like word from stdin, remembering whether it began with a
/// quote (so a quoted `"heavy"` or `"--goal"` is treated as literal scope text,
/// while `--goal="multi word"` still parses as an option).
struct Word {
    text: String,
    starts_quoted: bool,
}

/// Reads the next whitespace-separated token, respecting `'...'` and `"..."`
/// quotes and `\"` / `\\` escapes inside double quotes.
fn next_word(mut s: &str) -> Result<Option<(Word, &str)>, String> {
    s = s.trim_start();
    if s.is_empty() {
        return Ok(None);
    }
    let starts_quoted = s.starts_with(['\'', '"']);
    let mut out = String::new();
    let mut chars = s.char_indices().peekable();
    let mut end = s.len();
    while let Some((i, c)) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                end = i;
                break;
            }
            '\'' => {
                let mut closed = false;
                for (_, qc) in chars.by_ref() {
                    if qc == '\'' {
                        closed = true;
                        break;
                    }
                    out.push(qc);
                }
                if !closed {
                    return Err("unclosed single quote in consilium arguments".to_string());
                }
            }
            '"' => {
                let mut closed = false;
                while let Some((_, qc)) = chars.next() {
                    if qc == '"' {
                        closed = true;
                        break;
                    }
                    if qc == '\\'
                        && let Some(&(_, next)) = chars.peek()
                        && matches!(next, '"' | '\\')
                    {
                        chars.next();
                        out.push(next);
                        continue;
                    }
                    out.push(qc);
                }
                if !closed {
                    return Err("unclosed double quote in consilium arguments".to_string());
                }
            }
            _ => out.push(c),
        }
    }
    Ok(Some((
        Word {
            text: out,
            starts_quoted,
        },
        &s[end..],
    )))
}

/// Parses stdin arguments for `rival command consilium`:
/// `[-re|--effort <level>] [heavy|--heavy|normal] [--goal|-g <text>] [--] [scope]`.
/// Options may appear before or after the scope (except after `--`, which keeps
/// the remainder verbatim as the scope).
pub(crate) fn parse_consilium_input(raw: &str) -> Result<ParsedConsiliumInput, String> {
    let mut rest = raw.trim();
    let mut mode: Option<ConsiliumMode> = None;
    let mut effort = String::new();
    let mut goal = String::new();
    let mut scope_parts: Vec<String> = Vec::new();

    while let Some((word, after)) = next_word(rest)? {
        rest = after;
        if !word.starts_quoted && word.text == "--" {
            let tail = rest.trim();
            if !tail.is_empty() {
                scope_parts.push(tail.to_string());
            }
            break;
        }
        if !word.starts_quoted && scope_parts.is_empty() {
            let lower = word.text.to_ascii_lowercase();
            if lower == "heavy" || lower == "--heavy" {
                if mode == Some(ConsiliumMode::Normal) {
                    return Err(
                        "consilium mode conflicts: both normal and heavy requested".to_string()
                    );
                }
                mode = Some(ConsiliumMode::Heavy);
                continue;
            }
            if lower == "normal" {
                if mode == Some(ConsiliumMode::Heavy) {
                    return Err(
                        "consilium mode conflicts: both heavy and normal requested".to_string()
                    );
                }
                mode = Some(ConsiliumMode::Normal);
                continue;
            }
        }
        if !word.starts_quoted {
            if word.text == "--heavy" {
                if mode == Some(ConsiliumMode::Normal) {
                    return Err(
                        "consilium mode conflicts: both normal and heavy requested".to_string()
                    );
                }
                mode = Some(ConsiliumMode::Heavy);
                continue;
            }
            let (name, inline) = match word.text.split_once('=') {
                Some((n, v)) => (n, Some(v)),
                None => (word.text.as_str(), None),
            };
            if name == "-re" || name == "--effort" {
                let val = match inline {
                    Some(v) => v.trim().to_string(),
                    None => match next_word(rest)? {
                        Some((w, after_val)) => {
                            rest = after_val;
                            w.text.trim().to_string()
                        }
                        None => return Err(format!("option {name} requires a value")),
                    },
                };
                if val.is_empty() || val.starts_with('-') {
                    return Err(format!("option {name} requires a value"));
                }
                if !config::is_valid_effort(&val) {
                    return Err(format!(
                        "invalid effort {:?}, must be one of: {}",
                        val,
                        VALID_EFFORTS.join(", ")
                    ));
                }
                if !effort.is_empty() && effort != val {
                    return Err(format!(
                        "reasoning effort conflicts: {effort:?} and {val:?}"
                    ));
                }
                effort = val;
                continue;
            }
            if name == "--goal" || name == "-g" {
                let val = match inline {
                    Some(v) => v.trim().to_string(),
                    None => match next_word(rest)? {
                        Some((w, after_val)) => {
                            rest = after_val;
                            w.text.trim().to_string()
                        }
                        None => return Err(format!("option {name} requires a value")),
                    },
                };
                if val.is_empty() {
                    return Err(format!("option {name} requires a value"));
                }
                if !goal.is_empty() && goal != val {
                    return Err(format!("consilium goal conflicts: {goal:?} and {val:?}"));
                }
                goal = val;
                continue;
            }
            if word.text.starts_with('-') {
                return Err(format!(
                    "unknown consilium option {:?}; use -re/--effort, --goal/-g, heavy, or -- before a scope beginning with '-'",
                    word.text
                ));
            }
        }
        scope_parts.push(word.text);
    }

    let scope = scope_parts.join(" ");
    let (scope, auto_scope) = if scope.trim().is_empty() {
        (WHOLE_PROJECT.to_string(), true)
    } else {
        (scope, false)
    };
    Ok(ParsedConsiliumInput {
        mode,
        effort,
        goal,
        scope,
        auto_scope,
    })
}
