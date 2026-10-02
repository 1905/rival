# Rust rewrite: CLI + TUI only

**Date:** 2026-10-01
**Scope:** /Users/kass/dev/rival — new Rust workspace at the repo root replaces `rival/` (Go). `app/` (Swift) is not touched.
**Status:** approved (P4 Result tab added 2026-10-01 on user request)

## TL;DR

**Whole effort.**
- What: rewrite the `rival` CLI and `rival tui` in Rust, built next to Go, switched in one release when every command passes its scenarios (Rust only, expectations written from the Go source).
- Why: one language for the worker side, a cleaner base for later; the CLI is ~65% of the code and all the active work (reviews, queue, sessions).
- You decide: approve this spec. Nothing else until the P6 switch release.
- Does NOT: touch the Swift app (it keeps its own copied logic, drift accepted), change the session format, CLI flags, output, or the brew formula name.

**P1 — core foundations.** Workspace, paths, session read/write, config, log sanitizing, process info, JSON logging identical to Go's. Contract tests: Go writes → Rust reads, Rust writes → Go and Swift read.
**P2 — execution.** Reviewer executors (codex, claude, grok, kimi/opencode), subprocess supervision, the cross-process queue, detach, `wait`, review prompts/parsing/formatting, git scope, MR reviews.
**P3 — commands.** All commands and flags via clap, embedded skills + `install`, `update`, `version`, `sessions`, telemetry. Scenario runner green on every command.
**P4 — TUI in ratatui + Result view.** Same screens, keys, pages and palette as the Go TUI, plus the app's parsed view on the detail page: a Result tab (header with model/effort/rating/severity counts/summary, findings grouped by severity, rendered markdown, parse error with a hint to Raw) that opens by default for finished runs; Raw (the old Output) opens for live runs. Tabs: Result · Raw · Prompt · Info.
**P5 — Windows.** Detach, locks, stop and process info get Windows versions; CI builds and tests on Windows.
**P6 — switch.** Release pipeline builds Rust for darwin/linux/windows × amd64/arm64; brew formula points to the Rust binary; Go code removed.

⚠ Effort guess (not measured): P2 is the biggest and riskiest (~40%), then P4 (~20%), P3 (~15%), P1 (~10%), P5 (~10%), P6 (~5%).

## Problem(s)

1. **Two languages, logic copied by hand.** Go has the CLI/TUI (`rival/`, ~11.7k source + ~10.8k test lines); the Swift app re-implements parts of it. This spec removes Go; the Swift copy stays by user decision.
2. **No Windows build.** The CLI ships for darwin/linux only (goreleaser, 4 tarballs). It uses Unix-only mechanisms: `Setpgid`/`setsid` detach (`rival/cmd/detach_unix.go`), `flock` for the queue (`rival/internal/queue/queue.go:2`), SIGTERM stop (`rival/internal/dashboard/kill.go`), process start time per OS (`rival/internal/procinfo/start_{darwin,linux,other}.go`).
3. **Behaviour must not change.** Other tools depend on exact output: skills read `rival` stdout, `rival wait --log` parses the CLI's zerolog JSON stderr lines (`rival/cmd/wait.go`), the Swift app and TUI read session JSON files.

## Goals

1. (P1) Rust reads and writes session files byte-compatible in meaning with Go (same keys, same RFC3339 times, atomic tmp + rename).
2. (P1) stderr log lines have the same JSON fields as Go zerolog: `level`, `app`, `session`, `pid`, `status`, `time`, `message`, plus per-call fields.
3. (P2-P3) For every command and flag, Rust produces the stdout, stderr lines, log events, exit code, session files and queue tickets that the Go source defines (ported Go tests + scenario runner; no Go binary runs).
4. (P4) TUI parity: lists, tabs, filter, pages, detail, follow, kill confirm, palette — plus a Result tab that renders the parsed answer like Rival.app (finished runs open on Result, live runs on Raw).
5. (P5) The same binary works on Windows: detach, queue lock, stop, process start time, tests green on `windows-latest`.
6. (P6) One release ships Rust for 6 targets; `brew install 1905/tap/rival` installs it; `rival install` skills still work.

