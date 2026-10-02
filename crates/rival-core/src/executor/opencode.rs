//! Go: `internal/executor/opencode.go`.

#[cfg(test)]
mod tests;

use std::fmt;
use std::path::Path;

use anyhow::bail;

use super::Mirror;
use super::oscmd;
use super::subprocess::{Request, RunResult, run_subprocess};
use crate::cancel::Context;
use crate::config::{self, Config, SecurityModel};
use crate::gojson::quote_bytes;
use crate::gostd::quote;
use crate::session::Session;

/// Go `OpencodePreflightModel`: validates K3, Rival's sole OpenCode-backed
/// model. `workdir` seeds the Moonshot API-key `.env` walk-up for K3 (see
/// [`Config::kimi_api_key_from`]); pass "" when no workdir context exists.
pub fn opencode_preflight_model(cfg: &Config, model: &str, workdir: &str) -> anyhow::Result<()> {
    let Some(entry) = config::open_code_entry_for(model) else {
        bail!("unsupported OpenCode model {}", quote(model));
    };
    opencode_preflight_entry(cfg, &entry, workdir)
}

/// Go `OpencodePreflightEntry`: verifies one registry entry can run: the CLI
/// exists and its credential resolves. The two failures are reported
/// separately, because a present key does not help when the binary is
/// missing.
pub fn opencode_preflight_entry(
    cfg: &Config,
    entry: &SecurityModel,
    workdir: &str,
) -> anyhow::Result<()> {
    if oscmd::look_path(cfg, "opencode").is_err() {
        bail!("opencode CLI not installed. Install: curl -fsSL https://opencode.ai/install | bash");
    }
    if cfg
        .security_api_key_from(entry, Path::new(workdir))
        .is_empty()
    {
        bail!(
            "model {} requires {} — add it to the project .env or export it",
            entry.label,
            entry.key_env
        );
    }
    Ok(())
}

/// A read-only, workdir-scoped permission profile passed to opencode via
/// `OPENCODE_PERMISSION`. A code reviewer reads repo content that may
/// contain prompt-injection, so it must NOT write files, run shell commands,
/// OR read outside the reviewed workdir. read/grep/glob/list are allowed
/// (opencode auto-scopes these to the workdir + its own tool-output dirs);
/// external_directory is DENIED so a prompt-injected repo can't make the
/// reviewer read host secrets (~/.aws/credentials, ~/.ssh, a sibling repo's
/// .env) and exfiltrate them through the review output / logs / consilium
/// prompt. edit/bash/task and web access are denied.
pub(crate) const OPENCODE_READ_ONLY_PERMISSION: &str = r#"{"read":"allow","grep":"allow","glob":"allow","list":"allow","external_directory":"deny","edit":"deny","bash":"deny","task":"deny","webfetch":"deny","websearch":"deny"}"#;

/// Allows every tool except out-of-workdir native reads. Used only by the
/// standalone kimi runner's non-review mode, where the user explicitly asked
/// for a full-auto agent that can edit files and run commands in the
/// workdir. external_directory is denied to keep the native file tools on
/// the documented "in the workdir" promise — bash being allowed means this
/// is defense-in-depth, not containment (a shell can read anything the user
/// can). Review mode never uses this profile.
pub(crate) const OPENCODE_FULL_AUTO_PERMISSION: &str = r#"{"read":"allow","grep":"allow","glob":"allow","list":"allow","external_directory":"deny","edit":"allow","bash":"allow","task":"allow","webfetch":"allow","websearch":"allow"}"#;

/// Go `OpencodeRunOpts`: customizes one opencode execution beyond the
/// reviewer defaults. Zero values keep megareview behavior exactly:
/// read-only permission, the entry's own key lookup, no extra env drops.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct OpencodeRunOpts {
    /// `OPENCODE_PERMISSION` JSON; "" = the read-only reviewer profile.
    pub permission: String,
    /// The provider API key; "" = the entry's key from env or `.env`.
    pub api_key: String,
    /// Extra vars/prefixes stripped from the child (a trailing `_` is a
    /// prefix).
    pub drop_env: Vec<String>,
}

/// Never prints the key.
impl fmt::Debug for OpencodeRunOpts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpencodeRunOpts")
            .field("permission", &self.permission)
            .field(
                "api_key",
                &if self.api_key.is_empty() {
                    ""
                } else {
                    "<redacted>"
                },
            )
            .field("drop_env", &self.drop_env)
            .finish()
    }
}

