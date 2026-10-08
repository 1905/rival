//! Plan and doc review runs.
//!
//! Each requested model runs at once under one queue ticket. Results come
//! back in the requested order, whatever order the models finish in.

use std::io::Write;
use std::path::Path;

use anyhow::{anyhow, bail};

use super::language::repair_language;
use super::plan::{PlanCLIResult, PlanRunResult, parse_plan_log};
use super::slots::{GroupSlot, SkippedCLI, format_skipped, wait_for_group_slot};
use crate::cancel::{CancelFunc, Context, ContextError};
use crate::config::{self, Config};
use crate::duration;
use crate::executor;
use crate::logging;
use crate::paths::Paths;
use crate::result::PayloadKind;
use crate::session::{self, NewSession, Session};

#[cfg(test)]
mod tests;

/// Renders the plan-review prompt for a plan file at `abs_path`.
fn build_plan_prompt(abs_path: &str) -> String {
    config::PLAN_REVIEW_PROMPT.replace("{FILE}", abs_path)
}

/// A child of `ctx` bounded by
/// `mult × RIVAL_RUN_TIMEOUT`. With the timeout disabled or `mult <= 0` the
/// child has no deadline of its own (cancelling this child never touches
/// `ctx`). Call the returned cancel when
/// the run ends.
pub fn with_run_timeout(ctx: &Context, cfg: &Config, mult: i32) -> (Context, CancelFunc) {
    match cfg.run_timeout_budget(mult) {
        Some(nanos) => ctx.with_timeout_nanos(nanos),
        None => ctx.with_cancel(),
    }
}

/// Turns a failed run into a session-failure reason. When the run's context
/// hit the `RIVAL_RUN_TIMEOUT` deadline it reports the timeout, so a hung
/// provider is distinguishable from a normal failure; otherwise it returns
/// `fallback`, which is the provider's own error text.
///
/// `label` names the model in the message. Pass an empty label when the
/// caller has no model name to report.
pub fn run_timeout_reason(ctx: &Context, cfg: &Config, label: &str, fallback: &str) -> String {
    if ctx.err() != Some(ContextError::DeadlineExceeded) {
        return fallback.to_string();
    }
    let timeout = duration::format(i64::try_from(cfg.run_timeout().as_nanos()).unwrap_or(i64::MAX));
    if label.is_empty() {
        return format!("run timeout after {timeout} (RIVAL_RUN_TIMEOUT) — model did not finish");
    }
    format!("{label} run timeout after {timeout} (RIVAL_RUN_TIMEOUT) — model did not finish")
}

/// Reports why a run that exited 0 still produced no review: an empty log,
/// or a provider quota/rate-limit message when nothing parsed. It returns ""
/// otherwise; output that merely fails to parse is not a run failure (it
/// prints as unparsed). Quota is checked only when nothing parsed: the
/// signatures are plain substrings, so a review of code that mentions them
/// must not fail. `label` prefixes the reason; an empty label returns it bare.
pub fn run_failure_reason(label: &str, raw: &str, parsed: bool) -> String {
    let reason = if raw.trim().is_empty() {
        "produced no output (empty result); likely an auth/session failure"
    } else if !parsed && executor::is_quota_exhausted(raw) {
        "hit provider quota/rate limit (429)"
    } else {
        return String::new();
    };
    if label.is_empty() {
        return reason.to_string();
    }
    format!("{label} {reason}")
}

/// Keeps user-facing failures model-specific even though the implementation
/// uses local adapter binaries underneath.
fn plan_failure_reason(cli: &str, reason: &str) -> String {
    config::public_runtime_error(cli, plan_model_for_cli(cli), reason)
}

/// The model id recorded for a plan CLI's session.
fn plan_model_for_cli(cli: &str) -> &str {
    match cli {
        "codex" => config::CODEX_MODEL,
        "claude" => config::CLAUDE_MODEL,
        _ => cli,
    }
}

/// The raw outcome of running one CLI's plan review, before quota detection
/// and JSON parsing. [`assemble_plan_results`] turns a batch of these (plus
/// the pre-run skipped list) into the final [`PlanRunResult`], which keeps
/// that logic pure and testable without real CLIs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PlanCLIRun {
    cli: String,
    model: String,
    raw: String,
    exit_code: i64,
    /// The error text, `None` when the run returned no error.
    err: Option<String>,
    /// A human-readable failure reason set on the error path (a
    /// `RIVAL_RUN_TIMEOUT` message or the provider's own error), so a skipped
    /// CLI reports the real failure mode instead of a bare exit code.
    reason: String,
}

