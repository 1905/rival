//! `rival command security`: security review with the china model
//! (`china.model`).

use rival_core::cancel::Context;
use rival_core::config::{self, Config, PromptKind, public_runtime_log};
use rival_core::executor::{self, RunResult};
use rival_core::logging;
use rival_core::result::PayloadKind;
use rival_core::review::{self, GroupSlot};
use rival_core::session::{MODE_SECURITY, NewSession, Session};

use crate::command_plan::write_stdout_error;
use crate::gitscope_helper::{build_review_prompt, lens_prompt};
use crate::merge_request::reject_unresolved_mr;
use crate::model_command::{CancelOnDrop, OwnedSession, fail_session, read_log};
use crate::root::{CmdEnv, CmdError};
use crate::tree::Invocation;
use crate::workdir::resolve_workdir_or_exit;

#[cfg(test)]
mod tests;

pub const SECURITY_USAGE: &str = "Usage:
  /rival security — security review of the changed files (git auto-detect)
  /rival security src/api/ — review a specific scope
  rival command security --which — show which model will run and why

The model is china.model in ~/.rival/config.yaml, the /china model (default
moonshot/kimi-k3.1-pro), at max effort. Run --which to see the model and
whether opencode can run it. The run fails rather than falling back, because
a security review that silently skips is worse than one that refuses to
start.";

/// The effort of a security review: the top of the china menu.
const SECURITY_EFFORT: &str = "max";
/// The effort of the language pass after it.
const REPAIR_EFFORT: &str = "low";

type PreflightFn<'a> = dyn Fn(&Config) -> anyhow::Result<()> + 'a;

/// Runs the china model on the session: `(ctx, cfg, sess, prompt, effort,
/// workdir, log)`. `log` is the file that gets the output; `None` is the
/// session log.
type RunFn<'a> = dyn Fn(&Context, &Config, &mut Session, &str, &str, &str, Option<&str>) -> anyhow::Result<RunResult>
    + 'a;

/// The china preflight and run entry points; tests inject fakes.
pub struct SecurityExecutor<'a> {
    pub preflight: Box<PreflightFn<'a>>,
    pub run: Box<RunFn<'a>>,
}

impl SecurityExecutor<'static> {
    pub fn production() -> Self {
        SecurityExecutor {
            preflight: Box::new(executor::china_preflight),
            run: Box::new(|ctx, cfg, sess, prompt, effort, workdir, log| {
                executor::run_china(ctx, cfg, sess, prompt, effort, workdir, log, None)
            }),
        }
    }
}

/// The flags `command security` reads.
#[derive(Debug, Clone, Default)]
pub struct SecurityOptions {
    pub workdir: String,
    pub no_queue: bool,
    pub which: bool,
}

impl SecurityOptions {
    pub fn from_invocation(inv: &Invocation) -> Self {
        SecurityOptions {
            workdir: inv.string("workdir"),
            no_queue: inv.bool("no-queue"),
            which: inv.bool("which"),
        }
    }
}

/// [`run_command_security`] with the real china adapter.
pub fn command_security_action(env: &mut CmdEnv<'_>, inv: &Invocation) -> Result<(), CmdError> {
    run_command_security(
        env,
        &SecurityOptions::from_invocation(inv),
        &SecurityExecutor::production(),
    )
}

/// Builds the security-lens prompt. An empty
/// scope auto-detects the changed files. Returns `(prompt, scope)`.
pub(crate) fn security_scope_and_prompt(
    cfg: &Config,
    raw_scope: &str,
    workdir: &str,
) -> (String, String) {
    let scope = raw_scope.trim();
    let (prompt, _, scope) = build_review_prompt(
        lens_prompt(cfg, PromptKind::Security),
        scope,
        scope.is_empty(),
        cfg,
        workdir,
    );
    (prompt, scope)
}

