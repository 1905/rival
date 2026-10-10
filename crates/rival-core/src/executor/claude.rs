#[cfg(test)]
mod tests;

use std::path::Path;

use anyhow::{anyhow, bail};

use super::Mirror;
use super::claude_docker::{claude_docker_preflight, run_claude_docker_with};
use super::oscmd;
use super::subprocess::{Request, RunResult, run_subprocess};
use crate::cancel::Context;
use crate::config::{self, Config, ProxyProvider, Route};
use crate::logging;
use crate::proxy;
use crate::session::{self, Session};

/// The route value a proxied session records.
pub(crate) const ROUTE_PROXY: &str = "proxy";

/// The variables a proxied Claude run gets (by name in Docker).
pub(crate) const PROXY_URL_VAR: &str = "ANTHROPIC_BASE_URL";
pub(crate) const PROXY_KEY_VAR: &str = "ANTHROPIC_API_KEY";

/// Inherited variables that would send a proxied run elsewhere: another
/// credential, a model remap, or a cloud provider. The proxy's own two
/// variables are dropped too, so the injected values are the only ones.
pub(crate) const PROXY_DROP_ENV: [&str; 9] = [
    "CLAUDECODE",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    PROXY_URL_VAR,
    PROXY_KEY_VAR,
];

/// Checks that claude is available (native or docker) and, on the proxy
/// route, that the proxy serves the model.
pub fn claude_preflight(cfg: &Config) -> anyhow::Result<()> {
    claude_preflight_model(cfg, config::CLAUDE_MODEL)
}

/// Whether the Claude Code runtime runs `model`: Opus 5.5 or Fable 5.1.
pub fn is_claude_model(model: &str) -> bool {
    model == config::CLAUDE_MODEL || model == config::FABLE_MODEL
}

/// [`claude_preflight`] for one Claude model id.
pub fn claude_preflight_model(cfg: &Config, model: &str) -> anyhow::Result<()> {
    if oscmd::look_path(cfg, "claude").is_err() {
        claude_docker_preflight(cfg)?;
    }
    if let Some(route) = cfg.proxy_route(ProxyProvider::Claude)? {
        proxy::preflight(&route, ProxyProvider::Claude, model).map_err(|e| anyhow!(e))?;
    }
    Ok(())
}

/// Executes a prompt through the Claude Code CLI on `model` (Opus 5.5 or
/// Fable 5.1). `read_only` restricts tools and mounts the workdir
/// read-only: reviews and task runs.
#[allow(clippy::too_many_arguments)]
pub fn run_claude(
    ctx: &Context,
    cfg: &Config,
    sess: &mut Session,
    prompt: &str,
    effort: &str,
    workdir: &str,
    model: &str,
    read_only: bool,
    log: Option<&str>,
    mirror: Mirror<'_>,
) -> anyhow::Result<RunResult> {
    run_claude_model(
        cfg,
        sess,
        prompt,
        effort,
        workdir,
        model,
        read_only,
        log,
        |sess, req| run_subprocess(ctx, cfg.paths(), sess, req, mirror),
    )
}

/// Runs Claude through the Claude Code CLI,
/// auto-selecting native (claude on `PATH`) vs docker. `spawn` is the
/// subprocess step.
///
/// The caller passes `read_only` rather than this function deriving it
/// from the session mode, because this function replaces the mode with the
/// transport ("native" or "docker"): a second call on the same session
/// would lose it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_claude_model(
    cfg: &Config,
    sess: &mut Session,
    prompt: &str,
    effort: &str,
    workdir: &str,
    model: &str,
    read_only: bool,
    log: Option<&str>,
    spawn: impl FnOnce(&mut Session, &Request<'_>) -> anyhow::Result<RunResult>,
) -> anyhow::Result<RunResult> {
    if !is_claude_model(model) {
        bail!("unsupported Claude Code model {:?}", model);
    }
    let result = (|| {
        let route = cfg.proxy_route(ProxyProvider::Claude)?;
        if let Some(route) = &route {
            sess.route = ROUTE_PROXY.to_string();
            sess.wire_model = proxy::wire_id(&route.prefix, model);
            sess.account = ROUTE_PROXY.to_string();
        }
        if oscmd::look_path(cfg, "claude").is_ok() {
            set_claude_transport_mode(sess, "native");
            run_claude_native(
                cfg,
                sess,
                prompt,
                effort,
                workdir,
                model,
                route.as_ref(),
                read_only,
                log,
                spawn,
            )
        } else {
            set_claude_transport_mode(sess, "docker");
            run_claude_docker_with(
                cfg,
                sess,
                prompt,
                effort,
                workdir,
                model,
                route.as_ref(),
                read_only,
                log,
                spawn,
            )
        }
    })();
    result.map_err(|err| {
        let label = config::engine_label("claude", model);
        anyhow!(
            "{label} runtime: {}",
            config::public_runtime_error("claude", model, &format!("{err:#}"))
        )
    })
}

