# Parsed output — plan v1.0

**Date:** 2026-09-29
**Spec:** spec.md (approved)
**Status:** done (P1+P2 landed as one commit; P4 palette added on user request; Codex review + /simplify applied)
**Branch:** `feature/parsed-output` from `master`

## Corrections to the spec (verified 2026-09-29 against real logs)

- Codex prints the final answer **twice**: once streamed after the last `codex` line (with `hook: Stop` noise lines mixed in), then again, clean, after `tokens used` + a number line. Verified in `475aaaa0…log` (lines 10611, 10641-10669) and `18d2808c…log` (11999, 12068). Rule: after the last `codex` line, if a line `tokens used` followed by a number line exists, the answer is the text after that number line. Otherwise it is the text after the last `codex` line. No `codex` line → whole text.
- Theme names: `Theme.fail` (not `failed`), `Theme.running`, `Theme.dim`, `Theme.ok`, `Theme.fg`, `Theme.accent`.

## P1 — Parser + Result tab

### Task 1.1 — `ResultParser.swift` (RivalKit)
File: `app/Sources/RivalKit/ResultParser.swift` (new). Pure, `Sendable`.

```swift
public struct Finding: Decodable, Equatable, Sendable {
    file, severity, category, title, body, failureScenario?, suggestion?: String; line, confidence: Int
    // CodingKeys: failure_scenario. Missing strings decode as "", missing ints as 0.
}
public enum RunResult: Equatable, Sendable {
    case findings(summary: String, rating: Int?, findings: [Finding])   // findings sorted
    case markdown(String)
    case failed(String)
}
public func finalAnswer(_ raw: String) -> String          // rule above
public func jsonObjects(_ s: String) -> [Substring]      // port of Go jsonObjects, byte-level over utf8, validity via JSONSerialization
public func parseRunResult(raw: String) -> RunResult      // flow below
public func severityRank(_ s: String) -> Int              // critical 0, high 1, medium 2, low 3, else 4 (case-insensitive)
public func sortedFindings(_ f: [Finding]) -> [Finding]  // severity, then confidence desc, stable
```

`parseRunResult` flow (spec "Answer extraction", with the footer correction):
1. `answer = finalAnswer(raw)`.
2. `objs = jsonObjects(answer)`. From the last to the first: plan payload (keys summary+rating+findings present, rating 1...10, summary != "1-3 sentence overall assessment of the plan") → `.findings(rating:)`. Then again for reviewer payload (summary+findings, summary != "1-3 sentence reviewer summary") → `.findings(rating: nil)`. Drop placeholder findings exactly as Go `isPlaceholderFinding` (the three category literals, `path/to/file`, `critical|high|medium|low`).
3. No payload, and `answer` trimmed starts with `{` → `.failed("JSON answer did not decode: <last decode error or 'no summary/findings keys'>")`.
4. Else `sanitizeLog(answer)` trimmed non-empty → `.markdown`.
5. Else `.failed("no answer in the log")`.

Key-presence check must be real presence (decode to `[String: Any]`), like Go `hasJSONKey`.

### Task 1.2 — tests
File: `app/Tests/RivalKitTests/ResultParserTests.swift` (new). Cases from spec "Tests → Unit (ResultParserTests)", plus:
- codex double-answer: markdown answer appears once, without `hook: Stop` lines.
- codex double-answer with JSON: `.findings` decoded once.
- Port `rival/internal/review/parse_test.go` and `plan_test.go` string cases for `ParseReviewerOutput`, `ParsePlanOutput`, `FinalAnswer`, `jsonObjects` that are plain inline strings. Use `rival/internal/review/testdata/consilium_echoed_schema.log` by copying it to `app/Tests/Fixtures/logs/` only if it has no real private project names (grep for them; if it has any, skip it).

### Task 1.3 — raw tail for the parser
Files: `app/Sources/RivalApp/RunDetail.swift` (`LogSnapshot`).
- `LogSnapshot` gains `result: RunResult?`. `load(_:parse:)`: when `parse` is true, run `parseRunResult(raw: tail.text)` on the same detached task (raw, unsanitized). Parse only for non-live members.
- Cache: parsing is skipped when (path, size, mtime) is unchanged since the last snapshot — keep the last snapshot's key in `LogSnapshot` and pass the previous snapshot in.

