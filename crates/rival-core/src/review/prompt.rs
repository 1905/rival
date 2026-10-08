//! Reviewer prompts.
//!
//! Tests pin the texts by length and SHA-256.

use crate::config::{Config, PromptKind};

#[cfg(test)]
mod tests;

/// Builds the reviewer prompt by combining scope context with the lens
/// instructions and the JSON output contract.
///
/// Both lenses stay overridable through `roles.bug_hunter` and
/// `roles.security` in `~/.rival/config.yaml`. A blank override falls
/// through to the built-in text rather than producing an empty prompt. The
/// contract is always appended.
pub fn build_reviewer_prompt(cfg: &Config, scope: &str, kind: PromptKind) -> String {
    let mut sb = format!("Review scope: {scope}\n\n");
    let (key, builtin) = match kind {
        PromptKind::Security => ("security", SECURITY_INSTRUCTIONS),
        PromptKind::BugHunter => ("bug_hunter", BUG_HUNTER_INSTRUCTIONS),
    };
    match cfg.role_prompt_override(key) {
        Some(over) if !over.trim().is_empty() => sb.push_str(over),
        _ => sb.push_str(builtin),
    }
    sb.push_str(REVIEWER_JSON_CONTRACT);
    sb
}

// Macros so `concat!` can splice the shared pieces.

/// The one severity scale both code-review lenses use.
macro_rules! severity_rubric {
    () => {
        r#"Severity:
- critical: data loss, a security hole an attacker can reach, or a crash/outage on a normal path
- high: wrong result or broken flow on a realistic path; a race that can corrupt state
- medium: wrong result only on an edge case, or a real performance problem on a hot path
- low: minor defect with a cheap workaround
"#
    };
}

/// Makes every finding carry a concrete trigger and result. A finding the
/// model cannot ground this way is dropped, not guessed.
macro_rules! failure_scenario_rule {
    () => {
        "Each finding needs a concrete failure_scenario: the input or state that triggers it and the wrong result. If you cannot state one, drop the finding."
    };
}

/// The plain-writing rules for the free-text fields. The reviewer contract
/// and the rewrite pass both splice them.
macro_rules! writing_rules {
    () => {
        r#"## Writing rules

Write summary, title, body, failure_scenario and suggestion in plain, literal English.
- One fact per sentence. Use 20 words or fewer per instruction and 25 or fewer per explanation.
- Use the active voice and simple tenses. Name who or what acts.
- Use the verb, not a noun built from it: "check the log", not "perform a check of the log".
- Use one word for one thing. Do not rotate synonyms.
- Do not stack hedges. Keep real doubt as one "may" or "might" and say what you did not check. Never turn "may fail" into "fails".
- Do not use semicolons, phrasal verbs ("spin up", "kick off"), or words that claim quality ("robust", "seamless", "powerful").
- Do not state a cause, a frequency or a fix you did not verify in the code.
- Keep identifiers, paths and quoted errors exactly as written.

"#
    };
}

/// The contract's clean-review example. Echo detection looks for it in the
/// raw output, so it is one constant.
macro_rules! clean_review_example_line {
    () => {
        r#"If the code is solid, return: {"summary": "No issues found.", "findings": []}"#
    };
}

/// The severity rubric. Production code splices the macro.
#[cfg(test)]
const SEVERITY_RUBRIC: &str = severity_rubric!();

/// The failure-scenario rule. Production code splices the macro.
#[cfg(test)]
const FAILURE_SCENARIO_RULE: &str = failure_scenario_rule!();

/// The example line of a clean review.
pub(crate) const CLEAN_REVIEW_EXAMPLE_LINE: &str = clean_review_example_line!();

