#[cfg(test)]
mod tests;

use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, bail};

use super::Mirror;
use super::oscmd::{self, Output};
use super::subprocess::{Request, RunResult, run_subprocess, safe_env};
use crate::cancel::Context;
use crate::config::{self, Config};
use crate::session::Session;

/// The review profile on opencode v2. It denies every tool by default and
/// allows only the native reads. The v1 reviewer profile lists the denied
/// tools instead, and v2 adds tools it does not name (`execute` runs code
/// with network access, plus `skill`, `question`, `worktree_create`), which
/// would stay allowed.
pub(crate) const CHINA_READ_ONLY_PERMISSION: &str = r#"{"*":"deny","read":"allow","grep":"allow","glob":"allow","list":"allow","external_directory":"deny"}"#;

/// The raw-mode profile: every tool is allowed except out-of-workdir native
/// reads. The user asked for a full-auto agent that edits files and runs
/// commands in the workdir. Bash is allowed, so the external_directory deny
/// is defense in depth, not containment.
pub(crate) const OPENCODE_FULL_AUTO_PERMISSION: &str = r#"{"read":"allow","grep":"allow","glob":"allow","list":"allow","external_directory":"deny","edit":"allow","bash":"allow","task":"allow","webfetch":"allow","websearch":"allow"}"#;

/// Credential vars stripped from the child in raw (full-auto) mode, where
/// every tool call is allowed: a prompt-injected repo could otherwise read
/// an inherited secret with `env` and send it out. opencode needs none of
/// them. The other modes deny bash. This shrinks the blast radius but is
/// NOT a sandbox: the agent can still run any command as the user.
pub(crate) const RAW_DROP_ENV: [&str; 9] = [
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "GOOGLE_",
    "RIVAL_CLAUDE_TOKEN",
    // Trailing underscore = prefix drop: catches SESSION_TOKEN, PROFILE,
    // WEB_IDENTITY_TOKEN_FILE, container credential URIs — the whole family.
    "AWS_",
    "GITHUB_TOKEN",
    "GH_TOKEN",
    "GITLAB_TOKEN",
];

/// Keeps the project's own opencode config out of every china call: a
/// reviewed repo must not re-enable tools, and the model list must match
/// what a run can use.
pub(crate) const DISABLE_PROJECT_CONFIG: &str = "OPENCODE_DISABLE_PROJECT_CONFIG=1";

/// The opencode agent every china run uses. The user's own agents (such as
/// the default `build`) can carry permissions that override the top-level
/// `permission` block, so the run gets an agent of its own with the profile.
pub(crate) const CHINA_AGENT: &str = "rival-china";

/// Maps a rival effort onto Moonshot's menu (low, high, max). "" is the
/// default, max.
pub fn china_effort(requested: &str) -> anyhow::Result<&'static str> {
    Ok(match requested {
        "low" => "low",
        "medium" | "high" => "high",
        "" | "xhigh" | "ultra" | "max" => "max",
        other => bail!("invalid effort {other:?}; use one of: low, medium, high, xhigh, ultra"),
    })
}

/// Fails with the install hint when the opencode CLI is not on `$PATH`.
fn require_opencode(cfg: &Config) -> anyhow::Result<()> {
    if oscmd::look_path(cfg, "opencode").is_err() {
        bail!("opencode CLI not installed. Install: curl -fsSL https://opencode.ai/install | bash");
    }
    Ok(())
}

/// How long a one-shot opencode query (`--version`, `models`) may take.
const QUERY_TIMEOUT: Duration = Duration::from_secs(30);