### Task 1.4 — `ResultPane.swift` (RivalApp)
File: `app/Sources/RivalApp/ResultPane.swift` (new). Layout per spec "Result tab UX".
- `ResultPane(session:, snapshot:, onOpenRaw:)`.
- Header card: `modelName(session)` · effort · mode · duration; `rating N/10` right when present; severity count chips; summary.
- Sections per severity (CRITICAL, HIGH, MEDIUM, LOW, OTHER) in a `ScrollView { LazyVStack }`.
- Finding card: `file:line` (line omitted when 0) · category · `conf N`; title bold; body; `DisclosureGroup` for failure scenario and suggestion, collapsed.
- Colours: critical/high `Theme.fail`, medium `Theme.running`, low/other `Theme.dim`. Fonts: existing `Mono` styles.
- `.markdown`: `MarkdownBlocks(text:)` — block split (`#` headers, `-`/`*`/`N.` items, ``` fences, paragraphs); inline via `AttributedString(markdown:options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace))`, plain on throw.
- `.failed(msg)`: icon + "Couldn't parse this run's output." + `msg` + session `error` if any + `Button("Open Raw")`.
- Live member: "Run is still going — the result appears when it finishes." + Open Raw.
- `snapshot.placeholder` (missing log etc.): show it, + Open Raw.
- All text `.textSelection(.enabled)`.

### Task 1.5 — tab wiring (P1 keeps Output default)
Files: `RunDetailModel.swift`, `RunDetail.swift`.
- `DetailTab`: `result = "Result", raw = "Raw", prompt, info` — order Result, Raw, Prompt, Info.
- P1: `sync` still resets to `.raw` (old default). `RunDetail` shows `ResultPane` for `.result`, `OutputPane` for `.raw`. Log poll key: poll only when tab is `.raw` and live (unchanged semantics); load once with parse when `.result`.

Gate: `swift build && swift test`. Commit: `feat(app): Result tab — parsed findings and markdown`.

## P2 — Defaults + auto-switch

### Task 2.1 — model rules
File: `RunDetailModel.swift`.
- `sync(_:)`: new run → `tab = isLive(primary status) ? .raw : .result`.
- New stored `memberWasLive: Bool?`. `sync` on the same run: if the member was live and now is not, and `tab == .raw && follow` → `tab = .result`. Update `memberWasLive`.
- Tests in `RunDetailModelTests`: the five rows of the spec's tab table.
- Update the doc comment ("Opening another run resets to Output" → the new rule).

### Task 2.2 — view
`RunDetail.swift`: Open Raw buttons set `detail.tab = .raw`. Nothing else.

Gate: `swift build && swift test`. Commit: `feat(app): open finished runs on Result, live runs on Raw`.

## P3 — Fake data + screenshot

### Task 3.1 — fake names (implementer)
- `app/Tests/Fixtures/*.json`: `work_dir` → `/Users/dev/src/<fake>`: `acme-api`, `orbit-web`, `ledger`, `atlas-cli`, `pixel-lab`. Map the five real names one-to-one onto these. Keep `rival` (the project itself is public).
- `app/scripts/dev_bundle.py`: `MANY_PROJECTS` → the fake names; the docstring's real project name → `orbit-web`.
- Grep the whole `app/` and `plans/` for the real names and replace in fixtures/tests/scripts. Plans/docs: replace too.
- New fake logs in `app/Tests/Fixtures/logs/`: `plan-codex.log` (codex transcript header, a short echoed prompt, two `exec` blocks, `codex`, answer, `tokens used`, `12.345`, answer again as JSON with rating 6 and 5 findings over all severities, invented file paths under `internal/…` of a fake project), `review-markdown.log` (claude-style markdown answer), `broken-json.log`. `dev_bundle.py --fixture` writes these as the logs of matching fixture sessions (completed-plan.json → plan-codex.log, etc.) instead of `SAMPLE_LOG`.
- Tests keep passing (`swift test`).

### Task 3.2 — screenshot (orchestrator)
- `dev_bundle.py --build`, `--fixture /tmp/rival_shot/home --many 60 --select solo:<completed-plan id>`, launch with `RIVAL_HOME`, window 1180×760, Result tab shown by default (finished run).
- `screencapture -l <windowid>` of the main window. Check the image by eye: no real names, no raw log.
- Replace `assets/app.png`. README caption/alt if it mentions Output.
- Remove the fixture dir via move to `/tmp/trash`.

Commit: `docs: fake-data fixtures and Result-tab screenshot`.

## Implementer rules (stated verbatim in every dispatch)

"You write the code and unit tests your task names and run the focused unit tests for that task (`swift build`, `swift test --filter …`). You NEVER run e2e / integration / live / smoke tests, never launch the app, never run soak_test.py, never publish, deploy, push or touch infra, never run anything money-bearing. Do not commit; the orchestrator commits. Do not set any test-DB env var."
