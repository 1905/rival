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
pub const NAMES: [&str; 1] = ["rival"];

/// Retired plan skills that install archives outside the active skill tree.
pub const RETIRED_PLAN_SKILLS: [&str; 3] = ["rival-plan", "rival-plan-codex", "rival-plan-claude"];

/// Other legacy skills that install removes.
pub const DEPRECATED: [&str; 22] = [
    "rival-plan-sol", // use the native plan command
    "rival-claude-only",
    "rival-fable-only",
    "rival-codex-only",
    "rival-gpt-5-6-sol",
    "rival-claude-fable",
    "rival-astra",           // renamed to rival-codex in 3.34
    "rival-plan-astra",      // renamed to rival-plan-codex in 3.34
    "rival-plan-fable",      // use the native plan command
    "rival-kimi",            // renamed to rival-k3 before release
    "rival-antislop-plan",   // plan mode dropped on 2026-08-20
    "rival-review",          // megareview removed 2026-09-26
    "rival-antislop",        // code-slop review removed 2026-10-08
    "rival-astra-ultrafast", // the ultrafast.codex switch, 5.2.0
    "rival-sol-ultrafast",   // the ultrafast.sol switch, 5.2.0
    // The per-model skills, replaced by the one /rival skill in 6.0.0.
    "rival-codex",
    "rival-sol",
    "rival-claude",
    "rival-fable",
    "rival-k3",
    "rival-grok",
    "rival-security",
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

/// The description of the Codex `/rival` skill.
const CODEX_DESCRIPTION: &str = "Run a requested prompt, code review or security review through Rival from Codex, with the model set for Codex in rival config.";

/// The input rules of the Codex `/rival` skill. Like the Claude Code skill
/// it names no model: `skill.codex` selects it.
const CODEX_INPUT: &str = "Pass the user's arguments verbatim: `[-re level] review [scope]` for a code review, `security [scope]` for a security review, or `[-re level] <prompt>` for a raw prompt. With no arguments show usage and do not launch. The model comes from `skill.codex` in Rival's configuration (`rival config`, Models); the output names the model that ran. For `security`, first run `rival command security --which --workdir <absolute-repository>` and report the resolved model. If it fails, report the error and do not launch. Do not invent flags or substitute another model.";

/// The `rival command` arguments the Codex skill launches.
const CODEX_COMMAND: &str = "skill --host codex";

/// Builds a Codex skill: the same command and parser as the Claude Code
/// skill, with the Codex host and Codex's process lifecycle.
pub fn codex_skill(name: &str, version: &str) -> Result<Vec<u8>, String> {
    if name != "rival" {
        return Err(format!("no Codex skill for {:?}", name));
    }
    let mut content = format!(
        "---\nname: {name}\ndescription: {CODEX_DESCRIPTION}\nmetadata:\n  version: {version}\n---\n\n# {name}\n\n## Review input\n\n{CODEX_INPUT}\n\n"
    );
    content.push_str(&codex_workflow().replace("{{COMMAND}}", CODEX_COMMAND));
    Ok(content.into_bytes())
}