/// Records the transport for ordinary model
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
    route: Option<&Route>,
    read_only: bool,
    log: Option<&str>,
    spawn: impl FnOnce(&mut Session, &Request<'_>) -> anyhow::Result<RunResult>,
) -> anyhow::Result<RunResult> {
    let mut drop_env = vec!["CLAUDECODE"];
    let mut child_env: Vec<String> = Vec::new();
    let wire_model;
    let auth = match route {
        Some(route) => {
            // The proxy route wins over RIVAL_CLAUDE_AUTH: the proxy key is
            // the credential, whatever the mode says.
            if !cfg.getenv("RIVAL_CLAUDE_AUTH").trim().is_empty() {
                logging::debug()
                    .str("session", sess.id.clone())
                    .msg("proxy route wins over RIVAL_CLAUDE_AUTH");
            }
            drop_env = PROXY_DROP_ENV.to_vec();
            child_env = proxy_env(&route.url, route);
            wire_model = proxy::wire_id(&route.prefix, model);
            ROUTE_PROXY
        }
        None => {
            let auth = cfg.claude_auth()?;
            // Subscription mode: the claude CLI is already authed via
            // /login. An inherited ANTHROPIC_API_KEY would silently win over
            // that login and bill API credits — strip it so billing stays on
            // the subscription.
            if auth == config::CLAUDE_AUTH_SUBSCRIPTION {
                drop_env.extend(["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"]);
            }
            wire_model = model.to_string();
            auth
        }
    };
    sess.account = auth.to_string();
    logging::info()
        .str("session", sess.id.clone())
        .str("auth", auth)
        .str("model", config::engine_label("claude", model))
        .msg("model native auth mode");

    let args = claude_args(&wire_model, effort, read_only);

    let full_prompt = format!(
        "{}\n{prompt}",
        cfg.build_workdir_preamble(Path::new(workdir))
    );
    let req = Request {
        binary: "claude",
        args: &args,
        env: &child_env,
        prompt: &full_prompt,
        drop_env: &drop_env,
        environ: cfg.environ(),
        log,
    };
    spawn(sess, &req)
}

/// The two proxy variables, `KEY=VALUE`, with `url` as the base URL (Docker
/// passes a rewritten loopback host). They go in `Request.env`, never in
/// the arguments.
pub(crate) fn proxy_env(url: &str, route: &Route) -> Vec<String> {
    vec![
        format!("{PROXY_URL_VAR}={url}"),
        format!("{PROXY_KEY_VAR}={}", route.key()),
    ]
}

/// Restricts the available tools, not just auto-approved
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

/// CLI output fragments that indicate an
/// auth/billing failure rather than a model failure.
const CLAUDE_AUTH_MARKERS: [&str; 6] = [
    "Credit balance is too low",
    "Invalid API key",
    "Please run /login",
    "not logged in",
    "OAuth token has expired",
    "authentication_error",
];

/// Inspects a failed native run's log for auth/billing
/// errors and returns an actionable, auth-mode-specific explanation ("" if
/// none).
pub fn claude_auth_hint(cfg: &Config, model: &str, log_file: &Path) -> String {
    let Ok(data) = std::fs::read(log_file) else {
        return String::new();
    };
    // A byte search on the raw log: the ASCII markers match the same way in
    // the lossy text.
    let text = String::from_utf8_lossy(&data);
    if let Ok(Some(route)) = cfg.proxy_route(ProxyProvider::Claude) {
        return proxy_auth_hint(&route, model, &text);
    }
    if !CLAUDE_AUTH_MARKERS.iter().any(|m| text.contains(m)) {
        return String::new();
    }
    match cfg.claude_auth() {
        Err(err) => err.to_string(),
        Ok(config::CLAUDE_AUTH_API) => "rival: API auth failed (RIVAL_CLAUDE_AUTH=api) — check ANTHROPIC_API_KEY and its credit balance, or unset RIVAL_CLAUDE_AUTH to use the claude CLI subscription login".to_string(),
        Ok(_) => "rival: subscription auth failed — run `claude` once and /login (Pro/Max), or set RIVAL_CLAUDE_AUTH=api with a funded ANTHROPIC_API_KEY".to_string(),
    }
}

/// The proxy branch of [`claude_auth_hint`]: an account at its limit
/// (429), or a rejected key or unknown model; "" for other failures.
fn proxy_auth_hint(route: &Route, model: &str, text: &str) -> String {
    // The preflight fetched the list; a failure here only drops the
    // other-prefix names from the hint.
    let served = proxy::models(route).unwrap_or_default();
    if let Some(hint) = proxy::classify_limit(text, route, ProxyProvider::Claude, model, &served) {
        return format!("rival: {hint}");
    }
    let wire = proxy::wire_id(&route.prefix, model);
    if CLAUDE_AUTH_MARKERS.iter().any(|m| text.contains(m))
        || text.contains("401")
        || text.contains("unknown provider for model")
    {
        return format!(
            "rival: proxy request failed for {wire} — check the proxy key (rival config key set) and proxy.claude.model_prefix (now {:?}); a 429 means the account is at its limit",
            route.prefix
        );
    }
    String::new()
}
