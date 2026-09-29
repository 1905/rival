# Review Stack Cleanup + Reviewer Prompt Core Implementation Plan v1.2

**Date:** 2026-09-26
**Status:** done
**Changes from v1.1:** P6 (README rewrite) is moved out of this job into the Swift app job (user decision 2026-09-26: the web `server` is being replaced by a native app, so the README is written once, after that). Exit gate 1's antislop/plan smoke runs stay.
**Changes from v1.0:** Task 10b added. GitLab MR review lived only on the deleted megareview commands (found in P1), so it is ported to single-model review instead of being lost. P1 also deleted `credential_workdir.go`; Task 10b restores it if the ported path needs it.
**Spec:** ./spec.md

**Goal:** Remove megareview, the judge and Sol. Run antislop on codex+claude. Give every code review one prompt builder, a `failure_scenario` and one severity rubric, and print it formatted.
**Architecture:** Deletions first (P1, P2) shrink `internal/review` to the single-review, security and doc-review paths. P3-P5 then change prompts and output on that smaller surface. `review.BuildReviewerPrompt` is the only code-review prompt builder, and `review.FormatReviewResult` is the only single-review printer.
**Tech Stack:** Go, cobra, the existing rival packages.

> For agentic workers: use superpowers:subagent-driven-development to implement task-by-task. Checkbox syntax for tracking.

**Implementer rules (every dispatch states these verbatim):**
- Model: Opus 5.5 (`model: "opus"`).
- Scope: write the code and unit tests the task names, and run only `go test ./...` (unit tests in this module), `go build ./...` and `go vet ./...`. NEVER run e2e / integration / live / smoke tests. Never rent a GPU or pod, never call a provider API, never run a real `rival command …` / `rival run …` / `rival review`, never publish, deploy, push or touch infra, never run anything money-bearing.
- Tests that create sessions or config MUST `t.Setenv("HOME", t.TempDir())`. Never read or write `~/.rival`. Never set any test-DB env var.
- Don't commit. The orchestrator commits per phase.
- Never `rm`: `mkdir -p /tmp/trash && mv <path> /tmp/trash/<basename>.$(date +%Y%m%d-%H%M%S)`. `git add -A` then records the deletion.

## File map

**Delete:**
- `cmd/command_megareview.go`, `cmd/review.go`, `cmd/command_sol.go`, `cmd/run_sol.go`, and their `_test.go` files (`gpt56_sol_command_test.go`, `review_model_test.go` if it only covers megareview, plus any megareview cmd tests).
- `internal/review/dispatch.go`, `internal/review/consilium.go`, `internal/review/format.go`, and their tests.
- `internal/skills/rival-review/`.

**Modify:**
- `internal/review/runner.go`: reduce to `SkippedCLI`, `formatSkipped`, `WaitForGroupSlot` and whatever they need. Rename it to `slots.go`.
- `internal/review/prompt.go`, `parse.go`, `types.go`, `security.go`, `plan.go` (shared helper).
- `internal/config/config.go`, `internal/parser/parser.go`, `internal/parser/review.go`, `internal/executor/codex.go`, `internal/skills/embed.go`.
- `cmd/model_specs.go`, `cmd/model_command.go`, `cmd/model_run.go`, `cmd/gitscope_helper.go`, `cmd/command_antislop.go`, `cmd/command_security.go` (comment only), `cmd/wait.go`.
- `internal/skills/rival-antislop/SKILL.md` (default-model wording), `README.md`, `CHANGELOG.md`.

**Create:** `internal/review/review_format.go`, `review_format_test.go`, `cmd/review_output_test.go`.

**Unchanged (read-compat for history):** `internal/sessionview`, `internal/dashboard`, `internal/server`, `session.SortGroupMembers`, and the `GPT56SolModel`/`SolLabel` rows in `EngineLabel`, `ModelLabel` and `replaceConcreteModelIDs`.

## Locked interfaces

