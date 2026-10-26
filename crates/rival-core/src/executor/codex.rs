#[cfg(test)]
mod tests;

use std::path::Path;

use anyhow::{anyhow, bail};

use super::Mirror;
use super::claude::ROUTE_PROXY;
use super::oscmd::{self, Output};
use super::subprocess::{Request, RunResult, run_subprocess};
use crate::cancel::Context;
use crate::config::{self, Config, PROXY_KEY_ENV, ProxyProvider, Route, Speed};
use crate::proxy;
use crate::session::Session;

/// The provider name the proxy route gives Codex in its `-c` values.
const PROXY_PROVIDER: &str = "rival_proxy";

/// Who can use Ultrafast; the notice names it on the direct route.
const ULTRAFAST_REQUIREMENT: &str =
    "Ultrafast needs the Pro $500 plan, or an Enterprise/Edu workspace with Ultrafast on.";

/// Codex prints this line, then sends the request without the tier, when
/// the model catalog it loaded does not list the configured tier
/// (openai/codex `core/src/session/mod.rs`, `unsupported_service_tier_warning`).
const TIER_OMITTED_PREFIX: &str = "warning: Configured service tier `";
const TIER_OMITTED_MIDDLE: &str = "` is not advertised as supported for model `";
const TIER_OMITTED_SUFFIX: &str = "` and will be omitted from requests.";

/// Codex echoes the prompt near the start of its log, so the warning is
/// in this many bytes after the prompt's length.
const TIER_WARNING_WINDOW: usize = 64 * 1024;

/// Checks that codex is installed and authenticated,
/// naming the given model in its errors. On the proxy route the proxy is
/// the credential: the check asks the proxy, not `codex login status`.
pub fn codex_preflight_for(cfg: &Config, model: &str) -> anyhow::Result<()> {
    let label = config::engine_label("codex", model);
    if oscmd::look_path(cfg, "codex").is_err() {
        bail!("{label} runtime is not installed");
    }
    if let Some(route) = cfg.proxy_route(ProxyProvider::Codex)? {
        proxy_provider_args(&route.url)?;
        return proxy::preflight(&route, ProxyProvider::Codex, model).map_err(|e| anyhow!(e));
    }

    let (out, result) = oscmd::run(cfg, "codex", &["login", "status"], Output::Combined);
    if result.is_err() {
        bail!(
            "{label} authentication is unavailable\n{}",
            String::from_utf8_lossy(&out)
        );
    }
    Ok(())
}

/// Whether the codex runtime runs `model`: Codex or Sol 6.1.
pub fn is_codex_model(model: &str) -> bool {
    model == config::CODEX_MODEL || model == config::SOL_MODEL
}

/// The tier names Codex can print in its omitted-tier warning for `tier`:
/// Codex maps `fast` to the `priority` tier, so either name counts.
fn warned_tiers(tier: &str) -> &'static [&'static str] {
    match tier {
        "fast" => &["fast", "priority"],
        "ultrafast" => &["ultrafast"],
        _ => &[],
    }
}

/// Whether a codex log has the runtime warning that Codex omitted `tier`.
/// Codex prints its config warnings once, right after it echoes `prompt`
/// and before any turn output. So only the `warning:` lines that directly
/// follow the echo count: the same line inside the prompt or the reply does
/// not. No echo of `prompt` in the log: false.
pub(crate) fn tier_omitted(log: &str, prompt: &str, tier: &str) -> bool {
    let Some(at) = log.find(prompt) else {
        return false;
    };
    let names = warned_tiers(tier);
    log[at + prompt.len()..]
        .lines()
        .map(str::trim)
        .skip_while(|line| line.is_empty())
        .take_while(|line| line.starts_with("warning: "))
        .any(|line| {
            line.strip_prefix(TIER_OMITTED_PREFIX)
                .and_then(|rest| names.iter().find_map(|name| rest.strip_prefix(name)))
                .and_then(|rest| rest.strip_prefix(TIER_OMITTED_MIDDLE))
                .and_then(|rest| rest.strip_suffix(TIER_OMITTED_SUFFIX))
                .is_some_and(|slug| !slug.is_empty() && !slug.contains('`'))
        })
}

