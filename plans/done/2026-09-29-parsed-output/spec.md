# Parsed output view

**Date:** 2026-09-29
**Scope:** /Users/kass/dev/rival/app (Swift app only), README.md, assets/app.png
**Status:** done

## TL;DR

**P1 — Result tab.**
- What: a new **Result** tab in the run detail pane. It shows the model's final answer as a card: header (model, effort, duration, rating, severity counts, summary), then one card per finding grouped by severity. If the answer is prose, not JSON, it shows as rendered markdown.
- Why: today the pane shows only the raw log. A 700 KB codex transcript buries the answer at the very end.
- You decide: nothing.
- Does NOT: show the model's activity (commands it ran), touch the TUI or the Go CLI, or parse live output.

**P2 — Raw tab + parse errors.**
- What: the old Output tab is renamed **Raw** and is unchanged. Tab order: Result · Raw · Prompt · Info. Live runs open on Raw with follow. Finished runs open on Result. If the parse fails, Result shows the reason and an "Open Raw" button.
- Why: parsing will sometimes fail. Raw must stay one click away.
- You decide: nothing.
- Does NOT: switch tabs while you are reading. It switches only when a run you selected finishes, and only if you are still on Raw with follow on.

**P3 — Fake-data README screenshot.**
- What: new `assets/app.png` shows one selected finished run with the parsed Result view on the right. No raw log anywhere in the image. All data comes from fixtures with fake project names, paths and findings.
- Why: the current fixtures and screenshot use five real private project names. The current `assets/app.png` shows them. It was never pushed (checked 2026-09-29: not on `origin/master`), so replacing it before the first push keeps the names private.
- You decide: nothing. The old image is only in local, unpushed commits (`4951776`). This spec does NOT rewrite those commits.
- Does NOT: change `assets/tui.png` (check it separately).

**P4 — Quieter palette (added 2026-09-29 on user request: "colors are too bright, go easy on the eyes").**
- What: app-only palette C. Near-black green-tinted background, grey-green text, soft green for active marks only. The selected row and active tab get a dark tint with a green edge instead of a neon fill. Completed runs show dim; only running (amber) and failed (red) are coloured.
- Why: the neon `#39FF14` fills and all-mint text were tiring, and green on every completed row hid the failures.
- You decided: palette C over A, B and the old one (screenshots on fake data).
- Does NOT: change the TUI palette, the font or the layout.

## Problem(s)

1. **The answer is buried.** The Output tab renders the last 256 KB of the log as plain text (`app/Sources/RivalApp/RunDetail.swift:186-219`). A codex review log is 175-715 KB of echoed prompt and `exec` output. The answer comes after the last `codex` line, so the user has to scroll to the end and read raw JSON.
2. **JSON findings are unreadable as text.** Plan, antislop and fable runs end in a JSON payload (`summary`, `rating`, `findings[]`). The Go CLI already parses and formats it (`rival/internal/review/plan.go:30`, `parse.go:17`, `review_format.go:168`). The app shows it raw.
3. **Real names in public assets.** Fixtures use real project names (`app/Tests/Fixtures/*.json` `work_dir`, `app/scripts/dev_bundle.py:82` `MANY_PROJECTS`). The README screenshot `assets/app.png` shows them.

## Goals

1. (P1) Show the final answer as structured cards when it is a JSON payload, and as rendered markdown otherwise.
2. (P1) Use the same extraction rules as the Go CLI, so the app and `rival` agree on what the answer is.
3. (P2) Keep the raw log as its own tab. When parsing fails, show the reason and link to Raw.
4. (P2) Open the right tab by default: Raw for live runs, Result for finished runs.
5. (P3) Replace every real project name in fixtures, and regenerate `assets/app.png` from fixtures only.

## Non-goals

- An activity timeline of commands the model ran (option B, rejected).
- Parsing in the TUI or changing the Go CLI output.
- Clickable `file:line` links that open an editor.
- Re-parsing while the run is live.
- Full CommonMark. Tables, nested lists, images and HTML render as plain text lines.

## Answer extraction

