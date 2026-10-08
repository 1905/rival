//! User argument parsing. Go: `internal/parser`.

#[cfg(test)]
mod tests;

use anyhow::{Result, bail};

use crate::config::{self, VALID_EFFORTS, WHOLE_PROJECT};

/// The parsed user arguments. Go `ParseResult`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParseResult {
    pub effort: String,
    /// Exact megareview roster selectors; empty means the configured default
    /// (Go nil).
    pub models: Vec<String>,
    pub is_review: bool,
    /// True when a review has no explicit scope (use git detection).
    pub auto_scope: bool,
    pub review_scope: String,
    /// Raw prompt only; "" for a review (cmd builds that prompt).
    pub prompt: String,
    pub is_empty: bool,
    /// True when the scope was passed after "--" (take it verbatim).
    pub escaped: bool,
}

/// Parses raw arguments for the claude command (claude-opus-5-5).
/// Grammar: `[-re level] [review [scope] | prompt]`. An omitted effort stays
/// empty so the command can apply Claude's configured default.
pub fn parse_claude_args(raw: &str) -> Result<ParseResult> {
    parse_args_with_effort(raw, "", config::is_valid_effort, &VALID_EFFORTS)
}

/// Parses raw arguments for the grok command (grok-4.6). Identical grammar
/// to Claude: an omitted effort stays empty so the command can apply grok's
/// configured default.
pub fn parse_grok_args(raw: &str) -> Result<ParseResult> {
    parse_args_with_effort(raw, "", config::is_valid_effort, &VALID_EFFORTS)
}

/// Parses raw arguments for the kimi command. The -re flag is accepted for
/// grammar consistency, but the executor always runs Kimi K3 at max
/// reasoning — the model supports no other level.
pub fn parse_kimi_args(raw: &str) -> Result<ParseResult> {
    // K3 runs at max regardless (OpencodeVariant pins it), but rejecting the
    // level the docs advertise would be a trap, so accept the shared ladder
    // plus max and discard the value later.
    let mut names = VALID_EFFORTS.to_vec();
    names.push("max");
    parse_args_with_effort(
        raw,
        "",
        |e| config::is_valid_effort(e) || e == "max",
        &names,
    )
}

/// Parses raw arguments for the codex command (gpt-6-astra). Identical
/// grammar to Claude: an omitted effort stays empty so the command can apply
/// Codex's configured default.
pub fn parse_codex_args(raw: &str) -> Result<ParseResult> {
    parse_args_with_effort(raw, "", config::is_valid_effort, &VALID_EFFORTS)
}

fn parse_args_with_effort(
    raw: &str,
    default_effort: &str,
    valid_effort: impl Fn(&str) -> bool,
    effort_names: &[&str],
) -> Result<ParseResult> {
    let mut s = raw.trim();
    if s.is_empty() {
        return Ok(ParseResult {
            effort: default_effort.to_string(),
            is_empty: true,
            ..Default::default()
        });
    }

    let mut result = ParseResult {
        effort: default_effort.to_string(),
        ..Default::default()
    };

    // Step 1: Parse -re flag.
    if let Some(rest) = s.strip_prefix("-re ") {
        let rest = rest.trim();
        let mut parts = rest.splitn(2, ' ');
        let effort = parts.next().unwrap_or_default();
        if !valid_effort(effort) {
            bail!(
                "invalid effort level {:?}, must be one of: {}",
                effort,
                effort_names.join(", ")
            );
        }
        result.effort = effort.to_string();
        s = parts.next().map_or("", str::trim);
    }

    // Step 2: Check for review subcommand.
    let lower = s.to_lowercase();
    if lower == "review" || lower.starts_with("review ") {
        result.is_review = true;
        // Only an ASCII "review" lowers to "review", so the first
        // len("review") bytes of the original are the keyword.
        let scope = String::from_utf8_lossy(&s.as_bytes()["review".len()..]);
        let mut scope = scope.trim().to_string();
        if scope.is_empty() {
            result.auto_scope = true;
            scope = WHOLE_PROJECT.to_string();
        }
        // The review prompt is built by cmd (review.BuildReviewerPrompt), so a
        // review leaves Prompt empty.
        result.review_scope = scope;
        return Ok(result);
    }

    // Step 3: Otherwise it's a raw prompt.
    if s.is_empty() {
        result.is_empty = true;
        return Ok(result);
    }
    result.prompt = s.to_string();
    Ok(result)
}
