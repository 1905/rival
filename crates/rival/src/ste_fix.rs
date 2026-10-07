//! The second provider call of a review: rewrite findings that use words
//! from the Simplified Technical English not-approved list. Off unless
//! `ste_rewrite: true` is set in `~/.rival/config.yaml`.

use rival_core::cancel::Context;
use rival_core::config::Config;
use rival_core::logging;
use rival_core::review;
use rival_core::session::Session;

use crate::model_command::{CancelOnDrop, read_log};
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

/// Returns the log text the review is built from. It is `raw` unchanged
/// unless a rewrite ran and passed [`review::ste_accept_rewrite`]. Then it
/// is `raw` plus the rewritten review as one JSON line, and the parser
/// takes that last review. A failed or rejected rewrite leaves `raw` alone.
/// The provider's rewrite output stays in the session log either way.
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

    let Ok(data) = read_log(&sess.log_file) else {
        return raw.to_string();
    };
    let full = String::from_utf8_lossy(&data);
    // The log is append-only, so the rewrite is what follows the first run.
    let tail = full.strip_prefix(raw).unwrap_or(&full);
    let Ok(new) = review::parse_reviewer_output(review::final_answer(tail)) else {
        logging::warn().msg("review rewrite did not parse; keeping the original");
        return raw.to_string();
    };
    if !review::ste_accept_rewrite(&old, &new) {
        logging::warn()
            .int(
                "hits_after",
                review::ste_total(&review::ste_check_output(&new)) as i64,
            )
            .msg("review rewrite rejected; keeping the original");
        return raw.to_string();
    }
    logging::info()
        .int("hits_before", before as i64)
        .int(
            "hits_after",
            review::ste_total(&review::ste_check_output(&new)) as i64,
        )
        .msg("review rewrite accepted");
    format!("{raw}\n{}\n", review::ste_to_json(&new))
}
