use std::collections::HashMap;

use super::*;

fn skill(name: &str) -> String {
    String::from_utf8(read_file(&format!("{name}/SKILL.md")).unwrap().to_vec()).unwrap()
}

#[test]
fn grok_skill_is_embedded() {
    let name = "rival-grok";
    assert!(NAMES.contains(&name), "grok skill {name:?} is not active");
    assert!(
        !DEPRECATED.contains(&name),
        "grok skill {name:?} is deprecated"
    );
    let content = skill(name);
    for want in [
        "version: ",
        "name: rival-grok",
        "rival command grok --detach --workdir",
        "rival wait --log <rival_err>",
        "grok login",
    ] {
        assert!(content.contains(want), "grok skill missing {want:?}");
    }
    // A hardcoded watcher timeout re-introduces the bound that
    // RIVAL_QUEUE_TIMEOUT/RIVAL_RUN_TIMEOUT already own.
    assert!(
        !content.contains("rival wait --log <rival_err> --timeout"),
        "grok skill hardcodes a --timeout on the watcher"
    );
}

// Plan mode was dropped on 2026-08-20. The skill must stay deprecated so
// install removes copies already on disk.
#[test]
fn antislop_plan_skill_is_gone() {
    assert!(
        !NAMES.contains(&"rival-antislop-plan"),
        "rival-antislop-plan is active again; plan mode was removed"
    );
    assert!(
        DEPRECATED.contains(&"rival-antislop-plan"),
        "rival-antislop-plan must stay in Deprecated so installs clean it up"
    );
    assert!(
        read_file("rival-antislop-plan/SKILL.md").is_err(),
        "the plan skill file is still embedded"
    );
}

// The antislop review was removed on 2026-10-08. The skill must stay
// deprecated so install removes copies already on disk.
#[test]
fn antislop_skill_is_deprecated() {
    assert!(
        !NAMES.contains(&"rival-antislop"),
        "rival-antislop is active again; the review was removed"
    );
    assert!(
        DEPRECATED.contains(&"rival-antislop"),
        "rival-antislop must stay in Deprecated so installs clean it up"
    );
    assert!(
        read_file("rival-antislop/SKILL.md").is_err(),
        "the antislop skill file is still embedded"
    );
}

/// The earlier plan variants stay retired.
#[test]
fn plan_sol_and_fable_skills_stay_retired() {
    for name in ["rival-plan-sol", "rival-plan-fable"] {
        assert!(
            !NAMES.contains(&name) && DEPRECATED.contains(&name),
            "{name} must be retired and cleaned on install"
        );
        assert!(
            read_file(&format!("{name}/SKILL.md")).is_err(),
            "retired skill {name} remains embedded"
        );
        assert!(
            codex_skill(name, "test").is_err(),
            "retired Codex skill {name} remains available"
        );
    }
    for name in NAMES {
        let claude = skill(name);
        let codex = String::from_utf8(codex_skill(name, "test").unwrap()).unwrap();
        for text in [&claude, &codex] {
            for forbidden in ["rival-plan-sol", "rival-plan-fable", "gpt-5.6-sol"] {
                assert!(
                    !text.contains(forbidden),
                    "{name} still advertises {forbidden:?}"
                );
            }
        }
    }
}

/// Fable 5.1 and Sol 6.1 have their own skills again.
#[test]
fn fable_and_sol_skills_are_active() {
    for (name, command, model) in [
        ("rival-fable", "fable", "Fable 5.1"),
        ("rival-sol", "sol", "Sol 6.1"),
    ] {
        assert!(NAMES.contains(&name), "{name} is not active");
        assert!(!DEPRECATED.contains(&name), "{name} is still deprecated");
        let content = skill(name);
        for want in [
            format!("name: {name}\n"),
            "version: ".to_string(),
            format!("rival command {command} --detach --workdir"),
            format!("/{name}"),
            model.to_string(),
        ] {
            assert!(content.contains(&want), "{name} missing {want:?}");
        }
        for forbidden in ["rival command claude", "rival command codex", "Opus 5.5"] {
            assert!(!content.contains(forbidden), "{name} has {forbidden:?}");
        }
        let codex = String::from_utf8(codex_skill(name, "test").unwrap()).unwrap();
        assert!(codex.contains(model), "{name} does not name {model}");
        assert!(
            codex.contains(&format!("rival command {command} --detach --workdir")),
            "{name} launches the wrong command from Codex"
        );
    }
}

#[derive(Debug, serde::Deserialize)]
struct Header {
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    metadata: HashMap<String, String>,
}

