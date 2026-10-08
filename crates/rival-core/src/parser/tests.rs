//! Go: `internal/parser/parser_test.go` and `parser_kimi_test.go`.

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
