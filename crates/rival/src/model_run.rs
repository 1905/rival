//! `rival run <model>`: the terminal-facing workflow. Go:
//! `cmd/model_run.go`.

use rival_core::logging;
use rival_core::review::{self, GroupSlot};
use rival_core::session::NewSession;

use crate::merge_request::{ReviewTarget, reject_unresolved_mr};
use crate::model_command::{
    CancelOnDrop, OwnedSession, complete_session, fail_session, finish_review, prepare_review,
    read_log,
};
use crate::model_specs::{ModelSpec, RunCall};
use crate::root::{CmdEnv, CmdError};
use crate::ste_fix;
use crate::workdir::resolve_workdir;

#[cfg(test)]
mod tests;

/// Go `runOptions`: the run surface's flag values.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunOptions {
    pub workdir: String,
    pub no_queue: bool,
    pub effort: String,
    pub review_scope: String,
    /// `--review` was given (even empty).
    pub is_review: bool,
    pub prompt_stdin: bool,
}

/// Go `runModelRun`. It differs from the command surface on purpose: the
/// prompt comes from flags rather than parsed stdin args, output mirrors to
/// stdout as it arrives, only a successful review reads the log back (to
/// print the formatted findings after the mirror), and a nonzero exit
/// returns at once instead of falling through.
pub fn run_model_run(
    env: &mut CmdEnv<'_>,
    spec: &ModelSpec,
    opts: RunOptions,
) -> Result<(), CmdError> {
    let cfg = env.cfg;
    // Effort first. It is pure validation, so a bad value must fail fast
    // rather than hide behind an auth error or block on --prompt-stdin.
    let effort = spec
        .resolve_effort(cfg, &opts.effort)
        .map_err(CmdError::plain)?;

    // --review wins over --prompt-stdin when both are given.
    let mut prompt = String::new();
    if opts.is_review {
        // Built below, once an MR scope has its checkout.
    } else if opts.prompt_stdin {
        let data = env
            .stdin
            .read_all()
            .map_err(|e| CmdError::plain(format!("read stdin: {e}")))?;
        prompt = String::from_utf8_lossy(&data).into_owned();
        if prompt.is_empty() {
            return Err(CmdError::plain("empty prompt"));
        }
        reject_unresolved_mr(&prompt).map_err(CmdError::plain)?;
    } else {
        return Err(CmdError::plain("provide --prompt-stdin or --review"));
    }
    let workdir = resolve_workdir(cfg, &opts.workdir).map_err(CmdError::plain)?;
    // Preflight in the caller's workdir: that is where credentials live, even
    // when an MR review later runs in a temporary checkout.
    (spec.preflight)(cfg, &workdir).map_err(|e| CmdError::plain(format!("{e:#}")))?;

    let (ctx, _signals) = env.signal_context()?;

    let mut mode = "raw";
    let mut scope = opts.review_scope.clone();
    let mut target: Option<ReviewTarget> = None;
    if opts.is_review {
        mode = "review";
        let prepared = prepare_review(&ctx, env, &scope, scope.is_empty(), &workdir)?;
        prompt = prepared.prompt;
        scope = prepared.display_scope;
        target = Some(prepared.target);
    }
    let run_workdir = target
        .as_ref()
        .map_or_else(|| workdir.clone(), |t| t.workdir.clone());

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
        .msg(&format!("starting {}", spec.command_name));

    let group_id = sess.group_id.clone();
    let _release = review::wait_for_group_slot(
        &ctx,
        cfg,
        &GroupSlot {
            no_queue: opts.no_queue,
            workdir: &run_workdir,
            group_id: &group_id,
            mode,
        },
        std::slice::from_mut(&mut sess.0),
        env.stderr,
    )
    .map_err(|e| CmdError::plain(format!("{e:#}")))?;

    let (run_ctx, cancel_run) = review::with_run_timeout(&ctx, cfg, 1);
    let _cancel_run = CancelOnDrop(cancel_run);

    // The run surface is terminal-facing, so output mirrors to stdout live.
    let result = (spec.run)(RunCall {
        ctx: &run_ctx,
        cfg,
        sess: &mut sess,
        prompt: &prompt,
        effort: &effort,
        workdir: &run_workdir,
        cred_workdir: &workdir,
        review: opts.is_review,
        out: Some(&mut *env.stdout),
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

    if result.exit_code != 0 {
        let exit_msg = format!("{} exited with code {}", spec.label(), result.exit_code);
        // Record the provider's own exit code and reason before returning;
        // otherwise the drop would overwrite both with a generic interrupted
        // failure.
        let reason = review::run_timeout_reason(&run_ctx, cfg, &spec.label(), &exit_msg);
        fail_session(cfg.paths(), &mut sess, result.exit_code, &reason);
        // The hint goes to stderr here, because stdout already carries the
        // mirrored provider output.
        let hint = spec.auth_hint(cfg, &sess.log_file);
        if !hint.is_empty() {
            let _ = writeln!(env.stderr, "{hint}");
        }
        return Err(CmdError::exit(result.exit_code as i32, exit_msg));
    }

    if !opts.is_review {
        complete_session(cfg.paths(), &mut sess, &result);
        return Ok(());
    }
    let log = match read_log(&sess.log_file) {
        Ok(log) => String::from_utf8_lossy(&log).into_owned(),
        Err(e) => {
            fail_session(cfg.paths(), &mut sess, 1, &format!("read log file: {e}"));
            return Err(CmdError::plain(format!("read log file: {e}")));
        }
    };
    // A zero exit is not a review: quota errors and empty output also exit 0.
    let log_file = sess.log_file.clone();
    let log = ste_fix::refine(
        &ste_fix::Rerun {
            ctx: &ctx,
            cfg,
            spec,
            workdir: &run_workdir,
            cred_workdir: &workdir,
        },
        &mut sess,
        &log,
    );
    let out = match finish_review(cfg, spec, &mut sess, &log, &scope, &log_file) {
        Ok(out) => out,
        Err(reason) => {
            let _ = writeln!(env.stderr, "{reason}");
            return Err(CmdError::exit(1, reason));
        }
    };
    complete_session(cfg.paths(), &mut sess, &result);
    // The live mirror already showed the transcript; the formatted review
    // follows it.
    if let Err(e) = env.stdout.write_all(format!("\n{out}").as_bytes()) {
        return Err(crate::command_plan::write_stdout_error(&e));
    }
    Ok(())
}
