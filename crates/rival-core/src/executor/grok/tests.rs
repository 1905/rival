//! Go: `internal/executor/grok_test.go`, plus exact argv vectors, the prompt
//! file lifetime, error wrapping and preflight checks.

use super::*;
#[cfg(unix)]
use crate::executor::testutil::retry_busy;
use crate::executor::testutil::{Env, path_str, strings};

/// grok's argv for the given parts.
fn argv(model: &str, file: &str, effort: &str, workdir: &str, review: bool) -> Vec<String> {
    let mut args = strings(&[
        "--prompt-file",
        file,
        "-m",
        model,
        "--effort",
        effort,
        "--output-format",
        "plain",
        "--no-auto-update",
        "--yolo",
    ]);
    if !workdir.is_empty() {
        args.extend(strings(&["--cwd", workdir]));
    }
    if review {
        args.extend(strings(&["--sandbox", "read-only"]));
    }
    args
}

/// Go `argValue`: the value following `flag`.
fn arg_value<'a>(args: &'a [String], flag: &str) -> &'a str {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .map_or("", String::as_str)
}

/// Go: TestGrokEffort.
#[test]
fn grok_effort_cases() {
    for (effort, want) in [
        ("low", "low"),
        ("medium", "medium"),
        ("high", "high"),
        // grok-4.6 has no reasoning level above high; rival's richer menu
        // clamps down.
        ("xhigh", "high"),
        ("ultra", "high"),
        ("max", "high"),
        ("minimal", "low"),
        ("none", "low"),
    ] {
        assert_eq!(grok_effort(effort).unwrap(), want, "{effort}");
    }
    assert_eq!(
        grok_effort("").unwrap_err().to_string(),
        "effort is required for grok"
    );
    assert_eq!(
        grok_effort("turbo").unwrap_err().to_string(),
        "unsupported grok effort \"turbo\"; use one of: low, medium, high"
    );
}

/// Go: TestGrokRunArgs_AlwaysPassesPromptFileAndRuntimeFlags.
#[test]
fn grok_run_args_always_passes_prompt_file_and_runtime_flags() {
    let args = grok_run_args(
        config::GROK_MODEL,
        "/tmp/rival-grok-1.md",
        "high",
        "",
        false,
    )
    .unwrap();
    assert_eq!(
        args,
        argv(
            config::GROK_MODEL,
            "/tmp/rival-grok-1.md",
            "high",
            "",
            false
        )
    );
    // The prompt travels by file, never inline: a bare -p would both
    // duplicate it and expose it in the process table.
    assert!(!args.iter().any(|a| a == "-p"));
    assert!(!args.iter().any(|a| a == "--cwd"));
    assert!(!args.iter().any(|a| a == "--sandbox"));
}

/// Go: TestGrokRunArgs_WorkdirAndReviewSandbox.
#[test]
fn grok_run_args_workdir_and_review_sandbox() {
    for (name, workdir, review) in [
        ("raw_no_workdir", "", false),
        ("raw_with_workdir", "/repo", false),
        ("review_no_workdir", "", true),
        ("review_with_workdir", "/repo", true),
    ] {
        let args =
            grok_run_args(config::GROK_MODEL, "/tmp/p.md", "medium", workdir, review).unwrap();
        assert_eq!(
            args,
            argv(config::GROK_MODEL, "/tmp/p.md", "medium", workdir, review),
            "{name}"
        );
    }
}

/// Go: TestGrokModelOrDefault.
#[test]
fn grok_model_or_default_cases() {
    assert_eq!(grok_model_or_default(""), config::GROK_MODEL);
    assert_eq!(grok_model_or_default("   "), config::GROK_MODEL);
    assert_eq!(grok_model_or_default("grok-4.5-fast"), "grok-4.5-fast");
}

/// Go: TestGrokRunArgs_ThreadsExplicitModel.
#[test]
fn grok_run_args_threads_explicit_model() {
    let args = grok_run_args("grok-4.5-fast", "/tmp/p.md", "high", "/repo", true).unwrap();
    // config::GROK_MODEL is a prefix of the test model, so compare exactly.
    assert_eq!(arg_value(&args, "-m"), "grok-4.5-fast");
    let fallback = grok_run_args("", "/tmp/p.md", "high", "/repo", true).unwrap();
    assert_eq!(arg_value(&fallback, "-m"), config::GROK_MODEL);
}

