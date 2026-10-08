//! Reviewer payload types. [`crate::result`] decodes them.

use serde::Serialize;

use crate::result::{Finding, Payload};

/// The structured JSON every reviewer must emit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ReviewerOutput {
    pub summary: String,
    pub findings: Vec<ReviewerFinding>,
}

/// A single finding from one reviewer.
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

impl From<Finding> for ReviewerFinding {
    fn from(f: Finding) -> Self {
        ReviewerFinding {
            file: f.file,
            line: f.line,
            severity: f.severity,
            category: f.category,
            title: f.title,
            body: f.body,
            failure_scenario: f.failure_scenario.unwrap_or_default(),
            suggestion: f.suggestion.unwrap_or_default(),
            confidence: f.confidence,
        }
    }
}

/// The payload's summary and findings; a plan rating is dropped.
impl From<Payload> for ReviewerOutput {
    fn from(p: Payload) -> Self {
        ReviewerOutput {
            summary: p.summary,
            findings: p.findings.into_iter().map(ReviewerFinding::from).collect(),
        }
    }
}
