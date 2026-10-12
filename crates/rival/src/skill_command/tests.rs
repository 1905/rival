//! `rival command skill`: host to model routing, the security route, the
//! usage, `--host` errors and the preflight hint.

use std::cell::RefCell;

use super::*;
use crate::testutil::{
    FakeStdin, Fixture, SharedRun, execute, fake_run, fake_spec, no_mr, with_env,
};

/// What the fake routes saw.
#[derive(Default)]
struct Seen {
    /// The model names asked of the spec route, in order.
    names: Vec<String>,
    security_prompt: Option<String>,
}

/// A security executor whose preflight passes and whose run records the
/// prompt, then fails, so the test needs no valid security output.
fn security_fake(seen: &RefCell<Seen>) -> SecurityExecutor<'_> {
    SecurityExecutor {
        preflight: Box::new(|_, _, _| Ok(())),
        run: Box::new(move |_, _, _, prompt, _, _, _, _| {
            seen.borrow_mut().security_prompt = Some(prompt.to_string());
            anyhow::bail!("fake security run")
        }),
    }
}

/// Runs the skill command for `host` with `input` on stdin; the spec route
/// records the name and runs the fake provider `f`.
fn run_skill(
    fix: &Fixture,
    host: SkillHost,
    stdin: &mut FakeStdin,
    f: &SharedRun,
    seen: &RefCell<Seen>,
) -> crate::testutil::Outcome {
    let workdir = tempfile::tempdir().unwrap();
    let workdir = workdir.path().to_str().unwrap().to_string();
    let security = security_fake(seen);
    let spec = |name: &str| {
        seen.borrow_mut().names.push(name.to_string());
        Some(fake_spec(f))
    };
    with_env(fix, stdin, &*no_mr(), |env| {
        run_skill_command(
            env,
            host,
            &workdir,
            true,
            &SkillRoutes {
                spec: &spec,
                security: &security,
            },
        )
    })
}

/// With no `skill:` block, Claude Code runs sol and Codex runs claude.
#[test]
fn each_host_runs_its_default_model() {
    for (host, want) in [(SkillHost::Claude, "sol"), (SkillHost::Codex, "claude")] {
        let fix = Fixture::new();
        let f = fake_run("ok\n");
        let seen = RefCell::new(Seen::default());
        let mut stdin = FakeStdin::new("explain the auth flow");
        let out = run_skill(&fix, host, &mut stdin, &f, &seen);
        out.result.unwrap();
        assert_eq!(seen.borrow().names, [want], "{host:?}");
        assert_eq!(f.borrow().prompt, "explain the auth flow");
        assert_eq!(out.stdout, "ok\n");
        assert_eq!(fix.sessions().len(), 1);
    }
}

/// `skill.claude` and `skill.codex` pick each host's model.
#[test]
fn each_host_runs_its_configured_model() {
    let fix = Fixture::with_config_yaml("skill:\n  claude: fable\n  codex: opus\n");
    for (host, want) in [(SkillHost::Claude, "fable"), (SkillHost::Codex, "claude")] {
        let f = fake_run("ok\n");
        let seen = RefCell::new(Seen::default());
        let mut stdin = FakeStdin::new("-re high explain");
        let out = run_skill(&fix, host, &mut stdin, &f, &seen);
        out.result.unwrap();
        assert_eq!(seen.borrow().names, [want], "{host:?}");
        assert_eq!(f.borrow().effort, "high", "-re reaches the model run");
        assert_eq!(stdin.reads, 1, "stdin is read once");
    }
}

/// `security [scope]` runs the security review with the rest as its scope;
/// no model spec is resolved.
#[test]
fn security_goes_to_the_security_review() {
    for (input, scope) in [
        ("security src/api/", Some("src/api/")),
        ("  security\n", None),
        ("security\tinternal/auth/\n", Some("internal/auth/")),
    ] {
        let fix = Fixture::new();
        let f = fake_run("");
        let seen = RefCell::new(Seen::default());
        let mut stdin = FakeStdin::new(input);
        let out = run_skill(&fix, SkillHost::Claude, &mut stdin, &f, &seen);
        assert_eq!(
            out.result,
            Err(CmdError::plain("fake security run")),
            "{input:?}"
        );
        let seen = seen.borrow();
        assert!(seen.names.is_empty(), "{input:?}");
        assert!(!f.borrow().called, "{input:?}");
        let prompt = seen.security_prompt.as_deref().expect("security ran");
        assert!(prompt.contains("Security"), "{input:?}");
        if let Some(scope) = scope {
            assert!(prompt.contains(scope), "{input:?}: {prompt:.300}");
        }
        let sessions = fix.sessions();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].mode, "security");
    }
}

