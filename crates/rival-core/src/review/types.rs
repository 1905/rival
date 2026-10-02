//! Reviewer payload types and their Go-style JSON decoding. Go:
//! `internal/review/types.go` plus what `json.Unmarshal` does with them.

use serde::Serialize;
use serde_json::value::RawValue;

use crate::gojson::{self, TypeMismatch};

/// The structured JSON every reviewer must emit.
///
/// Go keeps a nil `Findings` apart from an empty one; it shows only in
/// `json.Marshal` output, which no command prints. Here both are empty.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ReviewerOutput {
    pub summary: String,
    pub findings: Vec<ReviewerFinding>,
}

/// A single finding from one reviewer. Go `int` fields are `i64`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ReviewerFinding {
    pub file: String,
    pub line: i64,
    pub severity: String,
    pub category: String,
    pub title: String,
    pub body: String,
    /// The input or state that triggers the defect and the wrong result.
    /// Older payloads and the plan schema do not carry it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub failure_scenario: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub suggestion: String,
    pub confidence: i64,
}

/// The JSON names of [`ReviewerFinding`], for Go's field lookup.
const FINDING_FIELDS: [&str; 9] = [
    "file",
    "line",
    "severity",
    "category",
    "title",
    "body",
    "failure_scenario",
    "suggestion",
    "confidence",
];

/// A top-level payload as Go's `json.Unmarshal` leaves it. `rating` stays
/// 0 unless the caller lists the field.
pub(crate) struct Payload {
    pub summary: String,
    pub rating: i64,
    pub findings: Vec<ReviewerFinding>,
}

/// Go: `json.Unmarshal(data, &v)` for `ReviewerOutput` or `PlanOutput`.
/// `go_struct` names the struct in error text; `fields` are its JSON names.
/// Keys match exactly, else case-insensitively; duplicate keys assign in
/// order; unknown keys are ignored; `null` keeps a value. The first type
/// mismatch is returned after the whole object is decoded, as Go does.
pub(crate) fn decode_payload(
    data: &str,
    go_struct: &str,
    fields: &[&'static str],
) -> Result<Payload, String> {
    let mut out = Payload {
        summary: String::new(),
        rating: 0,
        findings: Vec::new(),
    };
    let mut findings = GoFindings::default();
    let mut saved = None;
    let members = gojson::decode_object(data.as_bytes(), &format!("review.{go_struct}"))?;
    for (key, raw) in members {
        match gojson::match_field(fields, &key) {
            Some("summary") => {
                if let Err(m) = set_string(&mut out.summary, &raw) {
                    save(&mut saved, m, go_struct, "summary", "string");
                }
            }
            Some("rating") => {
                if let Err(m) = set_int(&mut out.rating, &raw) {
                    save(&mut saved, m, go_struct, "rating", "int");
                }
            }
            Some("findings") => findings.decode(&raw, go_struct, &mut saved),
            _ => {}
        }
    }
    out.findings = findings.into_vec();
    saved.map_or(Ok(out), Err)
}

/// Keeps the first error, as Go's `d.saveError` does. `field` is Go's
/// field stack joined with dots.
fn save(saved: &mut Option<String>, m: TypeMismatch, go_struct: &str, field: &str, ty: &str) {
    if saved.is_none() {
        *saved = Some(format!(
            "json: cannot unmarshal {} into Go struct field {go_struct}.{field} of type {ty}",
            m.0
        ));
    }
}

/// Go: decoding into a string field; `null` keeps the value.
fn set_string(dst: &mut String, raw: &RawValue) -> Result<(), TypeMismatch> {
    if let Some(v) = gojson::decode_string(raw)? {
        *dst = v;
    }
    Ok(())
}

/// Go: decoding into an `int` field; `null` keeps the value.
fn set_int(dst: &mut i64, raw: &RawValue) -> Result<(), TypeMismatch> {
    if let Some(v) = gojson::decode_int(raw)? {
        *dst = v;
    }
    Ok(())
}

/// The `Value` word of Go's `UnmarshalTypeError` for a non-null value that
/// does not fit the target kind.
fn value_kind(raw: &RawValue) -> &'static str {
    match raw.get().as_bytes().first() {
        Some(b'"') => "string",
        Some(b'{') => "object",
        Some(b'[') => "array",
        Some(b't' | b'f') => "bool",
        _ => "number",
    }
}

