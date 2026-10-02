use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::*;

fn skill(name: &str) -> String {
    String::from_utf8(read_file(&format!("{name}/SKILL.md")).unwrap().to_vec()).unwrap()
}

// Go: TestPlanCodexSkillPinsXhighEffort.
#[test]
fn plan_codex_skill_pins_xhigh_effort() {
    let content = skill("rival-plan-codex");
    for want in [
        "version: ",
        "argument-hint: \"<path-to-plan.md>\"",
        "rival command plan --model codex --effort xhigh --detach",
        "Always run at **xhigh**",
    ] {
        assert!(content.contains(want), "plan-codex skill missing {want:?}");
    }
    for forbidden in ["defaults to **high**", "[-re high|ultra]"] {
        assert!(
            !content.contains(forbidden),
            "plan-codex skill still advertises optional effort {forbidden:?}"
        );
    }
}

// Go: TestGrokSkillIsEmbedded.
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

// Go: TestPlanSkillRunsCodexAtXhigh.
#[test]
fn plan_skill_runs_codex_at_xhigh() {
    let name = "rival-plan";
    assert!(
        NAMES.contains(&name),
        "paired plan skill {name:?} is not active"
    );
    assert!(
        !DEPRECATED.contains(&name),
        "paired plan skill {name:?} is still deprecated"
    );
    let content = skill(name);
    for want in [
        "name: rival-plan",
        "Codex",
        "rival command plan --model codex --effort xhigh --detach",
    ] {
        assert!(content.contains(want), "paired plan skill missing {want:?}");
    }
}

// Go: TestAntislopPlanSkillIsGone. Plan mode was dropped on 2026-08-20. The
// skill must stay deprecated so install removes copies already on disk.
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

// Go: TestAntislopSkillsAreEmbedded.
#[test]
fn antislop_skills_are_embedded() {
    let cases: [(&str, &[&str]); 1] = [(
        "rival-antislop",
        &[
            "name: rival-antislop\n",
            "argument-hint: \"[<scope>]\"",
            "rival command antislop --detach --workdir",
            "rival wait --log <rival_err>",
            "never bugs",
        ],
    )];
    for (name, wants) in cases {
        assert!(NAMES.contains(&name), "skill {name:?} is not active");
        assert!(!DEPRECATED.contains(&name), "skill {name:?} is deprecated");
        let content = skill(name);
        for want in wants.iter().chain(&["version: "]) {
            assert!(content.contains(want), "{name} skill missing {want:?}");
        }
    }
}