Port of the Go rules, in a new `RivalKit/ResultParser.swift`. Input: the raw tail from `readTail` before `sanitizeLog` runs (JSON needs its escapes intact). Codex prints the answer at the end, so the tail always holds it unless one answer alone is longer than 256 KB (see failure modes).

```
raw tail
  │
  ├─ finalAnswer(raw)          text after the last line that is exactly "codex"
  │                            (Go review.FinalAnswer). No such line → whole text.
  │                            Then strip a trailing "tokens used\n<n>" footer.
  │
  ├─ jsonObjects(answer)       Go jsonObjects: brace-stack scan, keep spans that
  │                            are valid JSON.
  │
  ├─ last object with summary+rating+findings, rating 1..10, not the schema
  │  example                                     → .findings(rating: n, …)
  ├─ else last object with summary+findings, not the example
  │                                              → .findings(rating: nil, …)
  ├─ else a JSON-looking answer (first non-space char is "{") that did not
  │  decode                                      → .failed("JSON answer did not decode: …")
  ├─ else answer is non-empty after sanitizeLog  → .markdown(text)
  └─ else                                        → .failed("no answer in the log")
```

⚠ Check during the plan: that the `tokens used` footer really follows the answer in current codex logs. Verified so far: `codex` answer header line (line 4704 in one log). The footer is not verified.

Result shape:

```json
{
  "kind": "findings",
  "summary": "The plan is sound but orders the token rollout before its mitigations.",
  "rating": 6,
  "findings": [
    {"file": "B — Credential", "line": 99, "severity": "high", "category": "gap",
     "title": "…", "body": "…", "failure_scenario": "…", "suggestion": "…", "confidence": 7}
  ]
}
```

The schema-example checks and the placeholder-finding filter are ported from `isExampleSummary`, `planExampleSummary` and `dropPlaceholderReviewerFindings`. Severity order and labels come from `review_format.go:26-54`: critical, high, medium, low, then unknown.

## Result tab UX

```
┌──────────────────────────────────────────────────────┐
│ gpt-6-astra · xhigh · plan · 12m12s     rating 6/10   │
│ ● 1 critical  ● 2 high  ● 3 medium  ● 1 low           │
│ The plan is sound but orders the token rollout…      │
└──────────────────────────────────────────────────────┘
 CRITICAL ─────────────────────────────────────────────
 ┌ internal/auth/token.go:42 · security · conf 9 ─────┐
 │ Token stored before the scope check                │
 │ body text, wrapped                                 │
 │ ▸ Failure scenario   (collapsed by default)        │
 │ ▸ Suggestion         (collapsed by default)        │
 └────────────────────────────────────────────────────┘
 HIGH ─────────────────────────────────────────────────
 …
```