/// The lines that say Codex dropped the service tier of `sess`, or `None`.
/// The run still counts: Codex sent it at the standard tier. Only the
/// direct route of an Ultrafast run names the plan as the cause: through
/// the proxy, the catalog and the account are the proxy's.
pub fn tier_notice(sess: &Session) -> Option<String> {
    if !sess.service_tier_omitted {
        return None;
    }
    let tier = sess.service_tier.as_str();
    let label = config::engine_label(&sess.cli, &sess.model);
    let mut notice = format!(
        "rival: ⚠ {tier} not applied — Codex omitted service_tier={tier} for {label} because its model catalog does not list the tier; the run used the standard tier.\n"
    );
    if sess.route != ROUTE_PROXY && Speed::from_tier(tier) == Speed::Ultrafast {
        notice.push_str(&format!("rival: {ULTRAFAST_REQUIREMENT}\n"));
    }
    Some(notice)
}

/// The start of the log at `path`: enough to hold the prompt echo and the
/// warnings after it.
fn log_head(path: &str, prompt: &str) -> std::io::Result<String> {
    use std::io::Read;
    let mut head = Vec::new();
    std::fs::File::open(path)?
        .take((prompt.len() + TIER_WARNING_WINDOW) as u64)
        .read_to_end(&mut head)?;
    Ok(String::from_utf8_lossy(&head).into_owned())
}

