# Review stack cleanup + reviewer prompt core

**Date:** 2026-09-26
**Scope:** rival/ (cmd, internal/config, internal/review, internal/parser, internal/executor, internal/skills, README)
**Status:** done

## TL;DR

**P1 — Megareview and the judge are gone.**
- **What:** delete `rival command megareview`, top-level `rival review`, the consilium judge and the `/rival-review` skill. `rival install` removes the installed skill.
- **Why:** you no longer use multi-model megareview, so the judge and its roster code are dead weight.
- **You do:** nothing.
- **Not done:** old megareview runs still show in the TUI and web dashboard as history.

**P2 — No legacy models; antislop runs Codex + Claude.**
- **What:** Sol (`gpt-5.6-sol`) is removed: `rival command sol`, `rival run sol`, the `sol` alias and every hidden Sol fallback. A codex run with no model set uses `gpt-6-astra`.
- **Antislop:** runs Codex (`gpt-6-astra`) and Claude (Opus 5.5) by default, as two blocks. `-m` still picks one.
- **Why:** Sol is legacy but was still the silent default for any codex run with no model.
- **You do:** nothing. ⚠ Scripts calling `rival command sol` break; the release notes will say so.
- **Not done:** old Sol sessions still display.

**P3 — One reviewer prompt + structured single-model review.**
- **What:** `/rival-codex review`, `/rival-claude review`, `/rival-k3 review` and `/rival-grok review` use the bug-hunter prompt with JSON output. The command prints a formatted findings list plus the session log path, not the whole transcript.
- **Low-confidence findings:** findings below confidence 6 go in a short separate block, not dropped.
- **Why:** today's prompt returns prose and invites speculative "architecture/performance" findings, and command mode dumped a 495 KB transcript with the answer printed twice.
- **You do:** nothing.
- **Not done:** raw prompts (`/rival-codex 'explain X'`) are untouched. Unparseable JSON falls back to the raw log.

**P4 — Failure scenario + one severity rubric.**
- **What:** every finding from the code reviewers (bug-hunter and security) must include `failure_scenario`: the concrete input or state → the wrong result. Both share one severity rubric.
- **Why:** a finding with no triggering scenario is the main source of false positives, and the reviewers had no severity definitions at all.
- **You do:** nothing.
- **Not done:** plan and antislop keep their own severity wording.

**P5 — Plan prompt checks the repo; persona gone.**
- **What:** the plan-review prompt must check the plan's claims about existing code against the repo and cite `file:line` when it disagrees. "Ruthless senior staff engineer" is removed from the plan and antislop prompts.
- **Why:** the plan prompt judged plans only against the system "as described" in the plan, and the persona pushes models to over-report.
- **You do:** nothing.
- **Not done:** no change to the plan/antislop JSON schema or ratings.

**P6 — README rewrite (moved to the Swift app job, 2026-09-26).**
- **What:** `README.md` opens with a short TL;DR for humans (what it is, install, five main commands, TUI screenshot), then a detailed reference for agents (every command, flag, default, JSON contract, config key and exit code, each checked against the code).
- **Why:** you asked for it (2026-09-26), and the README still documents megareview and Sol.
- **You do:** nothing.
- **Not done:** no docs site, no other docs files beyond `docs/releasing.md` fixes.

## Problem(s)

1. **Megareview is unused dead weight.** You dropped it on 2026-09-26 ("no megareview anymore so judge not needed"). It still ships:
   - `cmd/command_megareview.go` and `cmd/review.go`;
   - `review.RunMegaReviewWithModels` plus the consilium session, judge pick and judge run (`internal/review/runner.go:67,328,491-597`);
   - judge prompt, contract, parser and formatter (`prompt.go:34-67,109-164,273-305`, `parse.go:42`, `format.go:11-95`, `consilium.go`);
   - the roster resolver (`internal/config/config.go:397-460`);
   - the `/rival-review` skill (`internal/skills/embed.go:6`).
2. **Sol is legacy but still the silent default.**
   - `executor.RunCodex` and `RunCodexModel` with an empty model run `gpt-5.6-sol` (`internal/executor/codex.go:40-50`).
   - `review.modelForCLI("codex")` returns Sol (`runner.go:586-589`).
   - `rival command sol` and `rival run sol` still exist (`cmd/command_sol.go`, `cmd/run_sol.go`, `cmd/model_specs.go:19-26`), and `sol` is a roster alias (`config.go:445`).
