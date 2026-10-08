//! Reviewer output parsing: the CLI side of [`crate::result`]'s shared
//! answer finder and decoder.

use anyhow::anyhow;

use super::types::ReviewerOutput;
use crate::result::{self, PayloadError, PayloadKind};

#[cfg(test)]
mod tests;

/// Extracts a reviewer's structured JSON from a provider log:
/// [`result::log_payload`], which finds the final answer
/// ([`result::final_answer`]) and takes the last genuine payload in it. A
/// plan payload's rating is dropped.
pub fn parse_reviewer_log(raw: &str) -> anyhow::Result<ReviewerOutput> {
    result::log_payload(raw, PayloadKind::Any)
        .map(ReviewerOutput::from)
        .map_err(|e| payload_error("reviewer", e))
}

/// The error text of a missing `kind` payload.
pub(crate) fn payload_error(kind: &str, e: PayloadError) -> anyhow::Error {
    match e {
        PayloadError::NoAnswer => anyhow!(result::NO_ANSWER),
        PayloadError::NotFound(None) => anyhow!("no {kind} JSON payload found in output"),
        PayloadError::NotFound(Some(e)) => {
            anyhow!("no valid {kind} JSON payload (last decode error: {e})")
        }
    }
}
