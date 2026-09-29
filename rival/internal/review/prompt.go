package review

import (
	"fmt"
	"strings"

	"github.com/1905/rival/internal/config"
)

// BuildReviewerPrompt builds the reviewer prompt by combining scope context
// with the bug-hunter instructions and the JSON output contract.
func BuildReviewerPrompt(scope string, kind config.PromptKind) string {
	var sb strings.Builder

	fmt.Fprintf(&sb, "Review scope: %s\n\n", scope)

	// Both prompts stay overridable through ~/.rival/config.yaml. An empty
	// override falls through rather than producing an empty prompt.
	key, builtin := "bug_hunter", bugHunterInstructions
	if kind == config.PromptSecurity {
		key, builtin = "security", securityInstructions
	}
	if override, ok := config.RolePromptOverride(key); ok && strings.TrimSpace(override) != "" {
		sb.WriteString(override)
	} else {
		sb.WriteString(builtin())
	}

	sb.WriteString(reviewerJSONContract())
	return sb.String()
}

func bugHunterInstructions() string {
	return `## Role: Implementation Bug Hunter

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
- ` + failureScenarioRule + `

` + severityRubric + `
Optimize for true positives, not completeness.

`
}

// securityInstructions is the vulnerability-hunting lens. It shares the JSON
// contract with the bug hunter so one parser and one formatter serve both.
//
// The twelve classes below are the taxonomy the plan review settled on. Each
// is asserted by a test, because a prompt that quietly loses a class produces
// a review that looks complete while never checking for it.
func securityInstructions() string {
	return `## Role: Security Reviewer

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
- ` + failureScenarioRule + `

` + severityRubric + `
`
}

// severityRubric is the one severity scale both code-review lenses use.
const severityRubric = `Severity:
- critical: data loss, a security hole an attacker can reach, or a crash/outage on a normal path
- high: wrong result or broken flow on a realistic path; a race that can corrupt state
- medium: wrong result only on an edge case, or a real performance problem on a hot path
- low: minor defect with a cheap workaround
`

// failureScenarioRule makes every finding carry a concrete trigger and
// result. A finding the model cannot ground this way is dropped, not guessed.
const failureScenarioRule = "Each finding needs a concrete failure_scenario: the input or state that triggers it and the wrong result. If you cannot state one, drop the finding."

func reviewerJSONContract() string {
	return `## Output Format

Return JSON only. No prose, no markdown, no explanation outside the JSON. Your entire response must be a single valid JSON object matching this schema:

` + "```json" + `
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
` + "```" + `

` + cleanReviewExampleLine + `
`
}

// cleanReviewExampleLine is the contract's clean-review example. Echo
// detection looks for it in the raw output, so it is one constant.
const cleanReviewExampleLine = `If the code is solid, return: {"summary": "No issues found.", "findings": []}`
