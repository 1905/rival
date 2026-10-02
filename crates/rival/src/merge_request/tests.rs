//! Go: `cmd/mr_guard_test.go`, plus the `prepareReviewTarget` contract the
//! `review_output_test.go` MR cases rely on (`fakeMR`). The full command
//! paths (`runCommandWith`) arrive with the command tree in P3.

use super::*;

use std::cell::Cell;
use std::collections::HashMap;
use std::path::Path;

use rival_core::paths::Paths;

const TEST_MR_URL: &str = "https://gitlab.example.com/team/app/-/merge_requests/42";

fn cfg(home: &Path) -> Config {
    Config::new(Paths::from_home(home), HashMap::new(), None).with_environ(Vec::new())
}

// ---- Go TestModelCommandRejectsMRInRawPrompt ----

#[test]
fn model_command_rejects_mr_in_raw_prompt() {
    let err = reject_unresolved_mr(&format!("сделай ревью МР {TEST_MR_URL}")).unwrap_err();
    assert!(err.contains("no reviewer was started"), "{err}");
    assert!(err.contains("rival command codex review <MR-URL>"), "{err}");
    assert_eq!(
        err,
        "GitLab MR URLs need a pinned review: use rival command codex review <MR-URL> (or /rival-codex review <MR-URL>) from a repository with the MR's remote; no reviewer was started"
    );
    // Any MR-shaped text counts, even a malformed URL.
    assert!(reject_unresolved_mr("see /-/merge_requests/ for details").is_err());
    assert_eq!(reject_unresolved_mr("explain the auth flow"), Ok(()));
}

/// Go `fakeMR`: the resolver returns a snapshot in a temp dir.
fn fake_snapshot(dir: &Path) -> Snapshot {
    Snapshot::new(
        dir.to_str().unwrap().to_string(),
        "PINNED-SNAPSHOT-SCOPE with the patch".to_string(),
        format!("GitLab MR: {TEST_MR_URL}"),
    )
}

#[test]
fn mr_scope_runs_in_the_snapshot_and_prints_identity_first() {
    let tmp = tempfile::tempdir().unwrap();
    let snapshot_dir = tmp.path().join("rival-mr-fake");
    std::fs::create_dir(&snapshot_dir).unwrap();
    let calls = Cell::new(0);
    let mut out = Vec::new();
    let mut target = prepare_review_target_with(
        &Context::background(),
        &cfg(tmp.path()),
        TEST_MR_URL,
        "/caller",
        &mut out,
        |_, _, scope, workdir| {
            calls.set(calls.get() + 1);
            assert_eq!((scope, workdir), (TEST_MR_URL, "/caller"));
            Ok(Some(fake_snapshot(&snapshot_dir)))
        },
    )
    .unwrap();
    assert_eq!(calls.get(), 1);
    assert_eq!(
        String::from_utf8(out).unwrap(),
        format!("GitLab MR: {TEST_MR_URL}\n\n")
    );
    assert_eq!(target.scope, "PINNED-SNAPSHOT-SCOPE with the patch");
    assert_eq!(target.display, TEST_MR_URL);
    assert_eq!(target.workdir, snapshot_dir.to_str().unwrap());
    // The checkout lives until close, and close is idempotent.
    assert!(snapshot_dir.exists());
    target.close();
    assert!(!snapshot_dir.exists(), "MR checkout was not closed");
    target.close();

    // Dropping the target also removes the checkout.
    std::fs::create_dir(&snapshot_dir).unwrap();
    let target = prepare_review_target_with(
        &Context::background(),
        &cfg(tmp.path()),
        TEST_MR_URL,
        "/caller",
        &mut Vec::new(),
        |_, _, _, _| Ok(Some(fake_snapshot(&snapshot_dir))),
    )
    .unwrap();
    drop(target);
    assert!(!snapshot_dir.exists());
}

#[test]
fn plain_scope_skips_the_mr_resolver() {
    let tmp = tempfile::tempdir().unwrap();
    let mut out = Vec::new();
    let target = prepare_review_target_with(
        &Context::background(),
        &cfg(tmp.path()),
        "src/",
        "/caller",
        &mut out,
        |_, _, _, _| panic!("resolver called for a plain scope"),
    )
    .unwrap();
    assert!(out.is_empty());
    assert_eq!(
        (
            target.scope.as_str(),
            target.display.as_str(),
            target.workdir.as_str()
        ),
        ("src/", "src/", "/caller")
    );
}

#[test]
fn resolver_none_and_errors_pass_through() {
    let tmp = tempfile::tempdir().unwrap();
    let mut out = Vec::new();
    let target = prepare_review_target_with(
        &Context::background(),
        &cfg(tmp.path()),
        TEST_MR_URL,
        "/caller",
        &mut out,
        |_, _, _, _| Ok(None),
    )
    .unwrap();
    assert_eq!(
        (
            target.scope.as_str(),
            target.display.as_str(),
            target.workdir.as_str()
        ),
        (TEST_MR_URL, TEST_MR_URL, "/caller")
    );
    let err = prepare_review_target_with(
        &Context::background(),
        &cfg(tmp.path()),
        TEST_MR_URL,
        "/caller",
        &mut out,
        |_, _, _, _| {
            Err(
                "GitLab response does not identify the requested MR; refusing to review"
                    .to_string(),
            )
        },
    )
    .unwrap_err();
    assert_eq!(
        err,
        "GitLab response does not identify the requested MR; refusing to review"
    );
    assert!(out.is_empty(), "printed an identity for a failed resolve");
}

/// The real resolver rejects a malformed MR scope before any git or glab.
#[test]
fn real_resolver_rejects_malformed_mr_scope() {
    let tmp = tempfile::tempdir().unwrap();
    let mut out = Vec::new();
    let err = prepare_review_target_with(
        &Context::background(),
        &cfg(tmp.path()),
        "review https://gitlab.example.com/team/app/-/merge_requests/42",
        "/does-not-exist",
        &mut out,
        mergerequest::prepare,
    )
    .unwrap_err();
    assert_eq!(
        err,
        "use one HTTPS GitLab merge request URL as the entire review scope"
    );
    assert!(out.is_empty());
}
