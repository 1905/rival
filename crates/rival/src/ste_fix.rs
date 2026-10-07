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
use crate::model_specs::{ModelSpec, RunCall, session_mode};

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

/// Returns the log text the review is built from. It is `raw` unchanged
/// unless a rewrite ran and passed [`review::ste_accept_rewrite`]. Then it
/// is `raw` plus the rewritten review as one JSON line, and the parser
/// takes that last review.
///
/// The session log ends the same way, because the TUI and Rival.app read
/// the last review in it. The provider's rewrite transcript moves to
/// [`sidecar_path`], so a failed or rejected rewrite never shows up there.
pub(crate) fn refine(r: &Rerun<'_>, sess: &mut Session, raw: &str) -> String {
    if !r.cfg.ste_rewrite() {
        return raw.to_string();
    }
    let Ok(old) = review::parse_reviewer_output(review::final_answer(raw)) else {
        return raw.to_string();
    };
    let hits = review::ste_check_output(&old);
    let before = review::ste_total(&hits);
    if before < review::STE_MIN_HITS {
        return raw.to_string();
    }
    let Ok(effort) = r.spec.resolve_effort(r.cfg, "low") else {
        return raw.to_string();
    };
    let prompt = review::ste_rewrite_prompt(&old, &hits);
    // Where the first run's output ends. The provider appends after it.
    let Ok(start) = std::fs::metadata(&sess.log_file).map(|m| m.len()) else {
        return raw.to_string();
    };

    let (run_ctx, cancel) = review::with_run_timeout(r.ctx, r.cfg, 1);
    let _cancel = CancelOnDrop(cancel);
    // The first run may have replaced the mode with its transport (Claude
    // records "native" or "docker"). Adapters derive read-only permissions
    // from the mode, so the rewrite must run as a review too.
    sess.mode = session_mode(true).to_string();
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
    let tail = match split_off_rewrite(&sess.log_file, start) {
        Ok(tail) => tail,
        Err(e) => {
            logging::warn()
                .err(e)
                .msg("could not move the rewrite transcript out of the session log");
            return raw.to_string();
        }
    };
    match ran {
        Ok(res) if res.exit_code == 0 => {}
        Ok(res) => {
            logging::warn()
                .int("exit_code", res.exit_code)
                .msg("review rewrite failed; keeping the original");
            return raw.to_string();
        }
        Err(e) => {
            logging::warn()
                .err(format!("{e:#}"))
                .msg("review rewrite failed; keeping the original");
            return raw.to_string();
        }
    }

    let Ok(new) = review::parse_reviewer_output(review::final_answer(&tail)) else {
        logging::warn().msg("review rewrite did not parse; keeping the original");
        return raw.to_string();
    };
    let after = review::ste_total(&review::ste_check_output(&new));
    if !review::ste_accept_rewrite(&old, &new) {
        logging::warn()
            .int("hits_after", after as i64)
            .msg("review rewrite rejected; keeping the original");
        return raw.to_string();
    }
    let line = format!("\n{}\n", review::ste_to_json(&new));
    if let Err(e) = sess
        .open_log()
        .and_then(|mut f| f.write_all(line.as_bytes()))
    {
        // The log still ends with the original review, so the command keeps
        // it too and every reader agrees.
        logging::warn()
            .err(e)
            .msg("could not record the rewrite; keeping the original");
        return raw.to_string();
    }
    logging::info()
        .int("hits_before", before as i64)
        .int("hits_after", after as i64)
        .msg("review rewrite accepted");
    format!("{raw}{line}")
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
