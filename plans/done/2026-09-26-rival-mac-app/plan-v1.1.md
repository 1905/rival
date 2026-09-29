# Rival Mac App Implementation Plan v1.1

**Date:** 2026-09-26
**Status:** done
**Changes from v1.0:** P5b added. The user asked on 2026-09-26: "all tab results or any list should be paginated". Their choices: Mac app + TUI, numbered pages of 50. Also added: `make run` / `make install` (c5da153).
**Spec:** ./spec.md

**Goal:** Remove `rival server` and ship `Rival.app` (menu bar + window, TUI parity), installed with `brew install --cask 1905/tap/rival-app`. Then rewrite the README.
**Architecture:**
- `app/` is a Swift package with two targets: `RivalKit`, pure logic (decode, group, filter, sanitize, PID guard, store) with XCTest coverage, and `RivalApp`, SwiftUI views plus `MenuBarExtra`.
- The app reads `~/.rival/sessions` directly.
- Release CI builds a universal ad-hoc-signed bundle and updates the cask, whose `postflight` strips quarantine. P0 proved this route works.

**Tech Stack:** Swift 6.0 / SwiftUI / Observation, macOS 14+, XCTest, Python (bundle script), GitHub Actions `macos-14`, Homebrew cask.

> For agentic workers: use superpowers:subagent-driven-development to implement task-by-task. Checkbox syntax for tracking.

**Implementer rules (every dispatch states these verbatim):**
- Model: Opus 5.5 (`model: "opus"`).
- Scope: write the code and unit tests the task names, and run only `go build ./... && go vet ./... && go test ./...` (Go tasks) or `cd app && swift build && swift test` (Swift tasks). NEVER run e2e / integration / live / smoke tests. Never launch the app against the real `~/.rival`, never call a provider API, never publish, push, tag, upload a release, edit the tap repo, or touch infra. Never run anything money-bearing. Never `brew install`.
- Swift tests use temp dirs (`FileManager.default.temporaryDirectory` + UUID) for the session root. Never read `~/.rival`. Tests never send real signals: use the injected `ProcessInspector`/`Signaller`.
- Don't commit. The orchestrator commits per phase.
- Never `rm`: `mkdir -p /tmp/trash && mv <path> /tmp/trash/<basename>.$(date +%Y%m%d-%H%M%S)`.

## File map

**Delete:** `rival/cmd/server.go`, `rival/internal/server/`.

**Create:**
- `app/Package.swift`
- `app/Sources/RivalKit/`: `Session.swift`, `Grouping.swift`, `Filter.swift`, `LogReader.swift`, `ProcessGuard.swift`, `SessionStore.swift`, `Theme.swift`
- `app/Sources/RivalApp/`: `RivalApp.swift`, `MainWindow.swift`, `RunList.swift`, `RunDetail.swift`, `LogView.swift`, `MenuBar.swift`, `Notifier.swift`, `Logo.swift`
- `app/Tests/RivalKitTests/*`, `app/Tests/Fixtures/*.json`
- `app/scripts/bundle.py`, `app/scripts/make_icon.py`, `app/Resources/AppIcon.icns`, `app/Resources/Info.plist.tmpl`

**Modify:**
- `.github/workflows/release.yml`
- `README.md`, `docs/*.md`, `CHANGELOG.md`
- `rival/cmd/root.go` (help text)
- `rival/internal/sessionview/*.go` (comments)

**External:** `1905/homebrew-tap/rival-app.rb`. The orchestrator writes it; implementers never do.

## Locked interfaces (Swift)

