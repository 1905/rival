# Rust CLI + TUI — Implementation Plan v1.1

**Date:** 2026-10-01
**Status:** superseded by v2.0
**Changes from v1.0:** TUI detail page gets the Rival.app parsed view (Result tab, default for finished runs) — new Tasks 4.6-4.8, Gate P4 extended, file map and type check updated.
**Spec:** ./spec.md (approved) · CLI reference: ./cli-surface.md (Go `--help` for all 21 commands)

**Goal:** a Rust `rival` (CLI + TUI) that matches the Go binary on every command, flag, output, exit code and file it writes, builds for darwin/linux/windows × amd64/arm64, and replaces Go in one release.
**Architecture:** Cargo workspace at the repo root: `crates/rival-core` (library, one module per Go package) and `crates/rival` (binary: clap commands + `tui` module). A parity harness runs the Go and Rust binaries on the same scenarios with fake reviewer CLIs and diffs everything they produce. Go stays the shipped binary until P6.
**Tech Stack:** Rust 1.98 · clap 4 (derive) · serde + serde_json · a maintained serde YAML crate (Task 1.4 picks it) · chrono · uuid · dotenvy · fd-lock · include_dir · sentry · ureq (update check) · notify (fs watch) · ratatui + crossterm · windows-sys (P5) · Python 3 for the harness.

> For agentic workers: use superpowers:subagent-driven-development to implement task-by-task. Checkbox syntax for tracking.

**Tags:** `heavy` = Opus 5.5 implementer · `light` = small Opus task · `gate` = orchestrator only (runs the harness, real reviews, CI, commits, merges).

**Implementer restriction (paste verbatim in every dispatch):** "You write the code and unit tests your task names and run the focused tests for that task (`cargo test -p <crate> <filter>`, `cargo build`, `go test ./internal/<pkg>/ -run <Name>` for the Go contract tests). You NEVER run the parity harness, real reviewer CLIs (codex, claude, grok, opencode, glab, docker), network calls, e2e / integration / live / smoke tests; never publish, deploy, push, touch infra, or run anything money-bearing. You never touch the real ~/.rival: every test uses a temp HOME/RIVAL_HOME. Do not commit; the orchestrator commits. Do not set any test-DB env var. Never delete files with rm; move them to /tmp/trash/<name>.<timestamp>. Port Go behaviour exactly, including bugs; list any Go bug you notice in your report instead of fixing it."

**Branches:** one per phase from `master` (`feature/rust-p1-core`, …), merged the same day its gate is green (merge hygiene). The Rust binary is built and tested in CI from P1 but not released or put on PATH until P6. Exec starts from a clean `master`.

## Contracts every task must keep

| Contract | Rule | Source |
|---|---|---|
| Session file | Same keys, same order, same `omitempty` behaviour as `session.Session` (e.g. `exit_code` 0 is omitted). RFC3339 times with Go's formatting (nanoseconds trimmed the way Go trims them). Atomic save: `<id>.json.tmp-*` then rename, mode 0600. | `rival/internal/session/session.go:152-190` |
| Queue ticket | Same keys/order as `queue.Ticket`; ticket files in `~/.rival/queue/`; all state changes inside one file-lock critical section. | `rival/internal/queue/{ticket,queue}.go` |
| stderr log | One JSON object per line: `level`, `app:"rival"`, per-call fields, `time` (RFC3339, seconds), `message`. Same field names as zerolog. | `rival/main.go:20` |
| `.env` | Loaded silently from the working directory at start (godotenv semantics). | `rival/main.go:17` |
| `wait` | Same exit codes 0 / 2 / 3 / 4 and the same stdout lines. | `rival/cmd/wait.go:21` |
| Flags | Same names, short forms, defaults and validation errors as `cli-surface.md`. `--help` text may differ (clap vs cobra); the harness does not diff help text. | `./cli-surface.md` |
| Paths | `~/.rival` (Windows: `%USERPROFILE%\.rival`), `config.yaml`, `sessions/`, `queue/`; same file names. | `rival/internal/config`, `session`, `queue` |
| Skills | Same embedded skill files, same install targets and `--force` behaviour. | `rival/internal/skills`, `rival/cmd/install.go` |

