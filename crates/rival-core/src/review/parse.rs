//! Reviewer output parsing. Go: `internal/review/parse.go`.

use std::sync::LazyLock;

use anyhow::anyhow;
use regex::Regex;

use super::types::{ReviewerFinding, ReviewerOutput, decode_payload};
use crate::gojson;

#[cfg(test)]
mod tests;

/// Extracts a reviewer's structured JSON from raw CLI output.
///
/// CLIs make this more than one decode: they echo the prompt (which
/// contains the schema example), print unrelated JSON (tool/telemetry
/// events), and wrap JSON in prose. So every top-level JSON object is
/// scanned, and the LAST one that is a genuine reviewer payload wins — it
/// must carry both "summary" and "findings" keys and must not be the schema
/// example itself.
///
/// This scans its whole argument. A provider log goes through
/// [`final_answer`] first; see [`parse_reviewer_log`].
pub fn parse_reviewer_output(raw: &str) -> anyhow::Result<ReviewerOutput> {
    let objs = json_objects(raw);
    let mut last_err = None;
    for c in objs.iter().rev() {
        if !has_json_key(c, "summary") || !has_json_key(c, "findings") {
            continue;
        }
        let out = match decode_payload(c, "ReviewerOutput", &["summary", "findings"]) {
            Ok(out) => out,
            Err(e) => {
                last_err = Some(e); // a payload-shaped candidate that failed to decode
                continue;
            }
        };
        if is_example_summary(&out.summary) {
            continue; // the echoed schema example, not a real answer
        }
        return Ok(ReviewerOutput {
            summary: out.summary,
            findings: drop_placeholder_reviewer_findings(out.findings),
        });
    }
    match last_err {
        Some(e) => Err(anyhow!(
            "no valid reviewer JSON payload (last decode error: {e})"
        )),
        None => Err(anyhow!("no reviewer JSON payload found in output")),
    }
}

/// Parses a provider log: [`parse_reviewer_output`] of its
/// [`final_answer`]. Go call sites spell this out as
/// `ParseReviewerOutput(FinalAnswer(raw))`.
pub fn parse_reviewer_log(raw: &str) -> anyhow::Result<ReviewerOutput> {
    parse_reviewer_output(final_answer(raw))
}

/// Reports whether `candidate` is a JSON object with the given top-level key
/// actually present (not merely defaulting to a zero value on decode). This
/// rejects unrelated JSON such as a `{"event":"done"}` tool/telemetry line.
/// The key match is exact, unlike the case-insensitive struct decoding.
pub(crate) fn has_json_key(candidate: &str, key: &str) -> bool {
    // Go: json.Unmarshal into map[string]json.RawMessage. A non-object
    // (including null) fails or leaves the map empty.
    gojson::decode_object(candidate.as_bytes(), "map[string]json.RawMessage")
        .is_ok_and(|members| members.iter().any(|(k, _)| k == key))
}

/// Matches the schema example summary from the prompt contract
/// (`reviewerJSONContract`). A real summary is never this exact string. The
/// clean-review example uses a real sentence ("No issues found.") so a
/// genuine clean review is not mistaken for the example.
fn is_example_summary(s: &str) -> bool {
    s.trim() == "1-3 sentence reviewer summary"
}

/// Matches a finding copied from the schema example: the enum fields hold
/// the literal pipe-delimited option lists, or the file is the contract's
/// placeholder. Real findings never have these field values. The category
/// is compared against every prompt contract's exact enum literal (code
/// review, plan, antislop) — exact matches only, because models under
/// uncertainty emit real dual categories like "bug|security" that a looser
/// pipe check would silently drop.
fn is_placeholder_finding(file: &str, severity: &str, category: &str) -> bool {
    match category {
        "bug|security|performance|concurrency|architecture|tests|ux"
        | "bug|gap|ambiguity|scope|verification"
        | "reuse|simplify|efficiency|altitude|compat|reinvention|slop|yagni" => return true,
        _ => {}
    }
    file == "path/to/file" || severity == "critical|high|medium|low"
}

/// Removes individual schema-example findings so a single echoed placeholder
/// item does not discard an otherwise real review.
pub(crate) fn drop_placeholder_reviewer_findings(
    findings: Vec<ReviewerFinding>,
) -> Vec<ReviewerFinding> {
    findings
        .into_iter()
        .filter(|f| !is_placeholder_finding(&f.file, &f.severity, &f.category))
        .collect()
}

/// Returns every balanced, valid JSON object in `s`, in order of appearance
/// (closing-brace order), including objects nested inside larger non-JSON
/// brace spans.
///
/// It is a single forward pass using an explicit stack of '{' indices (no
/// recursion, so adversarial deep nesting can't overflow the stack; and no
/// per-brace re-scan). Every time a brace closes, the span it delimits is
/// tested with Go's `json.Valid` rules, so:
///   - a lone/unbalanced brace from a grep/tool line never hides a later
///     payload (the payload's own brace pair is still tested when it
///     closes), and
///   - a valid payload nested inside a balanced-but-invalid span is still
///     found.
///
/// While outside any object (empty stack) prose and code are ignored
/// entirely — only '{' matters — so stray quotes or braces in surrounding
/// text can't desync the scan. JSON string/escape state is tracked only
/// inside an object.
pub(crate) fn json_objects(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut stack: Vec<usize> = Vec::new(); // indices of currently-open '{'
    let mut in_str = false;
    let mut esc = false;

    for (i, &c) in bytes.iter().enumerate() {
        if stack.is_empty() {
            if c == b'{' {
                stack.push(i);
                in_str = false;
                esc = false;
            }
            continue;
        }

        if esc {
            esc = false;
            continue;
        }
        if c == b'\\' {
            if in_str {
                esc = true;
            }
            continue;
        }
        if c == b'"' {
            in_str = !in_str;
            continue;
        }
        if in_str {
            continue;
        }
        match c {
            b'{' => stack.push(i),
            b'}' => {
                let start = stack.pop().expect("stack is non-empty");
                // Both ends are ASCII braces, so the span is on char boundaries.
                let candidate = &s[start..=i];
                if gojson::valid_value(candidate.as_bytes()).is_some() {
                    out.push(candidate);
                }
            }
            _ => {}
        }
    }
    out
}

/// The line codex prints before the assistant's final message. Everything
/// above it is the echoed prompt and tool output.
static CODEX_ANSWER_HEADER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^codex$").expect("valid regex"));

/// Returns the part of a provider log that holds the model's final answer.
/// For codex that is the text after the last "codex" header line, so
/// review-shaped JSON a tool printed earlier (a file the model read) can
/// never be taken for the review. Logs without that header are returned
/// whole.
pub fn final_answer(raw: &str) -> &str {
    match CODEX_ANSWER_HEADER.find_iter(raw).last() {
        Some(m) => &raw[m.end()..],
        None => raw,
    }
}