```swift
// RivalKit
public struct Session: Decodable, Identifiable, Hashable { /* fields per spec "Data model"; CodingKeys snake_case */ }
public struct RunItem: Identifiable, Hashable { public let id: String; public let sessions: [Session] }  // id = "group:<gid>" | "solo:<id>"
public enum RunStatus: String { case running, queued, completed, failed, unknown }
public enum StatusTab: CaseIterable { case all, running, failed, done }
public enum DaySection: String { case today = "TODAY", yesterday = "YESTERDAY", thisWeek = "THIS WEEK", older = "OLDER" }

public func groupRuns(_ sessions: [Session]) -> [RunItem]                     // == sessionview.Group order
public func runStatus(_ item: RunItem) -> RunStatus                           // == sessionview.Status
public func runKind(_ item: RunItem) -> String                                // review|plan|mega|sec|slop|raw (TUI kindLabel)
public func runElapsed(_ item: RunItem, now: Date) -> String                  // == sessionview.Elapsed
public func matches(_ item: RunItem, terms: [String]) -> Bool                 // AND, case-insensitive
public func section(for date: Date, now: Date, calendar: Calendar) -> DaySection
public func sanitizeLog(_ raw: String) -> String                              // strip ANSI, expand tabs (logfmt rules)
public func readTail(path: String, maxBytes: Int) throws -> (text: String, truncated: Bool)

public protocol ProcessInspector { func startTime(pid: Int32) -> Int64? }     // sysctl KERN_PROC_PID, ns
public protocol Signaller { func terminate(pid: Int32) -> Bool }
public enum StopOutcome: Equatable { case signalled([Int32]), nothingRunning, alreadyDead([String]) }
public func stop(_ item: RunItem, inspector: ProcessInspector, signaller: Signaller) -> StopOutcome

@MainActor @Observable public final class SessionStore {
  public init(root: URL, debounce: Duration = .milliseconds(250))
  public private(set) var runs: [RunItem]
  public func start(); public func stop()
}
public enum Theme { /* phosphor tokens as Color, matching dashboard/styles.go */ }
```

Session root: `RIVAL_HOME` env, else `~/.rival`. The orchestrator's fixture runs use `RIVAL_HOME`.

## Sanity check (before Task 1)

- [ ] Branch `feat/rival-mac-app` from a clean `master` (after the prompt-core merge).
- [ ] `cd rival && go build ./... && go test ./...` are green.
- [ ] `swift --version` is 6.0.x.

## Phase P1 — remove `rival server`

### Task 1
- [ ] Move `cmd/server.go` and `internal/server/` to /tmp/trash. Fix references in the root help and install hints. `sessionview` comments drop the web mentions.
- [ ] Test: `rootCmd.Find([]string{"server"})` doesn't resolve to `server`.
- [ ] The TUI tests and `sessionview` tests stay green.
- [ ] CHANGELOG `Unreleased`: "Breaking: `rival server` removed; use Rival.app (`brew install --cask 1905/tap/rival-app`) or `rival tui`".

**P1 gate:** build, vet and tests green. `git grep -n "internal/server"` is empty. Commit `feat!: remove rival server (web dashboard)`.

## Phase P2 — RivalKit

### Task 2 — package + model + fixtures
- [ ] `Package.swift`: platform `.macOS(.v14)`, Swift 6 mode, targets `RivalKit`, `RivalApp` (executable) and `RivalKitTests`.
- [ ] `Session` decoding: tolerant, snake_case keys, RFC3339 dates with fractional seconds (Go `time.Time` JSON).
- [ ] Fixtures copied from real Go test shapes: solo running, queued, completed, failed with error, a megareview group plus consilium (history), a Sol session (history), missing optional keys and an unknown key.
- [ ] Tests decode every fixture.

### Task 3 — grouping, status, kind, elapsed, filter, sections
- [ ] Port `sessionview.Group`, `Status`, `Kind` and `Elapsed` (`rival/internal/sessionview/group.go`) and the TUI's `kindLabel`/`sectionFor`/`matchesFilter` (`rival/internal/dashboard/session_list.go`).
- [ ] Parity tests use the same inputs and expected values as the Go tests: copy the cases from `group_test.go` and `session_list_test.go`.

### Task 4 — log reader + sanitize + PID guard
- [ ] `readTail` (256 KB cap, truncated flag) and `sanitizeLog`. Shared vectors come from `rival/internal/logfmt/*_test.go`.
- [ ] `stop(_:inspector:signaller:)` mirrors the TUI rule:
  - targets are the live members with pid > 0;
  - a member whose inspector start time ≠ `pidStart` (when set) is `alreadyDead` and gets no signal.
  - Tests use fakes.

### Task 5 — SessionStore
- [ ] Use `DispatchSource` on the dir fd plus a 2 s poll fallback, and re-decode only changed files (mtime and size).
- [ ] Ignore `.json.tmp`. A corrupt file is skipped and retried later. A missing root means it watches the parent until the folder appears.
- [ ] Tests use a temp root: write, rewrite, delete and corrupt files, then await the snapshot after the debounce.

