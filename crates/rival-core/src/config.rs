//! Configuration: model ids and public labels, log
//! and error scrubbing, effort resolution, env-driven limits, API-key lookup,
//! reviewer prompts, and `~/.rival/config.yaml`.
//!
//! A [`Config`] value owns a snapshot of the environment, the
//! working directory, the [`Paths`], and the loaded user config, so tests pass
//! explicit values instead of mutating globals.

use std::collections::{BTreeMap, HashMap};
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Deserializer};

use crate::duration;
use crate::paths::{self, Paths};

// GPT56_SOL_MODEL and SOL_LABEL name a removed model (2026-09-26). Nothing
// runs it; the executor rejects it.
/// read-compat: display of sessions recorded before Sol's removal
pub const GPT56_SOL_MODEL: &str = "gpt-5.6-sol";
// CODEX_MODEL shares the codex runtime with the removed Sol model, so
// engine_label must match it before the "codex" adapter fallback, which
// labels old sessions "sol". The public label equals the adapter name, so
// identity checks key on the model id, never on the bare word "codex".
pub const CODEX_MODEL: &str = "gpt-6-astra";
pub const CODEX_LABEL: &str = "codex";
pub const CLAUDE_MODEL: &str = "claude-opus-5-5";
/// read-compat: display of sessions recorded before Sol's removal
pub const SOL_LABEL: &str = "sol";
pub const CLAUDE_LABEL: &str = "claude";
pub const K3_LABEL: &str = "kimi-k3";
/// The CLI command word for K3. It differs from [`K3_LABEL`], which is the
/// public display and error name.
pub const K3_COMMAND_NAME: &str = "k3";
/// Kimi K3 via OpenCode's built-in Moonshot AI provider.
pub const KIMI_MODEL: &str = "moonshotai/kimi-k3";
pub const GROK_MODEL: &str = "grok-4.6";
pub const GROK_LABEL: &str = "grok";
pub const CLAUDE_DOCKER_IMAGE: &str = "rival-claude";
pub const CLAUDE_DOCKER_TOKEN_ENV: &str = "RIVAL_CLAUDE_TOKEN";

pub const DEFAULT_REVIEW_EFFORT: &str = "high";
pub const DEFAULT_PLAN_EFFORT: &str = "high";
pub const SESSION_DIR: &str = ".rival/sessions";
pub const QUEUE_DIR: &str = ".rival/queue";
pub const PROMPT_PREVIEW_LEN: usize = 100;
pub const PROMPT_DETAIL_MAX_LINES: usize = 10;

pub const DEFAULT_MAX_CONCURRENT: usize = 2;
pub const DEFAULT_QUEUE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
pub const DEFAULT_RUN_TIMEOUT: Duration = Duration::from_secs(30 * 60);
pub const QUEUE_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// The one effort ladder every surface accepts and advertises.
///
/// xhigh and ultra are NOT interchangeable: the codex runtime passes the value
/// through verbatim and treats ultra as its own reasoning level, so neither
/// may be aliased to the other. Runtimes that expose a shorter menu clamp at
/// their own boundary (see [`claude_effort_level`] and the grok adapter).
pub const VALID_EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "ultra"];

/// Maps a rival effort → claude CLI `--effort` value.
pub fn claude_effort_level(effort: &str) -> Option<&'static str> {
    match effort {
        "low" => Some("low"),
        "medium" => Some("medium"),
        "high" | "xhigh" | "ultra" => Some("max"),
        _ => None,
    }
}

/// K3's only provider-supported reasoning variant. Unknown models have no
/// supported variant because K3 is Rival's sole OpenCode-backed model.
pub fn opencode_variant(model: &str, _effort: &str) -> &'static str {
    if model != KIMI_MODEL {
        return "";
    }
    "max"
}

/// The stable public name for a concrete model id. Runtime model ids stay
/// internal so dashboards, console output, and API summaries use Rival's
/// short model names consistently.
pub fn model_label(model: &str) -> &'static str {
    match model {
        GPT56_SOL_MODEL | SOL_LABEL => SOL_LABEL, // read-compat
        CODEX_MODEL | CODEX_LABEL => CODEX_LABEL,
        CLAUDE_MODEL | CLAUDE_LABEL => CLAUDE_LABEL,
        KIMI_MODEL | K3_LABEL => K3_LABEL,
        GROK_MODEL | GROK_LABEL => GROK_LABEL,
        // Distinct from GROK_LABEL: this is Grok on OpenCode via OpenRouter,
        // a different runtime with a different credential.
        GROK_OPENROUTER_MODEL | GROK_OPENROUTER_LABEL => GROK_OPENROUTER_LABEL,
        _ => "retired-model",
    }
}

/// A human-facing reviewer label. Review output names the selected model
/// instead of the executable adapter used to launch it.
pub fn engine_label(cli: &str, model: &str) -> String {
    // Exact current ids win first.
    match model {
        GPT56_SOL_MODEL => return SOL_LABEL.to_string(), // read-compat
        // Checked before the adapter fallback: Codex and the removed Sol both
        // ran on codex, so falling through would label Codex as Sol.
        CODEX_MODEL => return CODEX_LABEL.to_string(),
        CLAUDE_MODEL => return CLAUDE_LABEL.to_string(),
        KIMI_MODEL => return K3_LABEL.to_string(),
        GROK_MODEL => return GROK_LABEL.to_string(),
        // Checked before the adapter fallback below: both this and K3 run on
        // opencode, so falling through would label Grok as K3.
        GROK_OPENROUTER_MODEL => return GROK_OPENROUTER_LABEL.to_string(),
        _ => {}
    }

    // Adapter identity is the reliable fallback for sessions written by older
    // releases with now-obsolete model ids.
    match cli {
        "codex" => return SOL_LABEL.to_string(), // read-compat
        GROK_LABEL => return GROK_LABEL.to_string(),
        "claude" | "fable" | "opencode" => return "retired-model".to_string(),
        _ => {}
    }
    if !model.is_empty() {
        return model_label(model).to_string();
    }
    cli.to_string()
}

