//! GitLab MR review targets for the commands.

use std::io::Write;

use rival_core::cancel::Context;
use rival_core::config::Config;
use rival_core::logging;
use rival_core::mergerequest::{self, Snapshot};

#[cfg(test)]
mod tests;

/// Where a single-model review runs and what it reviews. For an MR it owns
/// the temporary checkout: [`ReviewTarget::close`] (or dropping the target)
/// removes it, so keep the target alive until the run ends.
#[derive(Debug)]
pub struct ReviewTarget {
    /// What the prompt reviews; for an MR, the snapshot scope with the patch.
    pub scope: String,
    /// What the session records and the output shows; for an MR, the URL.
    pub display: String,
    /// Where the reviewer runs; for an MR, the temporary checkout.
    pub workdir: String,
    snapshot: Option<Snapshot>,
}

impl ReviewTarget {
    /// Removes the MR checkout and logs a failure;
    /// a no-op otherwise, and on every later call.
    pub fn close(&mut self) {
        if let Some(mut snapshot) = self.snapshot.take()
            && let Err(e) = snapshot.close()
        {
            logging::warn()
                .err(e)
                .str("path", snapshot.workdir.as_str())
                .msg("remove MR checkout");
        }
    }
}

impl Drop for ReviewTarget {
    fn drop(&mut self) {
        self.close();
    }
}

#[allow(
    dead_code,
    reason = "commands inject the resolver; kept for Task 3.2 callers"
)]
/// [`prepare_review_target_with`] with the real resolver
/// ([`mergerequest::prepare`]) and stdout.
pub fn prepare_review_target(
    ctx: &Context,
    cfg: &Config,
    scope: &str,
    workdir: &str,
) -> Result<ReviewTarget, String> {
    prepare_review_target_with(
        ctx,
        cfg,
        scope,
        workdir,
        &mut std::io::stdout(),
        mergerequest::prepare,
    )
}

/// Pins a GitLab MR scope to a snapshot checkout
/// and prints its identity line first on `out`, followed by a blank line.
/// Any other scope passes through unchanged and `prepare` is not called.
/// `prepare` is the MR resolver, which tests replace.
pub fn prepare_review_target_with(
    ctx: &Context,
    cfg: &Config,
    scope: &str,
    workdir: &str,
    out: &mut dyn Write,
    prepare: impl FnOnce(&Context, &Config, &str, &str) -> Result<Option<Snapshot>, String>,
) -> Result<ReviewTarget, String> {
    let plain = || ReviewTarget {
        scope: scope.to_string(),
        display: scope.to_string(),
        workdir: workdir.to_string(),
        snapshot: None,
    };
    if !mergerequest::contains(scope) {
        return Ok(plain());
    }
    let Some(snapshot) = prepare(ctx, cfg, scope, workdir)? else {
        return Ok(plain());
    };
    // The identity, a blank line; a write error is ignored.
    let _ = writeln!(out, "{}\n", snapshot.identity);
    Ok(ReviewTarget {
        scope: snapshot.scope.clone(),
        display: scope.to_string(),
        workdir: snapshot.workdir.clone(),
        snapshot: Some(snapshot),
    })
}

/// Raw prompts cannot resolve remote identity
/// inside a network-isolated model, so MR URLs are rejected before any
/// reviewer starts, with a pointer to the review path, which pins the
/// checkout first.
pub fn reject_unresolved_mr(prompt: &str) -> Result<(), String> {
    if mergerequest::contains(prompt) {
        return Err("GitLab MR URLs need a pinned review: use rival command codex review <MR-URL> (or /rival-codex review <MR-URL>) from a repository with the MR's remote; no reviewer was started".to_string());
    }
    Ok(())
}
