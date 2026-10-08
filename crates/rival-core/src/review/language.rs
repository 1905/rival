//! The language pass after a code, security or plan review: the one entry
//! point the commands call.

use std::io::Write;

use crate::lang;
use crate::logging;
use crate::result::PayloadKind;

/// The log of the repair call: the session log path plus `.repair.log`.
pub fn repair_log_path(session_log: &str) -> String {
    format!("{session_log}.repair.log")
}

/// Checks the review in `raw`, the text of `session_log`, and when the check
/// finds something makes one repair call with `run_again`. `kind` is
/// [`PayloadKind::Any`] for a code or security review and
/// [`PayloadKind::Plan`] for a plan review.
///
/// `run_again` gets the repair prompt. It must run the same model at low
/// effort, read-only, with [`repair_log_path`] as its log, and return that
/// log's text. When the guard accepts the repair, the repaired review is
/// appended to the session log as one JSON line. Every reader takes the last
/// review in the log, so the CLI, the TUI and Rival.app show it.
///
/// The result is the session log text after the pass: `raw` plus the line,
/// or `raw` unchanged.
pub fn repair_language(
    session_log: &str,
    raw: String,
    kind: PayloadKind,
    run_again: impl FnOnce(&str) -> anyhow::Result<String>,
) -> String {
    let Some(line) = lang::repair::repair(&raw, kind, run_again) else {
        return raw;
    };
    let appended = format!("\n{line}\n");
    let written = std::fs::OpenOptions::new()
        .append(true)
        .open(session_log)
        .and_then(|mut f| f.write_all(appended.as_bytes()));
    if let Err(e) = written {
        logging::debug()
            .err(e.to_string())
            .str("log", session_log)
            .msg("could not append the repaired review; the review is kept");
        return raw;
    }
    raw + &appended
}