/// Removes internal adapter and concrete model identifiers from an error
/// before it is shown to a user. Required executable paths and configuration
/// keys remain untouched.
pub fn public_runtime_error(cli: &str, model: &str, message: &str) -> String {
    let message = replace_concrete_model_ids(cli, model, message);
    let label = engine_label(cli, model);
    match cli {
        // "astra" is read-compat for plan sessions written before 3.34.
        "codex" | "astra" => {
            let title = title_label(&label);
            replace_ordered(
                &message,
                &[
                    ("OpenAI Codex", format!("{title} runtime")),
                    ("Codex CLI", format!("{title} runtime")),
                    ("codex CLI", format!("{title} runtime")),
                    (
                        "run codex login",
                        format!("authenticate the {title} runtime"),
                    ),
                    ("codex exited", format!("{label} exited")),
                    ("start codex:", format!("start {title} runtime:")),
                    ("subprocess codex:", format!("{title} runtime:")),
                    ("Codex", title.clone()),
                ],
            )
        }
        // "fable" is read-compat for plan sessions written before 3.34.
        "claude" | "fable" => {
            let title = title_label(&label);
            replace_ordered(
                &message,
                &[
                    ("Claude Code CLI", format!("{title} runtime")),
                    ("Claude CLI", format!("{title} runtime")),
                    ("claude CLI", format!("{label} runtime")),
                    (
                        "claude requires Docker",
                        format!("{title} runtime requires Docker"),
                    ),
                    ("claude exited", format!("{label} exited")),
                    ("start claude:", format!("start {title} runtime:")),
                    ("subprocess claude:", format!("{title} runtime:")),
                ],
            )
        }
        _ => message,
    }
}