#[test]
fn every_skill_has_valid_codex_variant() {
    for name in NAMES {
        let data = String::from_utf8(codex_skill(name, "9.8.7").unwrap()).unwrap();
        let parts: Vec<&str> = data.splitn(3, "---").collect();
        assert_eq!(parts.len(), 3, "{name}: missing frontmatter");
        let header: Header = serde_saphyr::from_str(parts[1])
            .unwrap_or_else(|e| panic!("{name}: invalid YAML: {e}"));
        assert!(
            header.name == name
                && !header.description.is_empty()
                && header.metadata.get("version").map(String::as_str) == Some("9.8.7"),
            "{name}: invalid header: {header:?}"
        );
        for unsupported in [
            "$ARGUMENTS",
            "run_in_background",
            "Write tool",
            "{{COMMAND}}",
        ] {
            assert!(
                !data.contains(unsupported),
                "{name}: unresolved or host-incompatible instruction {unsupported:?}"
            );
        }
    }
    assert!(
        codex_skill("rival-unknown", "1").is_err(),
        "missing command accepted"
    );
}

// megareview was removed on 2026-09-26. Its skill must stay deprecated so
// install removes copies already on disk, for both hosts.
#[test]
fn review_skill_is_retired() {
    let name = "rival-review";
    assert!(
        !NAMES.contains(&name) && DEPRECATED.contains(&name),
        "{name} must be retired and cleaned on install"
    );
    assert!(
        read_file(&format!("{name}/SKILL.md")).is_err(),
        "retired skill {name} remains embedded"
    );
    assert!(
        codex_skill(name, "test").is_err(),
        "retired Codex skill {name} remains available"
    );
}

/// The embedded skill dirs are exactly `NAMES`.
#[test]
fn embedded_dirs_are_exactly_names() {
    let mut names = NAMES.to_vec();
    names.sort_unstable();
    assert_eq!(embedded_dirs(), names);
    for name in NAMES {
        assert!(read_file(&format!("{name}/SKILL.md")).is_ok(), "{name}");
    }
}

/// `codex.md` is the Codex template, not a readable skill file.
#[test]
fn read_file_hides_root_files_and_reports_not_exist_error() {
    assert_eq!(
        read_file("codex.md").unwrap_err(),
        "open codex.md: file does not exist"
    );
    assert_eq!(
        read_file("rival-nope/SKILL.md").unwrap_err(),
        "open rival-nope/SKILL.md: file does not exist"
    );
    assert!(read_file("rival-codex/../codex.md").is_err());
}

#[test]
fn codex_skill_errors_quote_the_name() {
    assert_eq!(
        codex_skill("rival-unknown", "1").unwrap_err(),
        "no Codex skill for \"rival-unknown\""
    );
}

/// The command each Codex skill launches.
#[test]
fn codex_skill_commands() {
    for (name, command) in [
        ("rival-claude", "claude"),
        ("rival-codex", "codex"),
        ("rival-fable", "fable"),
        ("rival-sol", "sol"),
        ("rival-k3", "k3"),
        ("rival-grok", "grok"),
        ("rival-security", "security"),
    ] {
        let data = String::from_utf8(codex_skill(name, "1").unwrap()).unwrap();
        let line = format!("   rival command {command} --detach --workdir <absolute-repository>");
        assert!(data.contains(&line), "{name}: missing {line:?}");
    }
    let k3 = String::from_utf8(codex_skill("rival-k3", "1").unwrap()).unwrap();
    assert!(k3.contains(
        "description: Run a requested k3 prompt or code review through Rival from Codex.\n"
    ));
}

/// The embedded tree is `codex.md` plus one `SKILL.md` per name, nothing
/// else.
#[test]
fn embedded_files_are_codex_md_and_skill_md() {
    let mut got: Vec<String> = TREE
        .files()
        .map(|f| f.path().to_str().unwrap().to_string())
        .collect();
    for dir in TREE.dirs() {
        assert!(dir.dirs().next().is_none(), "{}", dir.path().display());
        for file in dir.files() {
            got.push(file.path().to_str().unwrap().replace('\\', "/"));
        }
    }
    got.sort();
    let mut want: Vec<String> = NAMES.iter().map(|n| format!("{n}/SKILL.md")).collect();
    want.push("codex.md".into());
    want.sort();
    assert_eq!(got, want);
}