## File map

**Create**
- `Cargo.toml`, `rust-toolchain.toml`, `Cargo.lock`
- `crates/rival-core/src/{lib,paths,logging,session,summary,reaper,config,logfmt,procinfo,queue,ticket,gitscope,mergerequest,parser,telemetry,update}.rs`
- `crates/rival-core/src/executor/{mod,subprocess,quota,codex,claude,claude_docker,grok,kimi,opencode}.rs`
- `crates/rival-core/src/review/{mod,types,prompt,parse,plan,planrun,review_format,security,slots}.rs`
- `crates/rival-core/src/skills/{mod,codex}.rs` + `crates/rival-core/skills/` (copied skill trees)
- `crates/rival-core/src/sessionview/{cache,group}.rs`
- `crates/rival/src/{main,root,detach,command,model_command,model_run,model_specs,run,queue_cmd,install,sessions,tui_cmd,update_cmd,version,wait,merge_request,gitscope_helper}.rs`
- `crates/rival/src/tui/{mod,model,session_list,detail_view,preview,logview,keys,kill,layout,styles,watcher,result_view,markdown}.rs`
- `crates/rival-core/src/result.rs` (Rival.app answer parser, used by the TUI only until after P6)
- `parity/{run.py,normalise.py,scenarios/*.yaml,fakes/*}` · `testdata/{sessions,logs,expected.json}`
- `rival/internal/session/testdata_contract_test.go`
- `.github/workflows/ci.yml`

**Modify:** `.gitignore` (`target/`), `.github/workflows/release.yml` (P6), `Makefile` (P6), `README.md`, `CHANGELOG.md` (P6)
**Remove (P6):** `rival/` (moved to /tmp/trash, `git add -A`)
**Out of scope:** `app/` (Swift), the cask, `scripts/` of the app.

## Self-test sanity check (before P1)

- [ ] Clean `master`; `cargo --version` 1.98.x; `go version` 1.27.x
- [ ] `cd rival && go test ./... && golangci-lint`-equivalent green (lint via `go run github.com/golangci/golangci-lint/v2/cmd/golangci-lint@v2.14.0 run ./...`, the local binary is too old)
- [ ] `/tmp/rival-go-ref` built from `master` (reference binary for the harness)

---

## P1 — foundations + harness (`feature/rust-p1-core`)

### Task 1.1 — workspace `light`
- [ ] `Cargo.toml` workspace (`crates/rival-core`, `crates/rival`), `rust-toolchain.toml` 1.98, release profile lto/codegen-units 1/strip; binary name `rival`. `.gitignore` `target/`.
- [ ] `cargo build` → 0 warnings; `cargo run -p rival -- version` prints a placeholder.

### Task 1.2 — paths + logging `heavy`
Port: `main.go` (logger, .env), path helpers from `config`/`session`. Rust: `paths.rs`, `logging.rs`.
- [ ] `logging::init()` → global JSON-lines writer to stderr with zerolog field names/order; `logging::info().str(k,v).int(k,v).msg(m)`-style builder (no `tracing`); levels debug/info/warn/error.
- [ ] Test: a line produced by Rust and the same call in Go (golden string in the test) parse to equal maps.
- [ ] `dotenvy` load from cwd, silent on missing.

### Task 1.3 — session model, save, summary, reaper `heavy`
Port: `internal/session/{session,summary,reaper}.go` + tests `{session,save,summary,reaper}_test.go`.
- [ ] `Session` struct field order = Go; serde attrs reproduce `omitempty` (skip zero/empty) exactly; custom time serialiser matching Go `time.Time` JSON.
- [ ] Atomic save, load, list, summary (no prompt), reaper (orphan → failed) with `procinfo`.
- [ ] All Go cases ported → `cargo test -p rival-core session summary reaper` green.