/// Ordered multi-pattern replace: one left-to-right pass without
/// overlapping matches; at each position the earliest listed pattern wins.
pub(crate) fn replace_ordered(text: &str, pairs: &[(&str, String)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    'scan: while !rest.is_empty() {
        for (old, new) in pairs {
            if rest.starts_with(old) {
                out.push_str(new);
                rest = &rest[old.len()..];
                continue 'scan;
            }
        }
        let c = rest.chars().next().expect("rest is non-empty");
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// Normalizes runtime banners and concrete model ids while preserving model
/// output, including any source paths that contain an adapter name. Persisted
/// logs stay lossless; every user-facing log reader calls this.
pub fn public_runtime_log(cli: &str, model: &str, raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    let raw = replace_concrete_model_ids(cli, model, raw);
    let label = engine_label(cli, model);
    let title = title_label(&label);

    let mut out = String::with_capacity(raw.len());
    let mut banner_seen = false;
    let mut header_open = true;
    let mut delimiters = 0;
    for (i, line) in raw.split_inclusive('\n').enumerate() {
        let (body, ending) = match line.strip_suffix('\n') {
            Some(body) => (body, "\n"),
            None => (line, ""),
        };
        let mut body = body.to_string();
        let mut trimmed = body.trim().to_string();
        let leading = body[..body.len() - body.trim_start_matches([' ', '\t']).len()].to_string();

        match cli {
            "codex" | "astra" => {
                if let Some(rest) = trimmed.strip_prefix("OpenAI Codex") {
                    // Codex and old Sol sessions share this runtime, so the
                    // banner takes the resolved label rather than a hardcoded
                    // "Sol". Title-cased to match the display form the banner
                    // has always used.
                    trimmed = format!("{} runtime{rest}", title_label(&engine_label(cli, model)));
                    body = format!("{leading}{trimmed}");
                    banner_seen = true;
                } else if let Some(rest) = trimmed.strip_prefix("Codex ")
                    && i == 0
                {
                    // read-compat: display of sessions recorded before Sol's removal
                    trimmed = format!("Sol runtime {rest}");
                    body = format!("{leading}{trimmed}");
                    banner_seen = true;
                }
            }
            "claude" | "fable" => {
                if let Some(rest) = trimmed.strip_prefix("Claude Code") {
                    trimmed = format!("{title} runtime{rest}");
                    body = format!("{leading}{trimmed}");
                    banner_seen = true;
                } else if let Some(rest) = trimmed.strip_prefix("Claude ")
                    && i == 0
                {
                    trimmed = format!("{title} runtime {rest}");
                    body = format!("{leading}{trimmed}");
                    banner_seen = true;
                }
            }
            _ => {}
        }

        if banner_seen && header_open && trimmed.to_lowercase().starts_with("model:") {
            body = format!("{leading}model: {label}");
        }
        if trimmed.starts_with("=== REVIEW FROM ") {
            body = format!("{leading}{}", public_review_header(&trimmed));
        }

        if trimmed == "--------" {
            delimiters += 1;
            if delimiters >= 2 {
                header_open = false;
            }
        }
        if trimmed.eq_ignore_ascii_case("user") {
            header_open = false;
        }
        out.push_str(&body);
        out.push_str(ending);
    }
    out
}

fn replace_concrete_model_ids(cli: &str, model: &str, text: &str) -> String {
    // Model ids overlap textually: "grok-4.6" is a substring of the label
    // "grok-4.6-openrouter". Replacing ids directly lets one substitution
    // corrupt another's output, so each id becomes a placeholder first and
    // only expands to its label once every id is consumed.
    let mut pairs: Vec<(String, String)> = Vec::with_capacity(7);
    if !model.is_empty() {
        // The run's own model wins, and is matched before the shared list so
        // a longer id is never shadowed by a shorter one it contains.
        pairs.push((model.to_string(), engine_label(cli, model)));
    }
    for (id, label) in [
        (GPT56_SOL_MODEL, SOL_LABEL), // read-compat
        (CODEX_MODEL, CODEX_LABEL),
        (CLAUDE_MODEL, CLAUDE_LABEL),
        (KIMI_MODEL, K3_LABEL),
        (GROK_OPENROUTER_MODEL, GROK_OPENROUTER_LABEL),
        (GROK_MODEL, GROK_LABEL),
    ] {
        pairs.push((id.to_string(), label.to_string()));
    }
    // Longest id first (stable): a short id must never consume part of a
    // longer one.
    pairs.sort_by_key(|(id, _)| std::cmp::Reverse(id.len()));

    let mut text = text.to_string();
    let mut labels: Vec<(String, String)> = Vec::new();

    // Protect public labels before touching ids. Text can already contain a
    // label — a re-normalized log, or a model naming itself — and
    // "grok-4.6-openrouter" contains the id "grok-4.6", so an unprotected
    // label would be rewritten into "grok-openrouter".
    // SOL_LABEL stays protected — read-compat.
    let mut protected = [
        GROK_OPENROUTER_LABEL,
        K3_LABEL,
        SOL_LABEL,
        CLAUDE_LABEL,
        GROK_LABEL,
        CODEX_LABEL,
    ];
    protected.sort_by_key(|label| std::cmp::Reverse(label.len()));
    for (i, label) in protected.iter().enumerate() {
        if !text.contains(label) {
            continue;
        }
        // Skip a label that only appears inside a concrete id we are about to
        // replace. Protecting it there would mask the id and leave it
        // un-normalized.
        let masks_an_id = pairs
            .iter()
            .any(|(id, _)| !id.is_empty() && id.contains(label) && text.contains(id.as_str()));
        if masks_an_id {
            continue;
        }
        let token = format!("\x00rival-label-{i}\x00");
        text = text.replace(label, &token);
        labels.push((token, label.to_string()));
    }

    for (i, (id, label)) in pairs.iter().enumerate() {
        if id.is_empty() || !text.contains(id.as_str()) {
            continue;
        }
        let token = format!("\x00rival-model-{i}\x00");
        text = text.replace(id.as_str(), &token);
        labels.push((token, label.clone()));
    }
    for (token, label) in &labels {
        text = text.replace(token.as_str(), label);
    }
    text
}

fn public_review_header(line: &str) -> String {
    const PREFIX: &str = "=== REVIEW FROM ";
    let rest = line.strip_prefix(PREFIX).unwrap_or(line);
    let Some(role_at) = rest.find(" [role:") else {
        return line.to_string();
    };
    let identity = &rest[..role_at];
    let role = &rest[role_at..];
    let Some(first) = identity.split_whitespace().next() else {
        return line.to_string();
    };
    let mut reviewer = first.trim_matches(['(', ')']).to_string();
    let lower_identity = identity.to_lowercase();
    if lower_identity.contains("retired-model") {
        return format!("{PREFIX}retired-model{role}");
    }
    match reviewer.to_lowercase().as_str() {
        "codex" => {
            // Sol and Codex share this adapter, so disambiguate by identity
            // before defaulting to Sol. The label is the adapter word itself,
            // so only a bare "codex" identity or the Codex model id means Codex.
            reviewer = if lower_identity == CODEX_LABEL
                || lower_identity.contains(CODEX_MODEL)
                || lower_identity.contains("astra")
            {
                CODEX_LABEL.to_string()
            } else {
                SOL_LABEL.to_string() // read-compat
            };
        }
        "claude" => {
            // Same collision as codex: a bare "claude" or the current id is
            // the Claude reviewer, anything else is a retired Claude-runtime
            // model.
            reviewer = if lower_identity == CLAUDE_LABEL || lower_identity.contains(CLAUDE_MODEL) {
                CLAUDE_LABEL.to_string()
            } else {
                "retired-model".to_string()
            };
        }
        "opencode" => {
            reviewer = if lower_identity.contains(K3_LABEL) {
                K3_LABEL.to_string()
            } else {
                "retired-model".to_string()
            };
        }
        GPT56_SOL_MODEL => reviewer = SOL_LABEL.to_string(), // read-compat
        CLAUDE_MODEL => reviewer = CLAUDE_LABEL.to_string(),
        KIMI_MODEL => reviewer = K3_LABEL.to_string(),
        GROK_LABEL | GROK_MODEL => reviewer = GROK_LABEL.to_string(),
        _ => {}
    }
    format!("{PREFIX}{reviewer}{role}")
}

/// Upper-cases a label's first letter for banner display.
/// Labels are lowercase everywhere else, and the codex banner has always read
/// "Sol runtime". Only the first byte changes; labels are ASCII.
fn title_label(label: &str) -> String {
    let mut chars = label.chars();
    match chars.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

/// Prepended as a system instruction to all CLI invocations.
pub const SYSTEM_PROMPT: &str = r#"Answer the user's question directly. Do not offer follow-up options, menus, walkthroughs, or ask if they want more. No filler, no sign-offs. Just deliver the answer and stop."#;

/// Tells the CLI which project directory it's operating in.
pub const WORKDIR_PREAMBLE: &str = r#"You are working in project directory: {WORKDIR}
Use your tools to read files, run git commands, and explore the codebase as needed.
"#;

/// Prepended to the reviewer prompt when git auto-detects changed files.
/// `{FILES}` is replaced with the newline-separated file list at runtime.
pub const DIFF_REVIEW_PREAMBLE: &str = r#"The following files have uncommitted changes (or were changed in the last commit). Focus your review on these files, but read other project files as needed for context.

Changed files:
```
{FILES}
```
{DIFFSTAT}
"#;

/// The plan/spec review template used by `rival command plan`. It targets a
/// single planning/spec markdown document (NOT source code) and asks codex to
/// rate it and surface bugs + gaps. `{FILE}` is replaced with the absolute
/// path at the call site. The model must emit ONE JSON object matching the
/// contract below so the output can be parsed structurally.
pub const PLAN_REVIEW_PROMPT: &str = r#"You review an engineering PLAN / SPEC document (not source code). Find the real problems that would make this plan fail, mislead an implementer, or ship the wrong thing. Do not report wording nitpicks.

Plan document to review: {FILE}

Read the file in full (use your tools). Judge it as an implementation blueprint. Look for:

1. **Bugs / logic flaws** — steps that are wrong, contradictory, out of order, or that would break when implemented as written.
2. **Gaps** — missing steps, unhandled edge cases, undefined error/failure behavior, absent rollback/migration/auth/validation, things the plan silently assumes.
3. **Ambiguity** — instructions vague enough that two engineers would build different things; unstated assumptions; undefined terms.
4. **Scope / feasibility** — unrealistic claims, hidden dependencies, under-estimated work, or parts that conflict with how the existing system actually works.

When the plan makes claims about existing code (files, functions, schemas, config, commands), open the repo and check them. A claim the code contradicts is a bug finding: cite the plan section in `file` and put the contradicting code location in `body`.
5. **Verification gaps** — no way to tell if the plan succeeded; missing tests, acceptance criteria, or rollback checks.

Rules:
- Only report issues you are confident are real. No speculative nitpicks, no style/grammar comments.
- If the plan is genuinely solid, say so in the summary and return few or zero findings. Do not invent problems.
- Rate the plan overall from 1 (unimplementable / dangerously wrong) to 10 (airtight, ready to execute).

Output: respond with EXACTLY ONE JSON object and nothing else (no prose before or after, no markdown fences). Schema:

{
  "summary": "1-3 sentence overall assessment of the plan",
  "rating": 7,
  "findings": [
    {
      "file": "section or heading the issue is in (or the filename)",
      "line": 0,
      "severity": "critical|high|medium|low",
      "category": "bug|gap|ambiguity|scope|verification",
      "title": "one-line description of the issue",
      "body": "what is wrong and why it matters for implementation",
      "suggestion": "concrete fix or what to add",
      "confidence": 8
    }
  ]
}

Severity guidance: critical = plan is wrong/will cause data loss or a broken build if followed; high = significant gap or flaw that blocks correct implementation; medium = real ambiguity or missing detail an implementer will trip on; low = minor gap or clarification. "line" may be 0 when not applicable. Sort findings by severity, highest first."#;

/// The review scope used when none is given and git detects no changed files.
pub const WHOLE_PROJECT: &str = "the entire project";

/// Selects which reviewer prompt is rendered: the bug hunter or the security
/// lens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    BugHunter,
    Security,
}

/// One model that can run the security review. Both entries run through the
/// OpenCode adapter, so the differences between them are data rather than
/// code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecurityModel {
    /// The config value: "k3" or "grok".
    pub name: &'static str,
    /// The upstream model id.
    pub model: &'static str,
    /// What `opencode -m` receives. OpenCode splits it at the first slash to
    /// choose the provider, so for OpenRouter-hosted models this differs from
    /// `model`.
    pub selector: &'static str,
    /// Names the block in the generated OpenCode config.
    pub provider: &'static str,
    /// The provider endpoint. Empty means OpenCode's own default, which is
    /// what the built-in Moonshot provider uses.
    pub base_url: &'static str,
    /// The environment variable holding the API key.
    pub key_env: &'static str,
    /// The public name shown in output. It must not collide with any other
    /// model's label or concrete id.
    pub label: &'static str,
    /// The reasoning level passed to the provider.
    pub variant: &'static str,
}

/// Accepted values of the `security.reviewer` config key.
pub const SECURITY_REVIEWER_K3: &str = "k3";
pub const SECURITY_REVIEWER_GROK: &str = "grok";

/// Grok 4.6 served through OpenRouter. A different runtime from
/// [`GROK_MODEL`], which reaches xAI through the grok CLI.
pub const GROK_OPENROUTER_MODEL: &str = "x-ai/grok-4.6";
pub const GROK_OPENROUTER_SELECTOR: &str = "openrouter/x-ai/grok-4.6";
/// Deliberately differs from [`GROK_LABEL`]. The two Groks are separate
/// models on separate runtimes with separate credentials, and a shared label
/// would make their sessions and logs indistinguishable.
pub const GROK_OPENROUTER_LABEL: &str = "grok-4.6-openrouter";
const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";

/// The registry. Adding an entry here is all a new security model needs: the
/// adapter reads provider, key, selector, and variant from it.
const SECURITY_MODELS: [SecurityModel; 2] = [
    SecurityModel {
        name: SECURITY_REVIEWER_K3,
        model: KIMI_MODEL,
        selector: KIMI_MODEL,
        provider: "moonshotai",
        base_url: "",
        key_env: "MOONSHOT_API_KEY",
        label: K3_LABEL,
        variant: "max",
    },
    SecurityModel {
        name: SECURITY_REVIEWER_GROK,
        model: GROK_OPENROUTER_MODEL,
        selector: GROK_OPENROUTER_SELECTOR,
        provider: "openrouter",
        base_url: OPENROUTER_BASE_URL,
        key_env: "OPENROUTER_API_KEY",
        label: GROK_OPENROUTER_LABEL,
        variant: "xhigh",
    },
];

fn security_model_named(name: &str) -> Option<SecurityModel> {
    SECURITY_MODELS.iter().find(|m| m.name == name).copied()
}

/// The accepted `security.reviewer` values, for error messages.
pub fn security_reviewer_names() -> [&'static str; 2] {
    [SECURITY_REVIEWER_K3, SECURITY_REVIEWER_GROK]
}

/// Looks up a registry entry by concrete model id, without consulting the
/// security config: with security.reviewer set to grok, a run that names K3
/// must still run K3.
pub fn open_code_entry_for(model: &str) -> Option<SecurityModel> {
    SECURITY_MODELS.iter().find(|m| m.model == model).copied()
}

/// Checks if the given effort level is in the allowlist.
pub fn is_valid_effort(effort: &str) -> bool {
    VALID_EFFORTS.contains(&effort)
}

// Claude auth modes for native runs (`RIVAL_CLAUDE_AUTH`).
/// The CLI's own /login (Pro/Max) — the default.
pub const CLAUDE_AUTH_SUBSCRIPTION: &str = "subscription";
/// Explicit `ANTHROPIC_API_KEY` billing.
pub const CLAUDE_AUTH_API: &str = "api";

/// Models whose effort is a property of the model rather than of the surface
/// invoking it. K3's provider exposes exactly one level; Codex defaults to
/// xhigh for correctness and plan reviews; Claude runs at medium.
fn pinned_model_effort(label: &str) -> Option<&'static str> {
    match label {
        "kimi-k3" => Some("max"),
        CODEX_LABEL => Some("xhigh"),
        // Claude runs Opus 5.5 at medium on every surface.
        CLAUDE_LABEL => Some("medium"),
        _ => None,
    }
}

