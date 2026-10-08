//! Plan runs and model selection, plus pins for persisted sessions,
//! concurrency, the queue ticket, cancellation and timeouts. Every test uses
//! a temp home; fakes stand in for the provider CLIs.

use std::sync::{Condvar, Mutex};
use std::time::Duration;

use tempfile::TempDir;

use super::*;
use crate::config::{
    CLAUDE_LABEL, CLAUDE_MODEL, CODEX_MODEL, GPT56_SOL_MODEL, KIMI_MODEL, SOL_LABEL,
};
use crate::review::parse_reviewer_log;
use crate::review::plan::PlanOutput;
use crate::review::testutil::config_in;
use crate::session::{MODE_PLAN, MODE_SECURITY};

/// A minimal valid plan payload `parse_plan_log` accepts.
const REAL_PLAN_JSON: &str = r#"{"summary":"The plan is correct.","rating":7,"findings":[]}"#;

/// How long a concurrent fake waits for its peer before it gives up. Only a
/// broken test reaches it; it turns a hang into a failure.
const PEER_TIMEOUT: Duration = Duration::from_secs(10);

const EMPTY_REASON: &str = "produced no output (empty result); likely an auth/session failure";
const QUOTA_REASON: &str = "hit provider quota/rate limit (429)";

/// A temp home with an optional `~/.rival/config.yaml`.
fn plan_test_config(contents: &str) -> (TempDir, Config) {
    plan_test_config_env(contents, &[])
}

fn plan_test_config_env(contents: &str, env: &[(&str, &str)]) -> (TempDir, Config) {
    let home = tempfile::tempdir().unwrap();
    if !contents.is_empty() {
        let dir = home.path().join(".rival");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.yaml"), contents).unwrap();
    }
    let cfg = config_in(home.path(), env);
    assert!(
        cfg.user_config_error().is_none(),
        "load config: {:?}",
        cfg.user_config_error()
    );
    (home, cfg)
}

fn clis(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| s.to_string()).collect()
}

fn batch<'a>(
    effort: &'a str,
    workdir: &'a str,
    group_id: &'a str,
    clis: &'a [String],
) -> ReviewBatch<'a> {
    ReviewBatch {
        effort,
        workdir,
        group_id,
        no_queue: true,
        clis,
    }
}

/// A fake executor whose preflight always passes.
fn fake<'a>(
    run: impl Fn(&Context, &mut Session, &str, &str, &str, &str) -> anyhow::Result<(Vec<u8>, i64)>
    + Sync
    + 'a,
) -> PlanExecutor<'a> {
    PlanExecutor {
        preflight: Box::new(|_| Ok(())),
        run: Box::new(move |ctx, sess, cli, prompt, effort, workdir, _log| {
            run(ctx, sess, cli, prompt, effort, workdir)
        }),
    }
}

fn ok_plan() -> anyhow::Result<(Vec<u8>, i64)> {
    Ok((REAL_PLAN_JSON.into(), 0))
}

/// The persisted session for `cli`.
fn persisted(cfg: &Config, cli: &str) -> Session {
    let all = Session::load_all(cfg.paths());
    let mut found: Vec<Session> = all.into_iter().filter(|s| s.cli == cli).collect();
    assert_eq!(found.len(), 1, "sessions for {cli}");
    found.remove(0)
}

fn run_plan(
    cfg: &Config,
    ex: &PlanExecutor<'_>,
    effort: &str,
    group_id: &str,
    clis: &[String],
) -> anyhow::Result<PlanRunResult> {
    let work = tempfile::tempdir().unwrap();
    let workdir = work.path().to_str().unwrap();
    let mut stderr = Vec::new();
    let out = run_plan_review_with(
        &Context::background(),
        cfg,
        ex,
        "/tmp/plan.md",
        &batch(effort, workdir, group_id, clis),
        &mut stderr,
    );
    assert!(stderr.is_empty(), "{}", String::from_utf8_lossy(&stderr));
    out
}

fn run_doc(
    cfg: &Config,
    ex: &PlanExecutor<'_>,
    doc: &DocReview<'_>,
    effort: &str,
    clis: &[String],
) -> anyhow::Result<PlanRunResult> {
    let work = tempfile::tempdir().unwrap();
    let workdir = work.path().to_str().unwrap();
    let mut stderr = Vec::new();
    run_doc_review_with(
        &Context::background(),
        cfg,
        ex,
        doc,
        &batch(effort, workdir, "doc", clis),
        &mut stderr,
    )
}

/// A one-shot flag that a peer thread waits on, bounded.
#[derive(Default)]
struct Signal {
    set: Mutex<bool>,
    cond: Condvar,
}

impl Signal {
    fn set(&self) {
        *self.set.lock().unwrap() = true;
        self.cond.notify_all();
    }

    /// Whether the flag was set within `PEER_TIMEOUT`.
    fn wait(&self) -> bool {
        let guard = self.set.lock().unwrap();
        let (guard, _) = self
            .cond
            .wait_timeout_while(guard, PEER_TIMEOUT, |set| !*set)
            .unwrap();
        *guard
    }
}

/// A bounded rendezvous: each caller arrives, then waits until `want`
/// callers have arrived. A missing peer becomes an error, not a hang.
struct Rendezvous {
    arrived: Mutex<usize>,
    cond: Condvar,
    want: usize,
}

impl Rendezvous {
    fn new(want: usize) -> Rendezvous {
        Rendezvous {
            arrived: Mutex::new(0),
            cond: Condvar::new(),
            want,
        }
    }

    fn arrive_and_wait(&self) -> anyhow::Result<()> {
        let mut n = self.arrived.lock().unwrap();
        *n += 1;
        self.cond.notify_all();
        let (n, _) = self
            .cond
            .wait_timeout_while(n, PEER_TIMEOUT, |n| *n < self.want)
            .unwrap();
        if *n < self.want {
            bail!("only {} of {} reviewers ran at once", *n, self.want);
        }
        Ok(())
    }
}

