//! Console rendering shared by the review, security and plan output.

use std::fmt::Write as _;

use anyhow::anyhow;

use super::security::bug_hunter_echo;
use super::types::{ReviewerFinding, ReviewerOutput};
use crate::config;

#[cfg(test)]
mod tests;

/// Splits a single-model review: findings below it go in a short "Low
/// confidence" block instead of the main list.
pub const DEFAULT_CONFIDENCE_THRESHOLD: i64 = 6;

/// The canonical severity ladder, most severe first, with the short label
/// each one displays as.
const SEVERITIES: [(&str, &str); 4] = [
    ("critical", "crit"),
    ("high", "high"),
    ("medium", "med"),
    ("low", "low"),
];

/// Orders severities critical-first. An unknown severity ranks
/// `SEVERITIES.len()`, after every known one.
fn severity_rank(s: &str) -> usize {
    let s = s.to_lowercase();
    SEVERITIES
        .iter()
        .position(|(name, _)| *name == s)
        .unwrap_or(SEVERITIES.len())
}

/// Reports whether `s` is one of the canonical severities.
pub(crate) fn known_severity(s: &str) -> bool {
    severity_rank(s) < SEVERITIES.len()
}

/// Maps a canonical severity to its short label (crit/high/med/low) in the
/// review, security and plan output. Unknown values pass through lowercased.
pub(crate) fn display_severity(s: &str) -> String {
    match SEVERITIES.get(severity_rank(s)) {
        Some((_, short)) => (*short).to_string(),
        None => s.to_lowercase(),
    }
}

/// Returns a copy of `findings` ordered by severity (critical first), then
/// confidence (highest first). The sort is stable, so equal findings keep
/// the model's order. Shared by the review, security and plan renderers.
pub(crate) fn sorted_findings(findings: &[ReviewerFinding]) -> Vec<ReviewerFinding> {
    let mut out = findings.to_vec();
    out.sort_by(|a, b| {
        severity_rank(&a.severity)
            .cmp(&severity_rank(&b.severity))
            .then(b.confidence.cmp(&a.confidence))
    });
    out
}

/// The one-line count, without a trailing newline. An unknown severity
/// counts as low.
pub(crate) fn severity_tally(findings: &[ReviewerFinding]) -> String {
    let mut counts = [0usize; SEVERITIES.len()];
    for f in findings {
        counts[severity_rank(&f.severity).min(SEVERITIES.len() - 1)] += 1;
    }
    let parts: Vec<String> = SEVERITIES
        .iter()
        .zip(counts)
        .map(|((_, short), n)| format!("{n} {short}"))
        .collect();
    format!("Findings: {} total — {}", findings.len(), parts.join(", "))
}

/// "file:line", or just the file when the line is unknown.
fn finding_location(f: &ReviewerFinding) -> String {
    if f.line > 0 {
        format!("{}:{}", f.file, f.line)
    } else {
        f.file.clone()
    }
}

/// Renders one numbered finding block, followed by a blank line. The
/// review, security and plan renderers share this exact layout.
pub(crate) fn write_finding(sb: &mut String, n: usize, f: &ReviewerFinding) {
    let _ = write!(sb, "{n}. [{}] {}", display_severity(&f.severity), f.title);
    let loc = finding_location(f);
    if !loc.is_empty() {
        let _ = write!(sb, " — {loc}");
    }
    sb.push('\n');
    let body = f.body.trim();
    if !body.is_empty() {
        let _ = writeln!(sb, "   {body}");
    }
    let scenario = f.failure_scenario.trim();
    if !scenario.is_empty() {
        let _ = writeln!(sb, "   Scenario: {scenario}");
    }
    let fix = f.suggestion.trim();
    if !fix.is_empty() {
        let _ = writeln!(sb, "   Fix: {fix}");
    }
    if f.category.is_empty() {
        let _ = writeln!(sb, "   (confidence {})", f.confidence);
    } else {
        let _ = writeln!(sb, "   ({}, confidence {})", f.category, f.confidence);
    }
    sb.push('\n');
}

/// The lens's own echo check: does `raw` show that `out` is the prompt's
/// example rather than a review?
pub(crate) type EchoCheck = fn(&ReviewerOutput, &str) -> bool;

/// Reports whether a parsed payload is a usable review. Parsing only checks
/// that the JSON decodes with the right keys, so none, an echoed prompt
/// example and an empty summary all get here looking like a clean review.
pub(crate) fn validate_output(
    out: Option<&ReviewerOutput>,
    raw: &str,
    echo: EchoCheck,
) -> anyhow::Result<()> {
    let Some(out) = out else {
        return Err(anyhow!("no structured output"));
    };
    if out.summary.trim().is_empty() {
        return Err(anyhow!("summary is empty"));
    }
    if !raw.is_empty() && echo(out, raw) {
        return Err(anyhow!(
            "output repeats the prompt's own example rather than a review"
        ));
    }
    Ok(())
}