fn builtin_model_effort(label: &str) -> &'static str {
    match label {
        // Codex is the deep-reasoning model and is pinned to xhigh.
        CODEX_LABEL => "xhigh",
        "kimi-k3" => "max",
        CLAUDE_LABEL => "medium",
        // grok-4.6's menu is low/medium/high, and high is its own default.
        GROK_LABEL => "high",
        _ => DEFAULT_REVIEW_EFFORT,
    }
}

fn known_effort_model(label: &str) -> bool {
    matches!(label, CODEX_LABEL | "kimi-k3" | CLAUDE_LABEL | GROK_LABEL)
}

fn valid_configured_model_effort(label: &str, effort: &str) -> bool {
    if label == "kimi-k3" {
        return effort == "max";
    }
    is_valid_effort(effort)
}

/// A config error carrying its exact message text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError(String);

impl ConfigError {
    fn new(msg: impl Into<String>) -> Self {
        ConfigError(msg.into())
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

/// claude-specific settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClaudeConfig {
    /// "team" or "personal".
    pub subscription: String,
}

/// Selects which model runs the security review.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SecurityConfig {
    pub reviewer: String,
}

/// Optional user configuration from `~/.rival/config.yaml`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserConfig {
    pub claude: ClaudeConfig,
    pub security: SecurityConfig,
    pub efforts: BTreeMap<String, String>,
    pub roles: BTreeMap<String, String>,
    /// Lets the skills apply fixes for CONFIRMED critical and high findings
    /// without asking. Medium and low are never auto-fixed.
    pub auto_fix_critical_high: bool,
}