/// The fake drains stdin before it exits. Runs the real executor against
/// a fake `codex`; `PATH` holds only the fake.
#[cfg(unix)]
#[test]
fn codex_plan_uses_codex_runtime_and_structured_output() {
    use crate::executor::codex::codex_run_args;
    use crate::executor::testutil::{Env, retry_busy};

    let mut env = Env::new();
    let args_file = env.work.path().join("args.txt");
    let args_path = args_file.to_str().unwrap().to_string();
    env.set("RIVAL_TEST_ARGS", Some(&args_path));
    // Drain the prompt with a shell builtin before the successful exit (PATH
    // holds only this fake): exiting unread races the prompt writer into EPIPE.
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = login ]; then exit 0; fi\nwhile IFS= read -r line; do :; done\nprintf '%s\\n' \"$@\" > \"$RIVAL_TEST_ARGS\"\nprintf '%s\\n' '{REAL_PLAN_JSON}'\n"
    );
    env.fake("codex", &script);
    let cfg = env.config();
    let repo = env.work_str();
    let plan = format!("{repo}/plan.md");
    let selected = clis(&["codex"]);

    let result = retry_busy(
        || {
            let mut stderr = Vec::new();
            run_plan_review(
                &Context::background(),
                &cfg,
                &plan,
                &ReviewBatch {
                    effort: "",
                    workdir: &repo,
                    group_id: "codex-proof",
                    no_queue: true,
                    clis: &selected,
                },
                &mut stderr,
            )
        },
        |r| format!("{r:?}"),
    )
    .unwrap();

    let raw = format!("{REAL_PLAN_JSON}\n");
    assert_eq!(
        result,
        PlanRunResult {
            results: vec![PlanCLIResult {
                cli: "codex".into(),
                model: CODEX_MODEL.into(),
                parsed: Some(PlanOutput {
                    summary: "The plan is correct.".into(),
                    rating: 7,
                    findings: vec![],
                }),
                raw: raw.clone(),
            }],
            skipped: vec![],
        }
    );

    let args = std::fs::read_to_string(&args_file).unwrap();
    let mut want = codex_run_args(CODEX_MODEL, "xhigh", &repo).join("\n");
    want.push('\n');
    assert_eq!(args, want);
    for part in [
        format!("-m\n{CODEX_MODEL}"),
        "model_reasoning_effort=xhigh".to_string(),
        "--sandbox\nread-only".to_string(),
    ] {
        assert!(
            args.contains(&part),
            "runtime arguments missing {part:?}: {args}"
        );
    }
    assert!(!args.contains(GPT56_SOL_MODEL), "Codex plan ran Sol");

    assert_eq!(
        format_plan_result_for(&result, "plan.md"),
        "\n═══ RIVAL PLAN REVIEW ═══\n\nFile: plan.md\nRating: 7/10\n\nSummary: The plan is correct.\n\nNo bugs or gaps found.\n"
    );

    let sess = persisted(&cfg, "codex");
    assert_eq!(sess.status, "completed");
    assert_eq!(sess.mode, MODE_PLAN);
    assert_eq!(sess.model, CODEX_MODEL);
    assert_eq!(sess.effort, "xhigh");
    assert_eq!(sess.review_scope, plan);
    assert_eq!(sess.group_id, "codex-proof");
    assert_eq!(sess.prompt, build_plan_prompt(&plan));
    assert_eq!(sess.exit_code, Some(0));
    assert_eq!(sess.output_bytes, raw.len() as i64);
    assert_eq!(sess.output_lines, 0);
    assert_eq!(sess.error_msg, "");
}

fn format_plan_result_for(result: &PlanRunResult, file: &str) -> String {
    crate::review::format_plan_result(Some(result), file)
}

fn run_ok(cli: &str, model: &str, raw: &str) -> PlanCLIRun {
    PlanCLIRun {
        cli: cli.into(),
        model: model.into(),
        raw: raw.into(),
        ..PlanCLIRun::default()
    }
}

fn skipped(cli: &str, model: &str, reason: &str) -> SkippedCLI {
    SkippedCLI {
        cli: cli.into(),
        model: model.into(),
        reason: reason.into(),
    }
}

#[test]
fn assemble_plan_results_all_failed() {
    let batch = vec![
        PlanCLIRun {
            cli: "codex".into(),
            exit_code: 1,
            ..PlanCLIRun::default()
        },
        PlanCLIRun {
            cli: "claude".into(),
            err: Some("boom".into()),
            ..PlanCLIRun::default()
        },
    ];
    let err = assemble_plan_results(batch, vec![]).unwrap_err();
    assert_eq!(
        err.to_string(),
        "all plan reviewers failed or hit quota limits (see skipped reasons): codex: exited with code 1; claude: boom"
    );
}

#[test]
fn assemble_plan_results_one_skipped_one_ok() {
    let batch = vec![run_ok("codex", GPT56_SOL_MODEL, REAL_PLAN_JSON)];
    // claude was unavailable at preflight → pre-run skipped list.
    let pre = vec![skipped("claude", "", "claude not found")];
    let res = assemble_plan_results(batch, pre.clone()).unwrap();
    assert_eq!(
        res,
        PlanRunResult {
            results: vec![PlanCLIResult {
                cli: "codex".into(),
                model: GPT56_SOL_MODEL.into(),
                parsed: Some(PlanOutput {
                    summary: "The plan is correct.".into(),
                    rating: 7,
                    findings: vec![],
                }),
                raw: REAL_PLAN_JSON.into(),
            }],
            skipped: pre,
        }
    );
}

#[test]
fn assemble_plan_results_nonzero_exit_skips() {
    let batch = vec![
        run_ok("codex", GPT56_SOL_MODEL, REAL_PLAN_JSON),
        PlanCLIRun {
            exit_code: 2,
            ..run_ok("claude", CLAUDE_MODEL, "partial")
        },
    ];
    let res = assemble_plan_results(batch, vec![]).unwrap();
    assert_eq!(res.results.len(), 1);
    assert_eq!(res.results[0].cli, "codex");
    assert_eq!(
        res.skipped,
        vec![skipped("claude", CLAUDE_MODEL, "exited with code 2")]
    );
}

#[test]
fn assemble_plan_results_quota_skips() {
    let batch = vec![
        run_ok("codex", GPT56_SOL_MODEL, "error: insufficient_quota"),
        run_ok("claude", CLAUDE_MODEL, REAL_PLAN_JSON),
    ];
    let res = assemble_plan_results(batch, vec![]).unwrap();
    assert_eq!(res.results.len(), 1);
    assert_eq!(res.results[0].cli, "claude");
    assert_eq!(
        res.skipped,
        vec![skipped("codex", GPT56_SOL_MODEL, QUOTA_REASON)]
    );
}

#[test]
fn assemble_plan_results_parse_fail_keeps_raw() {
    // Exit 0, no quota, but no parseable plan payload → keep raw, no parse.
    let batch = vec![run_ok("codex", GPT56_SOL_MODEL, "just some prose, no json")];
    let res = assemble_plan_results(batch, vec![]).unwrap();
    assert_eq!(
        res,
        PlanRunResult {
            results: vec![PlanCLIResult {
                cli: "codex".into(),
                model: GPT56_SOL_MODEL.into(),
                parsed: None,
                raw: "just some prose, no json".into(),
            }],
            skipped: vec![],
        }
    );
}