/// Executes a prompt with one explicit model. Review
/// pipelines use this entry point so the model recorded in the session is
/// also the model sent to the runtime. It runs Codex and Sol 6.1. When
/// [`Config::speed`] of `model` has a tier, the call asks for it and the
/// session records it.
///
/// The caller passes `read_only` rather than this function deriving it from
/// the session mode, as [`super::run_claude`] does: reviews and the language
/// pass must not touch the workdir, a raw prompt may edit it.
#[allow(clippy::too_many_arguments)]
pub fn run_codex_model(
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
    run_codex_model_with(
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

/// [`run_codex_model`] with the spawn step injected.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_codex_model_with(
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
    if !is_codex_model(model) {
        bail!("unsupported codex model {:?}", model);
    }
    let speed = cfg.speed(model);
    if let Some(tier) = speed.tier() {
        sess.service_tier = tier.to_string();
    }
    let result = (|| -> anyhow::Result<RunResult> {
        let (args, child_env) = match cfg.proxy_route(ProxyProvider::Codex)? {
            Some(route) => {
                let wire = proxy::wire_id(&route.prefix, model);
                let args =
                    codex_proxy_run_args(&wire, effort, workdir, speed, read_only, &route.url)?;
                sess.route = ROUTE_PROXY.to_string();
                sess.wire_model = wire;
                sess.account = ROUTE_PROXY.to_string();
                (args, proxy_env(&route))
            }
            None => (
                codex_run_args(model, effort, workdir, speed, read_only),
                Vec::new(),
            ),
        };

        let full_prompt = format!(
            "{}\n\n{}\n{prompt}",
            config::SYSTEM_PROMPT,
            cfg.build_workdir_preamble(Path::new(workdir))
        );
        let req = Request {
            binary: "codex",
            args: &args,
            env: &child_env,
            prompt: &full_prompt,
            drop_env: &[],
            environ: cfg.environ(),
            log,
        };
        let result = spawn(sess, &req)?;
        // The language pass is a second call on the same session; either
        // call can find the warning, and neither clears it.
        // Once a call of the session found the warning, later calls (the
        // language pass) need not read their log for it.
        if let Some(tier) = speed.tier()
            && !sess.service_tier_omitted
        {
            let path = log.unwrap_or(&sess.log_file);
            if let Ok(head) = log_head(path, &full_prompt)
                && tier_omitted(&head, &full_prompt, tier)
            {
                sess.service_tier_omitted = true;
            }
        }
        Ok(result)
    })();
    result.map_err(|err| {
        let label = config::engine_label("codex", model);
        let message = config::replace_ordered(
            &format!("{err:#}"),
            &[
                ("Codex", label.clone()),
                ("codex", label.clone()),
                (model, label.clone()),
            ],
        );
        anyhow!("{label} runtime: {message}")
    })
}

/// The `codex exec` argv. A `speed` with a tier adds `-c
/// service_tier="<tier>"` and `-c features.<tier>_mode=true` right after
/// `-m <model>`, so a local config that turned the feature off cannot drop
/// the tier unseen. Normal adds nothing. `read_only` selects the sandbox:
/// reviews and the language pass get `read-only`, a raw prompt gets
/// `workspace-write`, so the agent can edit files in the workdir.
pub(crate) fn codex_run_args(
    model: &str,
    effort: &str,
    workdir: &str,
    speed: Speed,
    read_only: bool,
) -> Vec<String> {
    // The codex runtime takes ultra as its own reasoning level (distinct from
    // xhigh), so preserve the requested value instead of normalizing it.
    let mut args: Vec<String> = [
        "exec",
        "-C",
        workdir,
        "-m",
        model,
        "-c",
        &format!("model_reasoning_effort={effort}"),
        "--sandbox",
        if read_only {
            "read-only"
        } else {
            "workspace-write"
        },
        "--ephemeral",
        "--skip-git-repo-check",
        "--color",
        "never",
        "-",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if let Some(tier) = speed.tier() {
        args.splice(
            5..5,
            [
                "-c".to_string(),
                format!("service_tier=\"{tier}\""),
                "-c".to_string(),
                format!("features.{tier}_mode=true"),
            ],
        );
    }
    args
}

/// [`codex_run_args`] on the proxy route: the two provider `-c` pairs go
/// right after `-m <wire>`, before any tier pairs. The key is not here; the
/// provider names the variable that holds it.
pub(crate) fn codex_proxy_run_args(
    wire: &str,
    effort: &str,
    workdir: &str,
    speed: Speed,
    read_only: bool,
    url: &str,
) -> anyhow::Result<Vec<String>> {
    let mut args = codex_run_args(wire, effort, workdir, speed, read_only);
    args.splice(5..5, proxy_provider_args(url)?);
    Ok(args)
}

/// `-c model_provider=…` and `-c model_providers.rival_proxy={…}`, each
/// value one argument, as P0 ran them against Codex CLI 0.161.0. A URL that
/// would need TOML escaping is a config error.
fn proxy_provider_args(url: &str) -> anyhow::Result<[String; 4]> {
    if url.chars().any(|c| c == '"' || c == '\\' || c.is_control()) {
        bail!(
            "proxy URL {url:?} cannot go in a codex -c value: it has a quote, a backslash or a control character — run rival config set proxy.url <url>"
        );
    }
    Ok([
        "-c".to_string(),
        format!("model_provider=\"{PROXY_PROVIDER}\""),
        "-c".to_string(),
        format!(
            "model_providers.{PROXY_PROVIDER}={{ name = \"rival proxy\", base_url = \"{url}/v1\", env_key = \"{PROXY_KEY_ENV}\", wire_api = \"responses\" }}"
        ),
    ])
}

/// `RIVAL_PROXY_KEY=<key>` for `Request.env`: the inherited variable is
/// blocked for every child, so this is the only way it reaches codex.
fn proxy_env(route: &Route) -> Vec<String> {
    vec![format!("{PROXY_KEY_ENV}={}", route.key())]
}

/// Inspects a failed run's log on the proxy route: an account at its
/// limit (429), or a rejected key (Codex retries, then exits 1). "" on the
/// direct route and for other failures.
pub fn codex_auth_hint(cfg: &Config, model: &str, log_file: &Path) -> String {
    let Ok(Some(route)) = cfg.proxy_route(ProxyProvider::Codex) else {
        return String::new();
    };
    let Ok(data) = std::fs::read(log_file) else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&data);
    // The preflight fetched the list; a failure here only drops the
    // other-prefix names from the hint.
    let served = proxy::models(&route).unwrap_or_default();
    if let Some(hint) = proxy::classify_limit(&text, &route, ProxyProvider::Codex, model, &served) {
        return format!("rival: {hint}");
    }
    if text.contains("401") || text.contains("403") || text.contains("Reconnecting...") {
        return format!(
            "rival: proxy request failed for {} — check the proxy key (rival config key set) and proxy.codex.model_prefix (now {:?}); a 429 means the account is at its limit",
            proxy::wire_id(&route.prefix, model),
            route.prefix
        );
    }
    String::new()
}
