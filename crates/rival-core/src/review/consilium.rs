//! 3-judge Consilium reviews (normal and heavy modes).
//!
//! Normal mode runs Opus 5.5 (`claude`), Sol 6.2 (`sol`) and Gemini 4 Neon
//! (`neon`, always at `xhigh`). Heavy mode runs Fable 5.2 (`fable`), Astra 6.2
//! (`codex`) and Gemini 4 Neon (`neon`, always at `xhigh`).
//!
//! Each judge independently evaluates the target against the requested goal
//! (bugs by default, or design, architecture, tests, etc.), assigns an overall
//! 1-10 rating with a detailed subsection breakdown, and returns actionable
//! findings so the user can hill-climb until every judge reaches 9/10+.
//!
//! All three judges run concurrently under one queue ticket and come back in
//! roster order. If a judge (for example Neon) is unavailable, fails, or hits
//! quota, it is skipped with an explicit `rival: ⚠` notice while the remaining
//! judges still deliver their reviews.

use std::fmt::Write as _;
use std::io::Write;
use std::path::Path;

use anyhow::{anyhow, bail};

use super::format::{one_line, write_review_body, write_summary};
use super::language::repair_language;
use super::parse::payload_error;
use super::planrun::{CancelOnDrop, run_failure_reason, run_timeout_reason, with_run_timeout};
use super::prompt::CONSILIUM_DEFAULT_GOAL;
use super::slots::{GroupSlot, SkippedCLI, format_skipped, wait_for_group_slot};
use super::types::{ReviewerFinding, ReviewerOutput};
use crate::cancel::Context;
use crate::config::{self, Config};
use crate::executor;
use crate::logging;
use crate::paths::Paths;
use crate::result::{self, PayloadKind, RatingBreakdown};
use crate::session::{self, NewSession, Session};

#[cfg(test)]
mod tests;

/// Consilium tier: normal (Opus 5.5 + Sol 6.2 + Neon xhigh) or heavy
/// (Fable 5.2 + Astra 6.2 + Neon xhigh).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ConsiliumMode {
    #[default]
    Normal,
    Heavy,
}

impl ConsiliumMode {
    /// Display name used in headers and errors.
    pub fn name(self) -> &'static str {
        match self {
            ConsiliumMode::Normal => "normal",
            ConsiliumMode::Heavy => "heavy",
        }
    }

    /// The three judge model ids in roster order.
    pub fn models(self) -> [&'static str; 3] {
        match self {
            ConsiliumMode::Normal => [config::CLAUDE_MODEL, config::SOL_MODEL, config::NEON_MODEL],
            ConsiliumMode::Heavy => [config::FABLE_MODEL, config::CODEX_MODEL, config::NEON_MODEL],
        }
    }
}

/// Structured JSON emitted by one consilium judge.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConsiliumOutput {
    pub summary: String,
    pub rating: u8,
    pub breakdown: Vec<RatingBreakdown>,
    pub findings: Vec<ReviewerFinding>,
}

/// Extracts a consilium judge's rated JSON payload (summary, 1-10 rating,
/// optional subsection breakdown, and findings) from its provider log.
pub fn parse_consilium_log(raw: &str) -> anyhow::Result<ConsiliumOutput> {
    let p =
        result::log_payload(raw, PayloadKind::Plan).map_err(|e| payload_error("consilium", e))?;
    Ok(ConsiliumOutput {
        summary: p.summary,
        rating: p.rating.unwrap_or(0),
        breakdown: p.breakdown,
        findings: p.findings.into_iter().map(ReviewerFinding::from).collect(),
    })
}

/// What a consilium run executes.
#[derive(Debug, Clone, Copy, Default)]
pub struct ConsiliumRequest<'a> {
    pub mode: ConsiliumMode,
    pub prompt: &'a str,
    pub scope: &'a str,
    pub goal: &'a str,
    /// Optional effort override for Claude/Codex judges; empty uses each
    /// model's configured or built-in default. Neon always runs at `xhigh`.
    pub effort: &'a str,
    pub workdir: &'a str,
    pub group_id: &'a str,
    /// Skip the queue (`--no-queue`).
    pub no_queue: bool,
}

