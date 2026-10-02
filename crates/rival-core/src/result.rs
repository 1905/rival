//! What a finished run's Result view shows. Port of the app's
//! `RivalKit/ResultParser.swift`.
//!
//! This is the app's newer answer extraction (codex footer, hook lines,
//! double-answer dedupe, unanswered-transcript rejection). It is separate from
//! [`crate::review::parse`], which keeps Go's CLI behavior for output parity.

use std::collections::HashMap;

use serde::de::DeserializeOwned;
use serde_json::Value;
use serde_json::value::RawValue;

use crate::logfmt;

#[cfg(test)]
mod tests;

/// One finding of a reviewer or plan payload. Missing or null strings decode
/// as "", missing or null ints as 0. A value of the wrong type is a decode
/// error.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Finding {
    pub file: String,
    pub line: i64,
    pub severity: String,
    pub category: String,
    pub title: String,
    pub body: String,
    /// None when the payload has none or it is empty.
    pub failure_scenario: Option<String>,
    /// None when the payload has none or it is empty.
    pub suggestion: Option<String>,
    pub confidence: i64,
}

/// The findings of one severity bucket, as the Result view lists them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeverityGroup {
    /// "critical", "high", "medium", "low", or "other" for anything else.
    pub severity: String,
    pub findings: Vec<Finding>,
}

/// What the Result view shows for a finished run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunResult {
    /// A JSON payload. `rating` is set for plan and antislop payloads, None
    /// for a code review. `groups` are placeholder-free, sorted and bucketed.
    Findings {
        summary: String,
        rating: Option<u8>,
        groups: Vec<SeverityGroup>,
    },
    /// A prose answer, sanitized for display.
    Markdown { text: String },
    /// No usable answer; the reason.
    Failed { reason: String },
}

/// The canonical severity ladder, most severe first.
pub const SEVERITY_NAMES: [&str; 4] = ["critical", "high", "medium", "low"];

/// The group name for severities off the ladder.
const OTHER_SEVERITY: &str = "other";

/// Critical 0, high 1, medium 2, low 3, anything else 4. Case-insensitive.
pub fn severity_rank(s: &str) -> usize {
    let lower = s.to_lowercase();
    SEVERITY_NAMES
        .iter()
        .position(|n| *n == lower)
        .unwrap_or(SEVERITY_NAMES.len())
}

/// `findings` ordered by severity (critical first), then confidence (highest
/// first). Stable: equal findings keep the model's order.
pub fn sorted_findings(mut findings: Vec<Finding>) -> Vec<Finding> {
    findings.sort_by(|a, b| {
        severity_rank(&a.severity)
            .cmp(&severity_rank(&b.severity))
            .then(b.confidence.cmp(&a.confidence))
    });
    findings
}

/// `findings` bucketed by severity in ladder order, unknown severities last
/// as "other". Empty buckets are left out; order inside a bucket is kept.
pub fn severity_groups(findings: Vec<Finding>) -> Vec<SeverityGroup> {
    let mut buckets: Vec<Vec<Finding>> = vec![Vec::new(); SEVERITY_NAMES.len() + 1];
    for f in findings {
        buckets[severity_rank(&f.severity)].push(f);
    }
    SEVERITY_NAMES
        .iter()
        .chain(std::iter::once(&OTHER_SEVERITY))
        .zip(buckets)
        .filter(|(_, fs)| !fs.is_empty())
        .map(|(name, fs)| SeverityGroup {
            severity: (*name).to_string(),
            findings: fs,
        })
        .collect()
}

// Final answer