/// Decodes YAML null into a string as "", and any other scalar as
/// its literal text; serde-saphyr rejects null for `String`.
#[derive(Default)]
struct GoString(String);

impl<'de> Deserialize<'de> for GoString {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(GoString(
            Option::<String>::deserialize(d)?.unwrap_or_default(),
        ))
    }
}

#[derive(Default, Deserialize)]
struct RawClaude {
    #[serde(default)]
    subscription: GoString,
}

#[derive(Default, Deserialize)]
struct RawSecurity {
    #[serde(default)]
    reviewer: GoString,
}

/// Unknown keys (for example the removed megareview's `review:` roster) are
/// ignored, like yaml.v3.
#[derive(Default, Deserialize)]
struct RawUserConfig {
    #[serde(default)]
    claude: Option<RawClaude>,
    #[serde(default)]
    security: Option<RawSecurity>,
    #[serde(default)]
    efforts: Option<BTreeMap<String, GoString>>,
    #[serde(default)]
    roles: Option<BTreeMap<String, GoString>>,
    #[serde(default)]
    auto_fix_critical_high: Option<bool>,
}

impl From<RawUserConfig> for UserConfig {
    fn from(raw: RawUserConfig) -> Self {
        let strings = |m: Option<BTreeMap<String, GoString>>| {
            m.unwrap_or_default()
                .into_iter()
                .map(|(k, v)| (k, v.0))
                .collect()
        };
        UserConfig {
            claude: ClaudeConfig {
                subscription: raw.claude.unwrap_or_default().subscription.0,
            },
            security: SecurityConfig {
                reviewer: raw.security.unwrap_or_default().reviewer.0,
            },
            efforts: strings(raw.efforts),
            roles: strings(raw.roles),
            auto_fix_critical_high: raw.auto_fix_critical_high.unwrap_or_default(),
        }
    }
}

/// Loads the user config from one file. `Ok(None)` means the file does not
/// exist. Entries are validated in sorted key order, so the reported
/// invalid entry is the same on every run.
pub fn load_user_config(path: &Path) -> Result<Option<UserConfig>, ConfigError> {
    let shown = path.display();
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        // A missing file is not an error; on Windows also a missing
        // parent directory (ERROR_PATH_NOT_FOUND), as in a fresh profile.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(ConfigError::new(format!(
                "read {shown}: open {shown}: {}",
                e
            )));
        }
    };
    let mut data = Vec::new();
    if let Err(e) = file.read_to_end(&mut data) {
        return Err(ConfigError::new(format!(
            "read {shown}: read {shown}: {}",
            e
        )));
    }
    let parse_err = |e: &dyn fmt::Display| ConfigError::new(format!("parse {shown}: {e}"));
    let text = String::from_utf8(data).map_err(|e| parse_err(&e))?;
    let raw: Option<RawUserConfig> = serde_saphyr::from_str(&text).map_err(|e| parse_err(&e))?;
    let mut cfg = UserConfig::from(raw.unwrap_or_default());

    // Sol was removed on 2026-09-26. An old efforts.sol entry configures
    // nothing that can run, so drop it instead of failing every command.
    cfg.efforts.remove(SOL_LABEL);
    for (label, raw) in cfg.efforts.iter_mut() {
        let effort = raw.trim().to_lowercase();
        if !known_effort_model(label) {
            return Err(ConfigError::new(format!(
                "invalid effort model {:?} in {shown}; use one of: codex, kimi-k3, claude, grok",
                label
            )));
        }
        if !valid_configured_model_effort(label, &effort) {
            let allowed = if label == "kimi-k3" {
                "max"
            } else {
                "low, medium, high, xhigh, ultra"
            };
            return Err(ConfigError::new(format!(
                "invalid effort {:?} for {label} in {shown}; use one of: {allowed}",
                raw
            )));
        }
        *raw = effort;
    }
    // Validate the security reviewer here, not only where it is resolved, so
    // a typo fails every command instead of waiting for a security run.
    let configured = cfg.security.reviewer.trim().to_string();
    if !configured.is_empty() {
        let name = configured.to_lowercase();
        if security_model_named(&name).is_none() {
            return Err(ConfigError::new(format!(
                "invalid security.reviewer {:?} in {shown}; use one of: {}",
                configured,
                security_reviewer_names().join(", ")
            )));
        }
        cfg.security.reviewer = name;
    }
    Ok(Some(cfg))
}

