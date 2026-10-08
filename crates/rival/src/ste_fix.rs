//! The second provider call of a review: rewrite findings that use words
//! from the Simplified Technical English not-approved list. Off unless
//! `ste_rewrite: true` is set in `~/.rival/config.yaml`.

use std::io::Write;

use rival_core::cancel::Context;
use rival_core::config::Config;
use rival_core::logging;
use rival_core::review;
use rival_core::session::Session;

use crate::model_command::CancelOnDrop;
use crate::model_specs::{ModelSpec, RunCall};

#[cfg(test)]
mod tests;

/// What the second call needs from the first.
pub(crate) struct Rerun<'a> {
    /// The command's own context, not the first run's timeout.
    pub ctx: &'a Context,
    pub cfg: &'a Config,
    pub spec: &'a ModelSpec,
    pub workdir: &'a str,
    pub cred_workdir: &'a str,
}

/// Returns the text to append to the log the review is built from: the
/// rewritten review as one JSON line, when a rewrite ran and was accepted.
/// The parser takes that last review. `None` keeps the original.
///
/// The session log gets the same line, because the TUI and Rival.app read
/// the last review in it. The provider's rewrite transcript moves to
/// [`sidecar_path`], so a failed or rejected rewrite never shows up there.
pub(crate) fn refine(r: &Rerun<'_>, sess: &mut Session, raw: &str) -> Option<String> {
    if !r.cfg.ste_rewrite() {
        return None;
    }
    let old = review::parse_reviewer_output(review::final_answer(raw)).ok()?;
    let hits = review::ste_check_output(&old);
    let before = review::ste_total(&hits);
    if before < review::STE_MIN_HITS {
        return None;
    }
    let effort = r.spec.resolve_effort(r.cfg, "low").ok()?;
    let prompt = review::ste_rewrite_prompt(&old, &hits);
    // Where the first run's output ends. The provider appends after it.
    let start = std::fs::metadata(&sess.log_file).ok()?.len();

    let (run_ctx, cancel) = review::with_run_timeout(r.ctx, r.cfg, 1);
    let _cancel = CancelOnDrop(cancel);
    logging::info()
        .str("session", sess.id.as_str())
        .int("hits", before as i64)
        .msg("rewriting review text");
    let ran = (r.spec.run)(RunCall {
        ctx: &run_ctx,
        cfg: r.cfg,
        sess,
        prompt: &prompt,
        effort: &effort,
        workdir: r.workdir,
        cred_workdir: r.cred_workdir,
        review: true,
        out: None,
    });
    let keep = |event: logging::Event, msg: &str| {
        event.msg(&format!("{msg}; keeping the original"));
        None
    };
    let tail = match split_off_rewrite(&sess.log_file, start) {
        Ok(tail) => tail,
        Err(e) => {
            return keep(
                logging::warn().err(e),
                "could not move the rewrite transcript out of the session log",
            );
        }
    };
    match ran {
        Ok(res) if res.exit_code == 0 => {}
        Ok(res) => {
            return keep(
                logging::warn().int("exit_code", res.exit_code),
                "review rewrite failed",
            );
        }
        Err(e) => {
            return keep(
                logging::warn().err(format!("{e:#}")),
                "review rewrite failed",
            );
        }
    }

    let Ok(new) = review::parse_reviewer_output(review::final_answer(&tail)) else {
        return keep(logging::warn(), "review rewrite did not parse");
    };
    let after = review::ste_total(&review::ste_check_output(&new));
    if after >= before || !review::ste_rewrite_keeps_shape(&old, &new) {
        return keep(
            logging::warn().int("hits_after", after as i64),
            "review rewrite rejected",
        );
    }
    let line = format!("\n{}\n", review::ste_to_json(&new));
    if let Err(e) = sess
        .open_log()
        .and_then(|mut f| f.write_all(line.as_bytes()))
    {
        // The log still ends with the original review, so the command keeps
        // it too and every reader agrees.
        return keep(logging::warn().err(e), "could not record the rewrite");
    }
    logging::info()
        .int("hits_before", before as i64)
        .int("hits_after", after as i64)
        .msg("review rewrite accepted");
    Some(line)
}

/// Where the rewrite transcript goes: the session log path plus
/// `.ste-rewrite`.
pub(crate) fn sidecar_path(log_file: &str) -> String {
    format!("{log_file}.ste-rewrite")
}

/// Cuts the session log back to byte `start` and returns what followed it.
/// The cut comes first, so the log never keeps an unchecked rewrite. The
/// cut text then goes to the sidecar; failing that write only loses the
/// transcript.
fn split_off_rewrite(log_file: &str, start: u64) -> std::io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};

    let mut log = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(log_file)?;
    log.seek(SeekFrom::Start(start))?;
    let mut tail = Vec::new();
    log.read_to_end(&mut tail)?;
    log.set_len(start)?;
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).write(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    if let Err(e) = opts
        .open(sidecar_path(log_file))
        .and_then(|mut f| f.write_all(&tail))
    {
        logging::warn()
            .err(e)
            .msg("could not save the rewrite transcript");
    }
    Ok(String::from_utf8_lossy(&tail).into_owned())
}
