//! Reviewer prompts, output parsing and console formatting. Go:
//! `internal/review/{types,prompt,parse,review_format,slots,security}.go`
//! and `plan.go`, plus plan and doc review runs (`planrun.go`).
//!
//! Every caller that parses a provider log passes it through
//! [`final_answer`] first ([`parse_reviewer_log`], [`parse_plan_log`]), so
//! review-shaped JSON a tool printed earlier is never taken for the answer.

mod format;
mod parse;
mod plan;
mod planrun;
mod prompt;
mod security;
mod slots;
mod ste;
#[cfg(test)]
mod testutil;
mod types;

pub use format::{DEFAULT_CONFIDENCE_THRESHOLD, format_review_console, format_review_result};
pub use parse::{final_answer, parse_reviewer_log, parse_reviewer_output};
pub use plan::{
    PlanCLIResult, PlanOutput, PlanRunResult, format_antislop_result, format_plan_console,
    format_plan_multi_console, format_plan_result, parse_plan_log, parse_plan_output,
};
pub use planrun::{
    DocReview, ReviewBatch, run_doc_review, run_failure_reason, run_plan_review,
    run_timeout_reason, with_run_timeout,
};
pub use prompt::build_reviewer_prompt;
pub use security::{format_security_console, format_security_result, validate_security_result};
pub use slots::{GroupSlot, SkippedCLI, SlotRelease, format_skipped, wait_for_group_slot};
pub use ste::{
    MIN_HITS as STE_MIN_HITS, check_output as ste_check_output,
    rewrite_keeps_shape as ste_rewrite_keeps_shape, rewrite_prompt as ste_rewrite_prompt,
    to_json as ste_to_json, total as ste_total,
};
pub use types::{ReviewerFinding, ReviewerOutput};
