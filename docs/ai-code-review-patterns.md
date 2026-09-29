# AI Code Review Patterns for Multi-Agent Review Systems

Distilled from field experience and industry research (DORA 2026, CSA, CodeScene). Applicable to any system that uses LLM agents to review LLM-generated code.

## The 6 AI Code Bug Patterns

These are the failure modes specific to AI-generated code. Train reviewers (human or AI) to look for these explicitly.

### 1. Hallucinated imports
Agent imports a package, function, or API that doesn't exist in the dependency tree. Looks plausible, compiles in the agent's imagination, fails on install.

**Detection:** diff new imports against `npm ls` / `pip freeze` / `go list -m all`. Flag any import not in the lockfile.

### 2. Happy-path-only logic
Code handles the normal case and crashes on the first edge case. No null checks, no empty array handling, no timeout logic.

**Detection:** for every external call, ask "what happens if this returns null/empty/error/timeout?"

### 3. Security anti-patterns
SQL injection via string concatenation. XSS via innerHTML. Secrets in source. Missing auth middleware on new routes.

**Detection:** search for `innerHTML`, string interpolation in queries, hardcoded tokens, missing auth decorators on new endpoints.

### 4. Over-abstraction
Simple task produces factories, interfaces, abstract base classes, and configuration systems. More abstraction = more surface area for bugs.

**Detection:** count new files vs spec scope. A 1-endpoint spec shouldn't create 5 files. If you can inline a helper and the code reads better, the abstraction shouldn't exist.

### 5. N+1 and performance traps
ORM calls inside loops. Unbounded queries without pagination. Synchronous operations that should be async.

**Detection:** look for database/API calls inside any `for`/`map`/`forEach`. Check for missing `LIMIT`/pagination on list queries.

### 6. Shallow test assertions
`expect(result).toBeTruthy()` instead of `expect(result.id).toBe(42)`. Tests that verify the function didn't throw instead of verifying it did the right thing.

**Detection:** search test files for `toBeTruthy`, `toBeDefined`, bare `not.toThrow`, or any assertion that doesn't check a specific value.

---

## Review Time Budget by Task Type

Not all agent output deserves equal scrutiny.

| Task type | Budget | Focus |
|-----------|--------|-------|
| Test generation | 3-5 min | Run tests, check coverage, verify assertions test real behavior |
| Documentation | 2-3 min | Spot-check against current code, look for hallucinated signatures |
| Simple CRUD | 5-8 min | Input validation, auth checks, error handling, N+1 queries |
| Business logic | 10-15 min | Trace logic against spec, check edge cases explicitly |
| Auth / security | 12-18 min | Every auth boundary, every input sanitization, every secret ref |
| Refactoring | 5-10 min | Functional equivalence, existing tests still pass, no API changes |

---

## Reject-and-Retry Decision Matrix

| Severity | Action | Time cost |
|----------|--------|-----------|
| Cosmetic (naming, formatting) | Fix it yourself | 1-2 min |
| Localized bug (one function) | Send failing test or exact error to agent | 5-10 min |
| Wrong approach (viable but suboptimal) | Follow-up prompt with constraints | 10-20 min |
| Fundamentally wrong (>30% rewrite) | Kill session, rewrite spec, fresh agent | 20-45 min |

**Two-strike rule:** if an agent fails to fix an issue after two follow-up prompts, restart from scratch. Iterating on a confused agent costs more than a clean start with a better spec.

---

## Diff Scope Validation

Before reading any code:

1. **Run the agent's tests.** If they fail, stop reviewing and send back with the failure.
2. **Check diff size vs spec scope.** `git diff --stat` should match the ask. Large diffs from small specs = over-engineering.
3. **Read commit messages.** If the summary doesn't match the spec, the agent misunderstood.

---

## Cross-Agent Integration Checks

When multiple agents work in parallel (worktrees, branches), these bugs appear only at merge time:

- **Type definitions** — did two agents define the same type differently?
- **API contracts** — do request/response shapes match between producer and consumer?
- **Database schema** — conflicting migrations on the same table?
- **Import paths** — one agent created `utils/auth.ts`, another expects `lib/auth.ts`
- **Shared state** — two agents writing to the same config file or global store

**Merge protocol:** one branch at a time, run full suite after each merge. Never batch-merge.

---

## Current Rival implementation

### Role prompt checklists

Every single-model code review (`/rival-codex review`, `/rival-claude review`,
`/rival-k3 review`, `/rival-grok review`) uses the bug-hunter role. That role's
prompt includes the implementation-focused patterns most useful for
high-confidence findings:

**Bug Hunter role** checks:

- Check for hallucinated imports (packages not in dependency tree)
- Verify error handling on every external call (DB, API, filesystem)
- Look for N+1 patterns (queries/API calls inside loops)
- Verify test assertions check specific values, not just truthiness

Every finding must also carry a `failure_scenario`: the input or state that
triggers it and the wrong result. Security patterns (pattern 3) have their own
lens, `/rival-security`, and over-abstraction (pattern 4) has `/rival-antislop`.

### Diff context in review preamble

When Rival auto-detects a Git review scope, the preamble lists the changed
files and adds `git diff --stat` output. This gives reviewers scope awareness:

```
Changed files:
src/api/handler.go
src/api/handler_test.go
internal/auth/middleware.go

Diff stats:
 src/api/handler.go          | 30 ++++++--
 src/api/handler_test.go     | 25 +++++++
 internal/auth/middleware.go |  2 +-
```

---

## Human vs AI Review Split

| Let AI review | Human must review |
|---------------|-------------------|
| Code style, formatting, naming | Business logic correctness against requirements |
| Common security patterns (SQLi, XSS) | Authorization logic and access control design |
| Test coverage gaps | Whether the right things are being tested |
| Unused imports, dead code, type errors | Architecture decisions and integration fit |
| Documentation accuracy | Whether approach matches team conventions |