type PreflightFn<'a> = dyn Fn(&str) -> anyhow::Result<()> + Sync + 'a;

/// Runs one CLI's plan review on the session: `(ctx, sess, cli, prompt,
/// effort, workdir, log)` → the raw log bytes and the exit code. `log` is the
/// file that gets the output; `None` is the session log.
type RunFn<'a> = dyn Fn(
        &Context,
        &mut Session,
        &str,
        &str,
        &str,
        &str,
        Option<&str>,
    ) -> anyhow::Result<(Vec<u8>, i64)>
    + Sync
    + 'a;

/// The preflight and run steps, as fields so tests inject fakes.
struct PlanExecutor<'a> {
    preflight: Box<PreflightFn<'a>>,
    run: Box<RunFn<'a>>,
}

fn default_plan_executor(cfg: &Config) -> PlanExecutor<'_> {
    PlanExecutor {
        preflight: Box::new(move |cli| match cli {
            "codex" => executor::codex_preflight_for(cfg, plan_model_for_cli(cli)),
            "claude" => executor::claude_preflight(cfg),
            _ => Err(anyhow!("unsupported plan cli: {cli}")),
        }),
        run: Box::new(move |ctx, sess, cli, prompt, effort, workdir, log| {
            let result = match cli {
                "codex" => executor::run_codex_model(
                    ctx,
                    cfg,
                    sess,
                    prompt,
                    effort,
                    workdir,
                    plan_model_for_cli(cli),
                    log,
                    None,
                )?,
                "claude" => {
                    executor::run_claude(ctx, cfg, sess, prompt, effort, workdir, true, log, None)?
                }
                _ => bail!("unsupported plan cli: {cli}"),
            };
            // `{e}`, not `{e:#}`: the error already prints as `op path: reason`.
            let raw = session::read_file(Path::new(log.unwrap_or(&sess.log_file)))
                .map_err(|e| anyhow!("read log: {e}"))?;
            Ok((raw, result.exit_code))
        }),
    }
}

/// The invocation settings shared by plan and doc reviews.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReviewBatch<'a> {
    /// An optional invocation override; empty lets each model resolve its
    /// own configured or default effort.
    pub effort: &'a str,
    pub workdir: &'a str,
    pub group_id: &'a str,
    /// Skip the queue (`--no-queue`).
    pub no_queue: bool,
    /// Adapter names ("codex", "claude") in the requested order.
    pub clis: &'a [String],
}

/// What a doc review runs: the mode, prompt, target and fallback effort.
#[derive(Debug, Clone, Copy, Default)]
pub struct DocReview<'a> {
    /// The session mode and the queue ticket label ("plan").
    /// Queue behavior does not depend on it: the concurrency limit is global.
    pub mode: &'a str,
    pub prompt: &'a str,
    /// Recorded as the sessions' review scope.
    pub target: &'a str,
    /// The surface's default when neither the invocation nor
    /// `~/.rival/config.yaml` names an effort. Empty keeps the plan-review
    /// defaults ([`config::DEFAULT_PLAN_EFFORT`]).
    pub fallback_effort: &'a str,
}

/// Reviews the plan/spec file at `abs_path` with each CLI in `batch.clis`,
/// concurrently, under a single queue ticket. There is no judge: each CLI's
/// structured 1-10 rating and findings come back independently. A CLI that
/// is unavailable, fails, or hits quota is recorded in `skipped` rather than
/// aborting the run; only when every CLI is unusable does this return an
/// error.
///
/// Queue progress goes to `stderr` (the process stderr in production).
pub fn run_plan_review(
    ctx: &Context,
    cfg: &Config,
    abs_path: &str,
    batch: &ReviewBatch<'_>,
    stderr: &mut dyn Write,
) -> anyhow::Result<PlanRunResult> {
    run_plan_review_with(
        ctx,
        cfg,
        &default_plan_executor(cfg),
        abs_path,
        batch,
        stderr,
    )
}

