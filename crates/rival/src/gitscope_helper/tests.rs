//! Pins of the scope and auto-detect branches against temp git repos.
//! The child env is explicit (process `PATH`, temp `HOME`); no test touches
//! this worktree's repo.

use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Stdio};

use rival_core::executor::process;
use rival_core::paths::Paths;

use super::*;

struct Fixture {
    home: tempfile::TempDir,
    vars: HashMap<String, String>,
}

impl Fixture {
    fn new() -> Fixture {
        let home = tempfile::tempdir().unwrap();
        let mut vars = HashMap::new();
        vars.insert(
            "PATH".to_string(),
            std::env::var("PATH").unwrap_or_default(),
        );
        vars.insert("HOME".to_string(), s(home.path()));
        vars.insert("GIT_CONFIG_NOSYSTEM".to_string(), "1".to_string());
        Fixture { home, vars }
    }

    fn config(&self) -> Config {
        Config::new(Paths::from_home(self.home.path()), self.vars.clone(), None)
    }

    fn git(&self, dir: &Path, args: &[&str]) {
        let mut cmd = Command::new("git");
        cmd.args(args)
            .current_dir(dir)
            .env_clear()
            .envs(&self.vars)
            .envs([
                ("GIT_AUTHOR_NAME", "test"),
                ("GIT_AUTHOR_EMAIL", "test@test"),
                ("GIT_COMMITTER_NAME", "test"),
                ("GIT_COMMITTER_EMAIL", "test@test"),
            ])
            // `output()`'s defaults, spawned under the fork lock.
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let out = process::spawn(&mut cmd)
            .and_then(std::process::Child::wait_with_output)
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn init_repo(&self) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        self.git(dir.path(), &["init"]);
        std::fs::write(dir.path().join("a.go"), "package a\n").unwrap();
        self.git(dir.path(), &["add", "."]);
        self.git(dir.path(), &["commit", "-m", "init"]);
        dir
    }
}

fn s(p: &Path) -> String {
    p.to_str().unwrap().to_string()
}

fn scope_template(scope: &str) -> String {
    "Review scope: {SCOPE}\n\nRules:\n- be brief\n".replace("{SCOPE}", scope)
}

#[test]
fn build_review_prompt_explicit_scope() {
    let fx = Fixture::new();
    let dir = tempfile::tempdir().unwrap();
    let (prompt, target, display) = build_review_prompt(
        scope_template,
        "src/api/",
        false,
        &fx.config(),
        &s(dir.path()),
    );
    assert!(prompt.contains("Review scope: src/api/"), "{prompt}");
    assert!(!prompt.contains("{SCOPE}"), "placeholder left in prompt");
    assert_eq!(
        (target.as_str(), display.as_str()),
        ("src/api/", "src/api/")
    );
}

#[test]
fn build_review_prompt_no_changes_falls_back_to_project() {
    // A fresh temp dir is not a git repo, so auto-detect finds nothing.
    let fx = Fixture::new();
    let dir = tempfile::tempdir().unwrap();
    let (prompt, target, display) = build_review_prompt(
        scope_template,
        "the entire project",
        true,
        &fx.config(),
        &s(dir.path()),
    );
    assert!(
        prompt.contains("Review scope: the entire project"),
        "{prompt}"
    );
    assert_eq!(
        (target.as_str(), display.as_str()),
        ("the entire project", "the entire project")
    );
    assert!(prompt.ends_with("Rules:\n- be brief\n"), "{prompt}");
}

// Rust-only pins.

#[test]
fn auto_scope_ignores_the_given_scope() {
    // No changes: WHOLE_PROJECT replaces whatever scope was passed.
    let fx = Fixture::new();
    let dir = tempfile::tempdir().unwrap();
    let got = build_review_prompt(
        |sc| format!("<{sc}>"),
        "src/",
        true,
        &fx.config(),
        &s(dir.path()),
    );
    assert_eq!(
        got,
        (
            "<the entire project>".to_string(),
            WHOLE_PROJECT.to_string(),
            WHOLE_PROJECT.to_string()
        )
    );
}