3. **Antislop runs only Codex.** `defaultAntislopModels = []string{config.CodexLabel}` (`cmd/command_antislop.go:37`). You want Codex and Opus 5.5 on every run.
4. **Single-model review is free-form prose.** `config.ReviewPrompt` (`config.go:533-550`) asks for grouped prose with CRITICAL/HIGH/MEDIUM only, and no confidence or JSON. The bug-hunter + JSON contract (`prompt.go:13-32,69-107,223-248`) served only megareview.
5. **It invites speculation.** "Architecture issues — missing abstractions, scalability bottlenecks" and "Performance problems — missing indexes" (`config.go:541-542`) sit against "No speculative nitpicks" (`config.go:546`).
6. **Command mode prints the whole transcript.** `cmd/model_command.go:177-183` writes the full session log to stdout. The 2026-09-26 branch review was 495 KB (12,548 lines), with the final answer twice.
7. **No failure scenario, and no reviewer severity rubric.** `ReviewerFinding` has no field for the triggering scenario (`internal/review/types.go:12-21`). Severity was defined only in the judge prompt (`prompt.go:157-161`).
8. **Plan review doesn't check code.** `PlanReviewPrompt` judges conflicts with the system "(as described)" (`config.go:566`).
9. **Persona.** "You are a ruthless senior staff engineer" opens `ReviewPrompt`, `PlanReviewPrompt` and `AntislopCodePrompt` (`config.go:533,557,626`).

## Goals

1. No megareview command, runner, judge, roster resolver or `/rival-review` skill. `rival install` removes the skill (Problem 1).
2. No runnable Sol path. Every codex default is `gpt-6-astra` (Problem 2).
3. Antislop defaults to `codex,claude` (Problem 3).
4. Every code-review surface builds its prompt with `review.BuildReviewerPrompt` and prints `FormatReviewResult` (Problem 4, Problem 5, Problem 6).
5. `failure_scenario` is in the reviewer contract, the types, the parser and the formatters. One `severityRubric` is shared by bug-hunter and security (Problem 7).
6. The plan prompt verifies codebase claims (Problem 8).
7. No persona in any prompt (Problem 9).

## Non-goals

- Deleting history. Old megareview, judge and Sol sessions keep rendering in the TUI and web dashboard. `sessionview.Kind`, `session.SortGroupMembers`, and the `GPT56SolModel`/`SolLabel` display mappings in `EngineLabel`/`replaceConcreteModelIDs` stay, commented as read-compat only.
- Changing `/rival-security` (K3/grok via `security.reviewer`), `/rival-plan*`, the raw-prompt path, `SystemPrompt`, `WorkdirPreamble` or `DiffReviewPreamble`.
- Changing plan/antislop JSON schemas or ratings.
- A new model, command or config key.

## Removal map (P1, P2)