/// Runs a plan-shaped review (summary + 1-10 rating + findings) with a
/// caller-built prompt. See [`run_plan_review`].
pub fn run_doc_review(
    ctx: &Context,
    cfg: &Config,
    doc: &DocReview<'_>,
    batch: &ReviewBatch<'_>,
    stderr: &mut dyn Write,
) -> anyhow::Result<PlanRunResult> {
    run_doc_review_with(ctx, cfg, &default_plan_executor(cfg), doc, batch, stderr)
}

fn run_plan_review_with(
    ctx: &Context,
    cfg: &Config,
    ex: &PlanExecutor<'_>,
    abs_path: &str,
    batch: &ReviewBatch<'_>,
    stderr: &mut dyn Write,
) -> anyhow::Result<PlanRunResult> {
    let prompt = build_plan_prompt(abs_path);
    let doc = DocReview {
        mode: session::MODE_PLAN,
        prompt: &prompt,
        target: abs_path,
        fallback_effort: "",
    };
    run_doc_review_with(ctx, cfg, ex, &doc, batch, stderr)
}

/// Fails every session still "queued" (never started) when dropped.
struct QueuedCleanup<'a> {
    paths: &'a Paths,
    sessions: Vec<Session>,
}

impl Drop for QueuedCleanup<'_> {
    fn drop(&mut self) {
        for s in &mut self.sessions {
            if s.status == "queued" {
                let _ = s.fail(self.paths, 1, "interrupted");
            }
        }
    }
}