**P2 gate:** `swift build && swift test` green. Commit `feat(app): RivalKit — session model, grouping, filter, logs, stop guard, store`.

## Phase P3 — window UI

### Task 6 — main window
- [ ] `NavigationSplitView` with a sidebar header (gradient ASCII logo, stats) and the run list.
  - `.searchable` → `matches`; a status tab picker with counts; day sections.
  - Rows: glyph (a spinner for running), kind, model id, effort, time, project.
- [ ] Theme: dark only, `Theme` tokens, SF Mono.

### Task 7 — detail
- [ ] Header: breadcrumb, status, elapsed, and a follow toggle.
- [ ] Tabs: Output / Prompt / Info.
  - Output: a monospaced `LogView` (`NSTextView` wrapped for performance) with live tail and follow-until-scroll-up. ⌘F finds with next/prev and highlight.
  - Prompt: the full prompt from the session file.
  - Info: every field plus the error.
- [ ] Group member picker, with the judge last.
- [ ] Toolbar: "Open full log" (`NSWorkspace`) and "Stop…", which opens a confirm sheet → `stop()` → a result toast.
- [ ] Logic tests only for the view models: `RunDetailModel` (follow state, member anchoring by id, search matches) in RivalKit.

**P3 gate:**
- `swift build && swift test` green.
- The orchestrator bundles, launches with a `RIVAL_HOME` fixture and takes `screencapture` screenshots of the list and the detail.
- Commit `feat(app): main window with run list and detail`.

## Phase P4 — menu bar, notifications, icon

### Task 8
- [ ] `MenuBarExtra(.window)`:
  - the label is a glyph plus the live count;
  - the popover lists live runs with elapsed time, and the last 5 finished;
  - it has an "Open Rival" button.
- [ ] `Notifier`: on a running/queued → completed/failed transition seen by the store, post `UNUserNotificationCenter`. Clicking opens the window on that run. A `UserDefaults` toggle `notifyOnFinish`, default on.
  - The transition detection lives in RivalKit (`FinishDetector`) and is tested.
- [ ] `make_icon.py`: renders a gradient "r" on black to `AppIcon.icns` with Pillow + `iconutil`. It's local and free; no paid image generation.

**P4 gate:** swift green; the orchestrator screenshots the menu bar popover and sees a notification by flipping a fixture session. Commit `feat(app): menu bar live view and finish notifications`.

## Phase P5 — packaging + release

### Task 9 — bundle script
- [ ] `bundle.py --version X --arch universal`:
  - `swift build -c release --arch arm64 --arch x86_64`;
  - assemble `Rival.app` (Info.plist from the template: `dev.1905.rival`, `LSMinimumSystemVersion` 14.0, `CFBundleShortVersionString` X, the icon);
  - `codesign --force --deep -s -`;
  - `ditto` zip to `dist/Rival-app.zip`;
  - print the sha256.
- [ ] Unit-test the plist rendering with a tiny Python test (stdlib `unittest`).

### Task 10 — release workflow
- [ ] `release.yml`: add job `app` on `macos-14` for tag pushes. It checks out, runs `swift test`, `bundle.py --version ${GITHUB_REF_NAME#v}` and `gh release upload`. Then it clones `1905/homebrew-tap` with `HOMEBREW_TAP_TOKEN`, writes `rival-app.rb` from `app/scripts/cask.rb.tmpl` (version, sha256), commits and pushes.
- [ ] The job waits for the goreleaser job (`needs:`) so the release exists.
- [ ] `app/scripts/cask.rb.tmpl`: the cask from the spec, including the `postflight` strip.
- [ ] Workflow lint: `actionlint` if installed, else a YAML parse check.

**P5 gate:**
- The orchestrator runs `bundle.py` locally, installs from a local-only tap with the same cask template (`file://` zip, like P0), launches, screenshots, then uninstalls and untaps.
- No push or tag in this phase.
- Commit `feat(app): universal ad-hoc bundle and cask release job`.

## Phase P5b — pagination (Mac app + TUI)

