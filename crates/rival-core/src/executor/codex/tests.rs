//! Codex executor tests: exact argv/request, error wrapping and preflight
//! checks against task-owned fakes.

use super::*;
#[cfg(unix)]
use crate::executor::testutil::retry_busy;
use crate::executor::testutil::{Env, Spawned, recorder, strings};

fn argv(effort: &str, workdir: &str) -> Vec<String> {
    let effort_arg = format!("model_reasoning_effort={effort}");
    strings(&[
        "exec",
        "-C",
        workdir,
        "-m",
        config::CODEX_MODEL,
        "-c",
        &effort_arg,
        "--sandbox",
        "read-only",
        "--ephemeral",
        "--skip-git-repo-check",
        "--color",
        "never",
        "-",
    ])
}

/// The explicit model and effort reach argv (exact vector).
#[test]
fn codex_run_args_uses_explicit_model_and_effort() {
    for effort in ["high", "ultra"] {
        let args = codex_run_args(config::CODEX_MODEL, effort, "/repo");
        assert_eq!(args, argv(effort, "/repo"));
        let joined = args.join(" ");
        assert!(joined.contains(&format!("-m {}", config::CODEX_MODEL)));
        assert!(joined.contains(&format!("model_reasoning_effort={effort}")));
        assert!(joined.contains("--sandbox read-only"));
    }
    // An empty workdir stays an empty argument after -C.
    assert_eq!(codex_run_args(config::CODEX_MODEL, "", ""), argv("", ""));
}

#[test]
fn codex_passes_ultra_and_xhigh_through_unaliased() {
    for effort in ["xhigh", "ultra"] {
        let args = codex_run_args(config::CODEX_MODEL, effort, "/tmp");
        assert!(
            args.contains(&format!("model_reasoning_effort={effort}")),
            "{args:?}"
        );
    }
}

/// Unsupported, `sol` and empty models are rejected. Nothing is spawned.
#[test]
fn run_codex_model_rejects_unsupported_sol_and_empty() {
    let env = Env::new();
    let cfg = env.config();
    for model in ["retired-model", config::GPT56_SOL_MODEL, ""] {
        let mut sess = Session::default();
        let mut seen = None;
        let err = run_codex_model_with(
            &cfg,
            &mut sess,
            "review",
            "high",
            "/repo",
            model,
            None,
            recorder(&mut seen, Ok(RunResult::default())),
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("unsupported codex model {:?}", model)
        );
        assert!(seen.is_none(), "{model}: spawned");
    }
}

#[test]
fn run_codex_model_builds_the_exact_request() {
    let mut env = Env::new();
    env.set("OPENAI_API_KEY", Some("kept-for-codex"));
    let cfg = env.config();
    let work = env.work_str();
    let mut sess = env.session("codex", "review", config::CODEX_MODEL, &work);
    let mut seen = None;
    let want = RunResult {
        exit_code: 3,
        output_bytes: 7,
        output_lines: 1,
    };
    let got = run_codex_model_with(
        &cfg,
        &mut sess,
        "find the bug",
        "xhigh",
        &work,
        config::CODEX_MODEL,
        None,
        recorder(&mut seen, Ok(want)),
    )
    .unwrap();
    assert_eq!(got, want);
    let seen = seen.unwrap();
    assert_eq!(
        seen,
        Spawned {
            binary: "codex".into(),
            args: argv("xhigh", &work),
            env: vec![],
            prompt: format!(
                "{}\n\n{}\nfind the bug",
                config::SYSTEM_PROMPT,
                cfg.build_workdir_preamble(Path::new(&work))
            ),
            drop_env: vec![],
            environ: cfg.environ().to_vec(),
            mode: "review".into(),
            account: String::new(),
            log: None,
        }
    );
}

/// "Codex", "codex" and the model id all become the label.
#[test]
fn run_codex_model_wraps_errors_with_the_public_label() {
    let env = Env::new();
    let cfg = env.config();
    let mut sess = Session::default();
    let mut seen = None;
    let err = run_codex_model_with(
        &cfg,
        &mut sess,
        "p",
        "high",
        "/repo",
        config::CODEX_MODEL,
        None,
        recorder(
            &mut seen,
            Err(anyhow!("start codex: OpenAI Codex failed for gpt-6-astra")),
        ),
    )
    .unwrap_err();
    assert_eq!(
        format!("{err:#}"),
        "codex runtime: start codex: OpenAI codex failed for codex"
    );
}

#[cfg(unix)]
#[test]
fn codex_preflight_reports_missing_runtime_and_auth() {
    let env = Env::new();
    let cfg = env.config();
    assert_eq!(
        codex_preflight_for(&cfg, config::CODEX_MODEL)
            .unwrap_err()
            .to_string(),
        "codex runtime is not installed"
    );

    // Logged out: the combined output follows the message.
    env.fake(
        "codex",
        "#!/bin/sh\n[ \"$1 $2\" = 'login status' ] || exit 90\necho 'Not logged in'\necho 'warn' >&2\nexit 1\n",
    );
    let err = retry_busy(
        || codex_preflight_for(&cfg, config::CODEX_MODEL),
        |r| format!("{r:?}"),
    );
    assert_eq!(
        err.unwrap_err().to_string(),
        "codex authentication is unavailable\nNot logged in\nwarn\n"
    );

    env.fake(
        "codex",
        "#!/bin/sh\n[ \"$1 $2\" = 'login status' ] || exit 90\necho 'Logged in'\n",
    );
    retry_busy(
        || codex_preflight_for(&cfg, config::CODEX_MODEL),
        |r| format!("{r:?}"),
    )
    .unwrap();
}

/// End to end through the real subprocess runner: the fake reads the whole
/// prompt, then prints its argv and the prompt it got.
#[cfg(unix)]
#[test]
fn run_codex_model_runs_the_fake_with_argv_and_prompt() {
    let env = Env::new();
    let cfg = env.config();
    env.fake(
        "codex",
        "#!/bin/sh\nin=$(/bin/cat)\nprintf '%s\\n' \"$@\"\nprintf '%s\\n' \"$in\" | /usr/bin/tail -n 1\n",
    );
    let work = env.work_str();
    let mut sess = env.session("codex", "raw", config::CODEX_MODEL, &work);
    let mut out = Vec::new();
    let result = retry_busy(
        || {
            out.clear();
            run_codex_model(
                &Context::background(),
                &cfg,
                &mut sess,
                "last line",
                "ultra",
                &work,
                config::CODEX_MODEL,
                None,
                Some(&mut out),
            )
        },
        |r| format!("{r:?}"),
    )
    .unwrap();
    assert_eq!(result.exit_code, 0);
    let mut want = argv("ultra", &work).join("\n");
    want.push_str("\nlast line\n");
    assert_eq!(String::from_utf8(out).unwrap(), want);
}

/// The adapter hands the caller's log file to the spawn step.
#[test]
fn run_codex_model_forwards_the_log_file() {
    let env = Env::new();
    let cfg = env.config();
    let work = env.work_str();
    let mut sess = env.session("codex", "review", config::CODEX_MODEL, &work);
    let mut seen = None;
    run_codex_model_with(
        &cfg,
        &mut sess,
        "p",
        "low",
        &work,
        config::CODEX_MODEL,
        Some("/home/s/sessions/x.log.repair.log"),
        recorder(&mut seen, Ok(RunResult::default())),
    )
    .unwrap();
    assert_eq!(
        seen.unwrap().log.as_deref(),
        Some("/home/s/sessions/x.log.repair.log")
    );
}