| remove | keep (still used) |
|---|---|
| `cmd/command_megareview.go`, `cmd/review.go` | `review.WaitForGroupSlot`, `SkippedCLI` (used by run/command surfaces and doc reviews) |
| `RunMegaReviewWithModels`, `newConsiliumSession`, `runConsilium`, `preferredJudgeForTargets`, `pickJudge`, `modelForCLI`, `formatSkipped` (if unused) | `newReviewerSession`/`runReviewer` only if security still calls them; otherwise remove too |
| `BuildConsiliumPrompt`, `consiliumInstructions`, `reviewerLensMap`, `consiliumJSONContract`, `failedReviewerStub`, `ParseConsiliumOutput`, `FormatConsole`, `publicFoundBy`, `FilterByConfidence`, `SortFindings`, consilium `Finding`/`ConsiliumOutput` types | `DefaultConfidenceThreshold` (moves to the single-review formatter), `severityRank` |
| `judgeRunnerFor` in `dispatch.go` | reviewer runners used by security |
| `config.DefaultReviewTargets`, `ResolveReviewTargets`, `ReviewTarget` (if security doesn't use it), roster aliases | `PromptKind` (bug-hunter vs security), `RolePromptOverride` |
| `rival-review` in `skills/embed.go` Names + `//go:embed`; the dir moves to /tmp/trash | Add `"rival-review"` to `Deprecated` |
| Sol: `cmd/command_sol.go`, `cmd/run_sol.go`, the sol entry in `model_specs.go`, `parser.ParseGPT56SolArgs`, `executor.RunCodex`, the Sol default in `RunCodexModel`, `builtinModelEffort`/`knownEffortModel` Sol cases, `sol` alias | `GPT56SolModel`, `SolLabel` and their `EngineLabel`/`ModelLabel`/`replaceConcreteModelIDs`/`SortGroupMembers` rows (display of old sessions) |
| `cmd/wait.go` consilium-specific log scanning (the second session ID "logged only after reviewers finish") | generic wait |
| README megareview/`/rival-review`/Sol sections | the rest of the README |

`RunCodexModel` with an empty model → `config.CodexModel`. With `GPT56SolModel` → error `sol was removed; use codex`.

## Prompt design (P3-P5)

**One builder.** `review.BuildReviewerPrompt(scope string, kind config.PromptKind) string` is the single entry point.
- `config.ReviewPrompt` is deleted.
- `internal/parser` stops building prompts. It returns `IsReview`, `ReviewScope` and `AutoScope`, and `cmd` builds the prompt (`parser` imports only `config`, and the builder lives in `review`).
- Auto-scope keeps `DiffReviewPreamble` in front, with scope `the changed files listed above` (as `cmd/gitscope_helper.go:43` does today).

**Bug-hunter** keeps its focus list and AI-code checklist, and adds:
- "Each finding needs a concrete failure_scenario: the input or state that triggers it and the wrong result. If you cannot state one, drop the finding."
- the shared rubric.

**Security** gets the same two additions. Its "state the attack" rule maps onto `failure_scenario`.

**Shared severity rubric** (one Go const):

```
Severity:
- critical: data loss, a security hole an attacker can reach, or a crash/outage on a normal path
- high: wrong result or broken flow on a realistic path; a race that can corrupt state
- medium: wrong result only on an edge case, or a real performance problem on a hot path
- low: minor defect with a cheap workaround
```

**Reviewer JSON contract**, with one new field:

```json
{
  "summary": "1-3 sentence reviewer summary",
  "findings": [
    {
      "file": "path/to/file", "line": 42,
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

The example `summary` string stays byte-identical, so `isExampleSummary` and echo detection keep working.

**Console format** (new `review.FormatReviewConsole`, used for single-model review):

```
═══ RIVAL REVIEW ═══

Model: codex (gpt-6-astra)
Scope: rival/internal/dashboard/

Summary: …

1. [high] Stop can signal a reused PID — rival/internal/dashboard/model.go:597
   Confirmation rechecks only the cached status; PIDStart is never checked.
   Scenario: run dies, PID reused by another process, user presses x → y → SIGTERM hits it.
   Fix: compare the procinfo start time to PIDStart before signalling.
   (bug, confidence 9)

Low confidence (1):
- [low] … — file:line (confidence 4)

Findings: 1 total — 0 crit, 1 high, 0 med, 0 low
Log: /Users/…/.rival/sessions/<id>.log
```

- Sorting and the tally share one helper with `FormatSecurityConsole` (no third copy). The threshold is `DefaultConfidenceThreshold` (6).
- On parse failure, or an echoed example: `═══ RIVAL REVIEW — UNPARSED OUTPUT ═══`, the problem, then `PublicRuntimeLog` of the raw log.
- Security prints `Scenario:` too.

**Run surface** (`rival run <model> --review`): same prompt, the live mirror stays, then the formatted block.

**Plan prompt**:
- Drop the persona.
- Replace "(as described)" with: "When the plan makes claims about existing code (files, functions, schemas, config, commands), open the repo and check them. A claim the code contradicts is a bug finding: cite the plan section in `file` and put the contradicting code location in `body`."

**Antislop prompt**: drop the persona only.

## File-level changes

| file | change |
|---|---|
| `cmd/command_megareview.go`, `cmd/review.go`, `cmd/command_sol.go`, `cmd/run_sol.go` | Removed (moved to /tmp/trash, deletion recorded by git). |
| `cmd/model_specs.go` | Drop the sol spec. |
| `cmd/command_antislop.go` | `defaultAntislopModels = {codex, claude}`. Usage and flag help say so. |
| `cmd/model_command.go`, `cmd/model_run.go`, `cmd/gitscope_helper.go` | Review prompts come from `review.BuildReviewerPrompt(scope, config.PromptBugHunter)`. After the run, review mode prints `review.FormatReviewResult(...)` (command) or appends it after the mirror (run). Raw prompts are unchanged. |
| `cmd/wait.go` | Drop the consilium-only log scanning. Generic waiting is unchanged. |
| `internal/review/runner.go`, `dispatch.go`, `consilium.go`, `format.go`, `parse.go`, `prompt.go`, `types.go` | Remove the megareview/judge code per the removal map. Add `severityRubric`, the failure-scenario rule, `FailureScenario` on `ReviewerFinding`, and `failure_scenario` in `reviewerJSONContract`. |
| `internal/review/review_format.go` (new) | `FormatReviewConsole`, `FormatReviewResult`, plus the sort/tally helper shared with security. |
| `internal/review/security.go` | Use the shared helper. Print `Scenario:`. |
| `internal/config/config.go` | Delete `ReviewPrompt`, `DefaultReviewTargets`, `ResolveReviewTargets` and the Sol effort cases. Mark the Sol display mappings read-compat. Remove the persona from plan/antislop, and add plan repo verification. |
| `internal/parser/parser.go`, `internal/parser/review.go` | Delete `ParseGPT56SolArgs`. Stop setting `Prompt` for reviews. |
| `internal/executor/codex.go` | Delete `RunCodex`. An empty model means `CodexModel`, and Sol is rejected. |
| `internal/skills/embed.go`, `internal/skills/rival-review/` | Drop from Names and embed; add to `Deprecated`. |
| `README.md`, `CHANGELOG.md` | Remove the megareview/Sol docs. Add an `Unreleased` entry listing the removals (breaking: `command sol`, `run sol`, `command megareview`, `review`, `/rival-review`). |
| `*_test.go` | Delete tests of removed code. Update the prompt-text assertions. Add the tests below. |

## Tests

**Removal (P1, P2)**
- `rival command megareview`, `rival review`, `rival command sol` and `rival run sol` → cobra "unknown command" (a cmd test walks `rootCmd` for these names).
- `skills.Names` has no `rival-review`, and `Deprecated` contains it. The install test removes an installed `rival-review` dir (temp HOME).
- `RunCodexModel` with an empty model builds args with `gpt-6-astra`. With `gpt-5.6-sol` it returns the removal error.
- The antislop default flag value is `[codex claude]`, and `-m claude` gives only claude.
- Old sessions: an old megareview group, a Sol session and a consilium session still render in the TUI list and the web API (existing parity/session_list tests plus one new fixture).
- `go build ./...` has no references to the removed symbols. `go vet` is clean.

**Prompt (P3-P5)**
- `BuildReviewerPrompt` for bug-hunter and security contains `severityRubric`, `failure_scenario` and the drop rule.
- No prompt contains "ruthless" (table over the exported prompts plus the builder outputs).
- `ParseReviewerOutput` round-trips `failure_scenario`, and payloads without it still parse.
- `FormatReviewConsole`: order, tally, `Scenario:`, the low-confidence block, `Log:`, and `No issues found.` for an empty list.
- `FormatReviewResult`: nil, echoed example or empty summary → UNPARSED plus the raw log.
- `FormatSecurityConsole` prints `Scenario:` only when it is set.
- `PlanReviewPrompt` has the repo-verification text. Plan and antislop example summaries are byte-identical to before.
- Parser: a review command yields `IsReview`, the scope and `AutoScope` with an empty `Prompt`; a raw prompt still carries its prompt.
- cmd, with a fake runner:
  - review + JSON answer → formatted review + `Log:`, no transcript;
  - non-JSON answer → UNPARSED + log;
  - raw prompt → log;
  - exit ≠ 0 → log + auth hint.

**Manual** (orchestrator only, one real run each): `/rival-codex review` on a small diff, `/rival-antislop` (two blocks: codex + claude) and `/rival-plan-codex` on this spec.

## Failure modes & decisions

| failure | behaviour |
|---|---|
| User runs a removed command (`megareview`, `review`, `sol`) | cobra `unknown command`. The CHANGELOG lists the replacement: `rival command codex review`. |
| Installed `/rival-review` skill left from 3.34 | `rival install` removes it (Deprecated list). |
| `~/.rival/config.yaml` still names `sol` or a megareview roster | Keys that no longer apply are ignored, with a one-line warning at load. There is no hard failure. |
| Model returns prose or broken JSON | `RIVAL REVIEW — UNPARSED OUTPUT` + problem + public raw log. Exit 0 (the run succeeded). |
| Model echoes the example JSON | Treated as unparsed (the `looksLikeEchoedPrompt` logic, generalised to the bug-hunter markers). |
| Finding without `failure_scenario` | Printed without a `Scenario:` line. It isn't second-guessed. |
| Antislop: claude fails preflight (no subscription/Docker) | Existing doc-review skip path: codex block + `Skipped: claude — <reason>`. |
| `prompts.bug_hunter` / `prompts.security` override in config | The override still wins, and the JSON contract is still appended. The rubric is not injected into overrides. |
| Provider exits ≠ 0 | Unchanged: log + auth hint, exit code propagated. |

## Out of scope

- Deleting or migrating old session files.
- Web/TUI rendering of structured findings.
- Changing the `/rival-*` skill text beyond removing `/rival-review`.
- Re-tuning `DefaultConfidenceThreshold`.
- Releasing. `/rival-release` stays a separate step you trigger.

## Rollout

- **P1** — remove megareview, judge, `rival review`, `/rival-review` (Deprecated), wait's consilium scanning, README section. Commit.
- **P2** — remove Sol paths and default codex to `gpt-6-astra`, antislop defaults to `codex,claude`, CHANGELOG `Unreleased`. Commit.
- **P3** — single builder for every review surface, `ReviewPrompt` deleted, `FormatReviewConsole`/`FormatReviewResult`, command and run surfaces print the formatted review. Commit.
- **P4** — `failure_scenario` + `severityRubric` in bug-hunter, security, types, parser and formatters. Commit.
- **P5** — plan repo verification, persona removed, the "no ruthless" test. Commit.

## As-built notes

Shipped 2026-09-26 on `feat/review-stack-cleanup`. The commits:
- P1 95d78f1
- P2 54618f6
- P3 cb5d9dc
- P4 a051045
- P5 9ee5a0f / 423d497
- smoke fix 4b0b1f6
- review fixes 12c9034
- simplify bcbb93c

Drift from the text above:

- **GitLab MR review** lived only on the removed megareview commands. It was ported to single-model review in P3 (plan v1.1, Task 10b) instead of being dropped. The pinned snapshot is reviewed, and provider credentials stay in the caller's workdir: `modelSpec.run` gained a `credWorkdir` parameter instead of restoring `credential_workdir.go`.
- **Echo detection** is split by lens. Single review counts as an echo only when no payload follows the last clean example, because codex logs contain the prompt and tool output may quote `prompt.go`. Security keeps the strict marker check, because opencode logs never contain the prompt.
- **Codex logs** are parsed from `review.FinalAnswer`, the text after the last `codex` header. Review-shaped JSON printed by a tool can no longer stand in for the answer.
- **Failures.** `review.RunFailureReason` (empty output, or quota text when nothing parsed) fails single-model and plan runs with exit 1. It restores the guard the megareview runner had.
- **Run surface.** `rival run <model> --review` with no scope now auto-detects changed files, like command mode.
- **Codex model.** `RunCodexModel` accepts only `gpt-6-astra`. An empty model or the Sol id errors: there are no production callers of either, so no silent default remains.
- **Config.** An old `efforts.sol` key in `~/.rival/config.yaml` is dropped silently instead of failing every command.
- **P6** (README) moved to the Mac app job (`plans/2026-09-26-rival-mac-app/`).
- **Not done here:** stale Sol/megareview text in `docs/runtime-reference.md` and `docs/ai-code-review-patterns.md` is handled by the Mac app job's P6.