- Uses the existing `Theme` colours and `Mono` fonts. Severity colours: critical/high = `Theme.failed`, medium = `Theme.running`, low = `Theme.dim`. Plan confirms these names exist.
- Zero findings: header card plus the line "No findings."
- Markdown answer: a small block renderer. Lines starting with `#` become headers. `-`/`*`/`1.` become list items. Fenced ``` blocks become monospaced code. Everything else is a paragraph. Inline styling goes through `AttributedString(markdown:options: .inlineOnlyPreservingWhitespace)`. If that throws, the line shows as plain text.
- Text is selectable. ⌘F keeps working on Raw only.
- A group run (consilium): Result shows the selected member, same as Raw does today.

## Tab behaviour

`DetailTab` becomes `result = "Result", raw = "Raw", prompt = "Prompt", info = "Info"`.

| Event | Tab |
|---|---|
| Open a finished run | Result |
| Open a live run (queued / running) | Raw, follow on |
| The selected live run finishes, user is on Raw with follow on | Result |
| The selected live run finishes, user scrolled up or is on another tab | no change |
| Result tab of a live run (user clicked it) | "Run is still going — the result appears when it finishes." + Open Raw button |

The log poll (`RunDetail.swift:74-84`) stays as it is. Result is parsed once per loaded snapshot of a finished member, off the main actor, and cached by (path, file size, mtime).

## File-level changes

| File | Change |
|---|---|
| `app/Sources/RivalKit/ResultParser.swift` (new) | `finalAnswer`, `jsonObjects`, `parseResult(raw:) -> RunResult`, the example/placeholder filters, severity ranking. Pure and unit-tested. |
| `app/Sources/RivalKit/RunDetailModel.swift` | New `DetailTab` cases. `sync` picks Result or Raw from the run status. New `runFinished()` switches Raw→Result under the table rules. |
| `app/Sources/RivalKit/LogReader.swift` | `LogSnapshot` (or a sibling) keeps the raw tail next to the sanitized text so the parser gets unsanitized JSON. |
| `app/Sources/RivalApp/ResultPane.swift` (new) | Header card, finding cards, markdown blocks, parse-error and still-running states with an "Open Raw" button. |
| `app/Sources/RivalApp/RunDetail.swift` | Tab switch adds `.result`. `.output` becomes `.raw`. Call `runFinished()` when the member's status changes from live to done. |
| `app/Tests/RivalKitTests/ResultParserTests.swift` (new) | See Tests. |
| `app/Tests/Fixtures/*.json`, `app/Tests/Fixtures/*.log` (new logs) | Fake project names and paths. New fake logs: a codex plan transcript with JSON, a codex review transcript with markdown, a claude native markdown answer, a broken-JSON answer. |
| `app/scripts/dev_bundle.py` | `MANY_PROJECTS` becomes fake names. `--fixture` copies the new fake logs. |
| `assets/app.png`, `README.md` | New screenshot: one selected finished plan run, right pane on the Result tab (parsed cards only, no raw log), taken from the fixture home. Update the alt text or caption if it names the tab. |

## Tests

**Unit (`ResultParserTests`)**
- codex transcript with JSON after the last `codex` line → `.findings` with rating. The echoed schema example in the prompt is skipped.
- JSON payload printed by an `exec` (a file the model read) before the answer → ignored.
- plain JSON log (fable plan) → `.findings`.
- review payload without `rating` → `.findings(rating: nil)`.
- markdown answer (claude native, codex review) → `.markdown`, text equals the answer.
- answer starting with `{` that fails to decode → `.failed` with the decode reason.
- empty log / only the prompt echo → `.failed("no answer in the log")`.
- rating 0 or 11 → payload rejected, like Go.
- placeholder findings dropped, severity order critical→high→medium→low→unknown.
- Parity: the Go testdata cases in `rival/internal/review/*_test.go` that exercise `ParsePlanOutput` / `ParseReviewerOutput` / `FinalAnswer`, copied as Swift cases where they are plain strings.

**Unit (`RunDetailModelTests`)**
- open finished → Result. Open live → Raw.
- live→finished on Raw with follow → Result. With follow off → stays. On Prompt → stays.

**Manual**
- `make run` against the real `~/.rival` (read-only). Open a codex plan, a codex review, a claude native run and a failed run. Each shows a card, markdown, or an error with Open Raw.
- Screenshot run: `dev_bundle.py --fixture DIR --many 60 --select …`, one finished plan run selected, Result tab.

## Failure modes & decisions

| Failure | Behaviour |
|---|---|
| Answer alone is longer than 256 KB, so the tail starts mid-answer | JSON fails to decode → `.failed` with the reason, plus Open Raw. Not read further. Why: bounded memory, and Raw offers "Open full log". |
| Log file missing or unreadable | Same placeholder as Raw today, shown in Result. |
| Failed run with no answer | `.failed("no answer in the log")`, plus the session `error` field under it. |
| Schema example is the only JSON in the log | Treated as no payload → markdown or failed, never cards. |
| Unknown severity string | Shown under an "OTHER" group, last. |
| Markdown inline parse throws | That line shows as plain text. |
| Very large finding list (100+) | `LazyVStack`. No cap. |

## Out of scope

- Activity timeline (option B).
- TUI Result view.
- Rewriting the unpushed local commit `4951776` that added the old `assets/app.png`.
- `assets/tui.png` (not checked for real names).
- Tables and nested lists in markdown.

## Rollout

- **P1** — ResultParser + tests, ResultPane, Result tab (still behind Output as default). One commit.
- **P2** — rename Output→Raw, default-tab rules, `runFinished`, parse-error state. One commit.
- **P3** — fake fixtures, `dev_bundle.py` names, new `assets/app.png`, README. One commit.