/// Go: decoding into an element of `[]ReviewerFinding`. The element may
/// already hold an earlier decode (a duplicate `findings` key, or a slot a
/// shorter array truncated), and the object's fields assign over it.
fn decode_finding_into(
    f: &mut ReviewerFinding,
    raw: &RawValue,
    go_struct: &str,
    saved: &mut Option<String>,
) {
    match raw.get().as_bytes().first() {
        // Go ignores null for a struct.
        Some(b'n') => return,
        Some(b'{') => {}
        _ => {
            let m = TypeMismatch(value_kind(raw).to_string());
            save(saved, m, go_struct, "findings", "review.ReviewerFinding");
            return;
        }
    }
    let members = match gojson::decode_object(raw.get().as_bytes(), "review.ReviewerFinding") {
        Ok(members) => members,
        Err(e) => {
            // Unreachable for a candidate that passed `valid_value`.
            saved.get_or_insert(e);
            return;
        }
    };
    for (key, raw) in members {
        let Some(name) = gojson::match_field(&FINDING_FIELDS, &key) else {
            continue;
        };
        let (result, ty) = match name {
            "file" => (set_string(&mut f.file, &raw), "string"),
            "line" => (set_int(&mut f.line, &raw), "int"),
            "severity" => (set_string(&mut f.severity, &raw), "string"),
            "category" => (set_string(&mut f.category, &raw), "string"),
            "title" => (set_string(&mut f.title, &raw), "string"),
            "body" => (set_string(&mut f.body, &raw), "string"),
            "failure_scenario" => (set_string(&mut f.failure_scenario, &raw), "string"),
            "suggestion" => (set_string(&mut f.suggestion, &raw), "string"),
            "confidence" => (set_int(&mut f.confidence, &raw), "int"),
            _ => unreachable!("{name} is not in FINDING_FIELDS"),
        };
        if let Err(m) = result {
            // Go names the innermost struct and the whole field stack.
            save(saved, m, "ReviewerFinding", &format!("findings.{name}"), ty);
        }
    }
}

/// Go: decoding into a `[]ReviewerFinding` field, which reuses the slice it
/// already holds. A duplicate key decodes into the earlier array's elements,
/// and truncated elements stay in the backing array until a later, longer
/// array exposes them again. `null` makes the slice nil; `[]` makes a fresh
/// empty one.
#[derive(Default)]
struct GoFindings {
    /// Every element decoded since the slice was last cleared; the slice is
    /// the first `len` of them.
    backing: Vec<ReviewerFinding>,
    len: usize,
}

impl GoFindings {
    fn decode(&mut self, raw: &RawValue, go_struct: &str, saved: &mut Option<String>) {
        match raw.get().as_bytes().first() {
            Some(b'n') => *self = GoFindings::default(),
            Some(b'[') => {
                let Ok(items) = serde_json::from_str::<Vec<Box<RawValue>>>(raw.get()) else {
                    // Unreachable for a candidate that passed `valid_value`.
                    let m = TypeMismatch("array".to_string());
                    save(saved, m, go_struct, "findings", "[]review.ReviewerFinding");
                    return;
                };
                for (i, item) in items.iter().enumerate() {
                    // Go grows only once every slot below capacity is exposed,
                    // so a new slot is always zero.
                    if i == self.backing.len() {
                        self.backing.push(ReviewerFinding::default());
                    }
                    self.len = self.len.max(i + 1);
                    decode_finding_into(&mut self.backing[i], item, go_struct, saved);
                }
                self.len = self.len.min(items.len());
                if items.is_empty() {
                    *self = GoFindings::default();
                }
            }
            _ => {
                let m = TypeMismatch(value_kind(raw).to_string());
                save(saved, m, go_struct, "findings", "[]review.ReviewerFinding");
            }
        }
    }

    fn into_vec(mut self) -> Vec<ReviewerFinding> {
        self.backing.truncate(self.len);
        self.backing
    }
}