### Task 1.4 — config `heavy`
Port: `internal/config/config.go` (1,069 lines) + `{config,codex,kimi,security,antislop}_test.go`.
- [ ] Pick the YAML crate: a serde-compatible, maintained crate (release within the last 12 months on crates.io; not the archived `serde_yaml`). Record the choice and its version in the report.
- [ ] Same defaults, env overrides, `UserConfigError` messages, model labels, public error/log scrubbing (`PublicRuntimeError`, `PublicRuntimeLog`, `replaceConcreteModelIDs`), `KimiAPIKeyFrom`.
- [ ] Ported tests green.

### Task 1.5 — logfmt + procinfo `light`
Port: `internal/logfmt`, `internal/procinfo` (darwin `proc_pidinfo`, linux `/proc/<pid>/stat`, other → None) + tests.
- [ ] Green on macOS; Linux path covered by `#[cfg]` tests that run in CI.

### Task 1.6 — testdata + contract tests `heavy`
- [ ] `testdata/sessions/*.json` + `logs/` = the fake fixtures in `app/Tests/Fixtures` (copy); `expected.json` lists decoded fields per file.
- [ ] Go test `rival/internal/session/testdata_contract_test.go`: decode all → equals `expected.json`; plus "Go writes a session from expected values; bytes equal the Rust writer's output for the same input" (Rust golden files under `testdata/written/`).
- [ ] Rust `crates/rival-core/tests/contract.rs`: same in reverse.
- [ ] `go test ./internal/session/ -run Contract` + `cargo test -p rival-core --test contract` green.

### Task 1.7 — parity harness `heavy`
- [ ] `parity/run.py --go <bin> --rust <bin> [--scenario <glob>]`: per scenario, two temp HOMEs, PATH = `parity/fakes` first, run both, collect stdout, stderr JSON lines (parsed), exit code, `~/.rival/**` files; `normalise.py` replaces UUIDs, pids, times, temp paths, durations with stable tokens; prints a unified diff per artefact; exit 1 on any diff.
- [ ] `parity/fakes/`: Python scripts named `codex`, `claude`, `grok`, `opencode`, `glab`, `docker`, `git` passthrough not faked. Behaviour driven by env `FAKE_<NAME>_SCRIPT=<file>` (canned stdout/stderr, exit code, delay, quota error text).
- [ ] Scenario format (YAML): `args`, `cwd_fixture` (a temp git repo built from `parity/repos/<name>`), `fake_scripts`, `expect_exit` (optional), `concurrent` (list of arg sets started together).
- [ ] Self-test: Go vs Go → 0 diffs on 3 smoke scenarios (`version`, `sessions` on empty home, `queue`).

### Task 1.8 — CI `light`
- [ ] `.github/workflows/ci.yml`: `cargo test --workspace` + `cargo clippy -D warnings` on macos-15 and ubuntu-latest; Go tests unchanged.

### Gate P1 `gate`
- [ ] Orchestrator: workspace tests green, Go contract green, harness self-test green, CI green after push of the branch. Merge to `master`.

---

## P2 — execution (`feature/rust-p2a-queue`, then `feature/rust-p2b-review`)

### Task 2.1 — queue + tickets `heavy`
Port: `internal/queue/{queue,ticket}.go` + `{queue,crossproc}_test.go`.
- [ ] fd-lock on the queue lock file; `WaitForSlot`, position callbacks, `ReapDead`, clear; ordering identical.
- [ ] Cross-process test spawns the test binary twice (Rust equivalent of `crossproc_test.go`).

### Task 2.2 — detach + wait `heavy`
Port: `cmd/{detach,detach_unix,detach_other,wait}.go` + `wait_test.go`. Rust: `crates/rival/src/{detach,wait}.rs`.
- [ ] Detach: re-exec self with the same args minus `--detach`, new session (`setsid` via `pre_exec`), stdio to the `rival_out`/`rival_err` files, print `rival: detached pid=<n>`; `#[cfg(windows)]` stub returning "unsupported" until P5.
- [ ] `wait --log <file>`: parse JSON lines, poll sessions, exit codes and output identical.