/// Runtime configuration: the environment and the user config, captured
/// once.
#[derive(Clone)]
pub struct Config {
    paths: Paths,
    env: HashMap<String, String>,
    /// Every environment entry in process order, non-UTF-8 included.
    environ: Vec<OsString>,
    cwd: Option<PathBuf>,
    user: Option<UserConfig>,
    user_err: Option<ConfigError>,
    /// The snapshot is this process's environment ([`Config::load`]), so
    /// OS calls that read the environment themselves (Windows
    /// `GetTempPath2W`) see the same values.
    process_env: bool,
}

/// The env snapshot holds API keys, so `Debug` shows only its size.
impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("paths", &self.paths)
            .field("env", &format_args!("<{} vars>", self.env.len()))
            .field("environ", &format_args!("<{} entries>", self.environ.len()))
            .field("cwd", &self.cwd)
            .field("user", &self.user)
            .field("user_err", &self.user_err)
            .finish()
    }
}

impl Config {
    /// Production constructor: snapshots the process environment and the
    /// working directory, then reads the user config.
    pub fn load(paths: &Paths) -> Self {
        let (env, environ, cwd) = process_snapshot();
        let mut cfg = Self::new(paths.clone(), env, cwd).with_environ(environ);
        cfg.process_env = true;
        cfg
    }

    /// The user config is read once, before `main` loads `.env`, while the
    /// environment and home directory are read at each use. This re-snapshots the process environment (and the working
    /// directory) after `.env` is loaded and takes the post-`.env` `paths`,
    /// while keeping the user config and its load error from [`Config::load`].
    pub fn reload_env(self, paths: Paths) -> Self {
        let (env, environ, cwd) = process_snapshot();
        let mut cfg = self.with_runtime_env(paths, env, environ, cwd);
        cfg.process_env = true;
        cfg
    }

    /// Pure form of [`Config::reload_env`]: replaces the paths, the `env`
    /// getters, the child environ and the working directory. The user config
    /// and its load error stay as they were; the config file is not read.
    pub fn with_runtime_env(
        mut self,
        paths: Paths,
        env: HashMap<String, String>,
        environ: Vec<OsString>,
        cwd: Option<PathBuf>,
    ) -> Self {
        self.paths = paths;
        self.env = env;
        self.environ = environ;
        self.cwd = cwd;
        self.process_env = false;
        self
    }

    /// Explicit constructor for tests and embedders. Reads
    /// `paths.config_file()`, and skips the file
    /// when no home directory is known (here: neither `RIVAL_HOME` nor
    /// `HOME` is set and non-empty in `env`).
    pub fn new(paths: Paths, env: HashMap<String, String>, cwd: Option<PathBuf>) -> Self {
        let mut environ: Vec<OsString> =
            env.iter().map(|(k, v)| format!("{k}={v}").into()).collect();
        environ.sort();
        let mut cfg = Config {
            paths,
            env,
            environ,
            cwd,
            user: None,
            user_err: None,
            process_env: false,
        };
        if !cfg.getenv(paths::STATE_ROOT_VAR).is_empty() || !cfg.getenv(paths::HOME_VAR).is_empty()
        {
            match load_user_config(&cfg.paths.config_file()) {
                Ok(user) => cfg.user = user,
                Err(e) => cfg.user_err = Some(e),
            }
        }
        cfg
    }

    /// Replaces the user config and clears any load error, so tests
    /// can set the user config directly.
    pub fn with_user_config(mut self, user: Option<UserConfig>) -> Self {
        self.user = user;
        self.user_err = None;
        self
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    /// The environment entries for child processes. [`Config::load`] keeps the
    /// process order and non-UTF-8 entries; [`Config::new`] builds it from
    /// `env`, sorted by entry.
    pub fn environ(&self) -> &[OsString] {
        &self.environ
    }

    /// Replaces the ordered child-process env (tests and embedders). The
    /// `env` getters are not changed.
    pub fn with_environ(mut self, environ: Vec<OsString>) -> Self {
        self.environ = environ;
        self
    }

    /// Whether `os.TempDir()` may ask the OS: true when the env snapshot is
    /// this process's own ([`Config::load`], [`Config::reload_env`]); an
    /// injected env ([`Config::new`], [`Config::with_runtime_env`]) is not.
    pub fn temp_dir_from_os(&self) -> bool {
        self.process_env
    }

    /// The snapshotted working directory; `None` means it could not be read.
    pub fn cwd(&self) -> Option<&Path> {
        self.cwd.as_deref()
    }

    pub fn user_config(&self) -> Option<&UserConfig> {
        self.user.as_ref()
    }

    /// Reads a variable from the snapshot: unset reads as "". Windows
    /// names are case-insensitive (`Path` answers `PATH`), like the Windows
    /// environment lookup; see [`getenv_in`].
    pub fn getenv(&self, key: &str) -> &str {
        getenv_in(cfg!(windows), &self.env, key)
    }

    /// An invalid `~/.rival/config.yaml`. Commands fail before doing any
    /// queue, session, or provider work rather than silently ignoring a typo
    /// in a requested model default.
    pub fn user_config_error(&self) -> Option<&ConfigError> {
        self.user_err.as_ref()
    }

    /// The user-configured prompt for a role, if any.
    pub fn role_prompt_override(&self, role: &str) -> Option<&str> {
        self.user.as_ref()?.roles.get(role).map(String::as_str)
    }

    /// The configured subscription type ("team", "personal", or "").
    pub fn claude_subscription(&self) -> &str {
        self.user
            .as_ref()
            .map_or("", |u| u.claude.subscription.as_str())
    }

    /// The fix policy `rival wait --log` prints for the skills:
    /// `auto_fix_critical_high` on gives "critical+high", else "off".
    pub fn auto_fix_policy(&self) -> &'static str {
        if self.user.as_ref().is_some_and(|u| u.auto_fix_critical_high) {
            "critical+high"
        } else {
            "off"
        }
    }