/// One judge's outcome inside a consilium run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConsiliumCLIResult {
    pub cli: String,
    pub model: String,
    pub parsed: Option<ConsiliumOutput>,
    pub raw: String,
    pub log_path: String,
}

/// The outcome of a 3-judge consilium run: the judges that produced output,
/// any skipped judges, and user-facing notices (tier fallbacks and skipped
/// judge warnings).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConsiliumRunResult {
    pub mode: ConsiliumMode,
    pub results: Vec<ConsiliumCLIResult>,
    pub skipped: Vec<SkippedCLI>,
    pub notices: Vec<String>,
}

/// The runtime (session `cli`) for a consilium judge model id.
fn consilium_cli(model: &str) -> &'static str {
    if executor::is_codex_model(model) {
        "codex"
    } else if executor::is_claude_model(model) {
        "claude"
    } else if executor::is_neon_model(model) {
        config::NEON_LABEL
    } else {
        "unknown"
    }
}

fn consilium_failure_reason(cli: &str, model: &str, reason: &str) -> String {
    config::public_runtime_error(cli, model, reason)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ConsiliumCLIRun {
    cli: String,
    model: String,
    raw: String,
    log_path: String,
    exit_code: i64,
    err: Option<String>,
    reason: String,
}

type PreflightFn<'a> = dyn Fn(&str, &str) -> anyhow::Result<()> + Sync + 'a;
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

struct ConsiliumExecutor<'a> {
    preflight: Box<PreflightFn<'a>>,
    run: Box<RunFn<'a>>,
}

fn default_consilium_executor(cfg: &Config) -> ConsiliumExecutor<'_> {
    ConsiliumExecutor {
        preflight: Box::new(move |cli, model| match cli {
            "codex" => executor::codex_preflight_for(cfg, model),
            "claude" => executor::claude_preflight_model(cfg, model),
            config::NEON_LABEL => executor::neon_preflight(cfg),
            _ => Err(anyhow!("unsupported consilium judge cli: {cli}")),
        }),
        run: Box::new(move |ctx, sess, cli, prompt, effort, workdir, log| {
            let model = sess.model.clone();
            let result = match cli {
                "codex" => executor::run_codex_model(
                    ctx, cfg, sess, prompt, effort, workdir, &model, true, log, None,
                )?,
                "claude" => executor::run_claude(
                    ctx, cfg, sess, prompt, effort, workdir, &model, true, log, None,
                )?,
                config::NEON_LABEL => executor::run_neon_model(
                    ctx, cfg, sess, prompt, effort, workdir, &model, true, log, None,
                )?,
                _ => bail!("unsupported consilium judge cli: {cli}"),
            };
            let raw = session::read_file(Path::new(log.unwrap_or(&sess.log_file)))
                .map_err(|e| anyhow!("read log: {e}"))?;
            Ok((raw, result.exit_code))
        }),
    }
}

/// Runs a 3-judge consilium review concurrently under one queue ticket.
/// Unavailable, failed, or quota-limited judges are recorded in `skipped` and
/// surfaced as explicit `rival: ⚠` notices; only when every judge is unusable
/// does this return an error.
pub fn run_consilium_review(
    ctx: &Context,
    cfg: &Config,
    req: &ConsiliumRequest<'_>,
    stderr: &mut dyn Write,
) -> anyhow::Result<ConsiliumRunResult> {
    run_consilium_review_with(ctx, cfg, &default_consilium_executor(cfg), req, stderr)
}

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