## Non-goals

- New features or flag changes, except the TUI Result tab (user request, 2026-10-01).
- Touching `app/` (Swift) or the cask.
- A Rust GUI.
- Changing the reviewer CLIs' invocation (same args to codex/claude/grok/opencode/glab).

## Architecture

```
Cargo.toml                 workspace
crates/
  rival-core/              lib
    paths, sessions (model, atomic save, reaper), config (yaml + env), logfmt,
    procinfo (start time per OS), queue (ticket files + file lock), executor
    (codex/claude/claude-docker/grok/kimi-opencode, subprocess + quota), review
    (prompts, parse, format, plan/antislop/security runs), gitscope, mergerequest (glab),
    skills (embedded via include_dir), telemetry (sentry), update check, logging (zerolog-compatible JSON),
    result (Rival.app's answer parser: double-answer dedupe, no-header fix, severity groups; used by the TUI Result tab)
  rival/                   bin `rival`: clap commands (command/run/queue/install/tui/sessions/update/version/wait)
    tui/                   ratatui + crossterm
parity/                    scenario runner: runs the Rust binary with fake CLIs, checks expected output
testdata/                  shared session/log fixtures (fake data)
rival/                     Go, unchanged until P6
```

### Scenario runner

```
scenario.yaml ─► fake CLIs on PATH (codex, claude, grok, opencode, glab, docker, brew: canned output, exit codes, delays)
             ─► local HTTP server for the update endpoint (debug-only override), no external network
             ─► run `rival <args>` (Rust, debug build) in a temp HOME
             ─► normalise (session ids, pids, times, temp paths)
             ─► check against expect: stdout golden, plain stderr lines, JSON log events, exit code,
                ~/.rival/sessions/*.json fields, *.log, queue tickets
```

Expectations are written from the Go source and Go tests (messages, formats, codes), not by running Go. Scenarios cover every command and flag, success and failure, timeouts, quota errors, detach + wait (arbitrary redirect files, unlinked input), two concurrent runs through the queue, MR review with a fake `glab`, `update` with a fake `brew`.

### Windows mapping

| Unix (Go today) | Windows (Rust) |
|---|---|
| detach: re-exec with `setsid`, inherits the caller's stdin/stdout/stderr | Console-aware flags per plan v2.3: `CREATE_NEW_PROCESS_GROUP`, adding `DETACHED_PROCESS` only when no stream needs the inherited console; preserve each file/pipe handle |
| queue `flock` | `LockFileEx` (via the `fd-lock` crate on both) |
| provider in its own process group; timeout SIGKILLs the group | provider in a Job Object with kill-on-close owned by the running `rival`; timeout = `TerminateJobObject` |
| stop from the TUI: signal the process group | console events can't reach a detached process: `TerminateProcess` on the owner `rival` pid (start-time checked) → its Job closes → whole tree dies → session reaped to `failed`, queue slot freed |
| process start time (`proc_pidinfo` / `/proc/<pid>/stat`) | `GetProcessTimes` |
| `~/.rival` | `%USERPROFILE%\.rival` (same layout) |

## File-level changes

| Path | Change |
|---|---|
| `Cargo.toml`, `crates/rival-core/**`, `crates/rival/**` (new) | The Rust implementation; module per Go package, tests ported from the Go `_test.go` files (~10.8k lines). |
| `parity/**` (new) | Scenario files, fake CLI scripts (incl. `brew`), local update-endpoint server, Python runner (`parity/run.py`), normaliser, mismatch report. |
| `testdata/**` (new) | Shared fake fixtures; Go contract test in `rival/internal/session` + Rust contract test; a Swift decoding test reads Rust-written sessions. |
| `.github/workflows/ci.yml` (new or extended) | `cargo test` on macOS, Linux, Windows; scenario runner on macOS + Linux; Swift decode test of Rust-written sessions on macOS (required from P1). |
| `.github/workflows/release.yml` | P6: goreleaser Go builds → Rust builds for 6 targets (goreleaser rust builder or cargo-dist, chosen in the plan), same formula update. |
| `Makefile`, `README.md`, `CHANGELOG.md` | P6: build/test commands, Windows install note. |
| `rival/**` | P6: moved out (mv to /tmp/trash, `git add -A`). |

