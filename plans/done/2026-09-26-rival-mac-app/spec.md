# Rival Mac app (replaces `rival server`)

**Date:** 2026-09-26
**Scope:** rival/ (Go: remove server) + app/ (new Swift package) + release pipeline + 1905/homebrew-tap + README
**Status:** done

## TL;DR

**P0 — Cask spike (decides the install route).**
- **What:** before any app code exists, prove that a throwaway ad-hoc-signed app installs from a *local-only* test tap with Homebrew 7.0.6 and opens without a Gatekeeper block.
- **Why:** Homebrew removed `--no-quarantine` on 2026-09-01. Whether a personal-tap cask may still strip quarantine is unverified.
- **You do:** nothing. If brew refuses, I stop and ask you: Developer ID ($99/yr), or `rival app install` (the Go binary downloads and installs the app itself, with no cask).
- **Not done:** nothing is published in P0.

**P1 — Remove `rival server`.**
- **What:** delete the `rival server` command and the web dashboard (`internal/server`, about 2,100 lines).
- **Why:** you are replacing it with a native app.
- **You do:** nothing.
- **Not done:** the TUI stays. `internal/sessionview` stays, because the TUI uses it.

**P2-P4 — Rival.app.**
- **What:** a SwiftUI macOS app (macOS 14+) that reads `~/.rival/sessions` directly.
- **Menu bar:** a live running/queued count, a mini list of live runs, and a notification when a run finishes.
- **Window:** the same features as the TUI, in the phosphor look with the gradient logo:
  - filter, status tabs and day sections;
  - real model ids;
  - detail with Output/Prompt/Info tabs, group members, live follow and search;
  - Stop, with a confirm and the PID/PIDStart check.
- **Why:** you asked for a native app.
- **You do:** nothing.
- **Not done:** no remote or multi-machine view, no editing of runs, no auto-update inside the app (brew handles updates).

**P5 — Distribution.**
- **What:** each release tag builds `Rival.app` on a macOS GitHub runner, ad-hoc signs it, attaches `Rival-app.zip` to the GitHub release and updates the cask `1905/tap/rival-app`. Install with `brew install --cask 1905/tap/rival-app`.
- **Why:** you asked for cask install.
- **You do:** nothing. The existing `HOMEBREW_TAP_TOKEN` secret is reused.
- **Not done:** no notarization. Strangers see an unsigned-developer app, which you accepted.

**P6 — README rewrite + docs cleanup (moved here from the prompt-core job).**
- **What:** `README.md` opens with a short TL;DR for humans (what, install CLI + app, five main commands, screenshots). A detailed reference for agents follows: every command, flag, default, JSON contract, config key and exit code, each checked against the code.
- **Docs:** stale `docs/` pages lose their Sol/megareview/server content.
- **Why:** the README still documents megareview, Sol and the server.
- **You do:** nothing.
- **Not done:** no docs website.

## Problem(s)

