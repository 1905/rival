//! Go: `cmd/command_antislop_test.go`, plus the `commandAntislopAction`
//! branches with a fake doc runner. No provider runs.
//! `TestBuildAntislopCodePrompt*` live in `gitscope_helper::tests`.

use super::*;

use std::cell::RefCell;
use std::path::Path;

use rival_core::config::{CLAUDE_MODEL, WHOLE_PROJECT};
use rival_core::review::{PlanCLIResult, PlanOutput};

use crate::testutil::{FakeStdin, Fixture, TEST_MR_URL, execute, invocation, no_mr, with_env};

fn s(p: &Path) -> String {
    p.to_str().unwrap().to_string()
}

// ---- TestAntislopStdinGrammar ----

#[test]
fn antislop_stdin_grammar() {
    let cases: &[(&str, &str, &str, &[&str])] = &[
        ("empty input is auto scope", "", "", &[]),
        (
            "options before scope",
            "-re high -m claude src/api/",
            "high",
            &["claude"],
        ),
        (
            "scope with model list",
            "-m codex,claude src/",
            "",
            &["codex", "claude"],
        ),
        ("escaped dash scope", "-- -weird/dir", "", &[]),
    ];
    for (name, raw, effort, models) in cases {
        let parsed = parse_review_args(raw).unwrap();
        assert_eq!(parsed.effort, *effort, "{name}");
        assert_eq!(
            parsed.models.len(),
            models.len(),
            "{name}: {:?}",
            parsed.models
        );
    }
}

// ---- TestAntislopDefaultModelsAreCodexAndClaude ----

#[test]
fn antislop_default_models_are_codex_and_claude() {
    let opts = AntislopOptions::from_invocation(&invocation(&["command", "antislop"]));
    assert_eq!(opts.models, ["codex", "claude"]);
    assert!(!opts.models_set);
    assert_eq!(
        parse_plan_models(&opts.models).unwrap(),
        ["codex", "claude"]
    );
}

// ---- TestAntislopModelClaudeRunsClaudeOnly ----

#[test]
fn antislop_model_claude_runs_claude_only() {
    assert_eq!(
        parse_plan_models(&["claude".to_string()]).unwrap(),
        ["claude"]
    );
}

// ---- commandAntislopAction ----

#[derive(Debug, Default, Clone)]
struct Seen {
    calls: usize,
    mode: String,
    prompt: String,
    target: String,
    fallback_effort: String,
    effort: String,
    workdir: String,
    group_id: String,
    no_queue: bool,
    clis: Vec<String>,
}

fn sample_result() -> PlanRunResult {
    PlanRunResult {
        results: vec![PlanCLIResult {
            cli: "claude".into(),
            model: CLAUDE_MODEL.into(),
            parsed: Some(PlanOutput {
                summary: "Lean.".into(),
                rating: 9,
                findings: vec![],
            }),
            raw: "{}".into(),
        }],
        skipped: vec![],
    }
}

struct Run {
    stdout: String,
    stderr: String,
    result: Result<(), CmdError>,
    seen: Seen,
}

fn run_antislop(
    fix: &Fixture,
    stdin: &mut FakeStdin,
    args: &[&str],
    outcome: impl Fn() -> anyhow::Result<PlanRunResult>,
) -> Run {
    let mut full = vec!["command", "antislop"];
    full.extend_from_slice(args);
    let opts = AntislopOptions::from_invocation(&invocation(&full));
    let seen = RefCell::new(Seen::default());
    let runner = |_: &Context,
                  _: &Config,
                  doc: &DocReview<'_>,
                  batch: &ReviewBatch<'_>,
                  _: &mut dyn Write|
     -> anyhow::Result<PlanRunResult> {
        let mut s = seen.borrow_mut();
        s.calls += 1;
        s.mode = doc.mode.to_string();
        s.prompt = doc.prompt.to_string();
        s.target = doc.target.to_string();
        s.fallback_effort = doc.fallback_effort.to_string();
        s.effort = batch.effort.to_string();
        s.workdir = batch.workdir.to_string();
        s.group_id = batch.group_id.to_string();
        s.no_queue = batch.no_queue;
        s.clis = batch.clis.to_vec();
        outcome()
    };
    let prepare = no_mr();
    let out = with_env(fix, stdin, &*prepare, |env| {
        run_command_antislop(env, &opts, &runner)
    });
    Run {
        stdout: out.stdout,
        stderr: out.stderr,
        result: out.result,
        seen: seen.into_inner(),
    }
}

fn never() -> anyhow::Result<PlanRunResult> {
    panic!("runner must not start")
}

