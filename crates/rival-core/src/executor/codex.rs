#[cfg(test)]
mod tests;

use std::path::Path;

use anyhow::{anyhow, bail};

use super::Mirror;
use super::claude::ROUTE_PROXY;
use super::oscmd::{self, Output};
use super::subprocess::{Request, RunResult, run_subprocess};
use crate::cancel::Context;
use crate::config::{self, Config, PROXY_KEY_ENV, ProxyProvider, Route};
use crate::proxy;
use crate::session::Session;

/// The provider name the proxy route gives Codex in its `-c` values.
const PROXY_PROVIDER: &str = "rival_proxy";

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

/// Executes a prompt with one explicit model. Review
/// pipelines use this entry point so the model recorded in the session is
/// also the model sent to the runtime. Codex is the only model it runs.
#[allow(clippy::too_many_arguments)]
pub fn run_codex_model(
    ctx: &Context,
    cfg: &Config,
    sess: &mut Session,
    prompt: &str,
    effort: &str,
    workdir: &str,
    model: &str,
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
    log: Option<&str>,
    spawn: impl FnOnce(&mut Session, &Request<'_>) -> anyhow::Result<RunResult>,
) -> anyhow::Result<RunResult> {
    if model != config::CODEX_MODEL {
        bail!("unsupported codex model {:?}", model);
    }
    let result = (|| {
        let (args, child_env) = match cfg.proxy_route(ProxyProvider::Codex)? {
            Some(route) => {
                let wire = proxy::wire_id(&route.prefix, model);
                let args = codex_proxy_run_args(&wire, effort, workdir, &route.url)?;
                sess.route = ROUTE_PROXY.to_string();
                sess.wire_model = wire;
                sess.account = ROUTE_PROXY.to_string();
                (args, proxy_env(&route))
            }
            None => (codex_run_args(model, effort, workdir), Vec::new()),
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
        spawn(sess, &req)
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

pub(crate) fn codex_run_args(model: &str, effort: &str, workdir: &str) -> Vec<String> {
    // The codex runtime takes ultra as its own reasoning level (distinct from
    // xhigh), so preserve the requested value instead of normalizing it.
    [
        "exec",
        "-C",
        workdir,
        "-m",
        model,
        "-c",
        &format!("model_reasoning_effort={effort}"),
        "--sandbox",
        "read-only",
        "--ephemeral",
        "--skip-git-repo-check",
        "--color",
        "never",
        "-",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// [`codex_run_args`] on the proxy route: the two provider `-c` pairs go
/// right after `-m <wire>`. The key is not here; the provider names the
/// variable that holds it.
pub(crate) fn codex_proxy_run_args(
    wire: &str,
    effort: &str,
    workdir: &str,
    url: &str,
) -> anyhow::Result<Vec<String>> {
    let mut args = codex_run_args(wire, effort, workdir);
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
