//! Plan output parsing and rendering, plus the [`PlanRunResult`] record.
//! Running the reviews is not here.

use std::fmt::Write as _;

use anyhow::bail;

use super::format::{severity_tally, sorted_findings, write_finding, write_summary};
use super::parse::payload_error;
use super::slots::SkippedCLI;
use super::types::ReviewerFinding;
use crate::config;
use crate::result::{self, PayloadKind};

#[cfg(test)]
mod tests;

/// The structured JSON a plan reviewer emits. It mirrors
/// [`super::ReviewerOutput`] but carries a 1-10 rating.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanOutput {
    pub summary: String,
    pub rating: i64,
    pub findings: Vec<ReviewerFinding>,
}

/// Extracts the plan reviewer's structured JSON from a provider log:
/// [`result::log_payload`] for a plan payload, which finds the final answer
/// and takes the last genuine plan payload in it (summary, rating and
/// findings keys, a 1-10 rating, not the schema example). A blank summary
/// makes the payload invalid, as it does for a code review.
pub fn parse_plan_log(raw: &str) -> anyhow::Result<PlanOutput> {
    let p = result::log_payload(raw, PayloadKind::Plan).map_err(|e| payload_error("plan", e))?;
    if p.summary.trim().is_empty() {
        bail!("plan summary is empty");
    }
    Ok(PlanOutput {
        summary: p.summary,
        rating: p.rating.map_or(0, i64::from),
        findings: p.findings.into_iter().map(ReviewerFinding::from).collect(),
    })
}

/// Renders a single-CLI [`PlanOutput`]: the header, the file, the 1-10
/// rating, the summary, then every finding grouped by severity
/// (crit→high→med→low) and numbered globally 1..N. No confidence filtering.
pub fn format_plan_console(out: &PlanOutput, file: &str) -> String {
    let mut sb = String::from("\n═══ RIVAL PLAN REVIEW ═══\n\n");
    let _ = writeln!(sb, "File: {file}");
    format_plan_body(out, &mut sb, PLAN_RATING_LABEL, PLAN_EMPTY_LINE);
    sb
}

// Body labels passed to `format_plan_body`.
const PLAN_RATING_LABEL: &str = "Rating";
const PLAN_EMPTY_LINE: &str = "No bugs or gaps found.";

/// Writes the rating/summary/findings/tally for one [`PlanOutput`]. It
/// carries no header or File line, so the single-model and multi-model
/// renderers share it; `rating_label` and `empty_line` are the only
/// flavor-specific strings.
fn format_plan_body(out: &PlanOutput, sb: &mut String, rating_label: &str, empty_line: &str) {
    let _ = write!(sb, "{rating_label}: {}/10\n\n", out.rating);
    write_summary(sb, &out.summary);

    // Sorted by severity (crit first), then confidence (highest first), so
    // findings come out grouped without a separate bucketing pass.
    let mut findings = sorted_findings(&out.findings);

    if findings.is_empty() {
        sb.push_str(empty_line);
        sb.push('\n');
        return;
    }

    for (i, f) in findings.iter_mut().enumerate() {
        // The plan schema has no failure_scenario; drop one a model
        // volunteers so doc reviews never print a Scenario line.
        f.failure_scenario.clear();
        write_finding(sb, i + 1, f);
    }

    // Severity tally for a quick read.
    sb.push_str(&severity_tally(&findings));
    sb.push('\n');
}

/// The outcome of a plan run: one result per model that ran,
/// plus the models that were skipped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanRunResult {
    pub results: Vec<PlanCLIResult>,
    pub skipped: Vec<SkippedCLI>,
}

/// One CLI's plan-review outcome. `parsed` is `None` when the CLI's output
/// could not be parsed into structured JSON; `raw` then holds the unparsed
/// output so nothing the model produced is lost.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanCLIResult {
    pub cli: String,
    pub model: String,
    pub parsed: Option<PlanOutput>,
    pub raw: String,
}

/// Renders a plan run. A single successful model reuses the single-model
/// layout ([`format_plan_console`] when parsed, else the raw output). Two or
/// more models use the multi-block layout. Any skipped models are surfaced.
pub fn format_plan_result(result: Option<&PlanRunResult>, file: &str) -> String {
    let Some(result) = result.filter(|r| !r.results.is_empty()) else {
        return "No plan review output.\n".to_string();
    };
    if let [r] = result.results.as_slice()
        && result.skipped.is_empty()
    {
        return match &r.parsed {
            Some(parsed) => format_plan_console(parsed, file),
            // Parse failed — preserve the raw model output while normalizing
            // any adapter banner to the concrete model name.
            None => config::public_runtime_log(&r.cli, &r.model, &r.raw),
        };
    }
    format_plan_multi_console(&result.results, &result.skipped, file)
}

/// Renders 2+ models' plan reviews as separate labelled blocks under one
/// header, followed by any skipped models. A result with no parsed output
/// prints its raw output so a parse failure never drops the model's work.
pub fn format_plan_multi_console(
    results: &[PlanCLIResult],
    skipped: &[SkippedCLI],
    file: &str,
) -> String {
    format_doc_multi_console(
        results,
        skipped,
        "RIVAL PLAN REVIEW",
        &format!("File: {file}"),
        PLAN_RATING_LABEL,
        PLAN_EMPTY_LINE,
    )
}

/// The multi-model renderer used by plan reviews;
/// `header_name`, `target_line` and the body labels set the text.
fn format_doc_multi_console(
    results: &[PlanCLIResult],
    skipped: &[SkippedCLI],
    header_name: &str,
    target_line: &str,
    rating_label: &str,
    empty_line: &str,
) -> String {
    let labels: Vec<String> = results
        .iter()
        .map(|r| config::engine_label(&r.cli, &r.model))
        .collect();
    let mut sb = format!("\n═══ {header_name} ({}) ═══\n\n", labels.join(" + "));
    sb.push_str(target_line);
    sb.push('\n');

    for (i, r) in results.iter().enumerate() {
        let _ = write!(sb, "\n── {} ──\n\n", config::engine_label(&r.cli, &r.model));
        match &r.parsed {
            Some(parsed) => format_plan_body(parsed, &mut sb, rating_label, empty_line),
            None => {
                // Parse failed — emit normalized raw output so nothing is lost
                // while internal runtime identifiers stay out of the public
                // result.
                sb.push_str("(could not parse structured output — raw output below)\n\n");
                sb.push_str(config::public_runtime_log(&r.cli, &r.model, &r.raw).trim());
                sb.push('\n');
            }
        }
        if i + 1 < results.len() {
            sb.push('\n');
        }
    }

    if !skipped.is_empty() {
        sb.push('\n');
        for s in skipped {
            let reason = config::public_runtime_error(&s.cli, &s.model, &s.reason);
            let _ = writeln!(sb, "Skipped: {} — {reason}", s.label());
        }
    }

    sb
}