#[test]
fn assemble_plan_results_empty_output_skips() {
    // An exit-0 run that wrote nothing must be skipped, not treated as a
    // successful (but empty) plan review.
    let batch = vec![
        run_ok("claude", CLAUDE_MODEL, "   \n  "),
        run_ok("codex", GPT56_SOL_MODEL, REAL_PLAN_JSON),
    ];
    let res = assemble_plan_results(batch, vec![]).unwrap();
    assert_eq!(res.results.len(), 1);
    assert_eq!(res.results[0].cli, "codex");
    assert_eq!(
        res.skipped,
        vec![skipped("claude", CLAUDE_MODEL, EMPTY_REASON)]
    );
}

#[test]
fn plan_engine_label() {
    assert_eq!(config::engine_label("codex", GPT56_SOL_MODEL), SOL_LABEL);
    assert_eq!(config::engine_label("claude", CLAUDE_MODEL), CLAUDE_LABEL);
}

#[test]
fn plan_failure_reason_uses_model_name() {
    let got = plan_failure_reason(
        "codex",
        "Codex CLI not installed; run codex login; gpt-6-astra",
    );
    assert!(
        got.contains("Codex runtime"),
        "failure reason missing model name: {got:?}"
    );
    assert!(
        !got.contains("Codex CLI") && !got.contains(CODEX_MODEL),
        "failure reason leaked adapter text or model id: {got:?}"
    );
    assert_eq!(
        got,
        "Codex runtime not installed; authenticate the Codex runtime; codex"
    );
    assert_eq!(
        plan_failure_reason("claude", &format!("{CLAUDE_MODEL} failed in Claude CLI")),
        "claude failed in Claude runtime"
    );
}

#[test]
fn format_plan_result_single_parsed() {
    let res = PlanRunResult {
        results: vec![PlanCLIResult {
            cli: "codex".into(),
            model: GPT56_SOL_MODEL.into(),
            parsed: Some(PlanOutput {
                summary: "s".into(),
                rating: 8,
                findings: vec![],
            }),
            raw: String::new(),
        }],
        skipped: vec![],
    };
    let out = format_plan_result_for(&res, "/tmp/plan.md");
    // Single-CLI must NOT use the multi header.
    assert_eq!(
        out,
        "\n═══ RIVAL PLAN REVIEW ═══\n\nFile: /tmp/plan.md\nRating: 8/10\n\nSummary: s\n\nNo bugs or gaps found.\n"
    );
}

#[test]
fn format_plan_result_single_parse_fail_returns_raw() {
    let res = PlanRunResult {
        results: vec![PlanCLIResult {
            cli: "codex".into(),
            model: GPT56_SOL_MODEL.into(),
            parsed: None,
            raw: "Codex raw output".into(),
        }],
        skipped: vec![],
    };
    assert_eq!(
        format_plan_result_for(&res, "/tmp/plan.md"),
        "Sol runtime raw output"
    );
}

#[test]
fn format_plan_result_multi_blocks_and_skipped() {
    let res = PlanRunResult {
        results: vec![
            PlanCLIResult {
                cli: "codex".into(),
                model: GPT56_SOL_MODEL.into(),
                parsed: Some(PlanOutput {
                    summary: "cx".into(),
                    rating: 6,
                    findings: vec![],
                }),
                raw: String::new(),
            },
            PlanCLIResult {
                cli: "claude".into(),
                model: CLAUDE_MODEL.into(),
                parsed: None,
                raw: "Claude raw dump".into(),
            },
        ],
        skipped: vec![skipped("opencode", KIMI_MODEL, "n/a")],
    };
    let out = format_plan_result_for(&res, "/tmp/plan.md");
    assert_eq!(
        out,
        concat!(
            "\n═══ RIVAL PLAN REVIEW (sol + claude) ═══\n\n",
            "File: /tmp/plan.md\n",
            "\n── sol ──\n\n",
            "Rating: 6/10\n\n",
            "Summary: cx\n\n",
            "No bugs or gaps found.\n",
            "\n",
            "\n── claude ──\n\n",
            "(could not parse structured output — raw output below)\n\n",
            "Claude runtime raw dump\n",
            "\n",
            "Skipped: kimi-k3 — n/a\n",
        )
    );
    // Plan output must use model names, not adapter names.
    assert!(!out.to_lowercase().contains("codex"), "{out}");
}

#[test]
fn assemble_plan_results_err_uses_reason() {
    // A timeout-style failure carries a reason that must surface in skipped,
    // instead of the bare error text.
    let reason =
        format!("{CLAUDE_MODEL} run timeout after 30m (RIVAL_RUN_TIMEOUT) — model did not finish");
    let batch = vec![
        PlanCLIRun {
            cli: "claude".into(),
            err: Some("context deadline exceeded".into()),
            reason: reason.clone(),
            exit_code: -1,
            ..PlanCLIRun::default()
        },
        run_ok("codex", GPT56_SOL_MODEL, REAL_PLAN_JSON),
    ];
    let res = assemble_plan_results(batch, vec![]).unwrap();
    assert_eq!(res.skipped, vec![skipped("claude", CLAUDE_MODEL, &reason)]);
}

#[test]
fn run_plan_cli_restores_plan_mode() {
    let (_home, cfg) = plan_test_config("");
    let work = tempfile::tempdir().unwrap();
    let workdir = work.path().to_str().unwrap();
    // The claude executor overwrites the mode with the transport ("native");
    // the terminal session must be recorded as a plan session regardless.
    let mut sess = Session::new_queued(
        cfg.paths(),
        NewSession {
            cli: "claude",
            mode: MODE_PLAN,
            model: CLAUDE_MODEL,
            effort: "high",
            workdir,
            prompt: "p",
            review_scope: "/tmp/plan.md",
            group_id: "g",
        },
    )
    .unwrap();
    sess.mark_running(cfg.paths()).unwrap();
    let ex = fake(|_, s, _, _, _, _| {
        s.mode = "native".into(); // simulate the Claude executor clobbering mode
        ok_plan()
    });
    let out = run_plan_cli(
        &Context::background(),
        &Context::background(),
        &cfg,
        &ex,
        &mut sess,
        "claude",
        "p",
        workdir,
        MODE_PLAN,
    );
    assert_eq!(out, run_ok("claude", CLAUDE_MODEL, REAL_PLAN_JSON));
    assert_eq!(sess.mode, MODE_PLAN);
    let saved = Session::load(cfg.paths(), &sess.id).unwrap();
    assert_eq!(saved.mode, MODE_PLAN);
    assert_eq!(saved.status, "completed");
    assert_eq!(saved.output_bytes, REAL_PLAN_JSON.len() as i64);
}

