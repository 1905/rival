//! Reviewer prompts, output parsing and console formatting, plus plan and
//! doc review runs.
//!
//! [`parse_reviewer_log`] and [`parse_plan_log`] use the shared answer
//! finder and decoder in [`crate::result`], so the CLI and the TUI read a log
//! the same way, and review-shaped JSON a tool printed earlier is never taken
//! for the answer.

mod format;
mod language;
mod parse;
mod plan;
mod planrun;
mod prompt;
mod security;
mod slots;
#[cfg(test)]
mod testutil;
mod types;

pub use format::{DEFAULT_CONFIDENCE_THRESHOLD, format_review_console, format_review_result};
pub use language::repair_language;
pub use parse::parse_reviewer_log;
pub use plan::{
    PlanCLIResult, PlanOutput, PlanRunResult, format_plan_console, format_plan_multi_console,
    format_plan_result, parse_plan_log,
};
pub use planrun::{
    DocReview, ReviewBatch, run_doc_review, run_failure_reason, run_plan_review,
    run_timeout_reason, with_run_timeout,
};
pub use prompt::build_reviewer_prompt;
pub use security::{format_security_console, format_security_result, validate_security_result};
pub use slots::{GroupSlot, SkippedCLI, SlotRelease, format_skipped, wait_for_group_slot};
pub use types::{ReviewerFinding, ReviewerOutput};