### Task 2.3 — subprocess + quota `heavy`
Port: `internal/executor/{subprocess,quota}.go` + tests.
- [ ] Process group, timeouts, kill on cancel, output capture to log file with byte/line counts, quota detection strings.

### Task 2.4 — executors `heavy`
Port: `internal/executor/{codex,claude,claude_docker,grok,kimi,opencode}.go` + tests.
- [ ] Exact argv per CLI and model, env, workdir handling; tests assert argv vectors (as Go tests do).

### Task 2.5 — parser + gitscope `light`
Port: `internal/parser/{parser,review}.go`, `internal/gitscope/{env,gitscope}.go`, `cmd/gitscope_helper.go` + tests.

### Task 2.6 — review: types, prompt, parse, format `heavy`
Port: `internal/review/{types,prompt,parse,review_format,slots,security}.go` + tests.
- [ ] Prompts byte-identical (golden tests copy the Go strings).
- [ ] `FinalAnswer`, `jsonObjects`, `ParseReviewerOutput`, `ParsePlanOutput`, placeholder filters, severity order — port the **Go** behaviour (not the Swift fixes); record the two known Go gaps in the "known Go bugs" table below.

### Task 2.7 — review runs (plan/antislop/security/doc) `heavy`
Port: `internal/review/{plan,planrun}.go` + `{plan,planrun,antislop,selection}_test.go`.

### Task 2.8 — merge requests `heavy`
Port: `internal/mergerequest/mergerequest.go` + test, `cmd/merge_request.go` + `mr_guard_test.go` (glab via `Command`, host-scoped credentials).

### Gate P2 `gate`
- [ ] Harness scenarios added for queue (2 concurrent runs), detach + wait (success, failure, crash, timeout), each executor with fake success/failure/quota, plan/antislop/security JSON outputs, MR with fake glab. Rust lib is exercised through a temporary `rival-p2` test binary until P3 wires the real commands; diffs only on paths already ported.
- [ ] Merge each P2 branch the day it is green.

---

## P3 — commands (`feature/rust-p3-commands`)

### Task 3.1 — root + model specs `heavy`
Port: `cmd/{root,model_specs,model_command,model_run,command,run,command_codex,command_claude,command_grok,command_k3,run_claude,run_grok,run_k3}.go` + `{model_spec,model_commands,grok_command}_test.go`.
- [ ] clap tree with the exact command/flag names from `cli-surface.md`; `PersistentPreRunE` behaviour (config error, detach, reap, update check) in the same order.

### Task 3.2 — plan / antislop / security commands `heavy`
Port: `cmd/{command_plan,command_antislop,command_security}.go` + tests, `review_output_test.go`.

### Task 3.3 — install + skills `heavy`
Port: `internal/skills/{embed,codex}.go` + tests, `cmd/install.go` + test. `include_dir!` over `crates/rival-core/skills/` (a copy of `rival/internal/skills/*`; `scripts/bump-skill-versions.sh` updated to bump both until P6).

### Task 3.4 — queue, sessions, version, update, telemetry `light`
Port: `cmd/{queue,sessions,version,update}.go` + `update_{check_,}test.go`, `internal/update/check.go`, `internal/telemetry/telemetry.go` (same DSN, same opt-out).

### Gate P3 `gate`
- [ ] Full scenario set (every command and flag in `cli-surface.md`, success + failure) → 0 diffs.
- [ ] Orchestrator manual check: one real `rival command codex review` and one `rival command plan` on this repo with each binary (temp HOME, real CLIs); compare by eye; note differences.
- [ ] Merge.

---

## P4 — TUI (`feature/rust-p4-tui`)

### Task 4.1 — data side `heavy`
Port: `internal/sessionview/{cache,group}.go` + tests, `internal/dashboard/watcher.go` (notify crate).

### Task 4.2 — model, keys, layout, styles `heavy`
Port: `internal/dashboard/{model,keys,layout,styles}.go` + `{keys,layout,styles,loader}_test.go`. Palette = current Go `styles.go` (dim phosphor, as on master `3989a88`).