/// Reports which model will run, and whether
/// it can. It fails when the model is unusable so a caller can check before
/// launching a detached run.
fn print_security_resolution(
    env: &mut CmdEnv<'_>,
    ex: &SecurityExecutor<'_>,
) -> Result<(), CmdError> {
    let cfg = env.cfg;
    let out = &mut *env.stdout;
    let model = cfg.china_model();
    let source = if cfg.china_model_configured() {
        "file"
    } else {
        "default"
    };
    let _ = writeln!(
        out,
        "Security reviewer: china ({})",
        config::display_model(config::CHINA_LABEL, model)
    );
    let _ = writeln!(out, "Config: china.model = {model} ({source})");
    let _ = writeln!(out, "Reasoning effort: {SECURITY_EFFORT}");
    let _ = writeln!(
        out,
        "Auth: opencode's own (opencode auth login or its config)"
    );

    // The error travels only through the returned error: the root prints
    // it, and printing it here as well would show the same line twice.
    if let Err(e) = (ex.preflight)(cfg) {
        let _ = write!(out, "\nNot usable.\n");
        return Err(CmdError::exit(1, format!("{e:#}")));
    }
    let _ = write!(out, "\nReady.\n");
    Ok(())
}

/// Runs `rival command security` with the given executor.
pub fn run_command_security(
    env: &mut CmdEnv<'_>,
    opts: &SecurityOptions,
    ex: &SecurityExecutor<'_>,
) -> Result<(), CmdError> {
    let cfg = env.cfg;
    let paths = cfg.paths();
    let workdir = resolve_workdir_or_exit(cfg, &opts.workdir, env.stdout)?;

    if opts.which {
        return print_security_resolution(env, ex);
    }
    let model = cfg.china_model().to_string();
    let label = config::engine_label(config::CHINA_LABEL, &model);

    // A terminal stdin means the command was run by hand with no piped
    // scope. Show usage rather than silently reviewing the whole project.
    if env.stdin.is_char_device() {
        let _ = writeln!(env.stdout, "{SECURITY_USAGE}");
        return Ok(());
    }

    // A stdin that fails to stat (closed fd 0) is not read: the scope stays
    // empty and auto-detects.
    let mut raw_scope = String::new();
    if !env.stdin.stat_failed() && !env.stdin.is_char_device() {
        let raw = env
            .stdin
            .read_all()
            .map_err(|e| CmdError::plain(format!("read stdin: {e}")))?;
        raw_scope = String::from_utf8_lossy(&raw).into_owned();
    }

    reject_unresolved_mr(&raw_scope).map_err(CmdError::plain)?;
    if let Err(e) = (ex.preflight)(cfg) {
        let err = format!("{e:#}");
        let _ = writeln!(
            env.stdout,
            "{err}\n\nSecurity reviews run on china.model ({model}); change it with: rival config set china.model <provider/model>"
        );
        return Err(CmdError::exit(1, err));
    }

    let (prompt, scope) = security_scope_and_prompt(cfg, &raw_scope, &workdir);

    let mut sess = OwnedSession::new_queued(
        paths,
        NewSession {
            cli: config::CHINA_LABEL,
            mode: MODE_SECURITY,
            model: &model,
            effort: SECURITY_EFFORT,
            workdir: &workdir,
            prompt: &prompt,
            review_scope: &scope,
            group_id: "",
        },
    )?;

    // `rival wait --log` only discovers a session from a line whose message
    // starts with "starting ". Any other wording leaves the watcher with no
    // session, and it then reports a healthy run as a crash.
    logging::info()
        .str("session", sess.id.as_str())
        .str("model", &label)
        .msg("starting security reviewer");

    // Locals drop in reverse order: run timeout, slot,
    // signals, unfinished session.
    let (ctx, _signals) = env.signal_context()?;

    let _release = review::wait_for_group_slot(
        &ctx,
        cfg,
        &GroupSlot {
            no_queue: opts.no_queue,
            workdir: &workdir,
            group_id: "",
            mode: MODE_SECURITY,
        },
        std::slice::from_mut(&mut sess.0),
        env.stderr,
    )
    .map_err(|e| CmdError::plain(format!("{e:#}")))?;

    let (run_ctx, cancel_run) = review::with_run_timeout(&ctx, cfg, 1);
    let _cancel_run = CancelOnDrop(cancel_run);

    let result = match (ex.run)(
        &run_ctx,
        cfg,
        &mut sess,
        &prompt,
        SECURITY_EFFORT,
        &workdir,
        None,
    ) {
        Ok(result) => result,
        Err(e) => {
            let err = format!("{e:#}");
            let reason = review::run_timeout_reason(&run_ctx, cfg, &label, &err);
            fail_session(paths, &mut sess, 1, &reason);
            return Err(CmdError::plain(err));
        }
    };

    let log_data = match read_log(&sess.log_file) {
        Ok(data) => data,
        Err(e) => {
            fail_session(paths, &mut sess, 1, &format!("read log: {e}"));
            return Err(CmdError::plain(format!("read log file: {e}")));
        }
    };
    let raw = String::from_utf8_lossy(&log_data);

    if result.exit_code != 0 {
        let exit_msg = format!("{label} exited with code {}", result.exit_code);
        let reason = review::run_timeout_reason(&run_ctx, cfg, &label, &exit_msg);
        fail_session(paths, &mut sess, result.exit_code, &reason);
        let _ = env
            .stdout
            .write_all(public_runtime_log(config::CHINA_LABEL, &model, &raw).as_bytes());
        return Err(CmdError::exit(result.exit_code as i32, exit_msg));
    }

    let raw = repair_security_language(&ctx, cfg, ex, &mut sess, &workdir, raw.into_owned());
    let log_file = sess.log_file.clone();
    let (out, valid) = format_security_output(&raw, &model, &scope, &log_file);
    env.stdout
        .write_all(out.as_bytes())
        .map_err(|e| write_stdout_error(&e))?;

    // A security gate must not exit 0 on output it cannot trust. Non-empty
    // output is not evidence a review happened: it can be an echoed prompt,
    // truncated JSON, or an unrecognized provider error.
    if let Err(e) = valid {
        let e = format!("{e:#}");
        fail_session(
            paths,
            &mut sess,
            1,
            &format!("unusable security output: {e}"),
        );
        return Err(CmdError::exit(
            1,
            format!("security review produced no usable findings: {e}"),
        ));
    }

    if let Err(e) = sess.complete(
        paths,
        result.exit_code,
        result.output_bytes,
        result.output_lines,
    ) {
        // Complete changes the in-memory status before saving, so the
        // session guard no longer sees a running session and skips its own
        // write. Exiting 0 here would leave the stored session running
        // forever, and a detached `rival wait` reports that as a crash.
        let e = format!("{e:#}");
        logging::warn()
            .err(&e)
            .str("session", sess.id.as_str())
            .msg("failed to save session completion");
        fail_session(
            paths,
            &mut sess,
            1,
            &format!("could not persist completion: {e}"),
        );
        return Err(CmdError::exit(1, format!("save session completion: {e}")));
    }
    Ok(())
}