// Go: TestSolSkillsAreRetired.
#[test]
fn sol_skills_are_retired() {
    for name in ["rival-sol", "rival-plan-sol"] {
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
            for forbidden in [
                "rival-sol",
                "rival-plan-sol",
                "Sol",
                "-m sol",
                "--model sol",
            ] {
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

// Go: TestEverySkillHasValidCodexVariant.
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

// Go: TestReviewSkillIsRetired. megareview was removed on 2026-09-26. Its
// skill must stay deprecated so install removes copies already on disk,
// for both hosts.
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

/// Go's `//go:embed` list is exactly `Names`.
#[test]
fn embedded_dirs_are_exactly_names() {
    let mut names = NAMES.to_vec();
    names.sort_unstable();
    assert_eq!(embedded_dirs(), names);
    for name in NAMES {
        assert!(read_file(&format!("{name}/SKILL.md")).is_ok(), "{name}");
    }
}

/// `codex.md` is the Codex template, not part of Go's `Files`.
#[test]
fn read_file_hides_root_files_and_reports_go_error() {
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

/// The whole generated file for one skill: Go's format string, the input,
/// and `codex.md` with `{{COMMAND}}` replaced.
#[test]
fn codex_skill_full_output_for_plan_claude() {
    let got = String::from_utf8(codex_skill("rival-plan-claude", "1.2.3").unwrap()).unwrap();
    let want = format!(
        "---\nname: rival-plan-claude\ndescription: Review a plan or specification document through Rival from Codex, returning ratings and findings.\nmetadata:\n  version: 1.2.3\n---\n\n# rival-plan-claude\n\n## Review input\n\nPass the document path and any requested options verbatim. If no document is specified, ask for its path before launching. Show all model results and report any skipped model. Codex plan reviews pin xhigh; Claude-only uses its configured effort (medium fallback) unless the user supplies -re.\n\n{}",
        codex_workflow().replace("{{COMMAND}}", "plan --model claude")
    );
    assert_eq!(got, want);
    assert!(got.contains(
        "rival command plan --model claude --detach --workdir <absolute-repository> < <input.txt>"
    ));
}

/// The command each Codex skill launches (Go's `command` switch).
#[test]
fn codex_skill_commands() {
    for (name, command) in [
        ("rival-claude", "claude"),
        ("rival-codex", "codex"),
        ("rival-k3", "k3"),
        ("rival-grok", "grok"),
        ("rival-plan", "plan --model codex --effort xhigh"),
        ("rival-plan-codex", "plan --model codex --effort xhigh"),
        ("rival-plan-claude", "plan --model claude"),
        ("rival-antislop", "antislop"),
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

// ---- Go tree parity. These read the Go sources and go away with them in P6.

fn go_skills_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../rival/internal/skills")
}

fn go_source(file: &str) -> String {
    let path = go_skills_dir().join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The quoted strings of the Go `var <name> = []string{...}` block.
fn go_string_list(src: &str, var: &str) -> Vec<String> {
    let start = src
        .find(&format!("var {var} = []string{{"))
        .unwrap_or_else(|| panic!("var {var} not found"));
    let block = &src[start..];
    let end = block.find('}').unwrap();
    block[..end]
        .lines()
        .map(|line| line.split("//").next().unwrap())
        .flat_map(|line| line.split('"').skip(1).step_by(2))
        .map(str::to_string)
        .collect()
}

#[test]
fn names_and_deprecated_match_go() {
    let src = go_source("embed.go");
    assert_eq!(go_string_list(&src, "Names"), NAMES);
    assert_eq!(go_string_list(&src, "Deprecated"), DEPRECATED);
    for name in NAMES {
        assert!(
            src.contains(&format!("//go:embed all:{name}\n")),
            "{name} is not in Go's embed list"
        );
    }
}

/// Every embedded Markdown file equals the Go tree's byte for byte, and
/// the Go tree holds no extra Markdown.
#[test]
fn embedded_markdown_matches_go_tree() {
    let mut rust_files: Vec<String> = vec!["codex.md".into()];
    for dir in TREE.dirs() {
        for file in dir.files() {
            rust_files.push(file.path().to_str().unwrap().replace('\\', "/"));
        }
    }
    rust_files.sort();
    let mut go_files: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(go_skills_dir()).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().into_string().unwrap();
        if entry.file_type().unwrap().is_dir() {
            for sub in std::fs::read_dir(entry.path()).unwrap() {
                let sub = sub.unwrap().file_name().into_string().unwrap();
                go_files.push(format!("{name}/{sub}"));
            }
        } else if name.ends_with(".md") {
            go_files.push(name);
        }
    }
    go_files.sort();
    assert_eq!(rust_files, go_files);
    for file in &rust_files {
        let want = std::fs::read(go_skills_dir().join(file)).unwrap();
        let got = TREE.get_file(file).unwrap().contents();
        assert!(got == want.as_slice(), "{file} differs from the Go tree");
    }
}

/// The description and input of every Codex skill are Go `codex.go` string
/// literals, and the frontmatter format string is Go's.
#[test]
fn codex_skill_text_matches_go_literals() {
    let src = go_source("codex.go");
    assert!(src.contains(
        r#""---\nname: %s\ndescription: %s\nmetadata:\n  version: %s\n---\n\n# %s\n\n## Review input\n\n%s\n\n""#
    ));
    for name in NAMES {
        let data = String::from_utf8(codex_skill(name, "1").unwrap()).unwrap();
        let description = data
            .lines()
            .find_map(|l| l.strip_prefix("description: "))
            .unwrap();
        let input = data
            .split("## Review input\n\n")
            .nth(1)
            .and_then(|rest| rest.split("\n\n").next())
            .unwrap();
        let description = if ["rival-codex", "rival-k3", "rival-grok"].contains(&name) {
            "Run a requested %s prompt or code review through Rival from Codex.".to_string()
        } else {
            description.to_string()
        };
        for text in [description.as_str(), input] {
            assert!(
                src.contains(&format!("\"{text}\"")),
                "{name}: {text:?} is not a codex.go literal"
            );
        }
    }
}