/// Runs `opencode <args>` once and returns its output. The child gets the
/// filtered env of the provider run: a reviewed repo's .env must not reach
/// it (LD_PRELOAD, NODE_OPTIONS). The run timeout starts only after the
/// preflight, so the call has its own bound.
fn opencode_query(cfg: &Config, args: &[&str]) -> anyhow::Result<String> {
    require_opencode(cfg)?;
    let mut environ = safe_env(cfg.environ());
    environ.push(DISABLE_PROJECT_CONFIG.into());
    let (ctx, _cancel) = Context::background().with_timeout(QUERY_TIMEOUT);
    let (out, res) = oscmd::with_context(&ctx, || {
        oscmd::run_env(cfg, "opencode", args, Output::Combined, &environ)
    });
    res.map_err(|e| anyhow!("opencode {}: {e}", args.join(" ")))?;
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// Every model opencode can run, as `provider/model` selectors, from
/// `opencode models --standalone`. It lists the providers opencode has auth
/// or config for, without the models the user's config disables.
pub fn china_models(cfg: &Config) -> anyhow::Result<Vec<String>> {
    Ok(parse_opencode_models(&opencode_query(
        cfg,
        &["models", "--standalone"],
    )?))
}

/// The selectors in `opencode models` output: one per line, sorted and
/// without repeats. Other lines (warnings) have no valid selector.
pub(crate) fn parse_opencode_models(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| config::parse_china_model(l).is_some())
        .map(String::from)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Checks that opencode v2 or later is installed. Auth is opencode's own
/// (`opencode auth login` or its config), so no key is checked here.
pub fn china_preflight(cfg: &Config) -> anyhow::Result<()> {
    let version = opencode_query(cfg, &["--version"])?;
    match opencode_major(&version) {
        Some(major) if major >= 2 => Ok(()),
        _ => bail!(
            "china needs opencode v2 or later, found {:?}. Update: opencode upgrade",
            version.trim()
        ),
    }
}

/// The major version in `opencode --version` output (`opencode v2.1.3`,
/// `2.1.3`, or v1's bare `1.4.0`).
pub(crate) fn opencode_major(version: &str) -> Option<u32> {
    let word = version.split_whitespace().last()?;
    let digits = word.strip_prefix('v').unwrap_or(word);
    digits.split('.').next()?.parse().ok()
}

/// Runs a prompt with the `china.model` through `opencode run --standalone`
/// (v2). The permission profile follows the session mode: raw is
/// full auto with known credentials stripped, every other mode is the
/// read-only reviewer profile.
#[allow(clippy::too_many_arguments)]
pub fn run_china(
    ctx: &Context,
    cfg: &Config,
    sess: &mut Session,
    prompt: &str,
    effort: &str,
    workdir: &str,
    log: Option<&str>,
    mirror: Mirror<'_>,
) -> anyhow::Result<RunResult> {
    run_china_with(cfg, sess, prompt, effort, workdir, log, |sess, req| {
        run_subprocess(ctx, cfg.paths(), sess, req, mirror)
    })
}

/// [`run_china`] with the spawn step injected.
pub(crate) fn run_china_with(
    cfg: &Config,
    sess: &mut Session,
    prompt: &str,
    effort: &str,
    workdir: &str,
    log: Option<&str>,
    spawn: impl FnOnce(&mut Session, &Request<'_>) -> anyhow::Result<RunResult>,
) -> anyhow::Result<RunResult> {
    let selector = sess.model.clone();
    let Some((provider, model)) = config::parse_china_model(&selector) else {
        bail!("invalid china.model {selector:?}; use an opencode provider/model");
    };
    let raw = sess.mode == "raw";
    let args = china_run_args(&selector);
    // opencode v2 has no `OPENCODE_PERMISSION` and no `--pure`: the profile
    // goes into the config, and `OPENCODE_DISABLE_PROJECT_CONFIG` stops a
    // reviewed repo's own opencode config from re-enabling tools. No
    // `OPENCODE_DB`: v2 keeps `opencode auth login` credentials in its own
    // DB, so a private per-run DB has none (OpenRouter then fails with "No
    // cookie auth credentials found") and leaves an ~8 MB file per run.
    let env = [
        format!(
            "OPENCODE_CONFIG_CONTENT={}",
            china_config_content(provider, model, effort, raw)
        ),
        DISABLE_PROJECT_CONFIG.to_string(),
    ];
    let full_prompt = format!(
        "{}\n\n{}\n{prompt}",
        config::SYSTEM_PROMPT,
        cfg.build_workdir_preamble(Path::new(workdir))
    );
    // safe_env already strips every inherited OPENCODE_ variable.
    let drop: &[&str] = if raw { &RAW_DROP_ENV } else { &[] };
    let req = Request {
        binary: "opencode",
        args: &args,
        env: &env,
        prompt: &full_prompt,
        drop_env: drop,
        environ: cfg.environ(),
        log,
    };
    spawn(sess, &req)
}

/// The `opencode run` argv. v2 reads the prompt on stdin and runs in the
/// child's cwd, the workdir. `--standalone` starts a private server, so the
/// env below applies to this run and not to a shared background service.
pub(crate) fn china_run_args(selector: &str) -> Vec<String> {
    [
        "run",
        "--standalone",
        "--agent",
        CHINA_AGENT,
        "-m",
        selector,
    ]
    .map(String::from)
    .to_vec()
}

/// The config merged over the user's opencode config. It declares the model
/// under its provider (v2 refuses an undeclared model) with the effort,
/// which reaches the API as `reasoning_effort`, and the [`CHINA_AGENT`] with
/// the permission profile. It holds no key.
pub(crate) fn china_config_content(provider: &str, model: &str, effort: &str, raw: bool) -> String {
    let profile = if raw {
        OPENCODE_FULL_AUTO_PERMISSION
    } else {
        CHINA_READ_ONLY_PERMISSION
    };
    let permission: serde_json::Value =
        serde_json::from_str(profile).expect("the permission profiles are valid JSON");
    serde_json::json!({
        "$schema": "https://opencode.ai/config.json",
        "permission": permission,
        "agent": {
            CHINA_AGENT: { "mode": "primary", "permission": permission }
        },
        "provider": {
            provider: {
                "models": {
                    model: { "options": { "reasoningEffort": effort } }
                }
            }
        },
    })
    .to_string()
}