## Tests

- **Unit:** every Go test file ported (table-driven Rust tests).
- **Contract:** Go ↔ Rust session files both directions; Swift app decodes Rust-written fixtures.
- **Scenario runner:** every command/flag scenario green before P6.
- **Release build (P6):** unpublished snapshot build of all 6 targets; names, checksums, `rival version`, formula diff checked before merge.
- **Cross-process:** two `rival` processes through the queue (ported `crossproc_test.go`) on all three OSes.
- **TUI:** model/update tests ported from `internal/dashboard/*_test.go` (3.1k lines); golden-frame snapshots with `ratatui`'s test backend.
- **Windows:** detach + wait, queue lock, stop on a real child process in CI.
- **Manual (orchestrator):** real codex/claude review on this repo with the Rust binary; output reads right and the session opens in the Swift app.

## Failure modes & decisions

| Failure | Behaviour |
|---|---|
| A Go behaviour is a bug | CLI: port it as-is, list it in the plan's "known Go bugs" table, fix after P6. Bugs fixed in Go before the port (2026-10-01) are ported fixed. |
| Output varies run to run (ids, times, map ordering) | Normaliser rule with a comment; never silently ignored. |
| Windows reviewer CLI missing or behaves differently | Out of our control; executor returns the same "not installed" error as on Unix. |
| Go and Rust write the same `~/.rival` during development | Dev and tests always use a temp HOME; the Rust binary is not on PATH until P6. |
| Rust log JSON differs from zerolog | `wait` scenario fails; fix before P6. |
| Swift app can't read a Rust-written session | Swift contract test fails in CI; fix the Rust writer. |

## Out of scope

- Swift app changes (copied logic stays).
- Notarization or Windows code signing (unsigned binaries; note in README).
- New commands or flag changes.

## As-built notes — implementation in progress, 2026-10-02

- `.env` uses a direct godotenv v1.5.1 parser port. dotenvy changed duplicate-key and expansion behavior.
- `gostd.rs` shares Go duration, quoting, case-folding and error-text semantics. Its Unicode 15.0.0 tables come from the release toolchain, Go 1.25.14, with the BSD license retained in `licenses/Go-LICENSE`.
- macOS process identity uses `sysctl kern.proc.pid`, matching Go. `proc_pidinfo` cannot inspect some processes Go can inspect.
- YAML uses serde-saphyr 1.3.0. Compatibility limit: Rival validation messages and the `parse <path>:` prefix match Go; malformed-YAML parser details use Rust's wording.
- P3 is merged at `bb95462`. All 84 scenarios pass on macOS/Linux; two real Rust-driven reviews rendered in the unchanged Swift app.
- P5 implementation correction, not yet verified: an owner cleanup Job protects provider creation before nested per-provider Job assignment. Console-aware detach flags preserve the approved stream behavior. This replaces the original flag recipe and closes the documented suspended-child orphan interval; plan v2.3 carries native regression requirements.

## Rollout

- **P1** core foundations + contract tests. One branch, merged the same day it is green.
- **P2** execution (executors, queue, detach, wait, review, MR). May split into P2a (queue/detach/wait) and P2b (executors/review) branches.
- **P3** commands + scenario runner green.
- **P4** ratatui TUI.
- **P5** Windows.
- **P6** release switch, Go removed; Codex review + /simplify on the final diff; release is your call.
- Rust binary is built and tested in CI from P1 but not released until P6.
