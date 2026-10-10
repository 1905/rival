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
pub const NAMES: [&str; 10] = [
    "rival-codex",
    "rival-sol",
    "rival-plan",
    "rival-plan-codex",
    "rival-plan-claude",
    "rival-claude",
    "rival-fable",
    "rival-k3",
    "rival-grok",
    "rival-security",
];

/// Legacy or superseded skills that install removes.
/// Re-enable a skill by adding it back to [`NAMES`] and its directory to
/// the embedded tree.
pub const DEPRECATED: [&str; 13] = [
    "rival-plan-sol", // rival-plan takes --model sol
    "rival-claude-only",
    "rival-fable-only",
    "rival-codex-only",
    "rival-gpt-5-6-sol",
    "rival-claude-fable",
    "rival-astra",         // renamed to rival-codex in 3.34
    "rival-plan-astra",    // renamed to rival-plan-codex in 3.34
    "rival-plan-fable",    // rival-plan takes --model fable
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
    let (description, command, input): (String, &str, &str) = match name {
        "rival-claude" => (
            "Review code with Opus 5.5 through Rival and the authenticated Claude Code CLI. Use for a requested independent Claude review from Codex.".into(),
            "claude",
            "Always run a code review. No arguments means `review`. A scope means `review <scope>`. If the user already supplied `review`, do not duplicate it. Move an explicit effort before review: `-re high review src/`. Omitted effort uses the configured Claude default (medium fallback). For a plan document use $rival-plan-claude. Requires the Claude Code CLI authenticated with `claude auth login`, or Rival's configured Docker transport. Rival selects Opus 5.5; do not replace it with another model.",
        ),
        "rival-fable" => (
            "Review code with Fable 5.1 through Rival and the authenticated Claude Code CLI. Use for a requested independent Fable review from Codex.".into(),
            "fable",
            "Always run a code review. No arguments means `review`. A scope means `review <scope>`. If the user already supplied `review`, do not duplicate it. Move an explicit effort before review: `-re high review src/`. Omitted effort uses the configured Fable default (medium fallback). For a plan document use $rival-plan with `--model fable`. Requires the Claude Code CLI authenticated with `claude auth login`, Rival's configured Docker transport, or Rival's proxy. Rival selects Fable 5.1; do not replace it with another model.",
        ),
        "rival-codex" | "rival-sol" | "rival-k3" | "rival-grok" => {
            let model = name.strip_prefix("rival-").unwrap_or(name);
            (
                format!("Run a requested {model} prompt or code review through Rival from Codex."),
                model,
                "Pass the user's arguments verbatim: `[-re level] review [scope]` for reviews, or `[-re level] <prompt>` for a raw prompt. With no arguments show usage and do not launch. Model defaults and provider setup are owned by Rival; do not invent flags or substitute another model.",
            )
        }
        "rival-plan" | "rival-plan-codex" | "rival-plan-claude" => (
            "Review a plan or specification document through Rival from Codex, returning ratings and findings.".into(),
            match name {
                "rival-plan-claude" => "plan --model claude",
                _ => "plan --model codex --effort xhigh",
            },
            match name {
                "rival-plan" => "Pass the document path and any requested options verbatim. If no document is specified, ask for its path before launching. Show all model results and report any skipped model. Codex plan reviews pin xhigh. To use other models, replace `--model codex --effort xhigh` with `--model <list>`: codex, sol, claude (or opus), fable, comma-separated, for example `--model opus,fable,sol`; each model then uses its configured effort unless the user supplies -re.",
                _ => "Pass the document path and any requested options verbatim. If no document is specified, ask for its path before launching. Show all model results and report any skipped model. Codex plan reviews pin xhigh; Claude-only uses its configured effort (medium fallback) unless the user supplies -re.",
            },
        ),
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