#[test]
fn run_plan_review_resolves_per_model_efforts() {
    struct Case {
        name: &'static str,
        config_yaml: &'static str,
        override_effort: &'static str,
        clis: &'static [&'static str],
        want: &'static [(&'static str, &'static str)],
    }
    let cases = [
        Case {
            name: "codex native fallback",
            config_yaml: "",
            override_effort: "",
            clis: &["codex"],
            want: &[("codex", "xhigh")],
        },
        Case {
            name: "codex configured effort",
            config_yaml: "efforts:\n  codex: low\n",
            override_effort: "",
            clis: &["codex"],
            want: &[("codex", "low")],
        },
        Case {
            name: "codex uses its xhigh pin",
            config_yaml: "",
            override_effort: "",
            clis: &["codex"],
            want: &[("codex", "xhigh")],
        },
        Case {
            name: "claude alone uses its medium pin",
            config_yaml: "",
            override_effort: "",
            clis: &["claude"],
            want: &[("claude", "medium")],
        },
        Case {
            name: "paired native plan keeps each pin",
            config_yaml: "",
            override_effort: "",
            clis: &["codex", "claude"],
            want: &[("codex", "xhigh"), ("claude", "medium")],
        },
        Case {
            name: "configured defaults resolve independently",
            config_yaml: "efforts:\n  codex: low\n  claude: ultra\n",
            override_effort: "",
            clis: &["codex", "claude"],
            want: &[("codex", "low"), ("claude", "ultra")],
        },
        Case {
            name: "explicit override wins for every model",
            config_yaml: "efforts:\n  codex: low\n  claude: medium\n",
            override_effort: "ultra",
            clis: &["codex", "claude"],
            want: &[("codex", "ultra"), ("claude", "ultra")],
        },
    ];

    for tc in cases {
        let (_home, cfg) = plan_test_config(tc.config_yaml);
        // (cli, session effort, executor effort)
        let observed = Mutex::new(Vec::<(String, String, String)>::new());
        let ex = fake(|_, sess, cli, _, effort, _| {
            observed
                .lock()
                .unwrap()
                .push((cli.into(), sess.effort.clone(), effort.into()));
            ok_plan()
        });
        let selected = clis(tc.clis);
        run_plan(&cfg, &ex, tc.override_effort, "efforts", &selected)
            .unwrap_or_else(|e| panic!("{}: run_plan_review: {e:#}", tc.name));

        let mut observed = observed.lock().unwrap().clone();
        observed.sort();
        let mut want: Vec<(String, String, String)> = tc
            .want
            .iter()
            .map(|(cli, e)| (cli.to_string(), e.to_string(), e.to_string()))
            .collect();
        want.sort();
        assert_eq!(observed, want, "{}", tc.name);
        for (cli, effort) in tc.want {
            assert_eq!(persisted(&cfg, cli).effort, *effort, "{}: {cli}", tc.name);
        }
    }
}

/// Codex cannot finish until Claude has, so this also proves the two run at
/// once.
#[test]
fn run_plan_review_preserves_requested_order_when_claude_finishes_first() {
    let (_home, cfg) = plan_test_config("");
    let claude_done = Signal::default();
    let ex = fake(|_, _, cli, _, _, _| {
        if cli == "claude" {
            claude_done.set();
        } else if !claude_done.wait() {
            bail!("claude never finished while codex ran");
        }
        ok_plan()
    });
    let selected = clis(&["codex", "claude"]);
    let result = run_plan(&cfg, &ex, "ultra", "ordered", &selected).unwrap();
    let order: Vec<&str> = result.results.iter().map(|r| r.cli.as_str()).collect();
    assert_eq!(order, ["codex", "claude"]);
    assert!(result.skipped.is_empty());
}