/// The language pass on a finished security review: the same model repairs
/// the wording, read-only, with its own log and its own run timeout from
/// `parent`, at low effort. It returns the session log text after the pass.
fn repair_security_language(
    parent: &Context,
    cfg: &Config,
    ex: &SecurityExecutor<'_>,
    sess: &mut Session,
    workdir: &str,
    raw: String,
) -> String {
    review::repair_language(
        parent,
        cfg,
        sess,
        raw,
        PayloadKind::Any,
        |ctx, sess, prompt, log| {
            let result = (ex.run)(ctx, cfg, sess, prompt, REPAIR_EFFORT, workdir, Some(log))?;
            Ok(result.exit_code)
        },
    )
}

/// Parses a zero-exit security run and renders
/// it. Only the final answer is parsed, as for code reviews, so a payload
/// printed by a tool (a file the model read) cannot pass for the review.
/// Validation still sees the whole log: its echo check looks for the prompt
/// anywhere in it. The returned result is the validation failure.
pub(crate) fn format_security_output(
    raw: &str,
    model: &str,
    scope: &str,
    log_path: &str,
) -> (String, anyhow::Result<()>) {
    let parsed = review::parse_reviewer_log(raw);
    if let Err(e) = &parsed {
        logging::warn()
            .err(format!("{e:#}"))
            .msg("security output did not parse");
    }
    review::format_security_result(
        parsed.as_ref().ok(),
        raw,
        config::CHINA_LABEL,
        model,
        scope,
        log_path,
    )
}
