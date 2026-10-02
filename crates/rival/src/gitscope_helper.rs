//! Git-scoped review prompts shared by the commands. Go:
//! `cmd/gitscope_helper.go`.
//!
//! Go's `lensPrompt` wraps `review.BuildReviewerPrompt` and lands with
//! Task 2.6, which ports that builder.

use rival_core::config::{ANTISLOP_CODE_PROMPT, Config, DIFF_REVIEW_PREAMBLE, WHOLE_PROJECT};
use rival_core::{gitscope, logging};

#[cfg(test)]
mod tests;

/// Builds the [`DIFF_REVIEW_PREAMBLE`] block (changed-file list + diff stats)
/// for `workdir`. `files == ""` means git detected no changes; the raw files
/// list is returned alongside so callers can record it as the review scope
/// without a second [`gitscope::resolve`] fork.
pub fn build_diff_preamble(cfg: &Config, workdir: &str) -> (String, String) {
    let files = gitscope::resolve(cfg, workdir);
    if files.is_empty() {
        return (String::new(), String::new());
    }
    let preamble = DIFF_REVIEW_PREAMBLE.replace("{FILES}", &files);
    let diff_stat = gitscope::diff_stat(cfg, workdir);
    let preamble = if diff_stat.is_empty() {
        preamble.replace("{DIFFSTAT}", "")
    } else {
        preamble.replace(
            "{DIFFSTAT}",
            &format!("\nDiff stats:\n```\n{diff_stat}\n```\n"),
        )
    };
    (preamble, files)
}

/// Builds a review prompt with `build`, which renders the prompt for one
/// scope string. With `auto_scope` it asks git for the changed files and
/// ignores `scope`: when there are some, the prompt is
/// [`DIFF_REVIEW_PREAMBLE`] + `build("the changed files listed above")`,
/// target is the file list and display is "changed files (git auto-detect)";
/// when there are none it reviews [`WHOLE_PROJECT`]. Otherwise it reviews
/// `scope` as given.
///
/// Returns `(prompt, target, display)`: target is what a session records;
/// display is the output's Scope line.
pub fn build_review_prompt(
    build: impl Fn(&str) -> String,
    scope: &str,
    auto_scope: bool,
    cfg: &Config,
    workdir: &str,
) -> (String, String, String) {
    let mut scope = scope;
    if auto_scope {
        let (preamble, files) = build_diff_preamble(cfg, workdir);
        if !files.is_empty() {
            logging::info()
                .str("files", files.as_str())
                .msg("git scope: auto-detected changed files");
            let prompt = preamble + &build("the changed files listed above");
            return (prompt, files, "changed files (git auto-detect)".to_string());
        }
        logging::debug().msg("git scope: no changes detected, falling back to full project");
        scope = WHOLE_PROJECT;
    }
    (build(scope), scope.to_string(), scope.to_string())
}

/// Renders the code-mode antislop prompt for `scope`.
pub fn antislop_code_prompt(scope: &str) -> String {
    ANTISLOP_CODE_PROMPT.replace("{SCOPE}", scope)
}