/// Rust-only: both reviewers are inside the executor at the same time, and
/// the batch result keeps the requested order with each model's own output.
#[test]
fn reviewers_run_simultaneously() {
    let (_home, cfg) = plan_test_config("");
    let together = Rendezvous::new(2);
    let ex = fake(|_, _, cli, _, _, _| {
        together.arrive_and_wait()?;
        let rating = if cli == "codex" { 4 } else { 9 };
        Ok((
            format!(r#"{{"summary":"{cli} view","rating":{rating},"findings":[]}}"#).into(),
            0,
        ))
    });
    let selected = clis(&["claude", "codex"]);
    let result = run_plan(&cfg, &ex, "", "together", &selected).unwrap();
    let got: Vec<(&str, i64)> = result
        .results
        .iter()
        .map(|r| (r.cli.as_str(), r.parsed.as_ref().unwrap().rating))
        .collect();
    assert_eq!(got, [("claude", 9), ("codex", 4)]);
}

/// A doc review passes its own prompt and a fallback effort; the target lands
/// as the session's review scope.
#[test]
fn run_doc_review_applies_fallback_effort_and_target() {
    let (_home, cfg) = plan_test_config("");
    // (effort, scope, prompt)
    let observed = Mutex::new(Vec::<(String, String, String)>::new());
    let ex = fake(|_, sess, _, prompt, effort, _| {
        observed
            .lock()
            .unwrap()
            .push((effort.into(), sess.review_scope.clone(), prompt.into()));
        ok_plan()
    });
    let doc = DocReview {
        mode: MODE_PLAN,
        prompt: "DOC PROMPT",
        target: "src/api/",
        fallback_effort: "high",
    };
    run_doc(&cfg, &ex, &doc, "", &clis(&["claude"])).unwrap();
    assert_eq!(
        observed.lock().unwrap().clone(),
        [(
            "medium".to_string(),
            "src/api/".to_string(),
            "DOC PROMPT".to_string()
        )]
    );
    let sess = persisted(&cfg, "claude");
    assert_eq!(sess.review_scope, "src/api/");
    assert_eq!(sess.prompt, "DOC PROMPT");
    assert_eq!(sess.mode, MODE_PLAN);
    assert_eq!(sess.status, "completed");
}

/// Doc runs carry the mode the caller passes.
#[test]
fn run_doc_review_records_the_requested_mode() {
    let (_home, cfg) = plan_test_config("");
    let observed = Mutex::new(Vec::<String>::new());
    let ex = fake(|_, sess, _, _, _, _| {
        observed.lock().unwrap().push(sess.mode.clone());
        ok_plan()
    });
    let doc = DocReview {
        mode: MODE_SECURITY,
        prompt: "PROMPT",
        target: "src/",
        fallback_effort: "xhigh",
    };
    run_doc(&cfg, &ex, &doc, "", &clis(&["claude"])).unwrap();
    assert_eq!(observed.lock().unwrap().clone(), [MODE_SECURITY]);
    assert_eq!(persisted(&cfg, "claude").mode, MODE_SECURITY);
}

#[test]
fn run_plan_review_still_records_plan_mode() {
    let (_home, cfg) = plan_test_config("");
    let observed = Mutex::new(Vec::<String>::new());
    let ex = fake(|_, sess, _, _, _, _| {
        observed.lock().unwrap().push(sess.mode.clone());
        ok_plan()
    });
    run_plan(&cfg, &ex, "", "mode", &clis(&["claude"])).unwrap();
    assert_eq!(observed.lock().unwrap().clone(), [MODE_PLAN]);
    assert_eq!(persisted(&cfg, "claude").mode, MODE_PLAN);
}

/// Quota wording inside a real review must not fail the run.
#[test]
fn run_failure_reason_ignores_quota_text_in_a_real_review() {
    let reason = |raw: &str| run_failure_reason("codex", raw, parse_reviewer_log(raw).is_ok());
    let raw = concat!(
        "exec cat quota.go\n\"insufficient_quota\"\ncodex\n",
        r#"{"summary": "One bug.", "findings": []}"#
    );
    assert_eq!(reason(raw), "", "real review failed");
    assert_eq!(
        reason("Error 429 (Too Many Requests)\n"),
        "codex hit provider quota/rate limit (429)"
    );
    assert_eq!(
        reason("   \n"),
        "codex produced no output (empty result); likely an auth/session failure"
    );
    assert_eq!(
        reason("just some prose\n"),
        "",
        "unparsed prose must stay UNPARSED"
    );
    assert_eq!(run_failure_reason("", "", false), EMPTY_REASON);
}

/// A codex log where an `exec` tool printed a valid plan assessment and the
/// final answer, after the last "codex" header, is `final_answer`.
fn codex_tool_plan_transcript(final_answer: &str) -> String {
    format!(
        "user\nreview the plan\nexec\ncat old-review.json\n{}\ncodex\n{final_answer}\n",
        r#"{"summary":"tool output, not the answer","rating":10,"findings":[]}"#
    )
}

/// A plan assessment printed by a tool is never the model's review.
#[test]
fn assemble_plan_results_parses_final_answer_not_tool_output() {
    let raw = codex_tool_plan_transcript("The plan looks broadly fine, a few nits.");
    let res = assemble_plan_results(vec![run_ok("codex", CODEX_MODEL, &raw)], vec![]).unwrap();
    assert_eq!(
        res,
        PlanRunResult {
            results: vec![PlanCLIResult {
                cli: "codex".into(),
                model: CODEX_MODEL.into(),
                parsed: None,
                raw,
            }],
            skipped: vec![],
        }
    );
}

/// The per-run session status also judges the final answer.
#[test]
fn run_doc_review_quota_final_answer_fails_despite_tool_plan_json() {
    let (_home, cfg) = plan_test_config("");
    let ex = fake(|_, _, _, _, _, _| {
        Ok((
            codex_tool_plan_transcript("Error 429 (Too Many Requests)").into(),
            0,
        ))
    });
    let doc = DocReview {
        mode: MODE_SECURITY,
        prompt: "PROMPT",
        target: "src/",
        fallback_effort: "high",
    };
    let err = run_doc(&cfg, &ex, &doc, "", &clis(&["codex"])).unwrap_err();
    assert_eq!(
        err.to_string(),
        "all plan reviewers failed or hit quota limits (see skipped reasons): codex: hit provider quota/rate limit (429)"
    );
    let sess = persisted(&cfg, "codex");
    assert_eq!(sess.status, "failed");
    assert_eq!(sess.exit_code, Some(1));
    assert_eq!(sess.error_msg, "codex hit provider quota/rate limit (429)");
}

#[test]
fn opencode_variant_per_curated_model() {
    for (model, effort, want) in [
        (KIMI_MODEL, "low", "max"),
        (KIMI_MODEL, "xhigh", "max"),
        (KIMI_MODEL, "ultra", "max"),
        ("unsupported-model", "high", ""),
    ] {
        assert_eq!(
            config::opencode_variant(model, effort),
            want,
            "{model} {effort}"
        );
    }
}

/// Rust-only: no models requested is an error before any session exists.
#[test]
fn no_models_requested() {
    let (_home, cfg) = plan_test_config("");
    let ex = fake(|_, _, _, _, _, _| ok_plan());
    let err = run_plan(&cfg, &ex, "", "g", &[]).unwrap_err();
    assert_eq!(err.to_string(), "no plan models requested");
    assert!(Session::load_all(cfg.paths()).is_empty());
}

/// Rust-only: a failed preflight skips the model with a public reason and
/// creates no session for it; the other model still runs.
#[test]
fn preflight_failure_is_skipped_with_a_public_reason() {
    let (_home, cfg) = plan_test_config("");
    let ex = PlanExecutor {
        preflight: Box::new(|cli| {
            if cli == "codex" {
                bail!("Codex CLI not installed; run codex login");
            }
            Ok(())
        }),
        run: Box::new(|_, _, _, _, _, _, _| ok_plan()),
    };
    let result = run_plan(&cfg, &ex, "", "g", &clis(&["codex", "claude"])).unwrap();
    assert_eq!(
        result.skipped,
        vec![skipped(
            "codex",
            CODEX_MODEL,
            "Codex runtime not installed; authenticate the Codex runtime"
        )]
    );
    assert_eq!(result.results.len(), 1);
    assert_eq!(result.results[0].cli, "claude");
    let sessions = Session::load_all(cfg.paths());
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].cli, "claude");
    assert_eq!(sessions[0].status, "completed");
}

/// Rust-only: every preflight failing is an error listing the reasons, and
/// no session is created.
#[test]
fn all_preflights_failing_is_an_error() {
    let (_home, cfg) = plan_test_config("");
    let ex = PlanExecutor {
        preflight: Box::new(|cli| bail!("{cli} CLI missing")),
        run: Box::new(|_, _, _, _, _, _, _| bail!("must not run")),
    };
    let err = run_plan(&cfg, &ex, "", "g", &clis(&["codex", "claude"])).unwrap_err();
    assert_eq!(
        err.to_string(),
        "no plan models available (see skipped reasons): codex: Codex runtime missing; claude: claude runtime missing"
    );
    assert!(Session::load_all(cfg.paths()).is_empty());
}

