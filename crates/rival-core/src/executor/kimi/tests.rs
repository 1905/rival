//! Kimi executor tests: the exact request per mode and
//! an end-to-end check of the env each mode hands the child.

use std::ffi::{OsStr, OsString};

use super::*;
use crate::executor::opencode::{OPENCODE_READ_ONLY_PERMISSION, opencode_run_env_with};
use crate::executor::subprocess::{drop_matches_case, safe_env};
#[cfg(unix)]
use crate::executor::testutil::retry_busy;
use crate::executor::testutil::{Env, path_str, recorder, strings};

fn k3() -> config::SecurityModel {
    config::open_code_entry_for(config::KIMI_MODEL).expect("K3 missing from the registry")
}

/// Entries ending in "_" are prefix
/// drops — "AWS_" must catch the whole credential family; exact entries
/// must not over-match.
#[test]
fn drop_matches_prefix_and_exact() {
    for (kv, want) in [
        ("AWS_ACCESS_KEY_ID=x", true),
        ("AWS_WEB_IDENTITY_TOKEN_FILE=/path", true),
        ("AWS_PROFILE=default", true),
        ("OPENAI_API_KEY=sk-x", true),
        ("OPENAI_API_KEY_BACKUP=sk-x", false), // exact entry, different name
        ("AWSOME_VAR=x", false),               // prefix requires the underscore
        ("PATH=/usr/bin", false),
    ] {
        assert_eq!(
            drop_matches_case(cfg!(windows), OsStr::new(kv), &KIMI_DROP_ENV),
            want,
            "{kv}"
        );
    }
}