```go
// internal/review/prompt.go
const severityRubric = "Severity:\n- critical: …\n- high: …\n- medium: …\n- low: …\n" // text exactly as in spec
func BuildReviewerPrompt(scope string, kind config.PromptKind) string   // unchanged signature

// internal/review/types.go
type ReviewerFinding struct { /* existing fields */ FailureScenario string `json:"failure_scenario,omitempty"` }

// internal/review/review_format.go
func FormatReviewConsole(out *ReviewerOutput, modelLabel, scope, logPath string) string
func FormatReviewResult(parsed *ReviewerOutput, raw, cli, model, scope, logPath string) string
func sortedFindings(in []ReviewerFinding) []ReviewerFinding                 // severity, then confidence; shared with security and plan
func severityTally(fs []ReviewerFinding) string                             // "Findings: N total — a crit, b high, c med, d low"
func validateReviewResult(out *ReviewerOutput, raw string) error            // nil/echo/empty summary → error (generalises ValidateSecurityResult)
const DefaultConfidenceThreshold = 6                                        // moved here from consilium.go

// internal/parser
// ParseResult.Prompt is "" when IsReview; ReviewScope/AutoScope as today.

// internal/executor/codex.go
var ErrSolRemoved = errors.New("sol was removed; use codex")
func RunCodexModel(ctx, sess, prompt, effort, workdir, model string, mirror io.Writer) (*Result, error) // "" → config.CodexModel; GPT56SolModel → ErrSolRemoved

// cmd
var defaultAntislopModels = []string{config.CodexLabel, config.ClaudeLabel}
func buildReviewPrompt(scope string, autoScope bool, workdir string) (prompt, target string) // wraps BuildReviewerPrompt + DiffReviewPreamble
```

## Sanity check (before Task 1)

- [ ] On branch `feat/review-stack-cleanup` from a clean `master`: `cd rival && go build ./... && go test ./...` → all ok.

## Phase P1 — megareview and the judge

### Task 1 — delete the commands
- [ ] Move `cmd/command_megareview.go` and `cmd/review.go` (and tests only for them) to /tmp/trash.
- [ ] Add a test in `cmd/review_output_test.go`: `rootCmd.Find([]string{"review"})` and `Find([]string{"command","megareview"})` do not resolve to a command named `review`/`megareview`.
- [ ] `go build ./...` shows the dangling references → fixed in Task 2.

### Task 2 — strip `internal/review`
- [ ] Delete `dispatch.go`, `consilium.go` and `format.go` (keep `severityRank` and `severityOrder`, moved to `review_format.go`).
- [ ] From `runner.go`, delete `RunMegaReviewWithModels`, `newReviewerSession`, `runReviewer`, `newConsiliumSession`, `runConsilium`, `preferredJudgeForTargets`, `pickJudge`, `modelForCLI`, `truncate` (if unused) and the `RunResult`/`cliResult` types. Keep `SkippedCLI`, `formatSkipped` and `WaitForGroupSlot`. Rename the file to `slots.go`.
- [ ] From `prompt.go`, delete `BuildConsiliumPrompt`, `reviewerLensMap`, `consiliumInstructions`, `consiliumJSONContract` and `failedReviewerStub`.
- [ ] From `parse.go`, delete `ParseConsiliumOutput` and `dropPlaceholderFindings` (keep the reviewer and plan parsing).
- [ ] From `types.go`, delete `ReviewInput`, `Finding` and `ConsiliumOutput`.
- [ ] Delete tests of the removed code, and keep every test of the kept code green.
- [ ] `go build ./... && go test ./internal/review/...`.

### Task 3 — config roster
- [ ] Delete `ReviewTarget`, `DefaultReviewTargets`, `ResolveReviewTargets`, the roster aliases and config's `DefaultConfidenceThreshold` if nothing else uses it. Keep `PromptKind` and `PromptSecurity`.
- [ ] Remove the corresponding config tests.
- [ ] If `~/.rival/config.yaml` parsing has roster keys, they are ignored with one `log.Warn` line (test with a temp HOME config).

### Task 4 — wait + skill + docs
- [ ] `cmd/wait.go`: remove the consilium-specific scanning (the late second session ID) and update its comments. Existing wait tests pass, and consilium-only tests are removed.
- [ ] `internal/skills/embed.go`: drop `rival-review` from `//go:embed` and `Names`, and add it to `Deprecated` with the comment `// megareview removed 2026-09-26`. Move `internal/skills/rival-review/` to /tmp/trash.
- [ ] Update the skills embed/install tests: an installed `rival-review` dir is removed by install (temp HOME).
- [ ] `README.md`: remove the megareview / `/rival-review` / `rival review` sections and mentions.
- [ ] `go build ./... && go vet ./... && go test ./...`.

**P1 gate (orchestrator):**
- Build, vet and tests green.
- `git grep -n "megareview\|consilium" -- rival/cmd rival/internal/review rival/internal/config` shows only read-compat comments.
- Commit `feat!: remove megareview and the consilium judge`.