/// Rust-only: the queued-session cleanup is armed before the creation loop,
/// so an error after one session exists still fails that session.
#[test]
fn error_mid_creation_fails_the_sessions_already_created() {
    let (_home, cfg) = plan_test_config("");
    let ex = fake(|_, _, _, _, _, _| bail!("must not run"));
    // codex pins xhigh; the unknown adapter falls back to the invalid value.
    let doc = DocReview {
        mode: MODE_PLAN,
        prompt: "PROMPT",
        target: "/tmp/plan.md",
        fallback_effort: "bogus",
    };
    let err = run_doc(&cfg, &ex, &doc, "", &clis(&["codex", "other"])).unwrap_err();
    assert_eq!(
        err.to_string(),
        "resolve retired-model plan effort: invalid fallback effort \"bogus\" for retired-model"
    );
    let sess = persisted(&cfg, "codex");
    assert_eq!(sess.status, "failed");
    assert_eq!(sess.exit_code, Some(1));
    assert_eq!(sess.error_msg, "interrupted");
}

/// Rust-only: an executor error fails the session with the public reason and
/// skips the model.
#[test]
fn run_error_fails_the_session_with_a_public_reason() {
    let (_home, cfg) = plan_test_config("");
    let ex = fake(|_, _, cli, _, _, _| {
        if cli == "claude" {
            bail!("Claude CLI crashed");
        }
        ok_plan()
    });
    let result = run_plan(&cfg, &ex, "", "g", &clis(&["codex", "claude"])).unwrap();
    assert_eq!(
        result.skipped,
        vec![skipped("claude", CLAUDE_MODEL, "Claude runtime crashed")]
    );
    let sess = persisted(&cfg, "claude");
    assert_eq!(sess.status, "failed");
    assert_eq!(sess.exit_code, Some(1));
    assert_eq!(sess.error_msg, "Claude runtime crashed");
    assert_eq!(persisted(&cfg, "codex").status, "completed");
}

/// Rust-only: session outcomes for a non-zero exit, an empty log, quota and
/// unparsed output.
#[test]
fn session_outcomes_follow_the_run() {
    struct Case {
        raw: &'static [u8],
        exit: i64,
        status: &'static str,
        exit_code: i64,
        error: &'static str,
        skipped: Option<&'static str>,
    }
    let cases = [
        Case {
            raw: b"partial",
            exit: 3,
            status: "failed",
            exit_code: 3,
            error: "claude exited with code 3",
            skipped: Some("exited with code 3"),
        },
        Case {
            raw: b" \n",
            exit: 0,
            status: "failed",
            exit_code: 1,
            error: "claude produced no output (empty result); likely an auth/session failure",
            skipped: Some(EMPTY_REASON),
        },
        Case {
            raw: b"Error 429 (Too Many Requests)\n",
            exit: 0,
            status: "failed",
            exit_code: 1,
            error: "claude hit provider quota/rate limit (429)",
            skipped: Some(QUOTA_REASON),
        },
        Case {
            raw: b"prose only\n",
            exit: 0,
            status: "completed",
            exit_code: 0,
            error: "",
            skipped: None,
        },
    ];
    for tc in cases {
        let (_home, cfg) = plan_test_config("");
        let ex = fake(|_, _, cli, _, _, _| {
            if cli == "claude" {
                return Ok((tc.raw.to_vec(), tc.exit));
            }
            ok_plan()
        });
        let result = run_plan(&cfg, &ex, "", "g", &clis(&["codex", "claude"])).unwrap();
        let sess = persisted(&cfg, "claude");
        assert_eq!(sess.status, tc.status, "{:?}", tc.raw);
        assert_eq!(sess.exit_code, Some(tc.exit_code), "{:?}", tc.raw);
        assert_eq!(sess.error_msg, tc.error, "{:?}", tc.raw);
        match tc.skipped {
            Some(reason) => {
                assert_eq!(
                    result.skipped,
                    vec![skipped("claude", CLAUDE_MODEL, reason)]
                );
                assert_eq!(result.results.len(), 1);
            }
            None => {
                assert!(result.skipped.is_empty());
                let claude = &result.results[1];
                assert_eq!(claude.parsed, None);
                assert_eq!(claude.raw.as_bytes(), tc.raw);
                assert_eq!(sess.output_bytes, tc.raw.len() as i64);
            }
        }
    }
}

/// Rust-only: `output_bytes` counts the log's bytes; invalid UTF-8 is
/// decoded lossily for parsing and the result.
#[test]
fn output_bytes_count_raw_bytes() {
    let (_home, cfg) = plan_test_config("");
    let ex = fake(|_, _, _, _, _, _| {
        let mut raw = b"\xff\n".to_vec();
        raw.extend_from_slice(REAL_PLAN_JSON.as_bytes());
        Ok((raw, 0))
    });
    let result = run_plan(&cfg, &ex, "", "g", &clis(&["claude"])).unwrap();
    assert_eq!(result.results[0].raw, format!("\u{fffd}\n{REAL_PLAN_JSON}"));
    assert_eq!(result.results[0].parsed.as_ref().unwrap().rating, 7);
    assert_eq!(
        persisted(&cfg, "claude").output_bytes,
        2 + REAL_PLAN_JSON.len() as i64
    );
}

/// Rust-only: Claude sessions record the configured subscription.
#[test]
fn claude_session_records_the_subscription() {
    let (_home, cfg) = plan_test_config("claude:\n  subscription: team\n");
    let ex = fake(|_, _, _, _, _, _| ok_plan());
    run_plan(&cfg, &ex, "", "g", &clis(&["codex", "claude"])).unwrap();
    assert_eq!(persisted(&cfg, "claude").account, "team");
    assert_eq!(persisted(&cfg, "codex").account, "");
}

/// Rust-only: a run that hits `RIVAL_RUN_TIMEOUT` reports the timeout; the
/// child deadline never cancels the caller's context.
#[test]
fn run_timeout_reports_the_deadline_and_leaves_the_parent_live() {
    let (_home, cfg) = plan_test_config_env("", &[("RIVAL_RUN_TIMEOUT", "50ms")]);
    let ex = fake(|ctx, _, _, _, _, _| match ctx.wait_timeout(PEER_TIMEOUT) {
        Some(err) => Err(anyhow!(err)),
        None => bail!("run deadline never fired"),
    });
    let (parent, _cancel) = Context::background().with_cancel();
    let work = tempfile::tempdir().unwrap();
    let selected = clis(&["claude"]);
    let mut stderr = Vec::new();
    let err = run_plan_review_with(
        &parent,
        &cfg,
        &ex,
        "/tmp/plan.md",
        &batch("", work.path().to_str().unwrap(), "g", &selected),
        &mut stderr,
    )
    .unwrap_err();
    let reason = "claude run timeout after 50ms (RIVAL_RUN_TIMEOUT) — model did not finish";
    assert_eq!(
        err.to_string(),
        format!(
            "all plan reviewers failed or hit quota limits (see skipped reasons): claude: {reason}"
        )
    );
    assert_eq!(parent.err(), None);
    let sess = persisted(&cfg, "claude");
    assert_eq!(sess.status, "failed");
    assert_eq!(sess.error_msg, reason);
}

