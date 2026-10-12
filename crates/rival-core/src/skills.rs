//! The skills `rival install` writes for Claude Code and Codex.
//!
//! The Markdown under `crates/rival-core/skills/` is the only skill tree.
//! `scripts/bump-skill-versions.sh` bumps its versions.

use include_dir::{Dir, include_dir};

#[cfg(test)]
mod tests;

/// Every embedded skill directory, plus `codex.md` at the root.
static TREE: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/skills");

/// All embedded skill directory names, in install order.
/// `scripts/bump-skill-versions.sh` reads this list.
pub const NAMES: [&str; 9] = [
    "rival-codex",
    "rival-sol",
    "rival-astra-ultrafast",
    "rival-sol-ultrafast",
    "rival-claude",
    "rival-fable",
    "rival-k3",
    "rival-grok",
    "rival-security",
];

/// Retired plan skills that install archives outside the active skill tree.
pub const RETIRED_PLAN_SKILLS: [&str; 3] = ["rival-plan", "rival-plan-codex", "rival-plan-claude"];

/// Other legacy skills that install removes.
pub const DEPRECATED: [&str; 13] = [
    "rival-plan-sol", // use the native plan command
    "rival-claude-only",
    "rival-fable-only",
    "rival-codex-only",
    "rival-gpt-5-6-sol",
    "rival-claude-fable",
    "rival-astra",         // renamed to rival-codex in 3.34
    "rival-plan-astra",    // renamed to rival-plan-codex in 3.34
    "rival-plan-fable",    // use the native plan command
    "rival-kimi",          // renamed to rival-k3 before release
    "rival-antislop-plan", // plan mode dropped on 2026-08-20
    "rival-review",        // megareview removed 2026-09-26
    "rival-antislop",      // code-slop review removed 2026-10-08
];

/// Reads an embedded skill file. Only the skill directories are visible, so
/// a root-level file such as `codex.md` is not part of this view.
pub fn read_file(path: &str) -> Result<&'static [u8], String> {
    let in_skill_dir = path
        .split_once('/')
        .is_some_and(|(dir, _)| TREE.get_dir(dir).is_some());
    match TREE.get_file(path) {
        Some(file) if in_skill_dir => Ok(file.contents()),
        // The same text as a missing file on disk.
        _ => Err(format!("open {path}: file does not exist")),
    }
}

/// The names of the embedded skill directories, sorted by name.
pub fn embedded_dirs() -> Vec<&'static str> {
    let mut dirs: Vec<&str> = TREE.dirs().filter_map(|d| d.path().to_str()).collect();
    dirs.sort_unstable();
    dirs
}

/// The Codex workflow text (`codex.md`).
fn codex_workflow() -> &'static str {
    TREE.get_file("codex.md")
        .and_then(|f| f.contents_utf8())
        .expect("codex.md is embedded as UTF-8")
}

/// Builds a Codex skill: shares the command parsers, model defaults, and
/// embedded release version with Claude's skills, but uses Codex's process
/// lifecycle.
pub fn codex_skill(name: &str, version: &str) -> Result<Vec<u8>, String> {
    // The Ultrafast input is built here; the match borrows it.
    let ultrafast_input: String;
    let (description, command, input): (String, &str, &str) = match name {
        "rival-claude" => (
            "Review code with Opus 5.5 through Rival and the authenticated Claude Code CLI. Use for a requested independent Claude review from Codex.".into(),
            "claude",
            "Always run a code review. No arguments means `review`. A scope means `review <scope>`. If the user already supplied `review`, do not duplicate it. Move an explicit effort before review: `-re high review src/`. Omitted effort uses the configured Claude default (medium fallback). Requires the Claude Code CLI authenticated with `claude auth login`, or Rival's configured Docker transport. Rival selects Opus 5.5; do not replace it with another model.",
        ),
        "rival-fable" => (
            "Review code with Fable 5.1 through Rival and the authenticated Claude Code CLI. Use for a requested independent Fable review from Codex.".into(),
            "fable",
            "Always run a code review. No arguments means `review`. A scope means `review <scope>`. If the user already supplied `review`, do not duplicate it. Move an explicit effort before review: `-re high review src/`. Omitted effort uses the configured Fable default (medium fallback). Requires the Claude Code CLI authenticated with `claude auth login`, Rival's configured Docker transport, or Rival's proxy. Rival selects Fable 5.1; do not replace it with another model.",
        ),
        "rival-sol" => (
            "Run a requested Sol 6.1 prompt or code review through Rival from Codex.".into(),
            "sol",
            "Pass the user's arguments verbatim: `[-re level] review [scope]` for reviews, or `[-re level] <prompt>` for a raw prompt. With no arguments show usage and do not launch. Rival selects Sol 6.1 (`gpt-6.1-sol`) on the Codex CLI. Omitted effort uses `efforts.sol` from Rival's configuration (xhigh fallback). Requires the Codex CLI authenticated with `codex login`, or Rival's configured Codex proxy route. Do not invent flags or substitute another model.",
        ),
        "rival-astra-ultrafast" | "rival-sol-ultrafast" => {
            let (model, slug, command, effort) = if name == "rival-sol-ultrafast" {
                ("Sol 6.1", "gpt-6.1-sol", "sol --ultrafast", "efforts.sol")
            } else {
                ("GPT-6 Astra", "gpt-6-astra", "codex --ultrafast", "efforts.codex")
            };
            ultrafast_input = format!(
                    "Pass the user's arguments verbatim: `[-re level] review [scope]` for reviews, or `[-re level] <prompt>` for a raw prompt. With no arguments show usage and do not launch. Rival selects {model} (`{slug}`) on the Codex CLI with `service_tier=\"ultrafast\"`; {} Omitted effort uses `{effort}` from Rival's configuration (xhigh fallback). When the output has a `rival: ⚠ ultrafast not applied` line, the run used the standard tier: repeat that line in your summary. Do not invent flags or substitute another model.",
                crate::ultrafast_requirement!()
            );
            (
                format!("Run a requested {model} prompt or code review on the Codex Ultrafast tier through Rival from Codex."),
                command,
                &ultrafast_input,
            )
        }
        "rival-codex" | "rival-k3" | "rival-grok" => {
            let model = name.strip_prefix("rival-").unwrap_or(name);
            (
                format!("Run a requested {model} prompt or code review through Rival from Codex."),
                model,
                "Pass the user's arguments verbatim: `[-re level] review [scope]` for reviews, or `[-re level] <prompt>` for a raw prompt. With no arguments show usage and do not launch. Model defaults and provider setup are owned by Rival; do not invent flags or substitute another model.",
            )
        }
        "rival-security" => (
            "Run Rival's dedicated security reviewer on changed code or a specified scope from Codex. Use for requested vulnerability reviews.".into(),
            "security",
            "First run `rival command security --which --workdir <absolute-repository>` and report the resolved model. If it fails, report the error and do not launch. Pass the user's scope verbatim; empty input reviews git-detected changes. The model is selected by security.reviewer in Rival's configuration.",
        ),
        _ => return Err(format!("no Codex skill for {:?}", name)),
    };
    let mut content = format!(
        "---\nname: {name}\ndescription: {description}\nmetadata:\n  version: {version}\n---\n\n# {name}\n\n## Review input\n\n{input}\n\n"
    );
    content.push_str(&codex_workflow().replace("{{COMMAND}}", command));
    Ok(content.into_bytes())
}
