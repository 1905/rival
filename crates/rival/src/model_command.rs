//! `rival command <model>`: the skill-facing workflow.

use rival_core::cancel::{CancelFunc, Context};
use rival_core::config::{Config, PromptKind, public_runtime_log};
use rival_core::executor::RunResult;
use rival_core::logging;
use rival_core::paths::Paths;
use rival_core::result::PayloadKind;
use rival_core::review::{self, GroupSlot};
use rival_core::session::{NewSession, Session};

use crate::gitscope_helper::{build_review_prompt, lens_prompt};
use crate::merge_request::{ReviewTarget, prepare_review_target_with, reject_unresolved_mr};
use crate::model_specs::{ModelSpec, RunCall, session_mode};
use crate::root::{CmdEnv, CmdError};
use crate::workdir::resolve_workdir_or_exit;

#[cfg(test)]
mod tests;

/// Reads args from stdin, runs the provider, then prints the result for the
/// calling skill to capture. A successful review prints the formatted
/// findings and the log path; a raw prompt, or any failed run, prints the
/// log. Every model's `rival command <name>` goes through it.
pub fn run_model_command(
    env: &mut CmdEnv<'_>,
    spec: &ModelSpec,
    workdir: &str,
    no_queue: bool,
) -> Result<(), CmdError> {
    let cfg = env.cfg;
    // A terminal stdin means no piped args, so show usage instead of hanging.
    if env.stdin.is_char_device() {
        let _ = writeln!(env.stdout, "{}", spec.usage);
        return Ok(());
    }

    let raw = env
        .stdin
        .read_all()
        .map_err(|e| CmdError::plain(format!("read stdin: {e}")))?;
    let raw = String::from_utf8_lossy(&raw).into_owned();

    let parsed = match (spec.parse)(&raw) {
        Ok(parsed) => parsed,
        Err(e) => {
            let msg = format!("{e:#}");
            let _ = writeln!(env.stdout, "{msg}");
            return Err(CmdError::exit(1, msg));
        }
    };
    if parsed.is_empty {
        let _ = writeln!(env.stdout, "{}", spec.usage);
        return Ok(());
    }
    // A review resolves an MR scope into a pinned checkout below; a raw
    // prompt cannot, so it is rejected before anything starts.
    if !parsed.is_review {
        reject_unresolved_mr(&raw).map_err(CmdError::plain)?;
    }

    let effort = spec
        .resolve_effort(cfg, &parsed.effort)
        .map_err(CmdError::plain)?;
    let workdir = resolve_workdir_or_exit(cfg, workdir, env.stdout)?;
    // Preflight in the caller's workdir: that is where credentials live, even
    // when an MR review later runs in a temporary checkout.
    (spec.preflight)(cfg, &workdir).map_err(|e| CmdError::plain(format!("{e:#}")))?;

    // Cancel the MR resolve, the queue wait and the child on SIGINT/SIGTERM
    // so the cleanup below runs. Locals drop in reverse order: run timeout,
    // slot, unfinished session, MR checkout, signals.
    let (ctx, _signals) = env.signal_context()?;

    let mut prompt = parsed.prompt.clone();
    let mut scope = parsed.review_scope.clone();
    let mut target: Option<ReviewTarget> = None;
    if parsed.is_review {
        let prepared = prepare_review(&ctx, env, &scope, parsed.auto_scope, &workdir)?;
        prompt = prepared.prompt;
        scope = prepared.display_scope;
        target = Some(prepared.target);
    }
    let run_workdir = target
        .as_ref()
        .map_or_else(|| workdir.clone(), |t| t.workdir.clone());

    let mode = session_mode(parsed.is_review);
    let mut sess = OwnedSession::new_queued(
        cfg.paths(),
        NewSession {
            cli: spec.cli,
            mode,
            model: spec.model,
            effort: &effort,
            workdir: &run_workdir,
            prompt: &prompt,
            review_scope: &scope,
            group_id: "",
        },
    )?;
    if spec.command_name == rival_core::config::CLAUDE_LABEL {
        sess.account = cfg.claude_subscription().to_string();
    }

    logging::info()
        .str("session", sess.id.as_str())
        .str("effort", effort.as_str())
        .str("mode", mode)
        .msg(&format!("starting {} (command mode)", spec.command_name));

    let group_id = sess.group_id.clone();
    let _release = review::wait_for_group_slot(
        &ctx,
        cfg,
        &GroupSlot {
            no_queue,
            workdir: &run_workdir,
            group_id: &group_id,
            mode,
        },
        std::slice::from_mut(&mut sess.0),
        env.stderr,
    )
    .map_err(|e| CmdError::plain(format!("{e:#}")))?;

    // Bound the run: a hung provider must not hold the slot forever. The
    // clock starts after slot promotion.
    let (run_ctx, cancel_run) = review::with_run_timeout(&ctx, cfg, 1);
    let _cancel_run = CancelOnDrop(cancel_run);

    // No stdout mirror in command mode; the skill reads the final output.
    let result = (spec.run)(RunCall {
        ctx: &run_ctx,
        cfg,
        sess: &mut sess,
        prompt: &prompt,
        effort: &effort,
        workdir: &run_workdir,
        cred_workdir: &workdir,
        review: parsed.is_review,
        log: None,
        out: None,
    });
    let result = match result {
        Ok(result) => result,
        Err(e) => {
            let err = format!("{e:#}");
            let reason = review::run_timeout_reason(&run_ctx, cfg, &spec.label(), &err);
            fail_session(cfg.paths(), &mut sess, 1, &reason);
            return Err(CmdError::plain(err));
        }
    };

    let mut exit_code = result.exit_code;
    let mut exit_msg = format!("{} exited with code {exit_code}", spec.label());
    let log = read_log(&sess.log_file);
    let log_text = log
        .as_ref()
        .map(|data| String::from_utf8_lossy(data).into_owned())
        .unwrap_or_default();
    let mut out = public_runtime_log(&sess.cli, &sess.model, &log_text);
    let log_text = if exit_code == 0 && parsed.is_review && log.is_ok() {
        repair_review_language(&ctx, cfg, spec, &mut sess, log_text, &run_workdir, &workdir)
    } else {
        log_text
    };
    if exit_code != 0 {
        let reason = review::run_timeout_reason(&run_ctx, cfg, &spec.label(), &exit_msg);
        fail_session(cfg.paths(), &mut sess, exit_code, &reason);
    } else if parsed.is_review && log.is_ok() {
        // A zero exit is not a review: quota errors and empty output also
        // exit 0.
        let log_file = sess.log_file.clone();
        match finish_review(cfg, spec, &mut sess, &log_text, &scope, &log_file) {
            Err(reason) => {
                exit_code = 1;
                exit_msg = reason;
            }
            Ok(formatted) => {
                complete_session(cfg.paths(), &mut sess, &result);
                out = formatted;
            }
        }
    } else {
        complete_session(cfg.paths(), &mut sess, &result);
    }
    if let Err(e) = log {
        return Err(CmdError::plain(format!("read log file: {e}")));
    }
    if let Err(e) = env.stdout.write_all(out.as_bytes()) {
        return Err(crate::command_plan::write_stdout_error(&e));
    }

    if exit_code != 0 {
        // The hint follows the log on stdout, so a skill capturing output
        // sees the failure before the explanation.
        let hint = spec.auth_hint(cfg, &sess.log_file);
        if !hint.is_empty() {
            let _ = writeln!(env.stdout, "\n{hint}");
        }
        return Err(CmdError::exit(exit_code as i32, exit_msg));
    }
    Ok(())
}