fn run_doc_review_with(
    ctx: &Context,
    cfg: &Config,
    ex: &PlanExecutor<'_>,
    doc: &DocReview<'_>,
    batch: &ReviewBatch<'_>,
    stderr: &mut dyn Write,
) -> anyhow::Result<PlanRunResult> {
    if batch.clis.is_empty() {
        bail!("no plan models requested");
    }

    // Preflight each CLI BEFORE enqueuing so a doomed run never occupies a
    // slot. Unavailable CLIs are skipped; sessions are created only for the
    // survivors.
    let mut plan_clis: Vec<&str> = Vec::new();
    let mut skipped: Vec<SkippedCLI> = Vec::new();

    // Created BEFORE the creation loop so an error mid-loop still cleans up
    // the sessions already created, not just the ones that started. Declared
    // before the slot guard, so the slot is released first.
    let mut created = QueuedCleanup {
        paths: cfg.paths(),
        sessions: Vec::new(),
    };

    for cli in batch.clis {
        let cli = cli.as_str();
        let model = plan_model_for_cli(cli);
        if let Err(err) = (ex.preflight)(cli) {
            let reason = plan_failure_reason(cli, &format!("{err:#}"));
            logging::warn()
                .str("reviewer", config::engine_label(cli, model))
                .str("reason", reason.clone())
                .msg("plan reviewer unavailable");
            skipped.push(SkippedCLI {
                cli: cli.to_string(),
                model: model.to_string(),
                reason,
            });
            continue;
        }
        let model_fallback = match doc.fallback_effort {
            "" if cli == "codex" => "xhigh",
            "" => config::DEFAULT_PLAN_EFFORT,
            fallback => fallback,
        };
        let effective_effort = cfg
            .resolve_effort(model, batch.effort, model_fallback)
            .map_err(|e| {
                anyhow!(
                    "resolve {} plan effort: {e}",
                    config::engine_label(cli, model)
                )
            })?;
        let mut sess = Session::new_queued(
            cfg.paths(),
            NewSession {
                cli,
                mode: doc.mode,
                model,
                effort: &effective_effort,
                workdir: batch.workdir,
                prompt: doc.prompt,
                review_scope: doc.target,
                group_id: batch.group_id,
            },
        )
        .map_err(|e| {
            anyhow!(
                "create {} plan session: {e:#}",
                config::engine_label(cli, model)
            )
        })?;
        if cli == "claude" {
            sess.account = cfg.claude_subscription().to_string();
        }
        plan_clis.push(cli);
        created.sessions.push(sess);
    }

    if plan_clis.is_empty() {
        bail!(
            "no plan models available (see skipped reasons): {}",
            format_skipped(&skipped)
        );
    }

    // One queue ticket covers all plan sessions; all of them are the run set.
    // The guard holds it until the whole batch is assembled.
    let _slot = wait_for_group_slot(
        ctx,
        cfg,
        &GroupSlot {
            no_queue: batch.no_queue,
            workdir: batch.workdir,
            group_id: batch.group_id,
            mode: doc.mode,
        },
        &mut created.sessions,
        stderr,
    )?;

    // Bound the run once a slot is held: a hung CLI must not keep the slot
    // (and the detached rival) alive forever. Single phase → mult 1. The
    // guard cancels the run context on every exit, after assembly and before
    // the slot is released.
    let (run_ctx, cancel_run) = with_run_timeout(ctx, cfg, 1);
    let _cancel_run = CancelOnDrop(cancel_run);

    // Run every CLI concurrently; join in the requested order.
    let runs: Vec<PlanCLIRun> = std::thread::scope(|scope| {
        let handles: Vec<_> = created
            .sessions
            .iter_mut()
            .zip(&plan_clis)
            .map(|(sess, cli)| {
                let run_ctx = &run_ctx;
                scope.spawn(move || {
                    run_plan_cli(
                        ctx,
                        run_ctx,
                        cfg,
                        ex,
                        sess,
                        cli,
                        doc.prompt,
                        batch.workdir,
                        doc.mode,
                    )
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_else(|p| std::panic::resume_unwind(p)))
            .collect()
    });

    assemble_plan_results(runs, skipped)
}

/// Cancels the run context when dropped, also on unwind.
pub(super) struct CancelOnDrop(pub(super) CancelFunc);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// Fails the session with "interrupted" when dropped while it is still
/// running or queued, also on unwind.
struct FailUnfinished<'a> {
    paths: &'a Paths,
    sess: &'a mut Session,
}

impl Drop for FailUnfinished<'_> {
    fn drop(&mut self) {
        if self.sess.status == "running" || self.sess.status == "queued" {
            let _ = self.sess.fail(self.paths, 1, "interrupted");
        }
    }
}

/// Executes one CLI's plan review and finalizes its session, returning the
/// raw outcome for [`assemble_plan_results`] to interpret.
#[allow(clippy::too_many_arguments)]
fn run_plan_cli(
    parent: &Context,
    ctx: &Context,
    cfg: &Config,
    ex: &PlanExecutor<'_>,
    sess: &mut Session,
    cli: &str,
    prompt: &str,
    workdir: &str,
    mode: &str,
) -> PlanCLIRun {
    // Never leave the session running or queued, on return or unwind.
    let guard = FailUnfinished {
        paths: cfg.paths(),
        sess,
    };
    run_plan_cli_inner(
        parent,
        ctx,
        cfg,
        ex,
        &mut *guard.sess,
        cli,
        prompt,
        workdir,
        mode,
    )
}

/// `parent` is the context before the run timeout: the repair call gets its
/// own timeout from it.
#[allow(clippy::too_many_arguments)]
fn run_plan_cli_inner(
    parent: &Context,
    ctx: &Context,
    cfg: &Config,
    ex: &PlanExecutor<'_>,
    sess: &mut Session,
    cli: &str,
    prompt: &str,
    workdir: &str,
    mode: &str,
) -> PlanCLIRun {
    let paths = cfg.paths();
    let model = plan_model_for_cli(cli);
    let label = config::engine_label(cli, model);

    logging::info()
        .str("session", sess.id.clone())
        .str("reviewer", label.clone())
        .msg("starting plan reviewer");

    let effort = sess.effort.clone();
    let ran = (ex.run)(ctx, sess, cli, prompt, &effort, workdir, None);

    // Keep this defensive restoration for injected/custom executors. The
    // built-in Claude executor preserves the session mode throughout the
    // live run.
    sess.mode = mode.to_string();

    let (raw, exit_code) = match ran {
        Ok(ran) => ran,
        Err(err) => {
            let err = format!("{err:#}");
            let reason = run_timeout_reason(ctx, cfg, &label, &plan_failure_reason(cli, &err));
            let _ = sess.fail(paths, 1, &reason);
            return PlanCLIRun {
                cli: cli.to_string(),
                model: model.to_string(),
                err: Some(err),
                reason,
                exit_code: -1,
                ..PlanCLIRun::default()
            };
        }
    };
    // The output size counts the log's bytes; the text is decoded lossily for parsing.
    let output_bytes = i64::try_from(raw.len()).unwrap_or(i64::MAX);
    let mut raw = String::from_utf8(raw)
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());

    // Parse only the final answer: a plan payload printed earlier by a tool
    // (a file the model read) must not pass for the review.
    let parsed = parse_plan_log(&raw).is_ok();
    if exit_code != 0 {
        let _ = sess.fail(
            paths,
            exit_code,
            &format!("{label} exited with code {exit_code}"),
        );
    } else {
        let reason = run_failure_reason(&label, &raw, parsed);
        if !reason.is_empty() {
            // Fail it so it is reported as skipped, not a "successful" plan
            // review that formats to an empty string while the command exits 0.
            let _ = sess.fail(paths, 1, &reason);
        } else {
            raw = repair_plan_language(parent, cfg, ex, sess, cli, workdir, mode, raw);
            let _ = sess.complete(paths, exit_code, output_bytes, 0);
        }
    }

    PlanCLIRun {
        cli: cli.to_string(),
        model: model.to_string(),
        raw,
        exit_code,
        ..PlanCLIRun::default()
    }
}