fn run_consilium_review_with(
    ctx: &Context,
    cfg: &Config,
    ex: &ConsiliumExecutor<'_>,
    req: &ConsiliumRequest<'_>,
    stderr: &mut dyn Write,
) -> anyhow::Result<ConsiliumRunResult> {
    let mut active: Vec<(&str, &str)> = Vec::new();
    let mut skipped: Vec<SkippedCLI> = Vec::new();
    let mut created = QueuedCleanup {
        paths: cfg.paths(),
        sessions: Vec::new(),
    };

    for model in req.mode.models() {
        let cli = consilium_cli(model);
        if let Err(err) = (ex.preflight)(cli, model) {
            let reason = consilium_failure_reason(cli, model, &format!("{err:#}"));
            logging::warn()
                .str("judge", config::engine_label(cli, model))
                .str("reason", reason.clone())
                .msg("consilium judge unavailable");
            skipped.push(SkippedCLI {
                cli: cli.to_string(),
                model: model.to_string(),
                reason,
            });
            continue;
        }
        let effective_effort = if cli == config::NEON_LABEL {
            executor::neon_effort(req.effort).map_err(|e| {
                anyhow!(
                    "resolve {} consilium effort: {e:#}",
                    config::engine_label(cli, model)
                )
            })?
        } else {
            cfg.resolve_effort(model, req.effort, "").map_err(|e| {
                anyhow!(
                    "resolve {} consilium effort: {e}",
                    config::engine_label(cli, model)
                )
            })?
        };
        let mut sess = Session::new_queued(
            cfg.paths(),
            NewSession {
                cli,
                mode: session::MODE_CONSILIUM,
                model,
                effort: &effective_effort,
                workdir: req.workdir,
                prompt: req.prompt,
                review_scope: req.scope,
                group_id: req.group_id,
            },
        )
        .map_err(|e| {
            anyhow!(
                "create {} consilium session: {e:#}",
                config::engine_label(cli, model)
            )
        })?;
        if cli == "claude" {
            sess.account = cfg.claude_subscription().to_string();
        }
        active.push((cli, model));
        created.sessions.push(sess);
    }

    if active.is_empty() {
        bail!(
            "no consilium judges available (see skipped reasons): {}",
            format_skipped(&skipped)
        );
    }

    let _slot = wait_for_group_slot(
        ctx,
        cfg,
        &GroupSlot {
            no_queue: req.no_queue,
            workdir: req.workdir,
            group_id: req.group_id,
            mode: session::MODE_CONSILIUM,
        },
        &mut created.sessions,
        stderr,
    )?;

    let (run_ctx, cancel_run) = with_run_timeout(ctx, cfg, 1);
    let _cancel_run = CancelOnDrop(cancel_run);

    let runs: Vec<ConsiliumCLIRun> = std::thread::scope(|scope| {
        let handles: Vec<_> = created
            .sessions
            .iter_mut()
            .zip(&active)
            .map(|(sess, &(cli, model))| {
                let run_ctx = &run_ctx;
                scope.spawn(move || {
                    run_consilium_cli(
                        ctx,
                        run_ctx,
                        cfg,
                        ex,
                        sess,
                        cli,
                        model,
                        req.prompt,
                        req.workdir,
                    )
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_else(|p| std::panic::resume_unwind(p)))
            .collect()
    });

    let notices: Vec<String> = created
        .sessions
        .iter()
        .filter_map(executor::tier_notice)
        .collect();
    let result = assemble_consilium_results(req.mode, runs, skipped, notices.clone());
    if result.is_err() {
        for notice in &notices {
            let _ = stderr.write_all(notice.as_bytes());
        }
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn run_consilium_cli(
    parent: &Context,
    ctx: &Context,
    cfg: &Config,
    ex: &ConsiliumExecutor<'_>,
    sess: &mut Session,
    cli: &str,
    model: &str,
    prompt: &str,
    workdir: &str,
) -> ConsiliumCLIRun {
    let guard = FailUnfinished {
        paths: cfg.paths(),
        sess,
    };
    run_consilium_cli_inner(
        parent,
        ctx,
        cfg,
        ex,
        &mut *guard.sess,
        cli,
        model,
        prompt,
        workdir,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_consilium_cli_inner(
    parent: &Context,
    ctx: &Context,
    cfg: &Config,
    ex: &ConsiliumExecutor<'_>,
    sess: &mut Session,
    cli: &str,
    model: &str,
    prompt: &str,
    workdir: &str,
) -> ConsiliumCLIRun {
    let paths = cfg.paths();
    let label = config::engine_label(cli, model);
    let log_path = sess.log_file.clone();

    // `rival wait --log` discovers sessions from lines starting with "starting ".
    logging::info()
        .str("session", sess.id.clone())
        .str("judge", label.clone())
        .msg("starting consilium judge");

    let effort = sess.effort.clone();
    let ran = (ex.run)(ctx, sess, cli, prompt, &effort, workdir, None);
    sess.mode = session::MODE_CONSILIUM.to_string();

    let (raw, exit_code) = match ran {
        Ok(ran) => ran,
        Err(err) => {
            let err = format!("{err:#}");
            let reason = run_timeout_reason(
                ctx,
                cfg,
                &label,
                &consilium_failure_reason(cli, model, &err),
            );
            let _ = sess.fail(paths, 1, &reason);
            return ConsiliumCLIRun {
                cli: cli.to_string(),
                model: model.to_string(),
                log_path,
                err: Some(err),
                reason,
                exit_code: -1,
                ..ConsiliumCLIRun::default()
            };
        }
    };

    let output_bytes = i64::try_from(raw.len()).unwrap_or(i64::MAX);
    let mut raw = String::from_utf8(raw)
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());

    let parsed = parse_consilium_log(&raw).is_ok();
    let mut fail_reason = String::new();
    if exit_code != 0 {
        let fallback = format!("{label} exited with code {exit_code}");
        let reason = run_timeout_reason(ctx, cfg, &label, &fallback);
        let _ = sess.fail(paths, exit_code, &reason);
        if reason != fallback {
            fail_reason = reason;
        }
    } else {
        let reason = run_failure_reason(&label, &raw, parsed);
        if !reason.is_empty() {
            let _ = sess.fail(paths, 1, &reason);
        } else {
            raw = repair_consilium_language(parent, cfg, ex, sess, cli, workdir, raw);
            let _ = sess.complete(paths, exit_code, output_bytes, 0);
        }
    }

    ConsiliumCLIRun {
        cli: cli.to_string(),
        model: model.to_string(),
        raw,
        log_path,
        exit_code,
        reason: fail_reason,
        ..ConsiliumCLIRun::default()
    }
}

fn repair_consilium_language(
    parent: &Context,
    cfg: &Config,
    ex: &ConsiliumExecutor<'_>,
    sess: &mut Session,
    cli: &str,
    workdir: &str,
    raw: String,
) -> String {
    let repair_effort = if cli == config::NEON_LABEL {
        "xhigh"
    } else {
        "low"
    };
    repair_language(
        parent,
        cfg,
        sess,
        raw,
        PayloadKind::Plan,
        |ctx, sess, prompt, log| {
            let ran = (ex.run)(ctx, sess, cli, prompt, repair_effort, workdir, Some(log));
            sess.mode = session::MODE_CONSILIUM.to_string();
            ran.map(|(_, exit_code)| exit_code)
        },
    )
}

/// Pure assembly of a consilium batch into [`ConsiliumRunResult`]. Failed,
/// unavailable, or quota-limited judges move into `skipped` and produce an
/// explicit `rival: ⚠ <label> failed and was skipped — <reason>` notice.
fn assemble_consilium_results(
    mode: ConsiliumMode,
    batch: Vec<ConsiliumCLIRun>,
    mut skipped: Vec<SkippedCLI>,
    mut notices: Vec<String>,
) -> anyhow::Result<ConsiliumRunResult> {
    let mut results = Vec::new();
    for r in batch {
        if let Some(err) = &r.err {
            let reason = if r.reason.is_empty() {
                consilium_failure_reason(&r.cli, &r.model, err)
            } else {
                r.reason
            };
            skipped.push(SkippedCLI {
                cli: r.cli,
                model: r.model,
                reason,
            });
            continue;
        }
        if r.exit_code != 0 {
            let reason = if r.reason.is_empty() {
                format!("exited with code {}", r.exit_code)
            } else {
                r.reason
            };
            skipped.push(SkippedCLI {
                cli: r.cli,
                model: r.model,
                reason,
            });
            continue;
        }

        let parsed = parse_consilium_log(&r.raw);
        let reason = run_failure_reason("", &r.raw, parsed.is_ok());
        if !reason.is_empty() {
            skipped.push(SkippedCLI {
                cli: r.cli,
                model: r.model,
                reason,
            });
            continue;
        }
        let parsed = match parsed {
            Ok(parsed) => Some(parsed),
            Err(err) => {
                logging::warn()
                    .str("judge", config::engine_label(&r.cli, &r.model))
                    .err(&err)
                    .msg("failed to parse consilium output, keeping raw");
                None
            }
        };
        results.push(ConsiliumCLIResult {
            cli: r.cli,
            model: r.model,
            parsed,
            raw: r.raw,
            log_path: r.log_path,
        });
    }

    if results.is_empty() {
        bail!(
            "all consilium judges failed or hit quota limits (see skipped reasons): {}",
            format_skipped(&skipped)
        );
    }

    for s in &skipped {
        let reason = config::public_runtime_error(&s.cli, &s.model, &s.reason);
        notices.push(format!(
            "rival: ⚠ {} failed and was skipped — {reason}\n",
            s.label()
        ));
    }

    Ok(ConsiliumRunResult {
        mode,
        results,
        skipped,
        notices,
    })
}

/// Renders a consilium review: top header with mode, goal, scope, and
/// per-judge ratings summary, then one section per judge with overall rating,
/// summary, subsection rating breakdown, findings, and log path, followed by
/// any skipped judges.
pub fn format_consilium_result(result: &ConsiliumRunResult, scope: &str, goal: &str) -> String {
    if result.results.is_empty() {
        return "No consilium output.\n".to_string();
    }
    let effective_goal = match goal.trim() {
        "" => CONSILIUM_DEFAULT_GOAL,
        g => g,
    };
    let labels: Vec<String> = result
        .results
        .iter()
        .map(|r| config::engine_label(&r.cli, &r.model))
        .collect();
    let mut sb = format!(
        "\n═══ RIVAL CONSILIUM [{}] ({}) ═══\n\n",
        result.mode.name(),
        labels.join(" + ")
    );
    let _ = writeln!(sb, "Goal: {}", one_line(effective_goal));
    let _ = writeln!(sb, "Scope: {}", one_line(scope));

    // Ratings summary line for quick hill-climb tracking.
    let mut score_parts: Vec<String> = Vec::new();
    let mut min_score: Option<u8> = Some(10);
    for r in &result.results {
        let label = config::engine_label(&r.cli, &r.model);
        match &r.parsed {
            Some(out) => {
                score_parts.push(format!("{label} {}/10", out.rating));
                min_score = min_score.map(|m| m.min(out.rating));
            }
            None => {
                score_parts.push(format!("{label} unparsed"));
                min_score = None;
            }
        }
    }
    if let Some(min) = min_score {
        score_parts.push(format!("min {min}/10"));
    }
    let _ = writeln!(sb, "Ratings: {}", score_parts.join(" · "));

    for (i, r) in result.results.iter().enumerate() {
        let label = format!("{} ({})", config::engine_label(&r.cli, &r.model), r.model);
        match &r.parsed {
            Some(out) => {
                let _ = write!(sb, "\n── {label} — {}/10 ──\n\n", out.rating);
                write_summary(&mut sb, &out.summary);
                if !out.breakdown.is_empty() {
                    sb.push_str("Rating breakdown:\n");
                    for item in &out.breakdown {
                        let notes = item.notes.trim();
                        if notes.is_empty() {
                            let _ = writeln!(sb, "- {}: {}/10", item.section.trim(), item.rating);
                        } else {
                            let _ = writeln!(
                                sb,
                                "- {}: {}/10 — {notes}",
                                item.section.trim(),
                                item.rating
                            );
                        }
                    }
                    sb.push('\n');
                }
                let review_out = ReviewerOutput {
                    summary: out.summary.clone(),
                    findings: out.findings.clone(),
                };
                write_review_body(&mut sb, &review_out, &r.log_path);
            }
            None => {
                let _ = write!(sb, "\n── {label} — unparsed ──\n\n");
                sb.push_str(
                    "The judge ran but its answer is not a structured consilium review.\nRaw output follows.\n\n",
                );
                let public = config::public_runtime_log(&r.cli, &r.model, &r.raw);
                sb.push_str(&public);
                if !public.is_empty() && !public.ends_with('\n') {
                    sb.push('\n');
                }
                if !r.log_path.is_empty() {
                    let _ = writeln!(sb, "\nLog: {}", r.log_path);
                }
            }
        }
        if i + 1 < result.results.len() {
            sb.push('\n');
        }
    }

    if !result.skipped.is_empty() {
        sb.push('\n');
        for s in &result.skipped {
            let reason = config::public_runtime_error(&s.cli, &s.model, &s.reason);
            let _ = writeln!(sb, "Skipped: {} — {reason}", s.label());
        }
    }

    sb
}