/// The description and input of every Codex skill, pinned as golden text.
#[test]
fn codex_skill_text_golden() {
    let review = "Pass the user's arguments verbatim: `[-re level] review [scope]` for reviews, or `[-re level] <prompt>` for a raw prompt. With no arguments show usage and do not launch. Model defaults and provider setup are owned by Rival; do not invent flags or substitute another model.";
    let cases = [
        (
            "rival-claude",
            "Review code with Opus 5.5 through Rival and the authenticated Claude Code CLI. Use for a requested independent Claude review from Codex.",
            "Always run a code review. No arguments means `review`. A scope means `review <scope>`. If the user already supplied `review`, do not duplicate it. Move an explicit effort before review: `-re high review src/`. Omitted effort uses the configured Claude default (medium fallback). Requires the Claude Code CLI authenticated with `claude auth login`, or Rival's configured Docker transport. Rival selects Opus 5.5; do not replace it with another model.",
        ),
        (
            "rival-fable",
            "Review code with Fable 5.1 through Rival and the authenticated Claude Code CLI. Use for a requested independent Fable review from Codex.",
            "Always run a code review. No arguments means `review`. A scope means `review <scope>`. If the user already supplied `review`, do not duplicate it. Move an explicit effort before review: `-re high review src/`. Omitted effort uses the configured Fable default (medium fallback). Requires the Claude Code CLI authenticated with `claude auth login`, Rival's configured Docker transport, or Rival's proxy. Rival selects Fable 5.1; do not replace it with another model.",
        ),
        (
            "rival-codex",
            "Run a requested codex prompt or code review through Rival from Codex.",
            review,
        ),
        (
            "rival-sol",
            "Run a requested Sol 6.1 prompt or code review through Rival from Codex.",
            "Pass the user's arguments verbatim: `[-re level] review [scope]` for reviews, or `[-re level] <prompt>` for a raw prompt. With no arguments show usage and do not launch. Rival selects Sol 6.1 (`gpt-6.1-sol`) on the Codex CLI. Omitted effort uses `efforts.sol` from Rival's configuration (xhigh fallback). Requires the Codex CLI authenticated with `codex login`, or Rival's configured Codex proxy route. Do not invent flags or substitute another model.",
        ),
        (
            "rival-k3",
            "Run a requested k3 prompt or code review through Rival from Codex.",
            review,
        ),
        (
            "rival-grok",
            "Run a requested grok prompt or code review through Rival from Codex.",
            review,
        ),
        (
            "rival-security",
            "Run Rival's dedicated security reviewer on changed code or a specified scope from Codex. Use for requested vulnerability reviews.",
            "First run `rival command security --which --workdir <absolute-repository>` and report the resolved model. If it fails, report the error and do not launch. Pass the user's scope verbatim; empty input reviews git-detected changes. The model is selected by security.reviewer in Rival's configuration.",
        ),
    ];
    let mut names: Vec<&str> = cases.iter().map(|(name, _, _)| *name).collect();
    names.sort_unstable();
    let mut active = NAMES.to_vec();
    active.sort_unstable();
    assert_eq!(names, active);
    for (name, description, input) in cases {
        let data = String::from_utf8(codex_skill(name, "1").unwrap()).unwrap();
        let head = format!(
            "---\nname: {name}\ndescription: {description}\nmetadata:\n  version: 1\n---\n\n# {name}\n\n## Review input\n\n{input}\n\n"
        );
        assert!(data.starts_with(&head), "{name}:\n{data}");
    }
}

#[test]
fn retired_plan_skills_are_unavailable_and_unadvertised() {
    for name in RETIRED_PLAN_SKILLS {
        assert!(!NAMES.contains(&name));
        assert!(read_file(&format!("{name}/SKILL.md")).is_err());
        assert!(codex_skill(name, "5.1.0").is_err());
    }
    for name in NAMES {
        for text in [
            skill(name),
            String::from_utf8(codex_skill(name, "5.1.0").unwrap()).unwrap(),
        ] {
            assert!(
                !text.contains("rival-plan"),
                "{name} advertises a retired skill"
            );
        }
    }
}

/// The Ultrafast skills are retired: install removes them, nothing embeds
/// them, and the Codex-runtime skills relay the "not applied" notice.
#[test]
fn ultrafast_skills_are_retired_and_the_base_skills_relay_the_notice() {
    for name in ["rival-astra-ultrafast", "rival-sol-ultrafast"] {
        assert!(!NAMES.contains(&name), "{name} is still active");
        assert!(
            DEPRECATED.contains(&name),
            "{name} is not removed on install"
        );
        assert!(codex_skill(name, "1").is_err(), "{name}");
    }
    for name in ["rival-codex", "rival-sol"] {
        assert!(
            skill(name).contains("`rival: ⚠ ultrafast not applied` line"),
            "{name}"
        );
        let codex = String::from_utf8(codex_skill(name, "1").unwrap()).unwrap();
        assert!(
            codex.contains("`rival: ⚠ ultrafast not applied` line"),
            "codex {name}"
        );
    }
    for name in NAMES {
        assert!(!skill(name).contains("--ultrafast"), "{name}");
    }
}