/// A review's prompt, recorded scope, and where it runs.
pub(crate) struct PreparedReview {
    pub prompt: String,
    /// What the session records and the output shows: the MR URL, the
    /// auto-detected file list, or the scope as given.
    pub display_scope: String,
    /// Where the reviewer runs. For an MR it owns the snapshot checkout,
    /// which is removed when the target drops.
    pub target: ReviewTarget,
}

/// Resolves a review's scope and builds its bug-hunter prompt. A GitLab MR
/// scope is pinned to a snapshot checkout and its identity line goes to
/// stdout first.
pub(crate) fn prepare_review(
    ctx: &Context,
    env: &mut CmdEnv<'_>,
    scope: &str,
    auto_scope: bool,
    workdir: &str,
) -> Result<PreparedReview, CmdError> {
    let cfg = env.cfg;
    let prepare = env.prepare_mr;
    let target =
        prepare_review_target_with(ctx, cfg, scope, workdir, env.stdout, |c, cfg, s, w| {
            prepare(c, cfg, s, w)
        })
        .map_err(CmdError::plain)?;
    // Bug-hunter records and shows the detected file list, not the display
    // placeholder the other commands use.
    let (prompt, recorded, _) = build_review_prompt(
        lens_prompt(cfg, PromptKind::BugHunter),
        &target.scope,
        auto_scope,
        cfg,
        &target.workdir,
    );
    let display_scope = if auto_scope {
        recorded
    } else {
        target.display.clone()
    };
    Ok(PreparedReview {
        prompt,
        display_scope,
        target,
    })
}