/// Validates a bug-hunter review.
pub(crate) fn validate_review_result(
    out: Option<&ReviewerOutput>,
    raw: &str,
) -> anyhow::Result<()> {
    validate_output(out, raw, bug_hunter_echo)
}

/// Renders a single-model review, or the raw log when the payload is
/// unusable. The model label in the output is "<label> (<model id>)".
pub fn format_review_result(
    parsed: Option<&ReviewerOutput>,
    raw: &str,
    cli: &str,
    model: &str,
    scope: &str,
    log_path: &str,
) -> String {
    let label = format!("{} ({model})", config::engine_label(cli, model));
    if let Err(err) = validate_review_result(parsed, raw) {
        return format_unusable(
            "RIVAL REVIEW — UNPARSED OUTPUT",
            &label,
            scope,
            &err.to_string(),
            "The model ran but its answer is not a structured review.\nRaw output follows.\n\n",
            &config::public_runtime_log(cli, model, raw),
            log_path,
        );
    }
    let out = parsed.expect("validation rejects a missing payload");
    format_review_console(out, &label, scope, log_path)
}

/// Renders a run whose output cannot be trusted as a review: the header,
/// the problem, an explanation, then the public raw log so nothing is lost.
/// It ends with the log path when one is known.
pub(crate) fn format_unusable(
    title: &str,
    label: &str,
    scope: &str,
    problem: &str,
    explain: &str,
    public: &str,
    log_path: &str,
) -> String {
    let mut sb = format!("\n═══ {title} ═══\n\n");
    let _ = write!(
        sb,
        "Model: {label}\nScope: {}\nProblem: {problem}\n\n",
        one_line(scope)
    );
    sb.push_str(explain);
    sb.push_str(public);
    if !public.is_empty() && !public.ends_with('\n') {
        sb.push('\n');
    }
    if !log_path.is_empty() {
        let _ = writeln!(sb, "\nLog: {log_path}");
    }
    sb
}

/// Renders a validated single-model review: findings at or above
/// [`DEFAULT_CONFIDENCE_THRESHOLD`] in full, the rest in a short block.
pub fn format_review_console(
    out: &ReviewerOutput,
    model_label: &str,
    scope: &str,
    log_path: &str,
) -> String {
    let mut sb = String::new();
    write_header(&mut sb, "RIVAL REVIEW", model_label, scope, &out.summary);

    if out.findings.is_empty() {
        sb.push_str("No issues found.\n");
    } else {
        let (low, main): (Vec<_>, Vec<_>) = sorted_findings(&out.findings)
            .into_iter()
            .partition(|f| f.confidence < DEFAULT_CONFIDENCE_THRESHOLD);
        if main.is_empty() {
            let _ = write!(
                sb,
                "No findings at confidence {DEFAULT_CONFIDENCE_THRESHOLD} or higher.\n\n"
            );
        }
        for (i, f) in main.iter().enumerate() {
            write_finding(&mut sb, i + 1, f);
        }
        if !low.is_empty() {
            let _ = writeln!(sb, "Low confidence ({}):", low.len());
            for f in &low {
                let _ = write!(sb, "- [{}] {}", display_severity(&f.severity), f.title);
                let loc = finding_location(f);
                if !loc.is_empty() {
                    let _ = write!(sb, " — {loc}");
                }
                let _ = writeln!(sb, " (confidence {})", f.confidence);
            }
            sb.push('\n');
        }
        sb.push_str(&severity_tally(&main));
        sb.push('\n');
    }

    if !log_path.is_empty() {
        let _ = writeln!(sb, "Log: {log_path}");
    }
    sb
}

/// Writes the review and security console header: the title, the Model and
/// Scope lines, then the summary.
pub(crate) fn write_header(sb: &mut String, title: &str, label: &str, scope: &str, summary: &str) {
    let _ = write!(sb, "\n═══ {title} ═══\n\n");
    let _ = writeln!(sb, "Model: {label}");
    let _ = write!(sb, "Scope: {}\n\n", one_line(scope));
    write_summary(sb, summary);
}

/// Writes the Summary line and a blank line, or nothing when the summary is
/// blank.
pub(crate) fn write_summary(sb: &mut String, summary: &str) {
    let s = summary.trim();
    if !s.is_empty() {
        let _ = write!(sb, "Summary: {s}\n\n");
    }
}

/// Joins a multi-line scope (the auto-detected changed-file list) so the
/// Scope line stays one line.
fn one_line(s: &str) -> String {
    s.split('\n')
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}
