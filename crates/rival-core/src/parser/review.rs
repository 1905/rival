//! Go: `internal/parser/review.go`.

use anyhow::{Result, bail};

use super::ParseResult;
use crate::config::{self, VALID_EFFORTS, WHOLE_PROJECT};
use crate::gostd;

/// The separators of Go's `popReviewToken` (`TrimLeft`/`IndexAny`).
const TOKEN_SPACE: [char; 4] = [' ', '\t', '\r', '\n'];

/// Parses raw review arguments (used by antislop).
/// Grammar: `[options] [scope]` — always a review, no "review" keyword
/// needed. Options may appear before or after scope tokens:
///
/// ```text
/// -re, --effort <level>
/// -m, --model <selector[,selector...]>
/// ```
///
/// Model options may be repeated. "--" ends option parsing so a scope
/// beginning with a dash can still be reviewed.
pub fn parse_review_args(raw: &str) -> Result<ParseResult> {
    let mut s = raw.trim();
    let mut result = ParseResult {
        is_review: true,
        ..Default::default()
    };
    let mut scope_parts: Vec<&str> = Vec::new();

    while !s.is_empty() {
        let (token, rest) = pop_review_token(s);
        if token == "--" {
            result.escaped = true;
            let rest = rest.trim();
            if !rest.is_empty() {
                scope_parts.push(rest);
            }
            break;
        }

        let (name, inline_value) = split_review_option(token);
        match name {
            "-h" | "--help" => {
                result.is_empty = true;
                return Ok(result);
            }
            "-re" | "--effort" => {
                let (value, remaining) = review_option_value(name, inline_value, rest)?;
                if !config::is_valid_effort(value) {
                    bail!(
                        "invalid effort level {}, must be one of: {}",
                        gostd::quote(value),
                        VALID_EFFORTS.join(", ")
                    );
                }
                result.effort = value.to_string();
                s = remaining;
            }
            "-m" | "--model" => {
                let (value, remaining) = review_option_value(name, inline_value, rest)?;
                result.models.extend(split_model_values(value)?);
                s = remaining;
            }
            _ => {
                if token.starts_with('-') {
                    bail!(
                        "unknown review option {}; use -m/--model, -re/--effort, or -- before a scope beginning with '-'",
                        gostd::quote(token)
                    );
                }
                scope_parts.push(token);
                s = rest.trim();
            }
        }
    }

    let mut scope = scope_parts.join(" ");
    if scope.is_empty() {
        result.auto_scope = true;
        scope = WHOLE_PROJECT.to_string();
    }
    result.review_scope = scope;
    Ok(result)
}

fn pop_review_token(s: &str) -> (&str, &str) {
    let s = s.trim_start_matches(TOKEN_SPACE);
    match s.find(TOKEN_SPACE) {
        Some(i) => (&s[..i], s[i..].trim_start_matches(TOKEN_SPACE)),
        None => (s, ""),
    }
}

/// Splits `name=value` at the first `=`; `None` when there is no `=`.
fn split_review_option(token: &str) -> (&str, Option<&str>) {
    match token.split_once('=') {
        Some((name, value)) => (name, Some(value)),
        None => (token, None),
    }
}

fn review_option_value<'a>(
    name: &str,
    inline_value: Option<&'a str>,
    rest: &'a str,
) -> Result<(&'a str, &'a str)> {
    if let Some(inline) = inline_value {
        if inline.trim().is_empty() {
            bail!("option {name} requires a value");
        }
        return Ok((inline.trim(), rest.trim()));
    }
    if rest.trim().is_empty() {
        bail!("option {name} requires a value");
    }
    let (value, remaining) = pop_review_token(rest);
    if value.starts_with('-') {
        bail!("option {name} requires a value");
    }
    Ok((value, remaining.trim()))
}

fn split_model_values(value: &str) -> Result<Vec<String>> {
    let mut models = Vec::new();
    for part in value.split(',') {
        let part = part.trim();
        if part.is_empty() {
            bail!("model selector cannot be empty");
        }
        models.push(part.to_string());
    }
    Ok(models)
}
