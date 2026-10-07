use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

use rival_core::cancel::Context;
use rival_core::executor::RunResult;
use rival_core::result::{self, RunResult as Shown};
use rival_core::review;
use rival_core::session::{NewSession, Session};

use super::*;
use crate::testutil::{Fixture, fake_run, fake_spec};

const DIRTY: &str = r#"{"summary":"One bug.","findings":[{"file":"a.rs","line":3,"severity":"high","category":"bug","title":"Lock not held","body":"Ensure the lock is held. Verify that it is held. Ensure the write is safe.","failure_scenario":"Two threads write the data","suggestion":"Hold the lock","confidence":8}]}"#;
const CLEAN: &str = r#"{"summary":"One bug.","findings":[{"file":"a.rs","line":3,"severity":"high","category":"bug","title":"Lock not held","body":"Make sure that the lock is held. Check that it is held. Make sure the write is safe.","failure_scenario":"Two threads write the data","suggestion":"Hold the lock","confidence":8}]}"#;
const MOVED: &str = r#"{"summary":"One bug.","findings":[{"file":"a.rs","line":9,"severity":"high","category":"bug","title":"Lock not held","body":"Make sure that the lock is held. Check that it is held. Make sure the write is safe.","failure_scenario":"Two threads write the data","suggestion":"Hold the lock","confidence":8}]}"#;

struct Setup {
    fx: Fixture,
    sess: Session,
    calls: Rc<RefCell<Vec<String>>>,
    /// The session mode each provider call saw.
    modes: Rc<RefCell<Vec<String>>>,
    spec: ModelSpec,
}

/// A fixture whose provider appends `second` to the log on each call, as the
/// real provider's append-only log does. `second` of `None` fails the call.
fn setup(yaml: &str, second: Option<&str>) -> Setup {
    let fx = Fixture::with_config_yaml(yaml);
    let sess = Session::new_queued(
        fx.cfg.paths(),
        NewSession {
            cli: "codex",
            mode: "review",
            model: "m",
            effort: "low",
            workdir: ".",
            prompt: "p",
            review_scope: "",
            group_id: "",
        },
    )
    .unwrap();
    let calls = Rc::new(RefCell::new(Vec::new()));
    let modes = Rc::new(RefCell::new(Vec::new()));
    let mut spec = fake_spec(&fake_run(""));
    let seen = Rc::clone(&calls);
    let seen_modes = Rc::clone(&modes);
    let second = second.map(str::to_string);
    spec.run = Box::new(move |c| {
        seen.borrow_mut().push(c.prompt.to_string());
        seen_modes.borrow_mut().push(c.sess.mode.clone());
        assert!(c.review, "the rewrite runs read-only");
        assert!(c.out.is_none(), "the rewrite is not mirrored");
        let Some(text) = &second else {
            anyhow::bail!("provider down");
        };
        let mut f = c.sess.open_log()?;
        writeln!(f, "{text}")?;
        Ok(RunResult {
            exit_code: 0,
            output_bytes: text.len() as i64,
            output_lines: 1,
        })
    });
    Setup {
        fx,
        sess,
        calls,
        modes,
        spec,
    }
}

fn run(s: &mut Setup, raw: &str) -> String {
    let (ctx, _cancel) = Context::background().with_cancel();
    refine(
        &Rerun {
            ctx: &ctx,
            cfg: &s.fx.cfg,
            spec: &s.spec,
            workdir: ".",
            cred_workdir: ".",
        },
        &mut s.sess,
        raw,
    )
}

/// What the TUI and Rival.app show for the session log.
fn shown(s: &Setup) -> (i64, String) {
    let log = std::fs::read_to_string(&s.sess.log_file).unwrap();
    match result::parse_run_result(&log) {
        Shown::Findings { groups, .. } => {
            let f = &groups[0].findings[0];
            (f.line, f.body.clone())
        }
        other => panic!("not a review: {other:?}"),
    }
}

fn sidecar(s: &Setup) -> String {
    std::fs::read_to_string(sidecar_path(&s.sess.log_file)).unwrap_or_default()
}

fn body_of(text: &str) -> String {
    review::parse_reviewer_output(review::final_answer(text))
        .unwrap()
        .findings[0]
        .body
        .clone()
}

#[test]
fn off_by_default_makes_no_call() {
    let mut s = setup("", Some(CLEAN));
    assert_eq!(run(&mut s, DIRTY), DIRTY);
    assert!(s.calls.borrow().is_empty());
}

#[test]
fn few_hits_make_no_call() {
    let mut s = setup("ste_rewrite: true\n", Some(CLEAN));
    let raw = CLEAN;
    assert_eq!(run(&mut s, raw), raw);
    assert!(s.calls.borrow().is_empty());
}

#[test]
fn unparsable_output_makes_no_call() {
    let mut s = setup("ste_rewrite: true\n", Some(CLEAN));
    assert_eq!(run(&mut s, "no json here"), "no json here");
    assert!(s.calls.borrow().is_empty());
}