### Task 4.3 — list, pagination `heavy`
Port: `session_list.go` + `{session_list,list_model,pagination}_test.go`.

### Task 4.4 — detail, preview, logview, viewport, kill `heavy`
Port: `{detail_view,preview,logview,kill}.go` + `{detail_view,preview,logview,viewport,kill_safety,parity}_test.go`.

### Task 4.5 — `rival tui` wiring `light`
Port: `cmd/tui.go`; no-op logger while the TUI runs (master fix `3989a88`); background reap.

### Task 4.6 — result parser (from the app) `heavy`
Port: `app/Sources/RivalKit/ResultParser.swift` (newest logic, incl. double-answer dedupe and the unanswered-transcript fix) + `app/Tests/RivalKitTests/ResultParserTests.swift` (41 cases). Rust: `crates/rival-core/src/result.rs`.
- [ ] API: `parse_run_result(raw: &str) -> RunResult` with `RunResult::{Findings{summary, rating: Option<u8>, groups: Vec<SeverityGroup>}, Markdown{text}, Failed{reason}}`; `SeverityGroup{severity, findings}`; `Finding{file, line, severity, category, title, body, failure_scenario, suggestion, confidence}`.
- [ ] Separate from `review::parse` (Go port, used by CLI output for parity). Unifying the two is a known-bugs follow-up after P6.
- [ ] The three fake logs in `testdata/logs` → findings (6, rating 6) / markdown / failed.

### Task 4.7 — markdown → terminal text `heavy`
Rust: `crates/rival/src/tui/markdown.rs`; dep `pulldown-cmark`.
- [ ] `render(md: &str, width: u16, theme: &Styles) -> ratatui::text::Text<'static>`: headings bold + accent, paragraphs wrapped to width, `-`/`*`/`1.` lists with hanging indent (lazy continuation lines join the item), fenced code as dim block with no wrapping, inline code in accent, bold/italic, links as `text (url)`, no raw HTML (shown as text).
- [ ] Unit tests on `Text` lines for each element and for the review-markdown fake log.