fn fail(msg: impl Into<String>) -> CmdError {
    CmdError::exit(1, msg)
}

#[test]
fn explicit_scope_runs_with_stdin_options() {
    let fix = Fixture::new();
    let dir = tempfile::tempdir().unwrap();
    let w = s(dir.path());
    let r = run_antislop(
        &fix,
        &mut FakeStdin::new("-m claude -re high src/api/"),
        &["--workdir", &w, "--no-queue"],
        || Ok(sample_result()),
    );
    assert_eq!(r.result, Ok(()));
    assert_eq!(
        r.stdout,
        review::format_antislop_result(Some(&sample_result()), "src/api/")
    );
    assert_eq!(r.stderr, "");
    assert_eq!(r.seen.calls, 1);
    assert_eq!(r.seen.mode, "antislop");
    assert_eq!(r.seen.prompt, antislop_code_prompt("src/api/"));
    assert_eq!(r.seen.target, "src/api/");
    assert_eq!(r.seen.fallback_effort, "high");
    assert_eq!(r.seen.effort, "high");
    assert_eq!(r.seen.workdir, w);
    assert!(r.seen.no_queue);
    assert_eq!(r.seen.clis, ["claude"], "stdin models replace the default");
    assert!(uuid::Uuid::parse_str(&r.seen.group_id).is_ok());
}

#[test]
fn empty_stdin_auto_detects_and_falls_back_to_the_whole_project() {
    let fix = Fixture::new();
    // A fresh temp dir is not a git repo, so auto-detect finds nothing.
    let dir = tempfile::tempdir().unwrap();
    let r = run_antislop(
        &fix,
        &mut FakeStdin::new(""),
        &["--workdir", &s(dir.path())],
        || Ok(sample_result()),
    );
    assert_eq!(r.result, Ok(()));
    assert_eq!(r.seen.prompt, antislop_code_prompt(WHOLE_PROJECT));
    assert_eq!(r.seen.target, WHOLE_PROJECT);
    assert_eq!(r.seen.effort, "", "the fallback effort applies per model");
    assert_eq!(r.seen.clis, ["codex", "claude"], "default models");
    assert_eq!(
        r.stdout,
        review::format_antislop_result(Some(&sample_result()), WHOLE_PROJECT)
    );
}

#[test]
fn flag_models_and_effort_apply_when_stdin_has_none() {
    let fix = Fixture::new();
    let dir = tempfile::tempdir().unwrap();
    let r = run_antislop(
        &fix,
        &mut FakeStdin::new("-- -weird/dir"),
        &[
            "--workdir",
            &s(dir.path()),
            "-m",
            "claude-opus-5-5,codex",
            "--effort",
            "low",
        ],
        || Ok(sample_result()),
    );
    assert_eq!(r.result, Ok(()));
    assert_eq!(r.seen.clis, ["claude", "codex"]);
    assert_eq!(r.seen.effort, "low");
    assert_eq!(r.seen.target, "-weird/dir");

    // A matching stdin effort is not a conflict.
    let r = run_antislop(
        &fix,
        &mut FakeStdin::new("-re low src/"),
        &["--workdir", &s(dir.path()), "--effort", "low"],
        || Ok(sample_result()),
    );
    assert_eq!(r.result, Ok(()));
    assert_eq!(r.seen.effort, "low");
}

#[test]
fn validation_errors_print_on_stdout_and_fail() {
    let fix = Fixture::new();
    let dir = tempfile::tempdir().unwrap();
    let w = s(dir.path());
    let cases: &[(&[&str], &str, &str)] = &[
        (
            &["--effort", "enormous"],
            "src/",
            "invalid effort \"enormous\", must be one of: [low medium high xhigh ultra]",
        ),
        (
            &["--effort="],
            "src/",
            "invalid effort \"\", must be one of: [low medium high xhigh ultra]",
        ),
        (
            &["-m", "claude"],
            "-m codex src/",
            "model selection was provided both as --model command flags and in arguments; use one form",
        ),
        (
            &[],
            "-m sol src/",
            "unknown plan model \"sol\"; use one of: codex, claude",
        ),
        (&["--model", ""], "src/", "no plan models selected"),
        (
            &["--effort", "low"],
            "-re high src/",
            "reasoning effort conflicts: command uses \"low\" but plan arguments request \"high\"",
        ),
    ];
    for (flags, input, want) in cases {
        let mut args = vec!["--workdir", w.as_str()];
        args.extend_from_slice(flags);
        let r = run_antislop(&fix, &mut FakeStdin::new(input), &args, never);
        assert_eq!(r.result, Err(fail(*want)), "{flags:?} {input:?}");
        assert_eq!(r.stdout, format!("{want}\n"), "{flags:?} {input:?}");
        assert_eq!(r.stderr, "");
    }
}