/// Project-loaded Moonshot variables
/// never pass through raw to a child; the key reaches OpenCode only through
/// OPENCODE_CONFIG_CONTENT.
#[test]
fn moonshot_env_prefixes_are_blocked() {
    let environ: Vec<OsString> = [
        "PATH=/bin",
        "MOONSHOT_API_KEY=sk-secret",
        "KIMI_API=sk-secret",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    let kept = safe_env(&environ);
    assert_eq!(kept, vec![OsString::from("PATH=/bin")]);
}

/// Run options by mode: only "raw" gets the full-auto profile and the
/// credential strip. Review, the task modes (plan, security) and any other
/// mode keep the read-only reviewer defaults.
#[test]
fn kimi_run_opts_by_mode() {
    let mut env = Env::new();
    env.set("MOONSHOT_API_KEY", Some("test-key"));
    let cfg = env.config();

    for mode in ["review", "plan", "security", "", "native"] {
        let opts = kimi_run_opts(&cfg, mode, &env.work_str());
        assert_eq!(
            opts.permission, "",
            "{mode}: keeps the read-only default profile"
        );
        assert!(
            opts.drop_env.is_empty(),
            "{mode}: needs no extra drops (bash is denied)"
        );
        assert_eq!(opts.api_key, "test-key", "{mode}");
    }

    let raw = kimi_run_opts(&cfg, "raw", &env.work_str());
    assert_eq!(raw.permission, OPENCODE_FULL_AUTO_PERMISSION);
    assert_eq!(raw.drop_env, strings(&KIMI_DROP_ENV));
    assert_eq!(raw.api_key, "test-key");
}

#[test]
fn moonshot_model_uses_kimi_key() {
    let mut env = Env::new();
    env.set("MOONSHOT_API_KEY", Some("sk-moonshot"));
    let joined = opencode_run_env_with(
        &env.config(),
        "sess-1",
        &k3(),
        "",
        &OpencodeRunOpts::default(),
    )
    .join("\n");
    assert!(joined.contains("sk-moonshot"), "{joined}");
    assert!(joined.contains(r#""moonshotai""#), "{joined}");
}

/// The fake opencode exists
/// for the preflight lookup only and fails loudly if it is ever run.
#[cfg(unix)]
#[test]
fn moonshot_fallback_walks_up_from_workdir() {
    let mut env = Env::new();
    env.set("MOONSHOT_API_KEY", Some(""))
        .set("KIMI_API", Some(""));
    env.fake(
        "opencode",
        "#!/bin/sh\necho 'fake opencode must not run' >&2\nexit 97\n",
    );
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join(".env"), "MOONSHOT_API_KEY=sk-walkup\n").unwrap();
    let sub = root.path().join("sub").join("dir");
    std::fs::create_dir_all(&sub).unwrap();
    let sub = path_str(&sub);
    let cfg = env.config();

    let joined =
        opencode_run_env_with(&cfg, "sess-4", &k3(), &sub, &OpencodeRunOpts::default()).join("\n");
    assert!(joined.contains("sk-walkup"), "{joined}");
    crate::executor::opencode_preflight_model(&cfg, config::KIMI_MODEL, &sub).unwrap();
    kimi_preflight(&cfg, &sub).unwrap();
}

#[test]
fn kimi_raw_env_uses_full_auto_permission() {
    let mut env = Env::new();
    env.set("MOONSHOT_API_KEY", Some("test-key"));
    let cfg = env.config();
    let opts = kimi_run_opts(&cfg, "raw", &env.work_str());
    let joined = opencode_run_env_with(&cfg, "sess-3", &k3(), "", &opts).join("\n");
    assert!(
        joined.contains(&format!(
            "OPENCODE_PERMISSION={OPENCODE_FULL_AUTO_PERMISSION}"
        )),
        "{joined}"
    );
}

#[cfg(unix)]
#[test]
fn kimi_preflight_branches() {
    let mut env = Env::new();
    assert_eq!(
        kimi_preflight(&env.config(), "").unwrap_err().to_string(),
        "opencode CLI not installed. Install: curl -fsSL https://opencode.ai/install | bash"
    );
    env.fake("opencode", "#!/bin/sh\nexit 97\n");
    assert_eq!(
        kimi_preflight(&env.config(), "").unwrap_err().to_string(),
        "MOONSHOT_API_KEY is not set — add it to the project .env or export it"
    );
    // The legacy KIMI_API alias also counts.
    env.set("KIMI_API", Some("sk-alias"));
    kimi_preflight(&env.config(), "").unwrap();
}

/// The exact request per mode: the key comes from `cred_workdir`, the run
/// from `workdir`; effort is pinned to K3's max variant.
#[test]
fn run_kimi_request_per_mode() {
    let env = Env::new();
    let cred = tempfile::tempdir().unwrap();
    std::fs::write(cred.path().join(".env"), "MOONSHOT_API_KEY=sk-cred\n").unwrap();
    let cred = path_str(cred.path());
    let cfg = env.config();
    let work = env.work_str();
    let provider = r#"{"$schema":"https://opencode.ai/config.json","provider":{"moonshotai":{"options":{"apiKey":"sk-cred"}}}}"#;

    for (mode, permission, extra_drop) in [
        ("review", OPENCODE_READ_ONLY_PERMISSION, &[][..]),
        ("raw", OPENCODE_FULL_AUTO_PERMISSION, &KIMI_DROP_ENV[..]),
        ("plan", OPENCODE_READ_ONLY_PERMISSION, &[][..]),
        ("security", OPENCODE_READ_ONLY_PERMISSION, &[][..]),
    ] {
        let mut sess = env.session("opencode", mode, config::KIMI_MODEL, &work);
        let mut seen = None;
        run_kimi_with(
            &cfg,
            &mut sess,
            "look",
            &work,
            &cred,
            None,
            recorder(&mut seen, Ok(RunResult::default())),
        )
        .unwrap();
        let seen = seen.unwrap();
        assert_eq!(seen.binary, "opencode");
        assert_eq!(
            seen.args,
            strings(&[
                "run",
                "--pure",
                "-m",
                config::KIMI_MODEL,
                "--variant",
                "max",
                "--dir",
                &work
            ])
        );
        assert_eq!(
            seen.env,
            vec![
                format!("OPENCODE_PERMISSION={permission}"),
                format!("OPENCODE_DB=rival-{}.db", sess.id),
                format!("OPENCODE_CONFIG_CONTENT={provider}"),
            ],
            "{mode}"
        );
        let mut want_drop = strings(&[
            "OPENCODE_PERMISSION",
            "OPENCODE_CONFIG_CONTENT",
            "OPENCODE_DB",
        ]);
        want_drop.extend(strings(extra_drop));
        assert_eq!(seen.drop_env, want_drop, "{mode}");
        assert_eq!(
            seen.prompt,
            format!(
                "{}\n\n{}\nlook",
                config::SYSTEM_PROMPT,
                cfg.build_workdir_preamble(Path::new(&work))
            )
        );
        assert_eq!(seen.mode, mode, "kimi never rewrites the session mode");
    }
}

/// End to end: the fake opencode drains the prompt and prints its env.
/// Review and plan keep inherited credentials (bash is denied there); raw
/// strips them. Blocked prefixes and inherited OPENCODE_* never pass.
#[cfg(unix)]
#[test]
fn kimi_child_env_per_mode() {
    let mut env = Env::new();
    env.set("MOONSHOT_API_KEY", Some("sk-env"))
        .set("AWS_PROFILE", Some("prod"))
        .set("GITHUB_TOKEN", Some("ghp"))
        .set("OPENAI_API_KEY_BACKUP", Some("kept"))
        .set("OPENCODE_PERMISSION", Some("{}"));
    env.fake("opencode", "#!/bin/sh\n/bin/cat >/dev/null\n/usr/bin/env\n");
    let cfg = env.config();
    let work = env.work_str();
    for (mode, stripped) in [("review", false), ("raw", true), ("plan", false)] {
        let mut sess = env.session("opencode", mode, config::KIMI_MODEL, &work);
        let mut out = Vec::new();
        retry_busy(
            || {
                out.clear();
                run_kimi(
                    &Context::background(),
                    &cfg,
                    &mut sess,
                    "p",
                    &work,
                    &work,
                    None,
                    Some(&mut out),
                )
            },
            |r| format!("{r:?}"),
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        let has = |kv: &str| text.lines().any(|l| l == kv);
        assert_eq!(has("AWS_PROFILE=prod"), !stripped, "{mode}");
        assert_eq!(has("GITHUB_TOKEN=ghp"), !stripped, "{mode}");
        assert!(has("OPENAI_API_KEY_BACKUP=kept"), "{mode}");
        assert!(!text.contains("MOONSHOT_API_KEY"), "{mode}");
        assert!(!has("OPENCODE_PERMISSION={}"), "{mode}");
        let permission = if mode == "raw" {
            OPENCODE_FULL_AUTO_PERMISSION
        } else {
            OPENCODE_READ_ONLY_PERMISSION
        };
        assert!(has(&format!("OPENCODE_PERMISSION={permission}")), "{mode}");
        assert!(text.contains(r#""apiKey":"sk-env""#), "{mode}");
    }
}

/// The adapter hands the caller's log file to the spawn step.
#[test]
fn run_kimi_forwards_the_log_file() {
    let env = Env::new();
    let cfg = env.config();
    let work = env.work_str();
    let mut sess = env.session("opencode", "review", config::KIMI_MODEL, &work);
    let mut seen = None;
    run_kimi_with(
        &cfg,
        &mut sess,
        "p",
        &work,
        &work,
        Some("/home/s/sessions/x.log.repair.log"),
        recorder(&mut seen, Ok(RunResult::default())),
    )
    .unwrap();
    assert_eq!(
        seen.unwrap().log.as_deref(),
        Some("/home/s/sessions/x.log.repair.log")
    );
}