#[test]
fn accepted_rewrite_becomes_the_last_review() {
    let mut s = setup("ste_rewrite: true\n", Some(CLEAN));
    std::fs::write(&s.sess.log_file, format!("{DIRTY}\n")).unwrap();
    let out = run(&mut s, &format!("{DIRTY}\n"));
    assert_eq!(s.calls.borrow().len(), 1);
    assert!(s.calls.borrow()[0].contains("ensure -> MAKE SURE (v)"));
    assert!(out.starts_with(DIRTY));
    assert!(body_of(&out).starts_with("Make sure that the lock"));
    // Every reader of the session log sees the same accepted review.
    assert_eq!(std::fs::read_to_string(&s.sess.log_file).unwrap(), out);
    let (line, body) = shown(&s);
    assert_eq!(line, 3);
    assert!(body.starts_with("Make sure that the lock"), "{body}");
    assert_eq!(sidecar(&s), format!("{CLEAN}\n"));
}

#[test]
fn rewrite_that_moves_a_finding_is_rejected() {
    let mut s = setup("ste_rewrite: true\n", Some(MOVED));
    std::fs::write(&s.sess.log_file, format!("{DIRTY}\n")).unwrap();
    let raw = format!("{DIRTY}\n");
    assert_eq!(run(&mut s, &raw), raw);
    assert_eq!(s.calls.borrow().len(), 1);
    // The rejected rewrite is not in the session log, so the TUI and the
    // command show the same finding.
    assert_eq!(std::fs::read_to_string(&s.sess.log_file).unwrap(), raw);
    assert_eq!(shown(&s).0, 3);
    assert_eq!(sidecar(&s), format!("{MOVED}\n"));
}

#[test]
fn rejected_rewrite_after_a_codex_transcript_leaves_the_log_alone() {
    let mut s = setup("ste_rewrite: true\n", Some(&format!("codex\n{MOVED}")));
    let raw = format!("thinking\ncodex\n{DIRTY}\ntokens used\n1,234\n");
    std::fs::write(&s.sess.log_file, &raw).unwrap();
    assert_eq!(run(&mut s, &raw), raw);
    assert_eq!(std::fs::read_to_string(&s.sess.log_file).unwrap(), raw);
    assert_eq!(shown(&s).0, 3);
}

#[test]
fn rewrite_that_changes_nothing_is_rejected() {
    let mut s = setup("ste_rewrite: true\n", Some(DIRTY));
    std::fs::write(&s.sess.log_file, format!("{DIRTY}\n")).unwrap();
    let raw = format!("{DIRTY}\n");
    assert_eq!(run(&mut s, &raw), raw);
}

#[test]
fn prose_instead_of_json_is_rejected() {
    let mut s = setup("ste_rewrite: true\n", Some("I rewrote it."));
    std::fs::write(&s.sess.log_file, format!("{DIRTY}\n")).unwrap();
    let raw = format!("{DIRTY}\n");
    assert_eq!(run(&mut s, &raw), raw);
}

#[test]
fn provider_failure_keeps_the_original() {
    let mut s = setup("ste_rewrite: true\n", None);
    let raw = format!("{DIRTY}\n");
    std::fs::write(&s.sess.log_file, &raw).unwrap();
    assert_eq!(run(&mut s, &raw), raw);
    assert_eq!(s.calls.borrow().len(), 1);
    assert_eq!(std::fs::read_to_string(&s.sess.log_file).unwrap(), raw);
}

/// Claude's first run records its transport ("native") as the session mode,
/// and Claude derives read-only permissions from the mode. The rewrite call
/// must still run as a review.
#[test]
fn rewrite_runs_as_a_review_after_the_transport_replaced_the_mode() {
    let mut s = setup("ste_rewrite: true\n", Some(CLEAN));
    s.sess.mode = "native".into();
    std::fs::write(&s.sess.log_file, format!("{DIRTY}\n")).unwrap();
    run(&mut s, &format!("{DIRTY}\n"));
    assert_eq!(*s.modes.borrow(), ["review"]);
}

/// A sidecar that cannot be written still leaves the log without the
/// rewrite, so every reader shows the original.
#[test]
fn unwritable_sidecar_still_cuts_the_rewrite_from_the_log() {
    let mut s = setup("ste_rewrite: true\n", Some(MOVED));
    let raw = format!("{DIRTY}\n");
    std::fs::write(&s.sess.log_file, &raw).unwrap();
    // A directory where the sidecar file would go makes its write fail.
    std::fs::create_dir(sidecar_path(&s.sess.log_file)).unwrap();
    assert_eq!(run(&mut s, &raw), raw);
    assert_eq!(std::fs::read_to_string(&s.sess.log_file).unwrap(), raw);
    assert_eq!(shown(&s).0, 3);
}