#[test]
fn parse_errors_keep_the_parser_text() {
    let fix = Fixture::new();
    let dir = tempfile::tempdir().unwrap();
    let want = format!("{:#}", parse_review_args("-re enormous src/").unwrap_err());
    let r = run_antislop(
        &fix,
        &mut FakeStdin::new("-re enormous src/"),
        &["--workdir", &s(dir.path())],
        never,
    );
    assert_eq!(r.result, Err(fail(want.clone())));
    assert_eq!(r.stdout, format!("{want}\n"));
}

#[test]
fn an_mr_url_is_rejected_plainly_before_model_checks() {
    let fix = Fixture::new();
    let dir = tempfile::tempdir().unwrap();
    // The model conflict would also fail; the MR check comes first.
    let r = run_antislop(
        &fix,
        &mut FakeStdin::new(&format!("-m codex {TEST_MR_URL}")),
        &["--workdir", &s(dir.path()), "-m", "claude"],
        never,
    );
    assert_eq!(
        r.result,
        Err(CmdError::plain(
            "GitLab MR URLs need a pinned review: use rival command codex review <MR-URL> (or /rival-codex review <MR-URL>) from a repository with the MR's remote; no reviewer was started"
        ))
    );
    assert_eq!(r.stdout, "", "a plain error is printed by the root only");
}

#[test]
fn terminal_stdin_or_help_prints_usage() {
    let fix = Fixture::new();
    let dir = tempfile::tempdir().unwrap();
    let mut tty = FakeStdin::new("");
    tty.char_device = true;
    tty.forbid_read = true;
    let r = run_antislop(&fix, &mut tty, &["--workdir", &s(dir.path())], never);
    assert_eq!(r.result, Ok(()));
    assert_eq!(r.stdout, format!("{ANTISLOP_USAGE}\n"));

    let r = run_antislop(
        &fix,
        &mut FakeStdin::new("src/ --help"),
        &["--workdir", &s(dir.path())],
        never,
    );
    assert_eq!(r.result, Ok(()));
    assert_eq!(r.stdout, format!("{ANTISLOP_USAGE}\n"));

    // A bad --effort is reported even on a terminal.
    let mut tty = FakeStdin::new("");
    tty.char_device = true;
    tty.forbid_read = true;
    let r = run_antislop(
        &fix,
        &mut tty,
        &["--workdir", &s(dir.path()), "--effort", "bogus"],
        never,
    );
    assert!(r.result.is_err());
}

#[test]
fn read_and_runner_errors_fail_plainly() {
    let fix = Fixture::new();
    let dir = tempfile::tempdir().unwrap();
    let mut stdin = FakeStdin::new("");
    stdin.read_error = Some("read /dev/stdin: bad file descriptor".into());
    let r = run_antislop(&fix, &mut stdin, &["--workdir", &s(dir.path())], never);
    assert_eq!(
        r.result,
        Err(CmdError::plain(
            "read stdin: read /dev/stdin: bad file descriptor"
        ))
    );

    let r = run_antislop(
        &fix,
        &mut FakeStdin::new("src/"),
        &["--workdir", &s(dir.path())],
        || {
            Err(anyhow::anyhow!(
                "no plan models available (see skipped reasons): x"
            ))
        },
    );
    assert_eq!(
        r.result,
        Err(CmdError::plain(
            "no plan models available (see skipped reasons): x"
        ))
    );
    assert_eq!(r.stdout, "");
}

#[test]
fn bad_workdir_is_reported_first() {
    let fix = Fixture::new();
    let r = run_antislop(
        &fix,
        &mut FakeStdin::new("src/"),
        &["--workdir", "/definitely/not/here", "--effort", "bogus"],
        never,
    );
    assert_eq!(
        r.result,
        Err(fail("workdir not found: /definitely/not/here"))
    );
    assert_eq!(r.stdout, "workdir not found: /definitely/not/here\n");
}

// ---- Through the root: both streams ----

#[test]
fn root_prints_validation_errors_on_both_streams() {
    let fix = Fixture::new();
    let dir = tempfile::tempdir().unwrap();
    let (code, stdout, stderr) = execute(
        &fix,
        &mut FakeStdin::new("-m codex src/"),
        &[
            "command",
            "antislop",
            "--workdir",
            &s(dir.path()),
            "-m",
            "claude",
        ],
    );
    let want = "model selection was provided both as --model command flags and in arguments; use one form\n";
    assert_eq!(code, 1);
    assert_eq!(stdout, want);
    assert_eq!(stderr, want);
}
