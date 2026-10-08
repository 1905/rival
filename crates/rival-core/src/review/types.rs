//! Reviewer payload types and their JSON decoding.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};

use crate::json;

/// The structured JSON every reviewer must emit.
///
/// Decoding: keys match exactly, unknown keys are ignored, and `null`
/// reads as the default (a `null` finding is an empty one).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewerOutput {
    #[serde(deserialize_with = "json::nullable")]
    pub summary: String,
    #[serde(deserialize_with = "findings")]
    pub findings: Vec<ReviewerFinding>,
}

/// A single finding from one reviewer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewerFinding {
    #[serde(deserialize_with = "json::nullable")]
    pub file: String,
    #[serde(deserialize_with = "json::nullable")]
    pub line: i64,
    #[serde(deserialize_with = "json::nullable")]
    pub severity: String,
    #[serde(deserialize_with = "json::nullable")]
    pub category: String,
    #[serde(deserialize_with = "json::nullable")]
    pub title: String,
    #[serde(deserialize_with = "json::nullable")]
    pub body: String,
    /// The input or state that triggers the defect and the wrong result.
    /// Older payloads and the plan schema do not carry it.
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "json::nullable"
    )]
    pub failure_scenario: String,
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "json::nullable"
    )]
    pub suggestion: String,
    #[serde(deserialize_with = "json::nullable")]
    pub confidence: i64,
}

/// Reads a findings array. `null` is an empty list and a `null` element is
/// an empty finding.
pub(crate) fn findings<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<ReviewerFinding>, D::Error> {
    let items: Option<Vec<Option<json::Object<ReviewerFinding>>>> = Option::deserialize(d)?;
    Ok(items
        .unwrap_or_default()
        .into_iter()
        .map(|f| f.unwrap_or_default().0)
        .collect())
}

/// Decodes one payload candidate, [`ReviewerOutput`] or the plan output.
pub(crate) fn decode_payload<T: DeserializeOwned>(data: &str) -> Result<T, String> {
    json::decode(data.as_bytes()).map_err(|e| e.to_string())
}