/// Rust-only: cancelling the caller's context is reported as the provider
/// error, not as a timeout.
#[test]
fn cancelled_run_is_not_a_timeout() {
    let (_home, cfg) = plan_test_config("");
    let (parent, cancel) = Context::background().with_cancel();
    let ex = fake(|ctx, _, _, _, _, _| {
        cancel.cancel();
        match ctx.wait_timeout(PEER_TIMEOUT) {
            Some(err) => Err(anyhow!(err)),
            None => bail!("cancel never reached the run"),
        }
    });
    let work = tempfile::tempdir().unwrap();
    let selected = clis(&["claude"]);
    let mut stderr = Vec::new();
    let err = run_plan_review_with(
        &parent,
        &cfg,
        &ex,
        "/tmp/plan.md",
        &batch("", work.path().to_str().unwrap(), "g", &selected),
        &mut stderr,
    )
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "all plan reviewers failed or hit quota limits (see skipped reasons): claude: context canceled"
    );
    assert_eq!(persisted(&cfg, "claude").error_msg, "context canceled");
}

/// Ticket files in the queue dir (the lock file is not one).
fn ticket_count(cfg: &Config) -> usize {
    std::fs::read_dir(cfg.paths().queue_dir()).map_or(0, |d| {
        d.filter(|e| {
            e.as_ref()
                .is_ok_and(|e| e.file_name().to_string_lossy().ends_with(".json"))
        })
        .count()
    })
}

/// Rust-only: one queue ticket covers the whole batch while every model
/// runs, and it is freed once the batch is done.
#[test]
fn one_queue_ticket_covers_the_whole_batch() {
    let (_home, cfg) = plan_test_config("");
    let together = Rendezvous::new(2);
    let seen = Mutex::new(Vec::<(usize, String)>::new());
    let ex = fake(|_, sess, _, _, _, _| {
        together.arrive_and_wait()?;
        seen.lock()
            .unwrap()
            .push((ticket_count(&cfg), sess.status.clone()));
        together_done(&together)?;
        ok_plan()
    });
    let work = tempfile::tempdir().unwrap();
    let selected = clis(&["codex", "claude"]);
    let mut stderr = Vec::new();
    let result = run_plan_review_with(
        &Context::background(),
        &cfg,
        &ex,
        "/tmp/plan.md",
        &ReviewBatch {
            no_queue: false,
            ..batch("", work.path().to_str().unwrap(), "queued", &selected)
        },
        &mut stderr,
    )
    .unwrap();
    assert_eq!(result.results.len(), 2);
    assert_eq!(
        seen.lock().unwrap().clone(),
        [(1, "running".to_string()), (1, "running".to_string())]
    );
    assert_eq!(ticket_count(&cfg), 0);
    assert!(stderr.is_empty(), "{}", String::from_utf8_lossy(&stderr));
}

/// The second meeting of a two-party rendezvous: both have recorded what
/// they saw before either returns.
fn together_done(r: &Rendezvous) -> anyhow::Result<()> {
    let mut n = r.arrived.lock().unwrap();
    *n += 1;
    r.cond.notify_all();
    let (n, _) = r
        .cond
        .wait_timeout_while(n, PEER_TIMEOUT, |n| *n < 2 * r.want)
        .unwrap();
    if *n < 2 * r.want {
        bail!("a reviewer finished before its peer looked at the queue");
    }
    Ok(())
}

/// Rust-only: the timeout reason with and without a label, and only for a
/// deadline.
#[test]
fn run_timeout_reason_texts() {
    let (_home, cfg) = plan_test_config_env("", &[("RIVAL_RUN_TIMEOUT", "90s")]);
    let (expired, _c) = Context::background().with_timeout_nanos(0);
    assert_eq!(
        run_timeout_reason(&expired, &cfg, "codex", "fallback"),
        "codex run timeout after 1m30s (RIVAL_RUN_TIMEOUT) — model did not finish"
    );
    assert_eq!(
        run_timeout_reason(&expired, &cfg, "", "fallback"),
        "run timeout after 1m30s (RIVAL_RUN_TIMEOUT) — model did not finish"
    );
    let (cancelled, cancel) = Context::background().with_cancel();
    cancel.cancel();
    assert_eq!(
        run_timeout_reason(&cancelled, &cfg, "codex", "fallback"),
        "fallback"
    );
    assert_eq!(
        run_timeout_reason(&Context::background(), &cfg, "codex", "fallback"),
        "fallback"
    );
}

/// Rust-only: `with_run_timeout` sets no deadline when the timeout is
/// disabled, and its cancel never reaches the parent.
#[test]
fn with_run_timeout_bounds_only_the_child() {
    let (_home, off) = plan_test_config_env("", &[("RIVAL_RUN_TIMEOUT", "0")]);
    let (parent, _c) = Context::background().with_cancel();
    let (child, cancel) = with_run_timeout(&parent, &off, 1);
    assert_eq!(child.deadline(), None);
    cancel.cancel();
    assert_eq!(parent.err(), None);

    let (_home, on) = plan_test_config_env("", &[("RIVAL_RUN_TIMEOUT", "1h")]);
    let (child, _c) = with_run_timeout(&parent, &on, 1);
    assert!(child.deadline().is_some());
    assert_eq!(child.err(), None);
}

/// Rust-only: the plan prompt names the file.
#[test]
fn plan_prompt_names_the_file() {
    let prompt = build_plan_prompt("/abs/plan.md");
    assert_eq!(
        prompt,
        config::PLAN_REVIEW_PROMPT.replace("{FILE}", "/abs/plan.md")
    );
    assert!(prompt.contains("/abs/plan.md"));
    assert!(!prompt.contains("{FILE}"));
}

/// The session finalizer also runs on a panic. A panicking executor leaves
/// the session failed with "interrupted".
#[test]
fn run_plan_cli_fails_the_session_on_unwind() {
    let (_home, cfg) = plan_test_config("");
    let mut sess = Session::new_queued(
        cfg.paths(),
        NewSession {
            cli: "claude",
            mode: MODE_PLAN,
            model: CLAUDE_MODEL,
            effort: "medium",
            workdir: "/w",
            prompt: "p",
            review_scope: "/tmp/plan.md",
            group_id: "g",
        },
    )
    .unwrap();
    sess.mark_running(cfg.paths()).unwrap();
    let ex = fake(|_, _, _, _, _, _| panic!("executor blew up"));
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_plan_cli(
            &Context::background(),
            &Context::background(),
            &cfg,
            &ex,
            &mut sess,
            "claude",
            "p",
            "/w",
            MODE_PLAN,
        )
    }));
    assert!(caught.is_err(), "the panic must propagate");
    let saved = Session::load(cfg.paths(), &sess.id).unwrap();
    assert_eq!(saved.status, "failed");
    assert_eq!(saved.exit_code, Some(1));
    assert_eq!(saved.error_msg, "interrupted");
}