    /// The raw `security.reviewer` value, or "" when unset.
    pub fn configured_security_reviewer(&self) -> &str {
        self.user
            .as_ref()
            .map_or("", |u| u.security.reviewer.trim())
    }

    /// The model that runs the security review. An empty config value means
    /// K3.
    pub fn resolve_security_model(&self) -> Result<SecurityModel, ConfigError> {
        let mut name = SECURITY_REVIEWER_K3.to_string();
        let configured = self.configured_security_reviewer();
        if !configured.is_empty() {
            name = configured.to_lowercase();
        }
        security_model_named(&name).ok_or_else(|| {
            ConfigError::new(format!(
                "invalid security.reviewer {:?}, must be one of: {}",
                name,
                security_reviewer_names().join(", ")
            ))
        })
    }

    /// The configured default for a concrete model id or public model label.
    /// Invalid user configuration is reported by [`Config::user_config_error`]
    /// before command side effects begin.
    ///
    /// Kimi K3 is thinking-only and supports exactly max. Other current models
    /// accept Rival's low/medium/high/xhigh/ultra ladder.
    pub fn default_effort_for_model(&self, model: &str) -> String {
        let label = model_label(model);
        let builtin = builtin_model_effort(label);
        match self.user.as_ref().and_then(|u| u.efforts.get(label)) {
            Some(effort) => effort.clone(),
            None => builtin.to_string(),
        }
    }

    /// The documented precedence for one concrete model: explicit invocation
    /// override, then `~/.rival/config.yaml`, then the supplied
    /// surface-specific fallback. Kimi K3 remains pinned to max because that
    /// provider exposes no other reasoning level.
    pub fn resolve_effort(
        &self,
        model: &str,
        override_effort: &str,
        fallback: &str,
    ) -> Result<String, ConfigError> {
        let label = model_label(model);
        let override_effort = override_effort.trim().to_lowercase();
        if !override_effort.is_empty() {
            if label == "kimi-k3" {
                return Ok("max".to_string());
            }
            if !is_valid_effort(&override_effort) {
                return Err(ConfigError::new(format!(
                    "invalid effort {:?} for {label}",
                    override_effort
                )));
            }
            return Ok(override_effort);
        }
        if let Some(effort) = self.user.as_ref().and_then(|u| u.efforts.get(label)) {
            return Ok(effort.clone());
        }
        let mut fallback = fallback.trim().to_lowercase();
        // A model that pins its own effort outranks a surface-specific
        // fallback. Without this, every caller that passes a non-empty
        // fallback (the review and plan paths both pass one) silently
        // overrides the pin, which is how Codex ran at high instead of xhigh.
        if let Some(pinned) = pinned_model_effort(label) {
            return Ok(pinned.to_string());
        }
        if fallback.is_empty() {
            fallback = builtin_model_effort(label).to_string();
        }
        if label == "kimi-k3" {
            return Ok("max".to_string());
        }
        if !is_valid_effort(&fallback) {
            return Err(ConfigError::new(format!(
                "invalid fallback effort {:?} for {label}",
                fallback
            )));
        }
        Ok(fallback)
    }

    /// The Moonshot AI API key for K3 runs. `MOONSHOT_API_KEY` is the
    /// canonical OpenCode variable; `KIMI_API` remains a backward-compatible
    /// alias. The process environment is checked first, then each project
    /// `.env` walking up from `workdir`. The key is injected per run into
    /// OpenCode's built-in moonshotai provider through
    /// `OPENCODE_CONFIG_CONTENT` and is never written to on-disk OpenCode
    /// config.
    pub fn kimi_api_key_from(&self, workdir: &Path) -> String {
        const NAMES: [&str; 2] = ["MOONSHOT_API_KEY", "KIMI_API"];
        for name in NAMES {
            let key = self.getenv(name).trim();
            if !key.is_empty() {
                return key.to_string();
            }
        }
        self.dotenv_key_from(workdir, &NAMES)
    }

    /// An entry's API key. Precedence matches [`Config::kimi_api_key_from`]:
    /// the process environment first, then the nearest `.env` found walking
    /// up from `workdir`.
    pub fn security_api_key_from(&self, entry: &SecurityModel, workdir: &Path) -> String {
        if entry.name == SECURITY_REVIEWER_K3 {
            // Delegate rather than reimplement: the K3 lookup checks the
            // legacy KIMI_API alias in every .env it walks, not only in the
            // process environment, and an existing installation may rely on
            // that.
            return self.kimi_api_key_from(workdir);
        }
        let key = self.getenv(entry.key_env).trim();
        if !key.is_empty() {
            return key.to_string();
        }
        self.dotenv_key_from(workdir, &[entry.key_env])
    }

    /// The `.env` walk shared by both key lookups: at most 8 directories,
    /// stopping after the home directory or the filesystem root. Files that
    /// fail to read or parse are skipped.
    fn dotenv_key_from(&self, workdir: &Path, names: &[&str]) -> String {
        if workdir.as_os_str().is_empty() {
            return String::new();
        }
        let Some(mut dir) = paths::abs(self.cwd.as_deref(), workdir) else {
            return String::new();
        };
        let home = OsStr::new(self.getenv(paths::HOME_VAR));
        for _ in 0..8 {
            if let Ok(vars) = paths::read_dotenv(&dir.join(".env")) {
                for name in names {
                    if let Some(key) = vars.get(*name).map(|v| v.trim())
                        && !key.is_empty()
                    {
                        return key.to_string();
                    }
                }
            }
            let parent = dir.parent().map_or_else(|| dir.clone(), Path::to_path_buf);
            if dir.as_os_str() == home || parent == dir {
                break;
            }
            dir = parent;
        }
        String::new()
    }

