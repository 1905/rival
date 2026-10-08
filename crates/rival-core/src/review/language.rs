//! The language pass after a code, security or plan review: the one entry
//! point the commands call. [`crate::lang::repair`] holds the check, the
//! prompt and the guard; this module runs the call and writes the log.

use std::io::Write;
use std::path::Path;

use anyhow::bail;

use super::planrun::{CancelOnDrop, with_run_timeout};
use crate::cancel::Context;
use crate::config::Config;
use crate::lang;
use crate::logging;
use crate::result::PayloadKind;
use crate::session::{self, Session};

/// Checks the review in `raw`, the text of the session log, and when the
/// check finds something makes one repair call. `kind` is
/// [`PayloadKind::Any`] for a code or security review and
/// [`PayloadKind::Plan`] for a plan review.
///
/// `run` makes the call: `(ctx, sess, prompt, log)` → the exit code. It
/// must run the same model at low effort, read-only, with `log` as its
/// output file. `ctx` carries the call's own run timeout, made from
/// `parent`. The log is the session log path plus `.repair.log`.
///
/// When the guard accepts the repair, the repaired review is appended to the
/// session log as one JSON line. Every reader takes the last review in the
/// log, so the CLI, the TUI and Rival.app show it. The result is the session
/// log text after the pass: `raw` plus the line, or `raw` unchanged.
pub fn repair_language(
    parent: &Context,
    cfg: &Config,
    sess: &mut Session,
    raw: String,
    kind: PayloadKind,
    run: impl FnOnce(&Context, &mut Session, &str, &str) -> anyhow::Result<i64>,
) -> String {
    let log = format!("{}.repair.log", sess.log_file);
    let line = lang::repair::repair(&raw, kind, |prompt| {
        let (ctx, cancel) = with_run_timeout(parent, cfg, 1);
        let _cancel = CancelOnDrop(cancel);
        let exit_code = run(&ctx, sess, prompt, &log)?;
        if exit_code != 0 {
            bail!("exited with code {exit_code}");
        }
        let out = session::read_file(Path::new(&log))?;
        Ok(String::from_utf8(out)
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()))
    });
    let Some(line) = line else {
        return raw;
    };
    let appended = format!("\n{line}\n");
    let written = session::open_append(Path::new(&sess.log_file))
        .and_then(|mut f| f.write_all(appended.as_bytes()));
    if let Err(e) = written {
        logging::debug()
            .err(e.to_string())
            .str("log", sess.log_file.as_str())
            .msg("could not append the repaired review; the review is kept");
        return raw;
    }
    raw + &appended
}
