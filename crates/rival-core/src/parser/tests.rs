//! Go: `internal/parser/parser_test.go`, `parser_kimi_test.go`, and the
//! parser-only `TestAntislopStdinGrammar` from `cmd/command_antislop_test.go`.

use super::*;

fn err_text(r: Result<ParseResult>) -> String {
    format!("{:#}", r.unwrap_err())
}

#[test]
fn parse_args_empty() {
    for input in ["", "  ", "\t\n"] {
        let r = parse_codex_args(input).unwrap();
        assert!(r.is_empty, "expected IsEmpty for {input:?}");
        assert_eq!(r.effort, "", "omitted effort for configured default");
    }
}

#[test]
fn parse_args_raw_prompt() {
    let r = parse_codex_args("explain the auth flow").unwrap();
    assert!(!r.is_empty && !r.is_review);
    assert_eq!(r.prompt, "explain the auth flow");
    assert_eq!(r.effort, "");
}

#[test]
fn parse_args_effort_with_prompt() {
    let r = parse_codex_args("-re xhigh find bugs in main.go").unwrap();
    assert_eq!(r.effort, "xhigh");
    assert_eq!(r.prompt, "find bugs in main.go");
}

#[test]
fn parse_args_ultra_effort() {
    let r = parse_codex_args("-re ultra review").unwrap();
    assert!(r.effort == "ultra" && r.is_review, "{r:?}");
}

#[test]
fn parse_args_invalid_effort() {
    assert_eq!(
        err_text(parse_codex_args("-re maximum review")),
        r#"invalid effort level "maximum", must be one of: low, medium, high, xhigh, ultra"#
    );
}

#[test]
fn parse_args_review_alone() {
    let r = parse_codex_args("review").unwrap();
    assert!(r.is_review);
    assert_eq!(r.review_scope, "the entire project");
    assert_eq!(r.prompt, "", "a review must leave Prompt empty");
}

#[test]
fn parse_args_review_with_scope() {
    let r = parse_codex_args("review src/").unwrap();
    assert!(r.is_review);
    assert_eq!(r.review_scope, "src/");
    assert!(!r.auto_scope && r.prompt.is_empty());
}