## Phase P2 — no Sol; antislop on codex+claude

### Task 5 — remove Sol
- [ ] Delete `cmd/command_sol.go`, `cmd/run_sol.go`, the sol spec in `model_specs.go`, `parser.ParseGPT56SolArgs`, `executor.RunCodex`, and the Sol cases in `builtinModelEffort`/`knownEffortModel`/`validConfiguredModelEffort`.
- [ ] `RunCodexModel`: `""` → `config.CodexModel`; `config.GPT56SolModel` → `ErrSolRemoved`. Tests for both.
- [ ] Mark the kept `GPT56SolModel`/`SolLabel` rows `// read-compat: display of sessions recorded before Sol's removal`.
- [ ] Test: `rootCmd` has no `sol` under `command` or `run`.

### Task 6 — antislop defaults
- [ ] `defaultAntislopModels = {codex, claude}`. Update the usage text and flag help ("Default models are codex and claude").
- [ ] Test: with no `-m`, the resolved CLIs are `[codex claude]`; `-m claude` gives `[claude]`. Use the flag default and `parsePlanModels`; no runs.
- [ ] `internal/skills/rival-antislop/SKILL.md`: the description and body say "Default models Codex and Claude (Opus 5.5)".

### Task 7 — CHANGELOG
- [ ] `CHANGELOG.md` `## Unreleased`: breaking removals (`rival review`, `rival command megareview`, `/rival-review`, `rival command sol`, `rival run sol`, the `sol` alias) with replacements, plus antislop's new default.

**P2 gate:** build + vet + tests, then commit `feat!: remove Sol; antislop runs codex and claude`.

## Phase P3 — one prompt, formatted single review

### Task 8 — parser stops building review prompts
- [ ] `parser.go` and `review.go`: for review commands, leave `Prompt` empty and keep `IsReview`, `ReviewScope` and `AutoScope`. Raw prompts are unchanged.
- [ ] Update the parser tests, and add assertions for an empty prompt on review.

### Task 9 — `review_format.go`
- [ ] Implement `sortedFindings`, `severityTally`, `validateReviewResult`, `FormatReviewConsole` and `FormatReviewResult` per the spec layout:
  - header;
  - Model line: `codex (gpt-6-astra)` = `EngineLabel (model id)`;
  - Scope, Summary;
  - numbered findings with body, `Fix:` and `(category, confidence N)`;
  - a `Low confidence (N):` block for findings below `DefaultConfidenceThreshold`;
  - tally, `Log: <path>`;
  - `No issues found.` when empty.
- [ ] Parse failure → `═══ RIVAL REVIEW — UNPARSED OUTPUT ═══`, `Problem:`, then `config.PublicRuntimeLog(cli, model, raw)`.
- [ ] Switch `FormatSecurityConsole` and `formatPlanBody` to use `sortedFindings`/`severityTally` (their output stays byte-identical, and the existing tests prove it).
- [ ] Generalise echo detection: `validateReviewResult` rejects the bug-hunter example summary when the raw output also contains `## Role: Implementation Bug Hunter`.
- [ ] Tests per the spec list.

### Task 10 — cmd wiring
- [ ] Add `buildReviewPrompt(scope, autoScope, workdir)` in `cmd/gitscope_helper.go`: `DiffReviewPreamble` + `BuildReviewerPrompt("the changed files listed above", PromptBugHunter)` when auto-scope finds changes; otherwise `BuildReviewerPrompt(scope, PromptBugHunter)`.
- [ ] `model_command.go`: review mode uses it. After the run, when exit is 0, parse the log and print `FormatReviewResult(parsed, raw, cli, model, scope, sess.LogFile)`. Exit ≠ 0 keeps today's log + hint. Raw prompts keep printing the log.
- [ ] `model_run.go`: same prompt. After a successful mirrored run, print a blank line + `FormatReviewResult`.
- [ ] Delete `config.ReviewPrompt` and fix the references. The `command_security.go:46` comment names `BuildReviewerPrompt`.
- [ ] `cmd/review_output_test.go`: inject a fake `modelSpec.run` that writes a log:
  - (a) valid JSON → stdout has `═══ RIVAL REVIEW ═══`, a `Log:` line and no transcript marker;
  - (b) prose → `UNPARSED OUTPUT` + log;
  - (c) raw prompt → log verbatim;
  - (d) exit 2 → log, and the error carries code 2.