/// Go `RunOpencode`: executes a K3 prompt through the opencode CLI. The
/// prompt is read from stdin in non-interactive `run` mode; the entry pins
/// opencode's `--variant` (provider-specific reasoning level). It runs under
/// a read-only permission profile (see [`OPENCODE_READ_ONLY_PERMISSION`])
/// rather than `--dangerously-skip-permissions`, so a prompt-injected repo
/// cannot make the reviewer write files or run commands. An empty model
/// falls back to K3.
#[allow(clippy::too_many_arguments)]
pub fn run_opencode(
    ctx: &Context,
    cfg: &Config,
    sess: &mut Session,
    prompt: &str,
    effort: &str,
    workdir: &str,
    model: &str,
    mirror: Mirror<'_>,
) -> anyhow::Result<RunResult> {
    run_opencode_with(
        ctx,
        cfg,
        sess,
        prompt,
        effort,
        workdir,
        model,
        &OpencodeRunOpts::default(),
        mirror,
    )
}

/// Go `RunOpencodeWith`: [`run_opencode`] with per-call overrides (see
/// [`OpencodeRunOpts`]). The standalone kimi runner uses it for its
/// full-auto mode and its moonshot-provider key; megareview reviewers stay
/// on the zero-value defaults.
#[allow(clippy::too_many_arguments)]
pub fn run_opencode_with(
    ctx: &Context,
    cfg: &Config,
    sess: &mut Session,
    prompt: &str,
    effort: &str,
    workdir: &str,
    model: &str,
    opts: &OpencodeRunOpts,
    mirror: Mirror<'_>,
) -> anyhow::Result<RunResult> {
    run_opencode_model_with(
        cfg,
        sess,
        prompt,
        effort,
        workdir,
        model,
        opts,
        |sess, req| run_subprocess(ctx, cfg.paths(), sess, req, mirror),
    )
}

/// [`run_opencode_with`] with the spawn step injected.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_opencode_model_with(
    cfg: &Config,
    sess: &mut Session,
    prompt: &str,
    effort: &str,
    workdir: &str,
    model: &str,
    opts: &OpencodeRunOpts,
    spawn: impl FnOnce(&mut Session, &Request<'_>) -> anyhow::Result<RunResult>,
) -> anyhow::Result<RunResult> {
    let model = if model.is_empty() {
        config::KIMI_MODEL
    } else {
        model
    };
    let Some(entry) = config::open_code_entry_for(model) else {
        bail!("unsupported OpenCode model {}", quote(model));
    };
    run_opencode_entry_with(cfg, sess, prompt, effort, workdir, &entry, opts, spawn)
}

/// Go `RunOpencodeEntry`: runs one registry entry. Everything
/// provider-specific — the `-m` selector, the config block, the credential,
/// the reasoning variant — comes from the entry, so adding a model is a
/// registry change rather than a code change.
#[allow(clippy::too_many_arguments)]
pub fn run_opencode_entry(
    ctx: &Context,
    cfg: &Config,
    sess: &mut Session,
    prompt: &str,
    effort: &str,
    workdir: &str,
    entry: &SecurityModel,
    opts: &OpencodeRunOpts,
    mirror: Mirror<'_>,
) -> anyhow::Result<RunResult> {
    run_opencode_entry_with(
        cfg,
        sess,
        prompt,
        effort,
        workdir,
        entry,
        opts,
        |sess, req| run_subprocess(ctx, cfg.paths(), sess, req, mirror),
    )
}

/// [`run_opencode_entry`] with the spawn step injected.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_opencode_entry_with(
    cfg: &Config,
    sess: &mut Session,
    prompt: &str,
    effort: &str,
    workdir: &str,
    entry: &SecurityModel,
    opts: &OpencodeRunOpts,
    spawn: impl FnOnce(&mut Session, &Request<'_>) -> anyhow::Result<RunResult>,
) -> anyhow::Result<RunResult> {
    let args = opencode_run_args(entry, effort, workdir);
    let env = opencode_run_env_with(cfg, &sess.id, entry, workdir, opts);

    let full_prompt = format!(
        "{}\n\n{}\n{prompt}",
        config::SYSTEM_PROMPT,
        cfg.build_workdir_preamble(Path::new(workdir))
    );
    // Drop any inherited OPENCODE_PERMISSION / OPENCODE_CONFIG_CONTENT
    // before appending ours. rival loads the reviewed repo's .env into the
    // process env, so a malicious repo could otherwise ship a permissive
    // OPENCODE_PERMISSION or a config that weakens the sandbox. (safe_env
    // already strips the OPENCODE_ prefix, so this is belt-and-suspenders.)
    let mut drop = vec![
        "OPENCODE_PERMISSION",
        "OPENCODE_CONFIG_CONTENT",
        "OPENCODE_DB",
    ];
    drop.extend(opts.drop_env.iter().map(String::as_str));
    let req = Request {
        binary: "opencode",
        args: &args,
        env: &env,
        prompt: &full_prompt,
        drop_env: &drop,
        environ: cfg.environ(),
    };
    spawn(sess, &req)
}