    /// The auth mode for native claude runs. Default is subscription: the
    /// claude CLI is already authed via /login, and an inherited
    /// `ANTHROPIC_API_KEY` must never silently switch billing to API credits.
    /// API billing is opt-in via `RIVAL_CLAUDE_AUTH=api` and then requires
    /// `ANTHROPIC_API_KEY` to be set. Any other value is a hard error — auth
    /// must be explicit, never guessed.
    pub fn claude_auth(&self) -> Result<&'static str, ConfigError> {
        match self.getenv("RIVAL_CLAUDE_AUTH") {
            "" | CLAUDE_AUTH_SUBSCRIPTION | "sub" => Ok(CLAUDE_AUTH_SUBSCRIPTION),
            CLAUDE_AUTH_API => {
                if self.getenv("ANTHROPIC_API_KEY").is_empty() {
                    return Err(ConfigError::new(
                        "RIVAL_CLAUDE_AUTH=api but ANTHROPIC_API_KEY is empty — set the key or unset RIVAL_CLAUDE_AUTH to use the claude CLI subscription login",
                    ));
                }
                Ok(CLAUDE_AUTH_API)
            }
            v => Err(ConfigError::new(format!(
                "invalid RIVAL_CLAUDE_AUTH={:?} — use {:?} (default) or {:?}",
                v, CLAUDE_AUTH_SUBSCRIPTION, CLAUDE_AUTH_API
            ))),
        }
    }

    /// How many reviews may run at once (`RIVAL_MAX_CONCURRENT`, default 2).
    pub fn max_concurrent(&self) -> usize {
        let v = self.getenv("RIVAL_MAX_CONCURRENT");
        if !v.is_empty()
            && let Ok(n) = v.parse::<i64>()
            && n > 0
        {
            return n as usize;
        }
        DEFAULT_MAX_CONCURRENT
    }

    /// The max time to wait for a queue slot (`RIVAL_QUEUE_TIMEOUT`, default
    /// 30m).
    pub fn queue_timeout(&self) -> Duration {
        let v = self.getenv("RIVAL_QUEUE_TIMEOUT");
        if !v.is_empty()
            && let Ok(d) = duration::parse(v)
            && d > 0
        {
            return Duration::from_nanos(d as u64);
        }
        DEFAULT_QUEUE_TIMEOUT
    }

    /// A safe upper bound on how long a detached run can legitimately take
    /// end-to-end: the full queue wait plus 2× run timeout (every current run
    /// holds a 1× budget; the second is headroom), plus a small margin for
    /// process startup, stdout flush, and reaper cycles. `rival wait` uses
    /// this as its default timeout so it never gives up on a run that is
    /// still within its configured limits. When the run timeout is disabled
    /// (0), only the queue wait + margin is bounded.
    pub fn max_run_wait(&self) -> i64 {
        // The budget is a signed nanosecond count; oversized configured budgets wrap.
        (self.queue_timeout().as_nanos() as i64)
            .wrapping_add((self.run_timeout().as_nanos() as i64).wrapping_mul(2))
            .wrapping_add(5 * 60 * 1_000_000_000)
    }

    /// The max wall-clock a single provider run may take once it holds a
    /// queue slot (`RIVAL_RUN_TIMEOUT`, default 30m). This is the hard
    /// guarantee that a detached rival always terminates even if the provider
    /// CLI hangs. The clock starts after slot promotion, so queue wait does
    /// not eat it. `RIVAL_RUN_TIMEOUT=0` disables it (returns zero); an unset,
    /// unparseable, or negative value falls back to the default.
    pub fn run_timeout(&self) -> Duration {
        let v = self.getenv("RIVAL_RUN_TIMEOUT");
        if v.is_empty() {
            return DEFAULT_RUN_TIMEOUT;
        }
        match duration::parse(v) {
            Ok(d) if d >= 0 => Duration::from_nanos(d as u64), // zero → no timeout
            _ => DEFAULT_RUN_TIMEOUT,
        }
    }

    /// The run deadline budget in signed nanoseconds, `mult × run_timeout()`, or `None` (no deadline) when the run
    /// timeout is disabled or `mult <= 0`. Every current caller passes 1.
    pub fn run_timeout_budget(&self, mult: i32) -> Option<i64> {
        let d = self.run_timeout();
        if d.is_zero() || mult <= 0 {
            return None;
        }
        Some((d.as_nanos() as i64).wrapping_mul(i64::from(mult)))
    }

    /// Whether queueing is bypassed via `RIVAL_NO_QUEUE`.
    pub fn queue_disabled(&self) -> bool {
        let v = self.getenv("RIVAL_NO_QUEUE");
        !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false")
    }

    /// The workdir preamble with the absolute path injected. An unresolvable
    /// path injects "" (the absolute-path error is ignored).
    pub fn build_workdir_preamble(&self, workdir: &Path) -> String {
        let abs = paths::abs(self.cwd.as_deref(), workdir).unwrap_or_default();
        WORKDIR_PREAMBLE.replace("{WORKDIR}", &abs.to_string_lossy())
    }
}

/// [`Config::getenv`] with the platform rule explicit, so both test
/// everywhere. An exact match wins. Case-insensitive (Windows) names compare
/// ASCII case-folded; a real Windows environment cannot hold two such names,
/// and in a test map the smallest matching name wins, so the answer is
/// stable.
pub fn getenv_in<'a>(
    case_insensitive: bool,
    env: &'a HashMap<String, String>,
    key: &str,
) -> &'a str {
    if let Some(v) = env.get(key) {
        return v;
    }
    if !case_insensitive {
        return "";
    }
    env.iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case(key))
        .min_by(|a, b| a.0.cmp(b.0))
        .map_or("", |(_, v)| v.as_str())
}

/// The process environment as `(UTF-8 getters, ordered environ, getwd)`.
fn process_snapshot() -> (HashMap<String, String>, Vec<OsString>, Option<PathBuf>) {
    let vars: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    let env: HashMap<String, String> = vars
        .iter()
        .filter_map(|(k, v)| Some((k.to_str()?.to_string(), v.to_str()?.to_string())))
        .collect();
    let cwd = paths::getwd(env.get("PWD").map(OsStr::new));
    let environ = vars
        .into_iter()
        .map(|(k, v)| {
            let mut kv = k;
            kv.push("=");
            kv.push(v);
            kv
        })
        .collect();
    (env, environ, cwd)
}

#[cfg(test)]
mod tests;