### Task 10b — GitLab MR review on single-model review
- [ ] `rival command <codex|claude|k3|grok> review <MR-URL>` and `rival run <model> --review <MR-URL>` resolve the MR the way the removed megareview path did (`git show master~2:rival/cmd/merge_request.go`, `…/cmd/review.go:78-85`):
  - `mergerequest.Prepare(ctx, scope, workdir)` → a pinned snapshot;
  - the review runs in `snapshot.Workdir` with `snapshot.Scope`;
  - the identity line prints first;
  - the checkout closes after the run.
- [ ] Restore `review.WithCredentialWorkdir`/`credentialWorkdir` (from `git show master:rival/internal/review/credential_workdir.go`) ONLY if an executor still reads it. Otherwise pass the credential workdir explicitly. Grep `credentialWorkdir` usage on master to decide.
- [ ] `rejectUnresolvedMR` stays for raw prompts. Its message points to `rival command codex review <MR-URL>`.
- [ ] Tests, with a fake `mergerequest` seam (a package var), no network:
  - an MR URL scope → the run gets the snapshot workdir and scope, and the checkout is closed;
  - a non-MR scope → unchanged;
  - a raw prompt with an MR URL → rejected with the new message.

**P3 gate:** build + vet + tests, then commit `feat(review): one bug-hunter prompt and formatted output for single-model review`.

## Phase P4 — failure scenario + severity rubric

### Task 11
- [ ] Add `severityRubric` (text from the spec) to `bugHunterInstructions` and `securityInstructions`, plus the rule "Each finding needs a concrete failure_scenario … If you cannot state one, drop the finding." In security, add "put the attack in failure_scenario".
- [ ] Add `failure_scenario` to `reviewerJSONContract` (between body and suggestion). Keep the example `summary` byte-identical.
- [ ] Add `FailureScenario` to `ReviewerFinding`.
- [ ] `FormatReviewConsole` and `FormatSecurityConsole` print `   Scenario: …` when it is set.
- [ ] Tests:
  - the prompt contains the rubric, the field and the rule, for both kinds;
  - the parse round-trips, and old payloads parse;
  - `Scenario:` is shown only when set;
  - the override path (`RolePromptOverride`) still appends the contract.

**P4 gate:** build + vet + tests, then commit `feat(review): failure_scenario on every finding and one severity rubric`.

## Phase P5 — plan prompt + persona

### Task 12
- [ ] `PlanReviewPrompt`: drop the persona sentence start. Replace the "(as described)" clause and add the repo-verification paragraph from the spec.
- [ ] `AntislopCodePrompt`: drop the persona.
- [ ] Keep the example summaries byte-identical (`planExampleSummary` guard).
- [ ] Test `TestNoPromptUsesAPersona`: no "ruthless" in `PlanReviewPrompt`, `AntislopCodePrompt`, `BuildReviewerPrompt(x, bug)` or `BuildReviewerPrompt(x, security)`.
- [ ] Test: the plan prompt contains "open the repo and check them".

**P5 gate:** build + vet + tests, then commit `feat(prompts): plan review verifies code claims; no persona`.

## Phase P6 — moved

The README rewrite moved to the Swift app job (`plans/2026-09-26-rival-mac-app/`).

## Exit gates (orchestrator)

1. Manual, real runs (orchestrator only):
   - `/rival-codex review` on a small diff: output is formatted, has scenarios, has a `Log:` line.
   - `/rival-antislop` on the same diff: two blocks (codex + claude).
   - `/rival-plan-codex` on `spec.md`: plan findings cite code.
2. `/rival-codex review` of the whole branch: verify every finding and fix what holds.
3. `/simplify` on the diff.
4. Spec `## As-built notes`, then plan → done, then `plans/done/`.
5. Merge to `master` the same day, rebuild `~/.local/bin/rival`, run `rival install --force` (removes `/rival-review` locally), `/notify`.

## Type-consistency check

`BuildReviewerPrompt`, `FormatReviewConsole`, `FormatReviewResult`, `sortedFindings`, `severityTally`, `validateReviewResult`, `DefaultConfidenceThreshold`, `severityRubric`, `ReviewerFinding.FailureScenario`, `ErrSolRemoved`, `RunCodexModel`, `defaultAntislopModels` and `buildReviewPrompt` keep the same names and signatures in Tasks 1-12. `SkippedCLI`, `formatSkipped` and `WaitForGroupSlot` keep their signatures after the file rename.