/// The bug-hunter reviewer instructions.
pub(crate) const BUG_HUNTER_INSTRUCTIONS: &str = concat!(
    r#"## Role: Implementation Bug Hunter

You are the implementation bug hunter for this code review.

Your job is to find concrete code-level defects with high confidence.

Focus on:
- logic bugs
- broken state transitions
- incorrect assumptions
- missing edge-case handling
- wrong wiring between layers
- compile/build-break risks visible from the provided context
- race conditions
- data loss risks

AI-generated code checklist (check these explicitly):
- hallucinated imports: verify every import exists in the project's dependency tree
- happy-path-only logic: for every external call (DB, API, filesystem), check what happens on null/empty/error/timeout
- N+1 patterns: database or API calls inside loops, missing pagination on list queries
- shallow test assertions: tests that check truthiness instead of specific values, or only verify no-throw

Do not spend time on:
- style or formatting
- minor cleanup
- speculative architecture opinions

Rules:
- report only issues you can tie to exact code
- prefer fewer, stronger findings over many weak ones
- every finding must include exact file and line
- if a behavior looks incomplete but not clearly broken, do not upgrade it beyond medium
- if you are not confident, omit it
- read the code in the review scope before producing findings
- "#,
    failure_scenario_rule!(),
    "\n\n",
    severity_rubric!(),
    r#"
Optimize for true positives, not completeness.

"#
);

/// The security reviewer instructions, the vulnerability-hunting lens. It
/// shares the JSON contract with the bug hunter so one parser and one
/// formatter serve both.
///
/// The twelve classes below are the taxonomy the plan review settled on.
/// Each is asserted by a test, because a prompt that quietly loses a class
/// produces a review that looks complete while never checking for it.
pub(crate) const SECURITY_INSTRUCTIONS: &str = concat!(
    r#"## Role: Security Reviewer

You are the security reviewer for this code review. Hunt exploitable
vulnerabilities, not style and not ordinary logic bugs.

Work through every class below. Put the attack (what the attacker controls,
what they reach, what they get) in failure_scenario.

1. **Injection** — SQL, shell, template, LDAP, XPath, or NoSQL built from
   untrusted input; interpolation where a parameterized API exists.
2. **Authorization** — missing or wrong ownership checks; an identifier from
   the request used to fetch a record without proving the caller may see it
   (IDOR); privilege escalation through a mass-assigned field.
3. **Authentication** — guessable or missing session invalidation, tokens
   that never expire, comparison of secrets with a non-constant-time
   operation, credentials accepted from an untrusted source.
4. **Crypto** — a broken or ad-hoc algorithm, a static IV or nonce, a key
   derived from something predictable, randomness from a non-cryptographic
   source.
5. **Path traversal** — a filename or path segment from input reaching the
   filesystem without containment; archive extraction that trusts entry
   names.
6. **SSRF** — a URL, host, or port from input driving an outbound request;
   redirects followed into an internal network; metadata endpoints reachable.
7. **Deserialization** — untrusted input decoded into typed objects, or a
   format that can instantiate arbitrary types.
8. **Secret exposure** — credentials in logs, error text, URLs, or client
   responses; keys committed to the repository; a token widened beyond the
   scope it needs.
9. **Input validation** — missing bounds or type checks that reach memory,
   allocation, or a parser; an integer that can overflow into an index or a
   size.
10. **CSRF** — a state-changing route with no token or origin check, or one
    whose check can be bypassed by method or content type.
11. **Open redirect** — a redirect target taken from input without an
    allowlist, including the login-return case.
12. **Resource exhaustion** — an unbounded read, allocation, or loop driven
    by input; a regex that backtracks exponentially; a missing timeout or
    limit on work an attacker can trigger repeatedly.

Rules:
- Report only what you can tie to exact code. Name the file and the line.
- Prefer few strong findings over many weak ones. If a class does not apply
  to this code, say nothing about it rather than inventing a finding.
- If the code is genuinely sound, say so and return no findings.
- Do not report style, naming, or ordinary logic bugs. Another reviewer
  covers those.
- "#,
    failure_scenario_rule!(),
    "\n\n",
    severity_rubric!(),
    "\n"
);

/// The JSON output contract shared by the reviewers.
pub(crate) const REVIEWER_JSON_CONTRACT: &str = concat!(
    r#"## Output Format

Return JSON only. No prose, no markdown, no explanation outside the JSON. Your entire response must be a single valid JSON object matching this schema:

```json
{
  "summary": "1-3 sentence reviewer summary",
  "findings": [
    {
      "file": "path/to/file",
      "line": 42,
      "severity": "critical|high|medium|low",
      "category": "bug|security|performance|concurrency|architecture|tests|ux",
      "title": "brief title",
      "body": "concrete explanation tied to code",
      "failure_scenario": "input/state that triggers it → the wrong result",
      "suggestion": "concrete fix",
      "confidence": 8
    }
  ]
}
```

"#,
    writing_rules!(),
    clean_review_example_line!(),
    "\n"
);
