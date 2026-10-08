//! Go: `internal/executor/claude.go`.

#[cfg(test)]
mod tests;

use std::path::Path;

use anyhow::{anyhow, bail};

use super::Mirror;
use super::claude_docker::{claude_docker_preflight, run_claude_docker_with};
use super::oscmd;
use super::subprocess::{Request, RunResult, run_subprocess};
use crate::cancel::Context;
use crate::config::{self, Config};
use crate::logging;
use crate::session::{self, Session};

/// Go `ClaudePreflight`: checks that claude is available (native or docker).
pub fn claude_preflight(cfg: &Config) -> anyhow::Result<()> {
    if oscmd::look_path(cfg, "claude").is_ok() {
        return Ok(());
    }
    claude_docker_preflight(cfg)
}

/// Go `RunClaude`: executes a prompt through the Claude Code CLI on Opus 5.5,
/// the only model on this path. `read_only` restricts tools and mounts the
/// workdir read-only: reviews and task runs.
#[allow(clippy::too_many_arguments)]
pub fn run_claude(
    ctx: &Context,
    cfg: &Config,
    sess: &mut Session,
    prompt: &str,
    effort: &str,
    workdir: &str,
    read_only: bool,
    mirror: Mirror<'_>,
) -> anyhow::Result<RunResult> {
    run_claude_model(
        cfg,
        sess,
        prompt,
        effort,
        workdir,
        config::CLAUDE_MODEL,
        read_only,
        |sess, req| run_subprocess(ctx, cfg.paths(), sess, req, mirror),
    )
}

/// Go `runClaudeModel`: runs Claude through the Claude Code CLI,
/// auto-selecting native (claude on `PATH`) vs docker. `spawn` is the
/// subprocess step.
///
/// Go derived `read_only` from the session mode. The caller passes it here
/// instead, because this function replaces the mode with the transport
/// ("native" or "docker"): a second call on the same session would lose it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_claude_model(
    cfg: &Config,
    sess: &mut Session,
    prompt: &str,
    effort: &str,
    workdir: &str,
    model: &str,
    read_only: bool,
    spawn: impl FnOnce(&mut Session, &Request<'_>) -> anyhow::Result<RunResult>,
) -> anyhow::Result<RunResult> {
    if model != config::CLAUDE_MODEL {
        bail!("unsupported Claude Code model {:?}", model);
    }
    let result = if oscmd::look_path(cfg, "claude").is_ok() {
        set_claude_transport_mode(sess, "native");
        run_claude_native(cfg, sess, prompt, effort, workdir, model, read_only, spawn)
    } else {
        set_claude_transport_mode(sess, "docker");
        run_claude_docker_with(cfg, sess, prompt, effort, workdir, model, read_only, spawn)
    };
    result.map_err(|err| {
        let label = config::engine_label("claude", model);
        anyhow!(
            "{label} runtime: {}",
            config::public_runtime_error("claude", model, &format!("{err:#}"))
        )
    })
}

/// Go `setClaudeTransportMode`: records the transport for ordinary model
/// runs while preserving a task session's identity throughout its live
/// execution.
///
/// Plan and security runs are named by their mode in both dashboards.
/// Writing the transport over that mode would label them for the whole live
/// run, so only ordinary runs record it.
pub(crate) fn set_claude_transport_mode(sess: &mut Session, transport: &str) {
    if session::is_task_mode(&sess.mode) {
        return;
    }
    sess.mode = transport.to_string();
}

#[allow(clippy::too_many_arguments)]
fn run_claude_native(
    cfg: &Config,
    sess: &mut Session,
    prompt: &str,
    effort: &str,
    workdir: &str,
    model: &str,
    read_only: bool,
    spawn: impl FnOnce(&mut Session, &Request<'_>) -> anyhow::Result<RunResult>,
) -> anyhow::Result<RunResult> {
    let auth = cfg.claude_auth()?;
    sess.account = auth.to_string();
    logging::info()
        .str("session", sess.id.clone())
        .str("auth", auth)
        .str("model", config::engine_label("claude", model))
        .msg("model native auth mode");

    // Subscription mode: the claude CLI is already authed via /login. An
    // inherited ANTHROPIC_API_KEY would silently win over that login and
    // bill API credits — strip it so billing stays on the subscription.
    let mut drop_env = vec!["CLAUDECODE"];
    if auth == config::CLAUDE_AUTH_SUBSCRIPTION {
        drop_env.extend(["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"]);
    }

    let args = claude_args(model, effort, read_only);

    let full_prompt = format!(
        "{}\n{prompt}",
        cfg.build_workdir_preamble(Path::new(workdir))
    );
    let req = Request {
        binary: "claude",
        args: &args,
        env: &[],
        prompt: &full_prompt,
        drop_env: &drop_env,
        environ: cfg.environ(),
    };
    spawn(sess, &req)
}

/// Go `claudeArgs`: restricts the available tools, not just auto-approved
/// tools. Safe mode prevents repository/user hooks and plugins from
/// executing around those tools while preserving the CLI's subscription
/// authentication.
pub(crate) fn claude_args(model: &str, effort: &str, read_only: bool) -> Vec<String> {
    let claude_effort = config::claude_effort_level(effort).unwrap_or("max");
    let mut args = vec![
        "-p",
        "--model",
        model,
        "--effort",
        claude_effort,
        "--output-format",
        "text",
        "--no-session-persistence",
        "--system-prompt",
        config::SYSTEM_PROMPT,
    ];
    if read_only {
        args.extend([
            "--safe-mode",
            "--setting-sources",
            "",
            "--settings",
            r#"{"disableAllHooks":true}"#,
            "--strict-mcp-config",
            "--mcp-config",
            r#"{"mcpServers":{}}"#,
            "--tools",
            "Read,Glob,Grep",
            "--allowedTools",
            "Read,Glob,Grep",
            "--disallowedTools",
            "mcp__*",
            "--permission-mode",
            "dontAsk",
        ]);
    } else {
        args.push("--dangerously-skip-permissions");
    }
    args.into_iter().map(str::to_string).collect()
}

/// Go `claudeAuthMarkers`: CLI output fragments that indicate an
/// auth/billing failure rather than a model failure.
const CLAUDE_AUTH_MARKERS: [&str; 6] = [
    "Credit balance is too low",
    "Invalid API key",
    "Please run /login",
    "not logged in",
    "OAuth token has expired",
    "authentication_error",
];

/// Go `ClaudeAuthHint`: inspects a failed native run's log for auth/billing
/// errors and returns an actionable, auth-mode-specific explanation ("" if
/// none).
pub fn claude_auth_hint(cfg: &Config, log_file: &Path) -> String {
    let Ok(data) = std::fs::read(log_file) else {
        return String::new();
    };
    // Go's strings.Contains on raw bytes: the ASCII markers match the same
    // way in the lossy text.
    let text = String::from_utf8_lossy(&data);
    if !CLAUDE_AUTH_MARKERS.iter().any(|m| text.contains(m)) {
        return String::new();
    }
    match cfg.claude_auth() {
        Err(err) => err.to_string(),
        Ok(config::CLAUDE_AUTH_API) => "rival: API auth failed (RIVAL_CLAUDE_AUTH=api) — check ANTHROPIC_API_KEY and its credit balance, or unset RIVAL_CLAUDE_AUTH to use the claude CLI subscription login".to_string(),
        Ok(_) => "rival: subscription auth failed — run `claude` once and /login (Pro/Max), or set RIVAL_CLAUDE_AUTH=api with a funded ANTHROPIC_API_KEY".to_string(),
    }
}