/// Controller feedback 1 and 2 through the whole batch: a panicking
/// reviewer propagates, its session ends "interrupted", and the run context
/// is cancelled on the way out while the caller's context stays live.
#[test]
fn panicking_reviewer_fails_its_session_and_cancels_the_run() {
    let (_home, cfg) = plan_test_config("");
    let run_ctx = Mutex::new(None::<Context>);
    let ex = fake(|ctx, _, cli, _, _, _| {
        if cli == "claude" {
            *run_ctx.lock().unwrap() = Some(ctx.clone());
            panic!("claude executor blew up");
        }
        ok_plan()
    });
    let (parent, _cancel) = Context::background().with_cancel();
    let work = tempfile::tempdir().unwrap();
    let selected = clis(&["codex", "claude"]);
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut stderr = Vec::new();
        run_plan_review_with(
            &parent,
            &cfg,
            &ex,
            "/tmp/plan.md",
            &batch("", work.path().to_str().unwrap(), "g", &selected),
            &mut stderr,
        )
    }));
    assert!(caught.is_err(), "the panic must propagate");
    let claude = persisted(&cfg, "claude");
    assert_eq!(claude.status, "failed");
    assert_eq!(claude.error_msg, "interrupted");
    assert_eq!(persisted(&cfg, "codex").status, "completed");
    let run_ctx = run_ctx.lock().unwrap().clone().unwrap();
    assert_eq!(run_ctx.err(), Some(ContextError::Canceled));
    assert_eq!(parent.err(), None);
}

/// Controller feedback 2: after a normal return the run context is
/// cancelled; the caller's context is not.
#[test]
fn run_context_is_cancelled_after_the_batch() {
    let (_home, cfg) = plan_test_config("");
    let run_ctx = Mutex::new(None::<Context>);
    let ex = fake(|ctx, _, _, _, _, _| {
        assert_eq!(ctx.err(), None, "run context done while running");
        *run_ctx.lock().unwrap() = Some(ctx.clone());
        ok_plan()
    });
    let (parent, _cancel) = Context::background().with_cancel();
    let work = tempfile::tempdir().unwrap();
    let selected = clis(&["claude"]);
    let mut stderr = Vec::new();
    run_plan_review_with(
        &parent,
        &cfg,
        &ex,
        "/tmp/plan.md",
        &batch("", work.path().to_str().unwrap(), "g", &selected),
        &mut stderr,
    )
    .unwrap();
    let run_ctx = run_ctx.lock().unwrap().clone().unwrap();
    assert_eq!(run_ctx.err(), Some(ContextError::Canceled));
    assert_eq!(parent.err(), None);
}

// ---- the language pass ----

/// A plan answer the checker flags: "utilize" is not an approved word.
fn flagged_plan(cli: &str) -> String {
    format!(r#"{{"summary":"The {cli} plan can utilize the cache.","rating":6,"findings":[]}}"#)
}

fn repaired_plan(cli: &str) -> String {
    format!(r#"{{"summary":"The {cli} plan can use the cache.","rating":6,"findings":[]}}"#)
}

/// Each CLI writes its flagged plan to the session log, then answers the
/// repair call in its repair log: claude with a good reply, codex with
/// `codex_reply`.
fn repairing_executor<'a>(
    calls: &'a Mutex<Vec<(String, String, Option<String>)>>,
    codex_reply: &'a str,
) -> PlanExecutor<'a> {
    PlanExecutor {
        preflight: Box::new(|_| Ok(())),
        run: Box::new(move |_, sess, cli, _prompt, effort, _, log| {
            calls.lock().unwrap().push((
                cli.to_string(),
                effort.to_string(),
                log.map(str::to_string),
            ));
            let (path, text) = match log {
                None => (sess.log_file.clone(), flagged_plan(cli)),
                Some(log) if cli == "codex" => (log.to_string(), codex_reply.to_string()),
                Some(log) => (log.to_string(), repaired_plan(cli)),
            };
            std::fs::write(&path, &text)?;
            Ok((text.into_bytes(), 0))
        }),
    }
}

#[test]
fn each_plan_model_repairs_its_own_block() {
    let (_home, cfg) = plan_test_config("");
    let calls = Mutex::new(Vec::new());
    let ex = repairing_executor(&calls, "not json");
    let out = run_plan(&cfg, &ex, "", "g", &clis(&["codex", "claude"])).unwrap();

    drop(ex);
    let mut calls = calls.into_inner().unwrap();
    calls.sort();
    let repair_calls: Vec<_> = calls.iter().filter(|c| c.2.is_some()).collect();
    assert_eq!(repair_calls.len(), 2, "{calls:?}");
    for (cli, effort, log) in repair_calls {
        assert_eq!(effort, "low");
        let sess = persisted(&cfg, cli);
        assert_eq!(
            log.as_deref(),
            Some(format!("{}.repair.log", sess.log_file).as_str())
        );
        assert_eq!(sess.status, "completed");
        assert_eq!(sess.mode, MODE_PLAN);
    }

    // Claude's repair passes; codex's bad reply keeps its review.
    let summary = |cli: &str| {
        let r = out.results.iter().find(|r| r.cli == cli).unwrap();
        r.parsed.as_ref().unwrap().summary.clone()
    };
    assert_eq!(summary("claude"), "The claude plan can use the cache.");
    assert_eq!(summary("codex"), "The codex plan can utilize the cache.");

    let claude_log = std::fs::read_to_string(persisted(&cfg, "claude").log_file).unwrap();
    assert_eq!(
        claude_log,
        format!("{}\n{}\n", flagged_plan("claude"), repaired_plan("claude"))
    );
    let codex_log = std::fs::read_to_string(persisted(&cfg, "codex").log_file).unwrap();
    assert_eq!(codex_log, flagged_plan("codex"));
}

#[test]
fn a_plan_repair_that_changes_the_rating_keeps_the_review() {
    let (_home, cfg) = plan_test_config("");
    let calls = Mutex::new(Vec::new());
    let moved = repaired_plan("codex").replace("\"rating\":6", "\"rating\":9");
    let ex = repairing_executor(&calls, &moved);
    let out = run_plan(&cfg, &ex, "", "g", &clis(&["codex"])).unwrap();
    let parsed = out.results[0].parsed.as_ref().unwrap();
    assert_eq!(parsed.rating, 6);
    assert_eq!(parsed.summary, "The codex plan can utilize the cache.");
}