/// Go `opencodeRunArgs`.
pub(crate) fn opencode_run_args(
    entry: &SecurityModel,
    _effort: &str,
    workdir: &str,
) -> Vec<String> {
    let mut args = vec![
        "run".to_string(),
        // --pure runs without external plugins / project-controlled config,
        // so a reviewed repo's own .opencode config can't re-enable denied
        // tools or otherwise weaken the read-only sandbox.
        // (OPENCODE_PERMISSION already wins over project config, but this
        // removes all reliance on that.)
        "--pure".to_string(),
        // OpenCode splits this at the first slash to choose the provider, so
        // an OpenRouter-hosted model needs the openrouter/ prefix here even
        // though its upstream id does not carry one.
        "-m".to_string(),
        entry.selector.to_string(),
    ];
    if !entry.variant.is_empty() {
        args.extend(["--variant".to_string(), entry.variant.to_string()]);
    }
    args.extend(["--dir".to_string(), workdir.to_string()]);
    args
}

/// Go `opencodeRunEnvWith`: the `KEY=VALUE` entries appended to the child
/// env. The provider key is inside `OPENCODE_CONFIG_CONTENT`, so the result
/// must never be logged.
pub(crate) fn opencode_run_env_with(
    cfg: &Config,
    session_id: &str,
    entry: &SecurityModel,
    workdir: &str,
    opts: &OpencodeRunOpts,
) -> Vec<String> {
    let permission = if opts.permission.is_empty() {
        OPENCODE_READ_ONLY_PERMISSION
    } else {
        &opts.permission
    };
    let mut env = vec![
        format!("OPENCODE_PERMISSION={permission}"),
        // Give each reviewer its OWN opencode session DB. The megareview runs
        // several opencode processes at once and they otherwise share one
        // SQLite DB (WAL + 5s busy_timeout), which intermittently loses the
        // write lock — observed as a reviewer failing with "database is
        // locked" (exit 1). A per-session DB (keyed on the unique session ID)
        // removes all contention.
        format!("OPENCODE_DB=rival-{session_id}.db"),
    ];

    // Inject the entry's key into its provider config. A caller may supply
    // an explicit key; otherwise resolve the entry's own variable from the
    // process env or the workdir .env walk-up. The key is never hardcoded or
    // written to disk.
    let key = if opts.api_key.is_empty() {
        cfg.security_api_key_from(entry, Path::new(workdir))
    } else {
        opts.api_key.clone()
    };
    if !key.is_empty() {
        let provider_config = opencode_provider_config(entry, &key);
        if !provider_config.is_empty() {
            env.push(format!("OPENCODE_CONFIG_CONTENT={provider_config}"));
        }
    }
    env
}

/// Go `opencodeProviderConfig`: the in-memory provider config for one
/// registry entry, as Go's `json.Marshal` of nested `map[string]any` writes
/// it (keys sorted by bytes, HTML-safe string escapes). An empty key is
/// rejected.
pub(crate) fn opencode_provider_config(entry: &SecurityModel, key: &str) -> String {
    if key.is_empty() {
        return String::new();
    }
    let mut options = vec![("apiKey", quote_bytes(key.as_bytes()))];
    if !entry.base_url.is_empty() {
        options.push(("baseURL", quote_bytes(entry.base_url.as_bytes())));
    }
    let provider = go_object(vec![("options", go_object(options))]);
    go_object(vec![
        ("$schema", quote_bytes(b"https://opencode.ai/config.json")),
        ("provider", go_object(vec![(entry.provider, provider)])),
    ])
}

/// A Go `map[string]any` JSON object from already-encoded values: keys in
/// byte order, as `encoding/json` sorts map keys.
fn go_object(mut members: Vec<(&str, String)>) -> String {
    members.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let body: Vec<String> = members
        .into_iter()
        .map(|(k, v)| format!("{}:{v}", quote_bytes(k.as_bytes())))
        .collect();
    format!("{{{}}}", body.join(","))
}