/// The language pass on one model's plan review: the same model repairs its
/// own block, with its own run timeout. It returns `raw` with the repaired
/// line, or `raw` unchanged.
#[allow(clippy::too_many_arguments)]
fn repair_plan_language(
    parent: &Context,
    cfg: &Config,
    ex: &PlanExecutor<'_>,
    sess: &mut Session,
    cli: &str,
    workdir: &str,
    mode: &str,
    raw: String,
) -> String {
    repair_language(
        parent,
        cfg,
        sess,
        raw,
        PayloadKind::Plan,
        |ctx, sess, prompt, log| {
            let ran = (ex.run)(ctx, sess, cli, prompt, "low", workdir, Some(log));
            sess.mode = mode.to_string();
            ran.map(|(_, exit_code)| exit_code)
        },
    )
}

/// Turns raw CLI runs (plus the pre-run skipped list) into the final
/// [`PlanRunResult`]. It is pure, so the full success/failure matrix is
/// testable without real CLIs. A CLI that errored, exited non-zero, or hit
/// quota is moved to `skipped`; a successful CLI whose output does not parse
/// keeps its raw output with no parsed result so nothing is lost. If no CLI
/// succeeds, it returns an error listing the skipped reasons.
fn assemble_plan_results(
    batch: Vec<PlanCLIRun>,
    mut skipped: Vec<SkippedCLI>,
) -> anyhow::Result<PlanRunResult> {
    let mut results = Vec::new();
    for r in batch {
        let model = if r.model.is_empty() {
            plan_model_for_cli(&r.cli).to_string()
        } else {
            r.model
        };
        if let Some(err) = &r.err {
            let reason = if r.reason.is_empty() {
                plan_failure_reason(&r.cli, err)
            } else {
                r.reason
            };
            skipped.push(SkippedCLI {
                cli: r.cli,
                model,
                reason,
            });
            continue;
        }
        if r.exit_code != 0 {
            skipped.push(SkippedCLI {
                cli: r.cli,
                model,
                reason: format!("exited with code {}", r.exit_code),
            });
            continue;
        }

        let parsed = parse_plan_log(&r.raw);
        let reason = run_failure_reason("", &r.raw, parsed.is_ok());
        if !reason.is_empty() {
            // A single-CLI run must never format to an empty string with exit 0.
            skipped.push(SkippedCLI {
                cli: r.cli,
                model,
                reason,
            });
            continue;
        }
        let parsed = match parsed {
            Ok(parsed) => Some(parsed),
            Err(err) => {
                logging::warn()
                    .str("reviewer", config::engine_label(&r.cli, &model))
                    .err(&err)
                    .msg("failed to parse plan output, keeping raw");
                None
            }
        };
        results.push(PlanCLIResult {
            cli: r.cli,
            model,
            parsed,
            raw: r.raw,
        });
    }

    if results.is_empty() {
        bail!(
            "all plan reviewers failed or hit quota limits (see skipped reasons): {}",
            format_skipped(&skipped)
        );
    }

    Ok(PlanRunResult { results, skipped })
}