/// Go: TestGrokRunArgs_PropagatesEffortError.
#[test]
fn grok_run_args_propagates_effort_error() {
    assert!(grok_run_args(config::GROK_MODEL, "/tmp/p.md", "turbo", "/repo", true).is_err());
}

/// Go: TestGrokFullPrompt_MatchesSharedComposition.
#[test]
fn grok_full_prompt_matches_shared_composition() {
    let cfg = Env::new().config();
    let want = format!(
        "{}\n\n{}\nreview this diff",
        config::SYSTEM_PROMPT,
        cfg.build_workdir_preamble(Path::new("/repo"))
    );
    assert_eq!(grok_full_prompt(&cfg, "review this diff", "/repo"), want);
}

/// The variable Go's `os.TempDir` reads first: `TMPDIR` on Unix, `TMP` on
/// Windows. Tests set it in the injected config only.
const TMP_VAR: &str = if cfg!(windows) { "TMP" } else { "TMPDIR" };

/// An env whose temp dir is a fresh dir, so the prompt file can be watched.
fn tmp_env() -> (Env, tempfile::TempDir) {
    let mut env = Env::new();
    let tmp = tempfile::tempdir().unwrap();
    env.set(TMP_VAR, Some(&path_str(tmp.path())));
    (env, tmp)
}

fn tmp_is_empty(tmp: &tempfile::TempDir) -> bool {
    std::fs::read_dir(tmp.path()).unwrap().count() == 0
}

/// Go: TestRunGrokModel_ThreadsModelToArgv. The prompt file exists with the
/// full prompt during the spawn, stdin carries nothing, and the file is
/// gone afterwards.
#[test]
fn run_grok_model_threads_model_to_argv() {
    for (name, model, want) in [
        ("explicit model", "grok-4.5-fast", "grok-4.5-fast"),
        ("empty model falls back", "", config::GROK_MODEL),
    ] {
        let (env, tmp) = tmp_env();
        let cfg = env.config();
        let work = env.work_str();
        let mut sess = Session::default();
        let mut seen = None;
        run_grok_model_with(
            &cfg,
            &mut sess,
            "prompt",
            "high",
            &work,
            model,
            true,
            |_, req| {
                let file = arg_value(req.args, "--prompt-file").to_string();
                let content = std::fs::read_to_string(&file).unwrap();
                seen = Some((
                    req.binary.to_string(),
                    req.args.to_vec(),
                    req.prompt.to_string(),
                    file,
                    content,
                ));
                Ok(RunResult::default())
            },
        )
        .unwrap();
        let (binary, args, stdin, file, content) = seen.unwrap();
        assert_eq!(binary, "grok", "{name}");
        assert_eq!(arg_value(&args, "-m"), want, "{name}");
        assert_eq!(args, argv(want, &file, "high", &work, true), "{name}");
        assert_eq!(stdin, "", "{name}: stdin must carry nothing");
        // Go os.CreateTemp joins with the host separator.
        assert!(
            file.starts_with(&format!(
                "{}{}rival-grok-",
                path_str(tmp.path()),
                std::path::MAIN_SEPARATOR
            )),
            "{file}"
        );
        assert!(file.ends_with(".md"), "{file}");
        assert_eq!(content, grok_full_prompt(&cfg, "prompt", &work));
        assert!(tmp_is_empty(&tmp), "{name}: prompt file left behind");
    }
}

