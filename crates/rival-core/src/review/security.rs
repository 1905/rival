//! Security review validation and rendering. Go:
//! `internal/review/security.go`.

use anyhow::anyhow;

use super::format::{
    format_unusable, known_severity, severity_tally, sorted_findings, validate_output,
    write_finding, write_header,
};
use super::parse::parse_reviewer_output;
use super::prompt::CLEAN_REVIEW_EXAMPLE_LINE;
use super::types::ReviewerOutput;
use crate::config;
use crate::gostd;

#[cfg(test)]
mod tests;

/// Validates a security payload against the raw output it came from. A
/// caller must treat a validation failure exactly like a parse failure:
/// `{"summary":"","findings":null}` parses cleanly, and the prompt carries a
/// parseable clean example, so an empty payload or an echoed prompt would
/// otherwise format as "no vulnerabilities" when nothing was reviewed.
pub fn validate_security_result(out: Option<&ReviewerOutput>, raw: &str) -> anyhow::Result<()> {
    validate_output(out, raw, security_echo)?;
    let out = out.expect("validation rejects a missing payload");
    for (i, f) in out.findings.iter().enumerate() {
        let n = i + 1;
        if f.file.trim().is_empty() {
            return Err(anyhow!("finding {n} has no file"));
        }
        if f.title.trim().is_empty() {
            return Err(anyhow!("finding {n} has no title"));
        }
        if !known_severity(&f.severity) {
            return Err(anyhow!(
                "finding {n} has an unknown severity {}",
                gostd::quote(&f.severity)
            ));
        }
    }
    Ok(())
}

/// Phrases from the security prompt. Security runs on opencode, whose log
/// never contains the prompt, so a marker in a clean security result means
/// the model echoed its instructions. A false "unusable" is the safe
/// failure for a security gate, so this stays strict.
const PROMPT_ECHO_MARKERS: [&str; 2] = [
    "## Role: Security Reviewer",
    "Work through every class below",
];

/// Reports whether `out` is exactly the prompt's clean example.
fn is_clean_example(out: &ReviewerOutput) -> bool {
    out.summary.trim() == "No issues found." && out.findings.is_empty()
}

/// The strict echo check for security reviews.
fn security_echo(out: &ReviewerOutput, raw: &str) -> bool {
    is_clean_example(out) && PROMPT_ECHO_MARKERS.iter().any(|m| raw.contains(m))
}

/// The echo check for bug-hunter reviews. Codex writes the whole prompt into
/// its log, and a reviewer may quote prompt.go (security markers included)
/// in tool output, so no marker is evidence. The output is an echo only
/// when no reviewer payload follows the last copy of the prompt's clean
/// example.
pub(crate) fn bug_hunter_echo(out: &ReviewerOutput, raw: &str) -> bool {
    if !is_clean_example(out) {
        return false;
    }
    let Some(i) = raw.rfind(CLEAN_REVIEW_EXAMPLE_LINE) else {
        return false;
    };
    parse_reviewer_output(&raw[i + CLEAN_REVIEW_EXAMPLE_LINE.len()..]).is_err()
}

/// Renders a security review, or falls back to the raw log when the payload
/// is unusable. It owns that choice because the inner formatter has neither
/// the raw output nor the CLI the normalizer needs. The returned result is
/// the validation failure: a security gate must not exit 0 on output it
/// cannot trust. The text is returned either way.
pub fn format_security_result(
    parsed: Option<&ReviewerOutput>,
    raw: &str,
    cli: &str,
    model: &str,
    scope: &str,
    log_path: &str,
) -> (String, anyhow::Result<()>) {
    let label = config::engine_label(cli, model);
    if let Err(err) = validate_security_result(parsed, raw) {
        let text = format_unusable(
            "RIVAL SECURITY REVIEW — UNUSABLE OUTPUT",
            &label,
            scope,
            &err.to_string(),
            "The model ran but did not return a review this can trust.\nRaw output follows for diagnosis.\n\n",
            &config::public_runtime_log(cli, model, raw),
            log_path,
        );
        return (text, Err(err));
    }
    let out = parsed.expect("validation rejects a missing payload");
    (format_security_console(out, &label, scope), Ok(()))
}

/// Renders a validated security review.
pub fn format_security_console(out: &ReviewerOutput, model: &str, scope: &str) -> String {
    let mut sb = String::new();
    write_header(&mut sb, "RIVAL SECURITY REVIEW", model, scope, &out.summary);

    if out.findings.is_empty() {
        sb.push_str("No vulnerabilities found.\n");
        return sb;
    }

    let findings = sorted_findings(&out.findings);
    for (i, f) in findings.iter().enumerate() {
        write_finding(&mut sb, i + 1, f);
    }
    sb.push_str(&severity_tally(&findings));
    sb.push('\n');
    sb
}
