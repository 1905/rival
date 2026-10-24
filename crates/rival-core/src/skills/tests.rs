use std::collections::HashMap;

use super::*;

fn skill(name: &str) -> String {
    String::from_utf8(read_file(&format!("{name}/SKILL.md")).unwrap().to_vec()).unwrap()
}

/// The one active skill is `rival`; it launches `rival command skill`
/// for its host and watches the run in the background.
#[test]
fn rival_skill_is_embedded() {
    assert_eq!(NAMES, ["rival", "china"]);
    assert!(!DEPRECATED.contains(&"rival"));
    let content = skill("rival");
    for want in [
        "version: ",
        "name: rival\n",
        "rival command skill --host claude --detach --workdir",
        "rival command security --which --workdir",
        "rival wait --log <rival_err>",
        "/rival security src/api/",
        "skill.claude",
    ] {
        assert!(content.contains(want), "rival skill missing {want:?}");
    }
    // A hardcoded watcher timeout re-introduces the bound that
    // RIVAL_QUEUE_TIMEOUT/RIVAL_RUN_TIMEOUT already own.
    assert!(
        !content.contains("rival wait --log <rival_err> --timeout"),
        "rival skill hardcodes a --timeout on the watcher"
    );
    let codex = String::from_utf8(codex_skill("rival", "1").unwrap()).unwrap();
    for want in [
        "   rival command skill --host codex --detach --workdir <absolute-repository>",
        "rival command security --which --workdir <absolute-repository>",
        "skill.codex",
    ] {
        assert!(codex.contains(want), "codex rival skill missing {want:?}");
    }
}

/// The per-model skills are gone: nothing embeds them and install removes
/// them on both hosts.
#[test]
fn model_skills_are_retired() {
    for name in [
        "rival-codex",
        "rival-sol",
        "rival-claude",
        "rival-fable",
        "rival-k3",
        "rival-grok",
        "rival-security",
    ] {
        assert!(!NAMES.contains(&name), "{name} is still active");
        assert!(
            DEPRECATED.contains(&name),
            "{name} is not removed on install"
        );
        assert!(read_file(&format!("{name}/SKILL.md")).is_err(), "{name}");
        assert!(codex_skill(name, "1").is_err(), "{name}");
    }
}

/// No skill text names a model: the user picks it in the config, and the
/// output names the model that ran. "Claude Code" and "Codex" as host app
/// names are allowed.
#[test]
fn skill_text_names_no_model() {
    for name in NAMES {
        let claude = skill(name);
        let codex = String::from_utf8(codex_skill(name, "1").unwrap()).unwrap();
        for (host, text) in [("claude", &claude), ("codex", &codex)] {
            let lower = text.to_lowercase();
            for word in [
                "opus", "sol 6", "sol ", "astra", "gpt-", "gpt6", "fable", "kimi", "k3", "grok",
                "sonnet",
            ] {
                assert!(!lower.contains(word), "{host} {name} names {word:?}");
            }
            // "Claude" only as "Claude Code", "codex" only as the host app,
            // its CLI, a config key or the host flag.
            let without_hosts = text
                .replace("Claude Code", "")
                .replace("rival command skill --host claude", "");
            assert!(
                !without_hosts.contains("Claude"),
                "{host} {name} names Claude as a model"
            );
            for line in text.lines() {
                let l = line.to_lowercase();
                for model_phrase in ["with codex", "run codex", "codex review", "codex default"] {
                    assert!(!l.contains(model_phrase), "{host} {name}: {line:?}");
                }
            }
        }
    }
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

/// `china` launches `rival command china` on both hosts and names its
/// config key; it has no security branch.
#[test]
fn china_skill_is_embedded() {
    let claude = skill("china");
    let codex = String::from_utf8(codex_skill("china", "1.2.3").unwrap()).unwrap();
    for want in [
        "name: china\n",
        "rival command china --detach --workdir",
        "rival wait --log <rival_err>",
        "china.model",
        "review with china",
    ] {
        assert!(claude.contains(want), "china skill missing {want:?}");
    }
    for want in [
        "name: china\n",
        "rival command china --detach --workdir",
        "china.model",
    ] {
        assert!(codex.contains(want), "codex china skill missing {want:?}");
    }
    for text in [&claude, &codex] {
        assert!(!text.contains("rival command security"), "{text}");
        assert!(!text.contains("skill --host"), "{text}");
    }
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
    assert!(read_file("rival/../codex.md").is_err());
}

#[test]
fn codex_skill_errors_quote_the_name() {
    assert_eq!(
        codex_skill("rival-unknown", "1").unwrap_err(),
        "no Codex skill for \"rival-unknown\""
    );
}

/// The command the Codex skill launches.
#[test]
fn codex_skill_commands() {
    let data = String::from_utf8(codex_skill("rival", "1").unwrap()).unwrap();
    let line = "   rival command skill --host codex --detach --workdir <absolute-repository>";
    assert!(data.contains(line), "missing {line:?}");
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

/// The description and input of the Codex skill, pinned as golden text.
#[test]
fn codex_skill_text_golden() {
    let description = "Run a requested prompt, code review or security review through Rival from Codex, with the model set for Codex in rival config.";
    let input = "Pass the user's arguments verbatim: `[-re level] review [scope]` for a code review, `security [scope]` for a security review, or `[-re level] <prompt>` for a raw prompt. With no arguments show usage and do not launch. The model comes from `skill.codex` in Rival's configuration (`rival config`, Models); the output names the model that ran. For `security`, first run `rival command security --which --workdir <absolute-repository>` and report the resolved model. If it fails, report the error and do not launch. Do not invent flags or substitute another model.";
    let data = String::from_utf8(codex_skill("rival", "1").unwrap()).unwrap();
    let head = format!(
        "---\nname: rival\ndescription: {description}\nmetadata:\n  version: 1\n---\n\n# rival\n\n## Review input\n\n{input}\n\n"
    );
    assert!(data.starts_with(&head), "{data}");
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
/// them, and the `/rival` skill relays the "not applied" notice.
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
    for name in NAMES {
        assert!(
            skill(name).contains("`rival: ⚠ … not applied` line"),
            "{name}"
        );
        let codex = String::from_utf8(codex_skill(name, "1").unwrap()).unwrap();
        assert!(
            codex.contains("`rival: ⚠ … not applied` line"),
            "codex {name}"
        );
    }
    for name in NAMES {
        assert!(!skill(name).contains("--ultrafast"), "{name}");
    }
}