/// Go: TestRunGrok_SendsDefaultModel, end to end: the fake grok reads the
/// prompt file while it runs and prints it after its argv.
#[cfg(unix)]
#[test]
fn run_grok_sends_default_model() {
    let (env, tmp) = tmp_env();
    env.fake(
        "grok",
        "#!/bin/sh\n/bin/cat >/dev/null\nprintf '%s\\n' \"$@\"\n/bin/cat \"$2\"\n",
    );
    let cfg = env.config();
    let work = env.work_str();
    let mut sess = env.session("grok", "raw", config::GROK_MODEL, &work);
    let mut out = Vec::new();
    let result = retry_busy(
        || {
            out.clear();
            run_grok(
                &Context::background(),
                &cfg,
                &mut sess,
                "prompt",
                "high",
                &work,
                false,
                Some(&mut out),
            )
        },
        |r| format!("{r:?}"),
    )
    .unwrap();
    assert_eq!(result.exit_code, 0);
    let text = String::from_utf8(out).unwrap();
    let args: Vec<String> = text.lines().take(12).map(str::to_string).collect();
    assert_eq!(arg_value(&args, "-m"), config::GROK_MODEL);
    let file = arg_value(&args, "--prompt-file").to_string();
    assert_eq!(args, argv(config::GROK_MODEL, &file, "high", &work, false));
    let prompt = grok_full_prompt(&cfg, "prompt", &work);
    assert_eq!(text, format!("{}\n{prompt}", args.join("\n")));
    assert!(tmp_is_empty(&tmp), "prompt file left behind");
}

#[test]
fn run_grok_model_errors_remove_the_prompt_file() {
    // An effort error comes after the file was written.
    let (env, tmp) = tmp_env();
    let cfg = env.config();
    let mut sess = Session::default();
    let mut spawned = false;
    let err = run_grok_model_with(&cfg, &mut sess, "p", "turbo", "/repo", "", true, |_, _| {
        spawned = true;
        Ok(RunResult::default())
    })
    .unwrap_err();
    assert_eq!(
        format!("{err:#}"),
        "grok runtime: unsupported grok effort \"turbo\"; use one of: low, medium, high"
    );
    assert!(!spawned);
    assert!(tmp_is_empty(&tmp));

    // A spawn error is wrapped once.
    let err = run_grok_model_with(&cfg, &mut sess, "p", "low", "/repo", "", false, |_, _| {
        Err(anyhow!(
            "start grok: exec: \"grok\": executable file not found in $PATH"
        ))
    })
    .unwrap_err();
    assert_eq!(
        format!("{err:#}"),
        "grok runtime: start grok: exec: \"grok\": executable file not found in $PATH"
    );
    assert!(tmp_is_empty(&tmp));

    // No temp dir: the create error is wrapped.
    let mut env = Env::new();
    let missing = path_str(&tmp.path().join("nonexistent-rival-tmp"));
    env.set(TMP_VAR, Some(&missing));
    let err = run_grok_model_with(
        &env.config(),
        &mut sess,
        "p",
        "low",
        "",
        "",
        false,
        |_, _| Ok(RunResult::default()),
    )
    .unwrap_err()
    .to_string();
    assert!(
        err.starts_with(&format!(
            "grok runtime: create prompt file: open {missing}{}rival-grok-",
            std::path::MAIN_SEPARATOR
        )),
        "{err}"
    );
    assert!(
        err.ends_with(&format!(".md: {}", crate::errtext::NO_SUCH_PATH)),
        "{err}"
    );
}

#[cfg(unix)]
#[test]
fn grok_preflight_branches() {
    let mut env = Env::new();
    assert_eq!(
        grok_preflight(&env.config()).unwrap_err().to_string(),
        "grok runtime is not installed"
    );

    env.fake("grok", "#!/bin/sh\nexit 97\n");
    let auth = format!("{}/.grok/auth.json", path_str(env.home.path()));
    assert_eq!(
        grok_preflight(&env.config()).unwrap_err().to_string(),
        format!("grok authentication is unavailable ({auth} not found); run `grok login`")
    );

    std::fs::create_dir(env.home.path().join(".grok")).unwrap();
    std::fs::write(&auth, "{}").unwrap();
    grok_preflight(&env.config()).unwrap();

    // $GROK_HOME is ignored: only the real home counts.
    env.set("GROK_HOME", Some("/nonexistent"));
    grok_preflight(&env.config()).unwrap();

    env.set("HOME", Some(""));
    assert_eq!(
        grok_preflight(&env.config()).unwrap_err().to_string(),
        "grok authentication is unavailable: $HOME is not defined; run `grok login`"
    );
}
