# Rust rewrite: shared core, tray process, on-demand window

**Date:** 2026-10-01
**Scope:** /Users/kass/dev/rival (new Rust workspace at the repo root; replaces `app/` Swift in P1, `rival/` Go in later phases)
**Status:** approved

## TL;DR

**Whole effort (P1 detailed here; P2-P5 get their own short specs).**
- What: one Rust workspace. `rival-core` holds all logic. The CLI, the TUI, the tray and the window are thin layers on it.
- Why: the Go CLI and the Swift app share zero code; ~1,600 Swift lines are hand copies of Go and they already drifted twice.
- You decide: approve P1. Later phases: P2 CLI port, P3 ratatui TUI, P4 Linux/Windows packaging, P5 delete Go.
- Does NOT: change what rival does, the session file format, or install names (`rival`, cask `rival-app`).

**P1 — `rival-core` + two-process app, replaces the Swift app.**
- What: an always-on **tray process** (`tray-icon` + native menu + `rival-core`, no web engine) and a **window process** (Tauri, custom UI in plain HTML/CSS/TypeScript, no UI framework, look taken from the Swift app) that the tray starts on "Open Rival" and that exits fully when you close the window.
- Why: lowest RAM with a rich custom UI. Measured on 6000 runs: tray alone 22 MB; one-process Tauri was 127-135 MB open and stayed at 55 MB after close because WebKit helpers never exit.
- You decide: nothing else. Gate: if idle after a close is above 30 MB, or the window is above 150 MB open, work stops and you get the numbers.
- Does NOT: port the CLI or TUI, ship Linux/Windows builds, notarize, or use Mantine/React.

⚠ Opening the window starts a fresh process plus WebKit. Guess: 0.3-1 s until it shows, not measured. P1a measures it.

## Problem(s)

1. **No shared code, real drift.** The Swift app re-implements Go logic: `app/Sources/RivalKit/ResultParser.swift` ports `rival/internal/review/parse.go`, `LogReader.swift` ports `rival/internal/logfmt`, `Grouping.swift` ports `rival/internal/sessionview`. They drifted: Swift handles codex's double-printed answer, Go's `FinalAnswer` (`rival/internal/review/parse.go:165`) does not; the "no answer header → whole log" fix exists in Swift only.
2. **Two toolchains, two release jobs.** goreleaser for Go, `app/scripts/bundle.py` for Swift (`.github/workflows/release.yml` jobs `release` and `app`).
3. **A web UI costs RAM even when nobody looks at it.** Measured 2026-10-01 on 6000 fake runs: a one-process Tauri prototype used 22 MB as tray only, 127-135 MB with the window open (WebKit renderer 58, GPU 33-40, network 5), and 55 MB 40 s after closing it, because WebKit's GPU and network processes outlive the window.

## Goals

1. (P1) `rival-core` is the only place with logic; tray, window and later CLI/TUI call it.
2. (P1) Idle RAM (tray only, before and after using the window) ≤ 30 MB on 6000 runs.
3. (P1) Window process ≤ 150 MB while open; 0 MB after close (process gone).
4. (P1) Feature parity with Swift app 4.1.x: tray live count + live/recent runs + notify toggle + Stop; window list/filter/tabs/pages, Result/Raw/Prompt/Info, finish notifications.
5. (P1) Same session contract tested from Go and Rust while both exist.
6. (P1) Same install: cask `rival-app`, DMG on releases, `/Applications/Rival.app`, bundle id `dev.1905.rival`.
7. (all) No macOS-only API in shared or UI code, so P4 is packaging only.

## Non-goals

- Mantine, React or any UI component framework.
- A theme system or light mode.
- CLI/TUI port in P1.
- Notarization.

## Architecture

```
Cargo.toml                       workspace
crates/
  rival-core/                    lib, all logic
    sessions  serde model, slim summary, store + rescan, finish detection
    view      grouping, counts, filter, day sections, pagination
    logs      tail read, ANSI/CR sanitize, tab expand
    result    final answer extraction, JSON payload scan, markdown → safe HTML (pulldown-cmark), severity order
    stop      pid + start-time guard, SIGTERM, stop marks
    paths     RIVAL_HOME / ~/.rival
    watchdog  own-process footprint, 1 GiB quit
  rival-tray/                    bin: tray-icon + muda menu + notifications; spawns the window
desktop/                         Tauri app = the window process
  src-tauri/                     commands that call rival-core; no logic
  ui/                            plain HTML + CSS + TypeScript (Vite), no framework
testdata/                        shared fixtures (fake data) for Go and Rust tests
```

```
            ~/.rival/sessions  (written by the Go CLI today)
                 │ read
   ┌─────────────┴─────────────┐
rival-tray (always on, ~22 MB)   rival window (only while open)
 tray-icon + native menu          Tauri + WebKit, custom HTML/CSS
 rival-core store                 rival-core store (own instance)
 notifications                    exits on window close → 0 MB
   │ "Open Rival" / "Open run X"
   └──► spawns `Rival.app/.../rival-window --select <run>`
        (if already running: forwards the run id over a local socket and the window focuses)
```

Why two processes and not one: closing a webview does not stop WebKit's GPU and network helpers (measured, 55 MB after close). Ending the whole window process does.

Single instance: the tray holds a lock file in `~/.rival/app/`; a second tray launch exits. The window process also takes a lock; a second "Open" sends `select <run>` on a Unix socket (named pipe on Windows) instead of starting another window.

### Window boundary (Tauri commands, JSON)