/// Turns a zero-exit review log into the formatted review. When the run
/// produced no review (empty log, or only a quota error) it records the
/// failure on `sess` and returns the reason instead; output that merely does
/// not parse still formats, as UNPARSED.
pub(crate) fn finish_review(
    cfg: &Config,
    spec: &ModelSpec,
    sess: &mut Session,
    raw: &str,
    scope: &str,
    log_path: &str,
) -> Result<String, String> {
    let parsed = review::parse_reviewer_log(raw);
    let reason = review::run_failure_reason(&spec.label(), raw, parsed.is_ok());
    if !reason.is_empty() {
        fail_session(cfg.paths(), sess, 1, &reason);
        return Err(reason);
    }
    if let Err(e) = &parsed {
        logging::warn().err(e).msg("review output did not parse");
    }
    Ok(review::format_review_result(
        parsed.as_ref().ok(),
        raw,
        spec.cli,
        spec.model,
        scope,
        log_path,
    ))
}

/// The language pass on a finished code review: the same model repairs the
/// wording at low effort, read-only, with its own log and its own run
/// timeout from `parent`. It returns the session log text after the pass.
pub(crate) fn repair_review_language(
    parent: &Context,
    cfg: &Config,
    spec: &ModelSpec,
    sess: &mut Session,
    raw: String,
    workdir: &str,
    cred_workdir: &str,
) -> String {
    let log = review::repair_log_path(&sess.log_file);
    let session_log = sess.log_file.clone();
    // K3 resolves to max, its only level.
    let effort = spec
        .resolve_effort(cfg, "low")
        .unwrap_or_else(|_| "low".to_string());
    let (run_ctx, cancel_run) = review::with_run_timeout(parent, cfg, 1);
    let _cancel_run = CancelOnDrop(cancel_run);
    review::repair_language(&session_log, raw, PayloadKind::Any, |prompt| {
        let result = (spec.run)(RunCall {
            ctx: &run_ctx,
            cfg,
            sess,
            prompt,
            effort: &effort,
            workdir,
            cred_workdir,
            review: true,
            log: Some(&log),
            out: None,
        })?;
        if result.exit_code != 0 {
            anyhow::bail!("{} exited with code {}", spec.label(), result.exit_code);
        }
        let data = read_log(&log).map_err(anyhow::Error::msg)?;
        Ok(String::from_utf8_lossy(&data).into_owned())
    })
}

/// Records a failure and logs when the record cannot be saved.
pub(crate) fn fail_session(paths: &Paths, sess: &mut Session, exit_code: i64, reason: &str) {
    if let Err(e) = sess.fail(paths, exit_code, reason) {
        logging::warn()
            .err(e)
            .str("session", sess.id.as_str())
            .msg("failed to save session failure");
    }
}

/// Records a successful run and logs when the record cannot be saved.
pub(crate) fn complete_session(paths: &Paths, sess: &mut Session, result: &RunResult) {
    if let Err(e) = sess.complete(
        paths,
        result.exit_code,
        result.output_bytes,
        result.output_lines,
    ) {
        logging::warn()
            .err(e)
            .str("session", sess.id.as_str())
            .msg("failed to save session completion");
    }
}

/// Reads the whole session log; an error is `<op> <path>: <errno text>`.
pub(crate) fn read_log(path: &str) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let open = std::fs::File::open(path).map_err(|e| format!("open {path}: {}", e))?;
    let mut data = Vec::new();
    let mut file = open;
    file.read_to_end(&mut data)
        .map_err(|e| format!("read {path}: {}", e))?;
    Ok(data)
}

/// A session this command owns. Dropping it while the session is still
/// queued or running records `interrupted`, also on an early return or a
/// panic.
pub(crate) struct OwnedSession<'p>(pub Session, &'p Paths);

impl<'p> OwnedSession<'p> {
    pub(crate) fn new_queued(paths: &'p Paths, input: NewSession<'_>) -> Result<Self, CmdError> {
        Session::new_queued(paths, input)
            .map(|s| OwnedSession(s, paths))
            .map_err(|e| CmdError::plain(format!("create session: {e:#}")))
    }
}

impl std::ops::Deref for OwnedSession<'_> {
    type Target = Session;
    fn deref(&self) -> &Session {
        &self.0
    }
}

impl std::ops::DerefMut for OwnedSession<'_> {
    fn deref_mut(&mut self) -> &mut Session {
        &mut self.0
    }
}

impl Drop for OwnedSession<'_> {
    fn drop(&mut self) {
        if self.0.status == "running" || self.0.status == "queued" {
            let _ = self.0.fail(self.1, 1, "interrupted");
        }
    }
}

/// Cancels the run on drop: [`CancelFunc`] does not cancel on drop.
pub(crate) struct CancelOnDrop(pub CancelFunc);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