/// The part of a provider log that holds the model's final answer.
///
/// - No line that is exactly "codex": the whole text.
/// - Else the lines after the last such line. Codex streams the answer there,
///   then prints it again, clean, and puts a "tokens used" + count footer and
///   `hook: …` status lines somewhere among them (stdout and stderr share the
///   log, so the footer can land inside the second copy). The footer and the
///   hook lines are dropped. When the rest is the same block twice, one copy
///   is the answer; else all of it is.
pub fn final_answer(raw: &str) -> String {
    let lines: Vec<&str> = raw.split('\n').collect();
    let Some(header) = lines.iter().rposition(|l| *l == "codex") else {
        return raw.to_string();
    };

    let mut kept: Vec<&str> = Vec::new();
    let mut footer_seen = false;
    let mut i = header + 1;
    while i < lines.len() {
        let line = lines[i];
        let t = trim_spaces(line);
        if !footer_seen && t == "tokens used" && i + 1 < lines.len() && is_token_count(lines[i + 1])
        {
            footer_seen = true;
            i += 2;
            continue;
        }
        if !footer_seen
            && let Some(count) = t.strip_prefix("tokens used ")
            && is_token_count(count)
        {
            footer_seen = true;
            i += 1;
            continue;
        }
        if !is_hook_line(line) {
            kept.push(line);
        }
        i += 1;
    }

    let blank = |l: &&str| l.chars().all(char::is_whitespace);
    let start = kept.iter().position(|l| !blank(l)).unwrap_or(kept.len());
    let end = kept
        .iter()
        .rposition(|l| !blank(l))
        .map_or(start, |p| p + 1);
    let mut body = &kept[start..end];
    let half = body.len() / 2;
    if half > 0 && body.len().is_multiple_of(2) && body[..half] == body[half..] {
        body = &body[..half];
    }
    body.join("\n")
}

/// Trims Unicode space separators and tabs, but not line breaks (Foundation
/// `CharacterSet.whitespaces`).
fn trim_spaces(s: &str) -> &str {
    s.trim_matches(|c: char| {
        c.is_whitespace()
            && !matches!(
                c,
                '\n' | '\r' | '\u{0B}' | '\u{0C}' | '\u{85}' | '\u{2028}' | '\u{2029}'
            )
    })
}

/// A token count: digits with "," or "." grouping ("78,402", "117.735").
fn is_token_count(s: &str) -> bool {
    let t = trim_spaces(s);
    t.starts_with(|c: char| c.is_ascii_digit())
        && t.chars()
            .all(|c| c.is_ascii_digit() || c == ',' || c == '.')
}

/// Codex's hook status lines: "hook: Stop", "hook: PreToolUse Completed".
fn is_hook_line(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("hook: ") else {
        return false;
    };
    let words: Vec<&str> = rest.split(' ').collect();
    let name = words[0];
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    words.len() == 1 || (words.len() == 2 && words[1] == "Completed")
}

// JSON objects

/// Every balanced, valid JSON object in `s`, in closing-brace order, including
/// objects nested inside larger non-JSON brace spans.
///
/// One forward pass over the bytes with a stack of open-brace positions.
/// Outside any object only "{" matters, so stray quotes in prose can't desync
/// the scan. String and escape state is tracked only inside an object. Every
/// closing brace tests the span it closes for validity.
pub fn json_objects(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut in_str = false;
    let mut esc = false;
    for (i, &c) in s.as_bytes().iter().enumerate() {
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
        match c {
            b'\\' => {
                if in_str {
                    esc = true;
                }
            }
            b'"' => in_str = !in_str,
            b'{' if !in_str => stack.push(i),
            b'}' if !in_str => {
                let start = stack.pop().expect("stack is non-empty");
                // Both ends are ASCII braces, so the span is on char boundaries.
                let candidate = &s[start..=i];
                if serde_json::from_str::<Value>(candidate).is_ok() {
                    out.push(candidate);
                }
            }
            _ => {}
        }
    }
    out
}

// Result

/// The schema example summaries from the prompt contracts. A real answer
/// never carries them verbatim.
const REVIEWER_EXAMPLE_SUMMARY: &str = "1-3 sentence reviewer summary";
const PLAN_EXAMPLE_SUMMARY: &str = "1-3 sentence overall assessment of the plan";

const NO_ANSWER: &str = "no answer in the log";

/// A finding copied from a schema example. Exact literals only: real dual
/// categories like "bug|security" stay.
pub fn is_placeholder_finding(f: &Finding) -> bool {
    match f.category.as_str() {
        "bug|security|performance|concurrency|architecture|tests|ux"
        | "bug|gap|ambiguity|scope|verification"
        | "reuse|simplify|efficiency|altitude|compat|reinvention|slop|yagni" => true,
        _ => f.file == "path/to/file" || f.severity == "critical|high|medium|low",
    }
}

struct Payload {
    summary: String,
    rating: i64,
    findings: Vec<Finding>,
}