#[test]
fn parse_args_review_quoted_scope() {
    let r = parse_codex_args(r#"review "only THIS file xxx""#).unwrap();
    assert!(r.is_review);
    assert_eq!(r.review_scope, r#""only THIS file xxx""#);
}

#[test]
fn parse_args_effort_with_review() {
    let r = parse_codex_args("-re high review").unwrap();
    assert_eq!(r.effort, "high");
    assert!(r.is_review);
    assert_eq!(r.review_scope, "the entire project");
}

#[test]
fn parse_args_effort_with_review_and_scope() {
    let r = parse_codex_args("-re high review src/api/").unwrap();
    assert_eq!(r.effort, "high");
    assert!(r.is_review);
    assert_eq!(r.review_scope, "src/api/");
}

#[test]
fn parse_args_effort_alone() {
    let r = parse_codex_args("-re high").unwrap();
    assert_eq!(r.effort, "high");
    assert!(r.is_empty, "expected IsEmpty when only -re flag provided");
}

#[test]
fn parse_args_auto_scope() {
    assert!(parse_codex_args("review").unwrap().auto_scope);
    assert!(!parse_codex_args("review src/").unwrap().auto_scope);
    assert!(parse_codex_args("-re high review").unwrap().auto_scope);
}

#[test]
fn parse_review_args_auto_scope() {
    // Empty megareview arguments run the default roster against git scope.
    let r = parse_review_args("").unwrap();
    assert!(r.auto_scope && !r.is_empty, "{r:?}");
    assert_eq!(r.effort, "", "configured-default sentinel");

    // Empty scope in megareview → AutoScope=true
    assert!(parse_review_args("-re high").unwrap().auto_scope);

    // Explicit scope → AutoScope=false
    let r = parse_review_args("src/api/").unwrap();
    assert!(!r.auto_scope);
    assert_eq!(r.prompt, "");
}

#[test]
fn parse_review_args_model_selection() {
    // full model id
    let r = parse_review_args("-m gpt-6-astra -re ultra").unwrap();
    assert!(
        r.auto_scope && r.effort == "ultra" && r.models == ["gpt-6-astra"],
        "{r:?}"
    );

    // model only auto-scopes
    let r = parse_review_args("-m k3").unwrap();
    assert!(r.auto_scope && r.models == ["k3"], "{r:?}");

    // flags in either order and comma list
    let r = parse_review_args("--model=k3,codex --effort high src/api and reports").unwrap();
    assert!(
        r.effort == "high" && !r.auto_scope && r.review_scope == "src/api and reports",
        "{r:?}"
    );
    assert_eq!(r.models, ["k3", "codex"]);

    let r = parse_review_args("-re low -m k3 -m codex src/").unwrap();
    assert!(r.effort == "low" && r.models == ["k3", "codex"], "{r:?}");

    let r = parse_review_args("src/api/ -m k3 -re medium").unwrap();
    assert!(
        r.review_scope == "src/api/" && r.effort == "medium" && r.models == ["k3"],
        "trailing flags must not become scope text: {r:?}"
    );

    // double dash escapes scope
    let r = parse_review_args("-m k3 -- -generated/path").unwrap();
    assert_eq!(r.review_scope, "-generated/path");
    assert!(r.escaped);
}

#[test]
fn parse_review_args_model_option_errors() {
    let cases = [
        ("-m", "option -m requires a value"),
        ("--model=", "option --model requires a value"),
        ("-m -re high", "option -m requires a value"),
        ("--model k3,,codex", "model selector cannot be empty"),
        (
            "--unknown value",
            r#"unknown review option "--unknown"; use -m/--model, -re/--effort, or -- before a scope beginning with '-'"#,
        ),
    ];
    for (raw, want) in cases {
        assert_eq!(err_text(parse_review_args(raw)), want, "{raw}");
    }
}

#[test]
fn parse_review_args_help() {
    for raw in ["-h", "--help"] {
        assert!(parse_review_args(raw).unwrap().is_empty, "{raw}");
    }
}

#[test]
fn parse_grok_args_raw_prompt() {
    let r = parse_grok_args("explain the auth flow").unwrap();
    assert!(!r.is_empty && !r.is_review);
    assert_eq!(r.prompt, "explain the auth flow");
    assert_eq!(r.effort, "");
}

#[test]
fn parse_grok_args_review_alone() {
    let r = parse_grok_args("review").unwrap();
    assert!(r.is_review && r.auto_scope, "{r:?}");
    assert_eq!(r.review_scope, "the entire project");
    assert_eq!(r.prompt, "");
}

#[test]
fn parse_grok_args_review_with_scope() {
    let r = parse_grok_args("review src/api/").unwrap();
    assert!(r.is_review && !r.auto_scope);
    assert_eq!(r.review_scope, "src/api/");
}

#[test]
fn parse_grok_args_effort_override() {
    let r = parse_grok_args("-re medium review src/").unwrap();
    assert_eq!(r.effort, "medium");
    assert!(r.is_review && r.review_scope == "src/", "{r:?}");
}

#[test]
fn parse_grok_args_effort_is_not_pinned() {
    // Unlike kimi, grok honours the requested level instead of forcing one.
    let r = parse_grok_args("-re low find bugs in main.go").unwrap();
    assert_eq!(r.effort, "low");
    assert_eq!(r.prompt, "find bugs in main.go");
}

#[test]
fn parse_grok_args_invalid_effort() {
    assert!(err_text(parse_grok_args("-re maximum review")).contains("invalid effort"));
}

#[test]
fn parse_grok_args_empty() {
    for input in ["", "  ", "\t\n"] {
        let r = parse_grok_args(input).unwrap();
        assert!(r.is_empty, "{input:?}");
        assert_eq!(r.effort, "");
    }
}

/// One ladder means every surface accepts the same levels. Claude used to
/// reject ultra while its own plan skill documented it.
#[test]
fn every_surface_accepts_the_shared_ladder() {
    type Parse = fn(&str) -> Result<ParseResult>;
    let parsers: [(&str, Parse); 4] = [
        ("claude", parse_claude_args),
        ("codex", parse_codex_args),
        ("grok", parse_grok_args),
        ("kimi", parse_kimi_args),
    ];
    for (name, parse) in parsers {
        for effort in VALID_EFFORTS {
            let r = parse(&format!("-re {effort} hello"));
            assert!(r.is_ok(), "{name} rejected -re {effort}: {r:?}");
        }
    }
}

// parser_kimi_test.go

/// Every advertised effort must parse — the value is ignored downstream (K3
/// runs max only), so rejecting "max"/"ultra" while the docs say "pinned to
/// max regardless of -re" would be a trap.
#[test]
fn parse_kimi_args_accepts_and_ignores_all_effort_names() {
    for effort in VALID_EFFORTS.iter().chain(&["max"]) {
        let parsed = parse_kimi_args(&format!("-re {effort} hello")).unwrap();
        assert_eq!(parsed.effort, *effort);
        assert_eq!(parsed.prompt, "hello");
    }
}

#[test]
fn parse_kimi_args_rejects_unknown_effort() {
    assert_eq!(
        err_text(parse_kimi_args("-re bogus hello")),
        r#"invalid effort level "bogus", must be one of: low, medium, high, xhigh, ultra, max"#
    );
    // max is kimi-only.
    assert!(parse_codex_args("-re max hello").is_err());
}

#[test]
fn parse_kimi_args_leaves_default_for_config_resolution() {
    let parsed = parse_kimi_args("review src/").unwrap();
    assert_eq!(parsed.effort, "", "configured-default sentinel");
    assert!(
        parsed.is_review && parsed.review_scope == "src/",
        "{parsed:?}"
    );
}

// cmd/command_antislop_test.go: TestAntislopStdinGrammar

#[test]
fn antislop_stdin_grammar() {
    let cases: [(&str, &str, &str, &[&str]); 4] = [
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
        assert_eq!(parsed.effort, effort, "{name}");
        assert_eq!(parsed.models, models, "{name}");
    }
}

// Rust-only: source-behavior pins.

#[test]
fn parse_args_token_joining_follows_go() {
    // -re needs a single space; anything else is a raw prompt.
    let r = parse_codex_args("-re\thigh hello").unwrap();
    assert_eq!(
        (r.effort.as_str(), r.prompt.as_str()),
        ("", "-re\thigh hello")
    );
    let r = parse_codex_args("-re").unwrap();
    assert_eq!(r.prompt, "-re");
    // Extra spaces after -re are trimmed; the rest is split at one space.
    let r = parse_codex_args("-re   high   say  hi ").unwrap();
    assert_eq!((r.effort.as_str(), r.prompt.as_str()), ("high", "say  hi"));
    // A tab after the level makes it part of the level.
    assert_eq!(
        err_text(parse_codex_args("-re high\tx")),
        r#"invalid effort level "high\tx", must be one of: low, medium, high, xhigh, ultra"#
    );
    // The review keyword is case-insensitive; the scope keeps its text.
    let r = parse_codex_args("ReViEw  Src/A ").unwrap();
    assert!(r.is_review && r.review_scope == "Src/A", "{r:?}");
    // "reviewer" is a raw prompt; "review\tx" too (needs a space).
    assert_eq!(parse_codex_args("reviewer").unwrap().prompt, "reviewer");
    assert_eq!(parse_codex_args("review\tx").unwrap().prompt, "review\tx");
    // Go's simple lowering maps İ to i, then slices the original at 6 bytes.
    let r = parse_codex_args("revİew x").unwrap();
    assert!(r.is_review, "{r:?}");
    assert_eq!(r.review_scope, "w x");
}

#[test]
fn parse_review_args_option_edges_follow_go() {
    // Scope tokens split on space/tab/CR/LF and rejoin with one space.
    let r = parse_review_args("a\tb\n c").unwrap();
    assert_eq!(r.review_scope, "a b c");
    // A token with "=" that is not an option stays scope text.
    assert_eq!(parse_review_args("k=v").unwrap().review_scope, "k=v");
    // -h=x still asks for help; options parsed before it are kept.
    let r = parse_review_args("-m k3 -h=x src").unwrap();
    assert!(
        r.is_empty && r.models == ["k3"] && r.review_scope.is_empty(),
        "{r:?}"
    );
    // Inline values are trimmed; inline -re is validated.
    let r = parse_review_args("--effort=high -m= k3 , codex").unwrap_err();
    assert_eq!(format!("{r:#}"), "option -m requires a value");
    assert_eq!(
        err_text(parse_review_args("-re=max")),
        r#"invalid effort level "max", must be one of: low, medium, high, xhigh, ultra"#
    );
    // A space after a comma ends the value: "k3," has an empty selector.
    assert_eq!(
        err_text(parse_review_args("-m k3, codex")),
        "model selector cannot be empty"
    );
    // A detached comma is not part of the value; it becomes scope text.
    let r = parse_review_args("-m k3 , codex").unwrap();
    assert_eq!(r.models, ["k3"]);
    assert_eq!(r.review_scope, ", codex");
    // "--" with nothing after still marks the scope escaped.
    let r = parse_review_args("--").unwrap();
    assert!(
        r.escaped && r.auto_scope && r.review_scope == WHOLE_PROJECT,
        "{r:?}"
    );
    // Everything after "--" is kept verbatim (inner spacing too).
    let r = parse_review_args("-- -a  -m x ").unwrap();
    assert_eq!(r.review_scope, "-a  -m x");
    // Unknown option text is quoted with Go %q.
    assert_eq!(
        err_text(parse_review_args("-é\"")),
        r#"unknown review option "-é\""; use -m/--model, -re/--effort, or -- before a scope beginning with '-'"#
    );
}