1. **The web dashboard is being replaced.** `rival server` (`rival/cmd/server.go`, `rival/internal/server/server.go` with 405 lines, `internal/server/templates/index.html` with 1,634 lines) serves a browser dashboard. You want a native app instead.
2. **There is no native way to watch runs.** You see a run finish only by keeping the TUI open or reading a Telegram notify.
3. **Casks now need Gatekeeper-clean apps.** Homebrew removed `--no-quarantine` and ended support for casks that fail Gatekeeper on 2026-09-01 (Homebrew/brew#20755). This machine has no Developer ID identity (`security find-identity -p codesigning` lists only "Image Studio Local Signing"), so notarization is not available.
4. **The README is stale.** It still documents megareview, Sol and the server, and it has no quick human-first overview.

## Goals

1. `rival server` and `internal/server` are gone. The TUI and `sessionview` are unchanged (Problem 1).
2. `Rival.app` gives TUI parity in a window, plus a menu bar live view and finish notifications (Problem 2).
3. `brew install --cask 1905/tap/rival-app` installs a launchable app, proven by the P0 spike (Problem 3).
4. The README has a human TL;DR first and an agent reference second, with every claim checked against the code (Problem 4).

## Non-goals

- Notarization or a Developer ID (you chose ad-hoc).
- A Windows or Linux app.
- The app starting reviews. It observes and stops runs only.
- A Go-side API or daemon for the app. The app reads the files directly.
- Changing the session JSON schema.

## Install route (P0 spike)

```
swift package (hello app) → bundle Hello.app → codesign -s - (ad-hoc)
→ zip → local tap `brew tap-new 1905/rivalspike --no-git` → cask file:
     url "file:///…/Hello.zip"
     postflight: xattr -dr com.apple.quarantine "#{appdir}/Hello.app"
→ brew install --cask 1905/rivalspike/hello
→ open -a Hello  → the process is running (pgrep), no Gatekeeper dialog (spctl -a -vv logged)
→ brew uninstall --cask hello; brew untap 1905/rivalspike   (cleanup)
```

**P0 result (run 2026-09-26, Homebrew 7.0.6, macOS 14.8.9 arm64): PASSED.**
- An ad-hoc-signed `Hello.app` installed from a local-only tap cask.
- With the `postflight` `xattr -dr`, only `com.apple.provenance` remains and the app launches (the process ran, with no syspolicy block logged).
- Without the `postflight`, brew sets `com.apple.quarantine`.
- `spctl -a` reports "rejected" either way. That is an assessment only, and it does not block the launch.
- Brew 7 printed a `brew trust` hint for non-official taps. The install worked without it. Whether other machines need `brew trust --tap 1905/tap` first is unverified; the README will note it.
- Spike artifacts are removed (cask uninstalled, tap untapped).

| spike result | route |
|---|---|
| installs and launches | P5 as written: the cask in `1905/homebrew-tap` with a `postflight` quarantine strip |
| brew refuses the cask or postflight | STOP and ask: Developer ID, or `rival app install` (the Go binary downloads the release zip, unzips it into `/Applications`, runs `xattr -dr`) |

## App design

**Project:** `app/` holds a Swift package (`Package.swift`). There is no .xcodeproj, so it builds with `swift build` and bundles through `app/scripts/bundle.py`.
- Targets: `RivalKit` (library: model, store, grouping, filter, process checks) and `RivalApp` (SwiftUI executable).
- Tests: `RivalKitTests` (XCTest via `swift test`).
- Minimum: macOS 14. Swift 6 language mode.

**Data model** mirrors the Go `session.Session` JSON (`rival/internal/session/session.go:39-72`). Decoding is tolerant: unknown keys are ignored and missing keys use defaults.

```swift
struct Session: Decodable, Identifiable {
  let id: String; let groupID: String?; let cli: String; let mode: String
  let model: String; let effort: String; let reviewScope: String?
  let prompt: String?; let promptPreview: String?; let status: String
  let startTime: Date; let queuedAt: Date?; let queuePosition: Int?
  let endTime: Date?; let exitCode: Int?; let duration: String?
  let workDir: String; let logFile: String; let outputBytes: Int64; let outputLines: Int
  let error: String?; let pid: Int32; let pidStart: Int64?
}
```

**Store:** `SessionStore` (`@Observable`, main actor).
- It watches `~/.rival/sessions` with `DispatchSource` / FSEvents.
- It re-decodes only files whose mtime or size changed (the same cache idea as `sessionview`).
- It groups by `groupID` with the same rules as `sessionview.Group`, `Status` and `Kind`, ported with tests that use fixtures copied from the Go tests.
- Refresh work is debounced to 250 ms.

**Window** (SwiftUI `NavigationSplitView`):
- **Left:** a searchable run list (`.searchable`, AND of terms like the TUI), status tabs with counts, day sections, and a row per run: glyph, kind, real model id, effort, time, project.
- **Right:** the detail header (breadcrumb, status, elapsed, a follow toggle), then Output / Prompt / Info tabs.
  - Output is a monospaced log with live tail. Follow stays on until you scroll up. ⌘F finds with next/prev.
  - Group runs get a member picker, with the judge last (history).
- **Stop button:** a confirm sheet, then SIGTERM only if the PID's start time (sysctl `KERN_PROC_PID` `p_starttime`) matches `pidStart`. Otherwise the run is shown as already dead. This is the same rule as the TUI fix `b497d86`.
- **Look:** dark only, phosphor palette (the same tokens as `internal/dashboard/styles.go`), SF Mono, and the ASCII logo with a violet→cyan→green gradient in the sidebar header.

**Menu bar** (`MenuBarExtra`, window style):
- The icon is a glyph plus a count while anything runs, and a plain glyph when idle.
- The popover lists running and queued runs with their elapsed time, plus the last 5 finished. It has an "Open Rival" button.
- Finish notifications (`UNUserNotificationCenter`) say "✓ review orbit-web · gpt-6-astra · 6m24s" or "✗ failed". Clicking one opens the run in the window.
- Notifications are on by default and can be turned off with a toggle stored in `UserDefaults`.

**Logs:** tail reads cap at 256 KB, like `logfmt.MaxTailBytes`. ANSI escapes are stripped and tabs expanded, the same rules as `logfmt`, with shared test vectors. "Open full log" uses `NSWorkspace` to open the file.

## Packaging & release (P5)

```
tag push → release.yml
  job goreleaser (ubuntu, unchanged)
  job app (macos-14):
    swift test (RivalKit)
    swift build -c release --arch arm64 --arch x86_64
    python3 app/scripts/bundle.py → Rival.app (Info.plist: CFBundleIdentifier dev.1905.rival, LSUIElement=false, version=tag)
    codesign --force --deep -s - Rival.app
    ditto -c -k --keepParent Rival.app Rival-app.zip ; shasum -a 256
    gh release upload $TAG Rival-app.zip
    update Casks/… in 1905/homebrew-tap: rival-app.rb (version, sha256, url), commit + push with HOMEBREW_TAP_TOKEN
```

Cask (`rival-app.rb` at the tap root, matching the current layout):

```ruby
cask "rival-app" do
  version "3.35.0"
  sha256 "…"
  url "https://github.com/1905/rival/releases/download/v#{version}/Rival-app.zip"
  name "Rival"
  desc "Menu bar + window dashboard for rival review runs"
  homepage "https://github.com/1905/rival"
  depends_on macos: ">= :sonoma"
  app "Rival.app"
  postflight do
    system_command "/usr/bin/xattr", args: ["-dr", "com.apple.quarantine", "#{appdir}/Rival.app"]
  end
end
```

The exact shape is fixed by the P0 spike result.

## File-level changes

| file | change |
|---|---|
| `rival/cmd/server.go`, `rival/internal/server/` | Removed (moved to /tmp/trash, deletion recorded by git). |
| `rival/cmd/*` referencing the server (root help, install hints) | Drop server mentions. |
| `rival/internal/sessionview` | Unchanged (the TUI uses it). Doc comments stop mentioning the web dashboard. |
| `app/Package.swift`, `app/Sources/RivalKit/*`, `app/Sources/RivalApp/*`, `app/Tests/RivalKitTests/*` | New Swift package as designed above. |
| `app/scripts/bundle.py` | Assembles `Rival.app` (Info.plist, binary, icon) and ad-hoc signs it. |
| `app/Resources/AppIcon.icns` | App icon: the gradient "r" glyph on black, generated once by a script (no paid image generation). |
| `.github/workflows/release.yml` | Add the `app` job (macos-14): test, build, bundle, sign, upload the zip, update the cask. |
| `1905/homebrew-tap/rival-app.rb` | New cask (P5 CI writes versions; the first version is committed by hand after the first tagged release). |
| `README.md`, `docs/*.md`, `CHANGELOG.md` | P6 rewrite and cleanup. The CHANGELOG `Unreleased` lists the server removal and the app. |

## Tests

**Go (P1):**
- `rival server` → unknown command.
- `go build ./...` has no `internal/server` references.
- The TUI and sessionview tests still pass.

**Swift (`swift test`, RivalKit):**
- Session decoding: every fixture shape (solo, group, consilium history, Sol history, missing optional keys, unknown keys).
- Grouping, status, kind and elapsed match the Go `sessionview` on shared fixtures.
- Filter: AND of terms, case-insensitive. Sections: today, yesterday, this week, older.
- Log sanitize: shared vectors (ANSI, tabs, wide runes).
- PID guard: a start-time mismatch means no signal. Tested through an injected `ProcessInspector` protocol; real signals are never sent in tests.
- Store: a temp dir with file writes, after the debounce, gives the expected snapshot.

**Manual** (orchestrator):
- `swift build` + bundle.
- Launch against a fixture HOME (`HOME=<fixture>` env for the app, supported by the store's root override).
- Screenshots of the window and the menu bar (`screencapture`).
- Stop confirm, then cancel.
- A notification appears when a fixture session flips to completed.
- The P0 spike and a P5 dry run: build the zip locally and install from a local tap, never pushing before the release.

## Failure modes & decisions

| failure | behaviour |
|---|---|
| The P0 spike fails (brew blocks the quarantine strip) | Stop, notify, and ask you: Developer ID, or `rival app install`. No app work is lost; only P5 changes. |
| `~/.rival/sessions` is missing | An empty state: "No runs yet. Run a review with rival." The store keeps watching the parent dir until the folder appears. |
| A corrupt or partial JSON file (a write in progress) | Skipped this refresh and retried on the next event. `.json.tmp` files are ignored. |
| A huge log | Tail cap of 256 KB plus "earlier output omitted — Open full log". |
| The PID is reused or the process is dead | Stop is refused and the run is marked "already dead", with no signal. |
| Notification permission is denied | The menu bar still works, and one inline hint appears in settings. |
| A non-arm64 Mac | A universal binary (arm64 + x86_64). |
| The ad-hoc signature is rejected on first launch | The cask `postflight` strip (P0 proves it). The README documents `xattr -dr com.apple.quarantine /Applications/Rival.app` as a manual fix. |

## Out of scope

- Notarization, Developer ID, the App Store.
- Starting reviews from the app, and editing or deleting sessions.
- Sparkle or in-app updates.
- Reviving the web dashboard or any HTTP API.

## Rollout

- **P0** — cask spike, local only. The result is recorded in the spec. There is no commit unless the route changes.
- **P1** — remove `rival server` and `internal/server`. Commit.
- **P2** — `app/`: RivalKit (model, store, grouping, filter, log sanitize, PID guard) plus tests. Commit.
- **P3** — window UI (list, detail tabs, follow, search, stop confirm, theme, logo). Commit.
- **P4** — menu bar extra plus finish notifications plus app icon. Commit.
- **P5** — `bundle.py`, the release.yml `app` job, the `rival-app` cask, and a local install dry run. Commit.
- **P6** — README rewrite (human TL;DR, then agent reference), docs cleanup, CHANGELOG. Commit.

## As-built notes

Shipped 2026-09-26 on `feat/rival-mac-app`. The commits:
- P1 43a5851
- P2 f6bdd7b
- P3 1d96c38
- P4 831d467
- P5 d6ca69c
- make targets c5da153
- P5b pagination 0e98a72
- P6 docs 4951776
- review fixes f2a42ab
- loader 4c6fdb9
- simplify 7eb6b4e

Drift from the text above:

- **Pagination** (plan v1.1, user request) runs in both UIs:
  - 50 runs per page, footer `‹ prev  page P/N  next ›  · T runs` ("1 run" singular);
  - TUI keys `n`/`p` and PgDn/PgUp; app keys ←/→, `[`/`]` and clickable arrows;
  - the menu bar LIVE list caps at 10.
- **Startup loader** (user request): a gradient "reading sessions" bar with done/total in both UIs, and `…` counts until the first snapshot.
  - The app reads session summaries like Go's `LoadSummaryFile`: about 0.16-0.3 s for 3000 sessions, from 0.4-1.5 s.
  - `rival tui` reaps orphans in the background.
- **`make run` / `make install` / `make test`** in the repo root. `make install` builds a native release bundle into `/Applications`, moving the old copy to /tmp/trash.
- **Stop safety** (branch review):
  - Only a process whose recorded, non-zero start time matches is signalled, in the app and the TUI. A run without one is never signalled or rewritten.
  - The app never writes signalled runs; their rival process finalizes them. It marks a dead run only if the owner process is gone too, and never overwrites a run that already finished.
- **Session writes:** `session.Save` and the app write through unique temp files (`<id>.json.tmp-*`), so the CLI, TUI and app cannot clobber each other.
- **Release job:**
  - It runs on `macos-15` (Xcode 16 / Swift 6 by default) and publishes the cask to `Casks/rival-app.rb` in `1905/homebrew-tap`.
  - Not yet exercised on GitHub. The first real run is the next release tag.
- **Server removal:** `sessionview.EngineLabels`/`JoinLabels` were removed with the server. Old megareview/Sol sessions still render as history.
- **README:** P6 was moved here from the prompt-core job. Screenshots are taken from a fixture without legacy sessions.
- **Help text:** `plan --effort` no longer advertises a default it ignores.
- **Not verified:**
  - the menu bar popover by eye;
  - the notification banner (needs the user to click Allow);
  - whether other machines need `brew trust --tap 1905/tap`.