/// Extracts a run's answer from the raw (unsanitized) log tail:
///
/// 1. [`final_answer`] of `raw`.
/// 2. The last JSON object with summary, rating and findings keys, a 1..=10
///    rating and not the plan schema example → `Findings` with a rating.
/// 3. Else the last object with summary and findings keys, no rating key, not
///    the reviewer schema example → `Findings` without a rating. An object
///    with a rating outside 1..=10 is rejected, not demoted to a review.
/// 4. Else, when the answer looks like JSON (starts with "{", or a ```json
///    fence) → `Failed` with the decode reason.
/// 5. Else a non-empty sanitized answer → `Markdown`, unless the log is a
///    codex transcript with no answer header: that text is the echoed prompt
///    and tool output, not an answer.
/// 6. Else `Failed("no answer in the log")`.
pub fn parse_run_result(raw: &str) -> RunResult {
    let answer = final_answer(raw);
    // A codex transcript with no answer header holds only the echoed prompt
    // and tool output. The prompt's own examples (the clean-review
    // {"summary": "No issues found.", "findings": []}) must never pass as the
    // answer, so this check comes before the JSON scan.
    if answer == raw && is_codex_transcript(raw) {
        return failed(NO_ANSWER.to_string());
    }

    // Key presence is real presence: a null value still counts.
    let mut candidates: Vec<(Fields, bool)> = Vec::new();
    for obj in json_objects(&answer) {
        let Ok(map) = serde_json::from_str::<Fields>(obj) else {
            continue;
        };
        if !map.contains_key("summary") || !map.contains_key("findings") {
            continue;
        }
        let has_rating = map.contains_key("rating");
        candidates.push((map, has_rating));
    }

    let mut last_error: Option<String> = None;
    let mut decode = |map: &Fields| match decode_payload(map) {
        Ok(p) => Some(p),
        Err(e) => {
            last_error = Some(e);
            None
        }
    };

    for (map, _) in candidates.iter().rev().filter(|(_, r)| *r) {
        let Some(p) = decode(map) else { continue };
        if p.summary.trim() == PLAN_EXAMPLE_SUMMARY || !(1..=10).contains(&p.rating) {
            continue;
        }
        let rating = u8::try_from(p.rating).ok();
        return findings_result(p.summary, rating, p.findings);
    }
    for (map, _) in candidates.iter().rev().filter(|(_, r)| !*r) {
        let Some(p) = decode(map) else { continue };
        if p.summary.trim() == REVIEWER_EXAMPLE_SUMMARY {
            continue;
        }
        return findings_result(p.summary, None, p.findings);
    }

    let trimmed = answer.trim();
    if let Some(json) = json_looking(trimmed) {
        let reason = last_error
            .or_else(|| {
                serde_json::from_str::<Value>(json)
                    .err()
                    .map(|e| e.to_string())
            })
            .unwrap_or_else(|| "no summary/findings keys".to_string());
        return failed(format!("JSON answer did not decode: {reason}"));
    }
    let text = logfmt::expand_tabs(&logfmt::sanitize(&answer), logfmt::TAB_WIDTH);
    let text = text.trim();
    if text.is_empty() {
        return failed(NO_ANSWER.to_string());
    }
    RunResult::Markdown {
        text: text.to_string(),
    }
}

fn failed(reason: String) -> RunResult {
    RunResult::Failed { reason }
}

fn findings_result(summary: String, rating: Option<u8>, findings: Vec<Finding>) -> RunResult {
    let real = findings
        .into_iter()
        .filter(|f| !is_placeholder_finding(f))
        .collect();
    RunResult::Findings {
        summary,
        rating,
        groups: severity_groups(sorted_findings(real)),
    }
}

/// A codex log: it opens with the "OpenAI Codex" banner, or (a tail that cut
/// the banner) has an "exec" tool line.
fn is_codex_transcript(raw: &str) -> bool {
    raw.trim_start().starts_with("OpenAI Codex") || raw.split('\n').any(|l| l == "exec")
}

/// The JSON body of an answer that looks like JSON: it starts with "{", or
/// with a ```json fence followed by "{". None otherwise.
fn json_looking(s: &str) -> Option<&str> {
    if s.starts_with('{') {
        return Some(s);
    }
    if !s.starts_with("```") {
        return None;
    }
    let mut body = s.split_once('\n').map_or("", |(_, rest)| rest);
    if let Some(stripped) = body.strip_suffix("```") {
        body = stripped;
    }
    let t = body.trim_start();
    t.starts_with('{').then_some(t)
}

// Payload decoding. Mirrors Swift `JSONDecoder` with `decodeIfPresent`:
// missing or null → default, wrong type → "<path>: <reason>".