Shared rules, identical in both UIs:
- **PAGE_SIZE = 50 runs.** Section headers (TODAY…) are not counted. A page shows the section headers of the runs on it.
- **Footer:** `‹ prev  page P/N  next ›  · T runs`. When N ≤ 1 it shows only `T runs`.
- **Keys:**
  - TUI list mode: `n` next, `p` prev, PgDn/PgUp too.
  - App: ←/→ and `[`/`]` when the list has focus, plus clickable ‹ › in the footer.
  - `j`/`k` (TUI) or ↑/↓ (app) past the last or first row of a page moves to the next or previous page. `g`/`G` or Home/End go to the first row of page 1 and the last row of the last page.
- **Resets:** a tab or filter change resets to page 1, with the cursor on the first run.
- **Refresh:** the selection is anchored by run id, so after a refresh the page is the one that holds the selected run. If the run vanished, the page clamps to the last page.
- **Menu bar popover:** LIVE caps at 10 rows plus "+N more — open Rival". RECENT stays at 5.

### Task 11 — TUI (Go)
- **Files:** `rival/internal/dashboard/session_list.go`, `model.go`, `keys.go` + tests.
- **Code:** `listPane` gets a `page int`, plus `pageCount()` and `pageRows()`. `view` renders only the page, with the footer on the last line of the pane (`listRows` accounts for it). `keys.go` gets `NextPage`/`PrevPage` in list help.
- **Tests:**
  - page slicing with sections;
  - cursor wrap across pages with `j`/`k`;
  - `n`/`p` bounds;
  - a tab or filter change resets to page 1;
  - anchoring: a new run above moves the selection to the right page;
  - the footer text;
  - frame width and height stay exact at 60/90/120/200.

### Task 12 — App (Swift)
- **Files:** RivalKit `Pagination.swift` (pure: `pageCount`, `pageSlice`, `pageFor(runID:)`, `move(selection:by:)` across pages), `RunList.swift`, `MainWindow.swift`, `MenuBar.swift` + tests.
- **Tests:** RivalKit only, with the same cases as the TUI list.

**P5b gate:**
- `go test ./...` and `swift test` pass.
- The orchestrator takes a TUI tmux capture showing the footer.
- The orchestrator takes an app screenshot of page 2 against a fixture with ≥120 runs. `dev_bundle.py --fixture` gets `--many N` to generate them.
- Commit `feat: paginate run lists (50 per page) in the app and the TUI`.

## Phase P6 — README + docs

### Task 11
- [ ] `README.md`, cold-audience register (formal, no contractions).
  - **Part 1, the human TL;DR (first screen):**
    - what rival is;
    - install: `brew install 1905/tap/rival`, `rival install`, and `brew install --cask 1905/tap/rival-app`;
    - a 5-row command table;
    - app and TUI screenshots (`assets/app.png`, `assets/tui.png`);
    - one note on the ad-hoc app and `brew trust --tap 1905/tap` if brew asks.
  - **Part 2, the agent reference:**
    - every skill and native command: syntax, flags, defaults, exit codes (`rival wait` 0/2/3/4);
    - the detached flow;
    - the single-review JSON contract (`failure_scenario`) and console format;
    - the plan/antislop contract;
    - the config keys that exist (checked in `internal/config`);
    - sessions dir, TUI, app;
    - removed features and their replacements.
- [ ] Every command, flag, default and key is checked against `rival … --help` and the code. The report lists 10 spot checks.
- [ ] `docs/runtime-reference.md`, `docs/ai-code-review-patterns.md`, `docs/releasing.md`: drop Sol, megareview and server content, and add the app release step.
- [ ] CHANGELOG `Unreleased`: the app.

**P6 gate:** the orchestrator reads the README end to end. Commit `docs: README — TL;DR for humans, reference for agents`.

## Exit gates (orchestrator)

1. `/rival-codex review` of the branch (Go and Swift diff): verify every finding and fix what holds.
2. `/simplify`.
3. Spec as-built notes, plan → done, then `plans/done/`.
4. Merge to `master` the same day. Rebuild `~/.local/bin/rival`, and bundle and install the app locally from the local tap route (not the real tap: the real cask appears only with the next `/rival-release`).
5. `/notify` with screenshots.

## Type-consistency check

The names above (`Session`, `RunItem`, `groupRuns`, `runStatus`, `runKind`, `runElapsed`, `matches`, `section`, `sanitizeLog`, `readTail`, `ProcessInspector`, `Signaller`, `StopOutcome`, `stop`, `SessionStore`, `Theme`, `FinishDetector`, `RunDetailModel`, `bundle.py` flags) are used identically in Tasks 1-11.