#[test]
fn auto_scope_with_changes_prepends_the_preamble() {
    let fx = Fixture::new();
    let cfg = fx.config();
    let dir = fx.init_repo();
    let wd = s(dir.path());

    // Untracked-only: files are found, but the diff stat is empty.
    std::fs::write(dir.path().join("new.go"), "package new\n").unwrap();
    let (preamble, files) = build_diff_preamble(&cfg, &wd);
    assert_eq!(files, "new.go");
    assert_eq!(
        preamble,
        "The following files have uncommitted changes (or were changed in the last commit). \
         Focus your review on these files, but read other project files as needed for context.\n\n\
         Changed files:\n```\nnew.go\n```\n\n"
    );

    // A tracked change adds the stat block.
    std::fs::write(dir.path().join("a.go"), "package a // x\n").unwrap();
    let (prompt, target, display) =
        build_review_prompt(|sc| format!("<{sc}>"), "ignored", true, &cfg, &wd);
    assert_eq!(target, "a.go\nnew.go");
    assert_eq!(display, "changed files (git auto-detect)");
    let stat = rival_core::gitscope::diff_stat(&cfg, &wd);
    let want = DIFF_REVIEW_PREAMBLE
        .replace("{FILES}", "a.go\nnew.go")
        .replace("{DIFFSTAT}", &format!("\nDiff stats:\n```\n{stat}\n```\n"))
        + "<the changed files listed above>";
    assert!(stat.starts_with("a.go | 2 +-"), "{stat:?}");
    assert_eq!(prompt, want);

    // Explicit scope never asks git.
    let got = build_review_prompt(|sc| format!("<{sc}>"), "src/", false, &cfg, &wd);
    assert_eq!(got.0, "<src/>");
}

#[test]
fn no_changes_gives_an_empty_preamble() {
    let fx = Fixture::new();
    let dir = fx.init_repo();
    assert_eq!(
        build_diff_preamble(&fx.config(), &s(dir.path())),
        (String::new(), String::new())
    );
}

/// The security prompt is always the security lens, and an empty scope
/// falls back to the whole project, through `lens_prompt`.
#[test]
fn lens_prompt_renders_the_selected_lens() {
    let fx = Fixture::new();
    let cfg = fx.config();
    let dir = tempfile::tempdir().unwrap();
    let wd = s(dir.path());
    let bug_hunter = review::build_reviewer_prompt(&cfg, "x", PromptKind::BugHunter);
    for (scope, auto) in [("src/api/", false), ("", true)] {
        let (prompt, target, display) = build_review_prompt(
            lens_prompt(&cfg, PromptKind::Security),
            scope,
            auto,
            &cfg,
            &wd,
        );
        assert!(prompt.contains("## Role: Security Reviewer"), "{scope:?}");
        assert!(
            !prompt.contains("## Role: Implementation Bug Hunter"),
            "{scope:?}"
        );
        assert!(!prompt.contains(&bug_hunter[..80]), "{scope:?}");
        assert!(!target.is_empty() && target == display, "{scope:?}");
    }
    // A temp dir is not a git repo, so auto-scope reviews the whole project.
    let (prompt, target, _) =
        build_review_prompt(lens_prompt(&cfg, PromptKind::Security), "", true, &cfg, &wd);
    assert_eq!(target, WHOLE_PROJECT);
    assert_eq!(
        prompt,
        review::build_reviewer_prompt(&cfg, WHOLE_PROJECT, PromptKind::Security)
    );
    let (prompt, _, _) = build_review_prompt(
        lens_prompt(&cfg, PromptKind::BugHunter),
        "internal/auth/",
        false,
        &cfg,
        &wd,
    );
    assert_eq!(
        prompt,
        review::build_reviewer_prompt(&cfg, "internal/auth/", PromptKind::BugHunter)
    );
}