/// One JSON object's members. Values stay source tokens: integers parse
/// exactly, and other numbers go through Rust's correctly rounded f64 parser
/// (serde_json without `float_roundtrip` parses floats best-effort).
type Fields = HashMap<String, Box<RawValue>>;

fn decode_payload(map: &Fields) -> Result<Payload, String> {
    Ok(Payload {
        summary: decode_str(map, "", "summary")?,
        rating: decode_int(map, "", "rating")?,
        findings: decode_findings(map)?,
    })
}

fn decode_findings(map: &Fields) -> Result<Vec<Finding>, String> {
    let Some(t) = token(map, "findings") else {
        return Ok(Vec::new());
    };
    if !t.starts_with('[') {
        return Err(mismatch("findings", "Array<Any>", &parse(t, "findings")?));
    }
    let items: Vec<Box<RawValue>> = parse(t, "findings")?;
    items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let path = format!("findings.{i}");
            let t = item.get();
            if t.starts_with('{') {
                decode_finding(&parse(t, &path)?, &path)
            } else {
                Err(mismatch(
                    &path,
                    "Dictionary<String, Any>",
                    &parse(t, &path)?,
                ))
            }
        })
        .collect()
}

fn decode_finding(m: &Fields, path: &str) -> Result<Finding, String> {
    let opt = |key: &str| -> Result<Option<String>, String> {
        let s = decode_str(m, path, key)?;
        Ok((!s.is_empty()).then_some(s))
    };
    Ok(Finding {
        file: decode_str(m, path, "file")?,
        line: decode_int(m, path, "line")?,
        severity: decode_str(m, path, "severity")?,
        category: decode_str(m, path, "category")?,
        title: decode_str(m, path, "title")?,
        body: decode_str(m, path, "body")?,
        failure_scenario: opt("failure_scenario")?,
        suggestion: opt("suggestion")?,
        confidence: decode_int(m, path, "confidence")?,
    })
}

fn key_path(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}.{key}")
    }
}

/// The source token of `key`, or None when it is missing or null.
fn token<'a>(m: &'a Fields, key: &str) -> Option<&'a str> {
    m.get(key).map(|v| v.get()).filter(|t| *t != "null")
}

/// Decodes a token of an already validated object; `path` names it on error.
fn parse<T: DeserializeOwned>(token: &str, path: &str) -> Result<T, String> {
    serde_json::from_str(token).map_err(|e| format!("{path}: {e}"))
}

fn decode_str(m: &Fields, path: &str, key: &str) -> Result<String, String> {
    let Some(t) = token(m, key) else {
        return Ok(String::new());
    };
    let path = key_path(path, key);
    match parse(t, &path)? {
        Value::String(s) => Ok(s),
        other => Err(mismatch(&path, "String", &other)),
    }
}

/// A number token as i64, the way Foundation's `JSONDecoder` reads `Int`: a
/// plain integer that fits is exact; any other token is read as f64 and must
/// be integral and inside [-2^63, 2^63). So "1.0", "10e-1", "1e-400" and
/// plain "-9223372036854775809" (rounds to i64::MIN) decode, while
/// "9223372036854775808" and "9223372036854775807.0" (both round to 2^63)
/// do not.
fn decode_int(m: &Fields, path: &str, key: &str) -> Result<i64, String> {
    // 2^63 as f64: the first value past i64::MAX.
    const LIMIT: f64 = 9_223_372_036_854_775_808.0;
    let Some(t) = token(m, key) else {
        return Ok(0);
    };
    let path = key_path(path, key);
    if !t.starts_with(|c: char| c == '-' || c.is_ascii_digit()) {
        return Err(mismatch(&path, "Int", &parse(t, &path)?));
    }
    t.parse::<i64>()
        .ok()
        .or_else(|| {
            t.parse::<f64>()
                .ok()
                .filter(|f| f.fract() == 0.0 && (-LIMIT..LIMIT).contains(f))
                .map(|f| f as i64)
        })
        .ok_or_else(|| format!("{path}: Number {t} is not representable as Int."))
}

fn mismatch(path: &str, want: &str, got: &Value) -> String {
    let found = match got {
        Value::String(_) => "a string",
        Value::Number(_) => "number",
        Value::Bool(_) => "bool",
        Value::Null => "null",
        Value::Object(_) => "a dictionary",
        Value::Array(_) => "an array",
    };
    format!("{path}: Expected to decode {want} but found {found} instead.")
}