/// A word that only starts with `security` is a prompt for the model.
#[test]
fn a_longer_word_is_not_the_security_route() {
    let fix = Fixture::new();
    let f = fake_run("ok\n");
    let seen = RefCell::new(Seen::default());
    let mut stdin = FakeStdin::new("securityfix the parser");
    run_skill(&fix, SkillHost::Codex, &mut stdin, &f, &seen)
        .result
        .unwrap();
    assert_eq!(seen.borrow().names, ["claude"]);
    assert!(seen.borrow().security_prompt.is_none());
    assert_eq!(f.borrow().prompt, "securityfix the parser");
}

/// A terminal stdin, empty args and `-re high` alone print the `/rival`
/// usage, not the resolved model's.
#[test]
fn usage_is_the_rival_usage() {
    for input in ["", "  \n", "-re high"] {
        let fix = Fixture::new();
        let f = fake_run("");
        let seen = RefCell::new(Seen::default());
        let mut stdin = FakeStdin::new(input);
        let out = run_skill(&fix, SkillHost::Claude, &mut stdin, &f, &seen);
        out.result.unwrap();
        assert_eq!(out.stdout, format!("{RIVAL_USAGE}\n"), "{input:?}");
        assert!(!f.borrow().called);
        assert!(fix.sessions().is_empty());
    }
    let fix = Fixture::new();
    let f = fake_run("");
    let seen = RefCell::new(Seen::default());
    let mut stdin = FakeStdin::new("");
    stdin.char_device = true;
    stdin.forbid_read = true;
    let out = run_skill(&fix, SkillHost::Codex, &mut stdin, &f, &seen);
    out.result.unwrap();
    assert_eq!(out.stdout, format!("{RIVAL_USAGE}\n"));
    assert!(seen.borrow().names.is_empty());
}

/// The usage names no model, so the skills can print it as is.
#[test]
fn usage_names_no_model() {
    let lower = RIVAL_USAGE.to_lowercase();
    for word in [
        "opus", "sol ", "sol.", "astra", "gpt", "fable", "kimi", "k3", "grok",
    ] {
        assert!(!lower.contains(word), "{word:?} in RIVAL_USAGE");
    }
}

/// A failed preflight says how to pick another model for the host.
#[test]
fn preflight_failure_names_the_config_key() {
    let fix = Fixture::new();
    let f = fake_run("");
    let seen = RefCell::new(Seen::default());
    let security = security_fake(&seen);
    let spec = |_: &str| {
        let mut spec = fake_spec(&f);
        spec.preflight = Box::new(|_, _| anyhow::bail!("codex runtime is not installed"));
        Some(spec)
    };
    let workdir = tempfile::tempdir().unwrap();
    let mut stdin = FakeStdin::new("explain");
    let out = with_env(&fix, &mut stdin, &*no_mr(), |env| {
        run_skill_command(
            env,
            SkillHost::Claude,
            workdir.path().to_str().unwrap(),
            true,
            &SkillRoutes {
                spec: &spec,
                security: &security,
            },
        )
    });
    assert_eq!(
        out.result,
        Err(CmdError::plain(
            "codex runtime is not installed\nset another model: rival config set skill.claude <model> (codex, sol, claude, fable, k3, grok)"
        ))
    );
    assert!(fix.sessions().is_empty());
}

/// `--host` is required and takes only claude or codex; the error comes
/// before stdin is read.
#[test]
fn host_must_be_claude_or_codex() {
    for (args, want) in [
        (
            vec!["command", "skill"],
            "rival command skill: --host is required: claude or codex\n",
        ),
        (
            vec!["command", "skill", "--host", "sol"],
            "rival command skill: invalid --host \"sol\"; use claude or codex\n",
        ),
    ] {
        let fix = Fixture::new();
        let mut stdin = FakeStdin::new("explain");
        stdin.forbid_read = true;
        let (code, out, err) = execute(&fix, &mut stdin, &args);
        assert_eq!(
            (code, out.as_str(), err.as_str()),
            (1, "", want),
            "{args:?}"
        );
    }
}

/// Every name `skill.<host>` accepts has a spec with that command name.
#[test]
fn every_skill_model_name_has_a_spec() {
    for name in SKILL_MODEL_NAMES {
        let spec = spec_for_name(name).expect(name);
        assert_eq!(spec.command_name, name);
    }
    assert!(spec_for_name("opus").is_none());
}