```json
// list_runs({ "status": "all", "filter": "plan astra", "page": 1, "focus": null })
{ "runs": [ { "id": "solo:a1b2…", "status": "completed", "kind": "plan", "model": "gpt-6-astra",
              "effort": "xhigh", "elapsed": "6m24s", "project": "acme-api", "section": "Today", "members": 1 } ],
  "page": 1, "pages": 121, "counts": { "all": 6008, "live": 2, "failed": 858, "done": 5148 },
  "loading": false }
// run_detail({ "run_id": "group:8a13…" })          → members + info rows
// run_result({ "session_id": "…" })                → { "kind": "findings", "summary", "rating", "groups": [...] }
//                                                    | { "kind": "markdown", "html": "<sanitized>" }
//                                                    | { "kind": "failed", "reason" } | { "kind": "live" }
// log_tail / prompt / stop_preview / stop_run
```

Events: `runs-changed`, `select-run`.

### UI

Custom design taken from the Swift app (palette C, gradient ASCII logo, monospace), plain CSS with variables. Layout and states as in prototype A (`scratchpad/gui/proto-a`, throwaway): left header (logo, stats, stacked done/failed bar, status tabs with counts, filter), day sections, rows, pagination footer; right breadcrumb, tabs Result/Raw/Prompt/Info, finding cards with severity bar and collapsible details, markdown, Raw follow, Info table, Stop modal. Markdown HTML comes from `rival-core` already sanitized; the UI never renders raw log HTML.

## File-level changes

| Path | Change |
|---|---|
| `Cargo.toml` (new) | Workspace: `crates/rival-core`, `crates/rival-tray`, `desktop/src-tauri`. |
| `crates/rival-core/**` (new) | Modules above, ported from `app/Sources/RivalKit/*.swift` (newest logic), checked against Go. Tests ported from `app/Tests/RivalKitTests` (187 cases). |
| `crates/rival-tray/**` (new) | Tray icon with title = live count, native menu (Open Rival, 10 live, 5 recent, Notify on finish, Quit), finish notifications (`notify-rust`), spawns/forwards to the window, lock file, watchdog. |
| `desktop/src-tauri/**` (new) | Window-only Tauri app; exits when its window closes; socket listener for `select`; commands above. |
| `desktop/ui/**` (new) | HTML/CSS/TS from prototype A, cleaned up; vitest for any non-trivial TS helper. |
| `testdata/**` (moved) | Fixtures from `app/Tests/Fixtures`; a Go test in `rival/internal/session` decodes them too. |
| `scripts/soak_test.py`, `scripts/dev_bundle.py` | Moved from `app/scripts`; soak covers both processes and the open → close cycle. |
| `scripts/bundle.py`, `render_cask.py` | Build one `Rival.app` containing both binaries (tray is the main executable, window a helper), ad-hoc sign, zip for cask, DMG. |
| `.github/workflows/release.yml` | `app` job: Rust + Node + Tauri instead of Swift. |
| `Makefile` | `run/install/test/soak` target the new app. |
| `app/**` | Swift app moved out once P1 reaches parity. |
| `README.md`, `CHANGELOG.md`, `assets/app.png` | New app, new screenshot from fixtures. |

## Tests

- **rival-core unit:** all Swift test cases ported (parser, grouping, filter, pagination, logs, stop guard, finish detector, summary, store), plus markdown sanitizing (no `<script>`, no `javascript:` links).
- **Contract:** Go and Rust decode every `testdata/` session with equal fields.
- **Tray:** menu model built from a fixture store (pure function, unit-tested); spawn/forward logic with a fake launcher.
- **Window commands:** one test per command against a fixture `RIVAL_HOME`.
- **Soak (gate, orchestrator):** 6000-run fixture; samples tray idle, window open (app + WebKit processes), 10 s and 40 s after close; fails above the Goals limits or on steady growth; watchdog proof.
- **Manual (orchestrator):** screenshots of every state on fake data; open-latency measured.

## Failure modes & decisions

| Failure | Behaviour |
|---|---|
| Idle after close > 30 MB, or window > 150 MB open | Stop after P1a, report numbers, you decide. |
| Window open latency > 1.5 s | Report; option: keep the window process alive for N minutes after close (costs RAM) — your call. |
| Tray and window disagree briefly (each has its own store) | Accepted; both rescan every 2 s on dir change. |
| Window process crashes | Tray unaffected; next "Open" starts a new one. |
| Tray crashes | Window keeps working; relaunching Rival.app starts the tray again (lock is released with the process). |
| Second "Open Rival" while open | Forwarded over the socket; the window focuses. |
| Log content contains HTML/JS | Rendered only as escaped text or core-sanitized markdown HTML. |
| Linux tray cannot show text | P4 draws the count into the icon image. |

## Out of scope

- P2 CLI, P3 TUI, P4 Linux/Windows packaging, P5 Go removal (own specs).
- Notarization, auto-update.
- Light theme.

## Rollout

- **P1a** — `rival-core` read side + ported tests + Go/Rust contract test; tray process + window process skeleton; RAM and open-latency gate measured on 6000 runs. One commit; stop if the gate fails.
- **P1b** — window UI from prototype A, all tabs, cleaned up and on `rival-core` commands. One commit.
- **P1c** — tray menu complete, notifications, Stop, watchdog, soak test. One commit.
- **P1d** — bundle/DMG/cask pipeline, README + screenshot, Swift app removed, release; then Codex review + /simplify on the whole branch.