### Task 4.8 — Result tab `heavy`
Rust: `crates/rival/src/tui/{result_view,detail_view,keys,model}.rs`.
- [ ] Tabs: `Result · Raw · Prompt · Info` (Raw = the Go TUI's Output view, unchanged). Keys `1`-`4`; `tab`-cycling as today.
- [ ] Default tab: finished run → Result; live run → Raw with follow. When the selected live run finishes while on Raw with follow on → switch to Result; otherwise stay (same rules as Rival.app `RunDetailModel`).
- [ ] Result layout (scrollable viewport, same scroll keys as Raw):
  - header box: `model · effort · mode · elapsed` + `rating N/10` (amber) right-aligned; severity counts `● 1 critical ● 2 high …` in severity colours; summary wrapped.
  - per severity: a rule line `CRITICAL ────`, then each finding: `file:line · category · conf N` (accent/dim), title bold, body wrapped; `failure scenario` and `suggestion` collapsed behind `▸` lines, `enter`/`space` on the focused finding toggles them; `j/k` moves focus between findings.
  - markdown answers via Task 4.7; parse failure: `⚠ Couldn't parse this run's output.` + reason + session error + `press 2 for Raw`; live run: `Run is still going — press 2 for Raw.`
- [ ] Parse once per finished member (cache by path + size + mtime), off the UI thread; reads the 256 KB tail only.
- [ ] Model tests: default-tab rules (5 cases as in the app's `RunDetailModelTests`), focus/toggle, cache reuse.

### Gate P4 `gate`
- [ ] ratatui `TestBackend` golden frames for: empty, loading, list page 1/2, filter, detail tabs, kill confirm, **Result findings (expanded + collapsed), Result markdown, Result parse failure, Result live note**.
- [ ] Orchestrator: vhs screenshot of the Rust TUI Result tab on the fake plan run next to the Rival.app Result screenshot; fix loop (max 3).
- [ ] Orchestrator: vhs screenshots of Go TUI vs Rust TUI on the same fake fixture; side by side by eye; fix loop (max 3).
- [ ] Merge.

---

## P5 — Windows (`feature/rust-p5-windows`)

### Task 5.1 — Windows process layer `heavy`
- [ ] `procinfo`: `GetProcessTimes`. Detach: `CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS`. Stop/kill: `GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT)` then `TerminateProcess` after the same grace period as Unix. Subprocess groups: Job Objects so children die with the run.
- [ ] Paths: `%USERPROFILE%\.rival`; `.cmd`/`.exe` resolution for reviewer CLIs (`codex.cmd` from npm).

### Task 5.2 — Windows CI `light`
- [ ] `ci.yml` adds `windows-latest`: `cargo test --workspace` (cross-process queue test included); the parity harness stays macOS/Linux (Go reference behaviour is Unix).

### Gate P5 `gate`
- [ ] CI green on all three; orchestrator reviews the Windows-only code paths. Merge.

---

## P6 — switch (`feature/rust-p6-switch`)

### Task 6.1 — release pipeline `heavy`
- [ ] `release.yml`: replace the goreleaser Go build with Rust builds for darwin/linux × amd64/arm64 and windows amd64/arm64. Option order: goreleaser's Rust builder (keeps the existing `brews:` formula config) → else cargo-dist. Verify the chosen tool's current docs before writing; same archive names `rival_<os>_<arch>.tar.gz` (zip on Windows), checksums, formula update.
- [ ] `Makefile`, README (Windows install: download zip, unsigned binary note), CHANGELOG.

### Task 6.2 — remove Go `light`
- [ ] Move `rival/` to `/tmp/trash/rival-go.<ts>`; `git add -A`; skills tree lives only under `crates/rival-core/skills/`; `bump-skill-versions.sh` points there; `testdata` contract test now Rust-only plus a Swift decode test in `app/Tests` reading `testdata/written/`.

### Gate P6 `gate`
- [ ] Final parity run against `/tmp/rival-go-ref` (last Go build) → 0 diffs.
- [ ] `/rival-codex review` on the whole Rust tree vs `master` before P1; verify findings, fix; `/simplify`.
- [ ] Merge; release is the user's call (version bump, SSH-alias push per project memory, `gh run watch`, brew upgrade, `rival version`).

---

## Known Go bugs (port as-is, fix after P6)

| Bug | Where | Note |
|---|---|---|
| No `codex` header → whole log scanned; a prompt example JSON can pass as the answer | `rival/internal/review/parse.go:165` | fixed in Swift only |
| Codex double-printed answer not deduped | same | Swift dedupes |
| CLI output parser (`review::parse`, Go port) and TUI Result parser (`result`, app port) are two implementations | `crates/rival-core/src/{review/parse,result}.rs` | unify after P6: CLI moves to `result` once parity no longer binds it |
| (implementers append here) | | |

## Type-consistency check

- Module names mirror Go packages 1:1 (`session`, `queue`, `executor::*`, `review::*`, `sessionview`, `tui::*`), so every "Port:" line maps to one Rust file.
- `logging` builder used by every module; no `println!` to stderr outside it.
- Paths only via `paths.rs`; tests always pass a temp root.
- Binary name `rival` everywhere; harness flags `--go` / `--rust`.
- `RunResult` / `SeverityGroup` / `Finding` (Task 4.6) are the only types `result_view.rs` (Task 4.8) consumes; tab enum `DetailTab::{Result, Raw, Prompt, Info}` shared by `keys.rs`, `model.rs`, `detail_view.rs`.

## Self-review notes

- Every spec goal maps: G1 → 1.3, 1.6; G2 → 1.2, 2.2; G3 → 1.7 + every gate; G4 → P4 (Tasks 4.1-4.8); G5 → P5; G6 → 6.1.
- Biggest risks: byte-identical prompts (Task 2.6 golden tests), Go `time.Time` JSON format (Task 1.3), cross-process queue ordering (Task 2.1), Windows Job Objects (Task 5.1).
- No Docker on this Mac: `claude_docker` is covered by argv tests and a fake `docker` in the harness only.
