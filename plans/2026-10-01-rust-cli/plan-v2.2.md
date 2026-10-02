# Rust CLI + TUI — Implementation Plan v2.2

**Date:** 2026-10-01
**Status:** in-progress — approved for implementation 2026-10-02
**Changes from v2.0:** Go review fixes (branch `fix/go-review-findings`, 2026-10-01) wired in: port base is master *after* that merge; Tasks 2.3, 2.6/2.7, 3.1/3.2 carry the fixed behaviour and its regression tests.
**Changes from v2.1:** Codex plan review (6/10, 7 findings, all confirmed) applied: exit_code contract, detach inherits stdio, Windows stop via the owning process + Job Object, isolated `update` scenarios, stderr expectations, Swift decode test in branch CI, unpublished 6-target build gate.
**Changes from v1.1 (user, 2026-10-01):** no Go-vs-Rust parity harness and no Go reference binary. CLI logic is ported exactly from the Go source and proven by the ported Go unit tests + contract tests + Rust golden tests. TUI is "about the same design", free to improve; Go TUI code is read only for logic (filter, paging, kill safety, watcher). TUI frame-by-frame look-alike checks removed.
**Spec:** ./spec.md (approved) · CLI reference: ./cli-surface.md (Go `--help` for all 21 commands)

**Goal:** a Rust `rival` whose commands behave exactly like today's (same logic, flags, output, files, exit codes), with a TUI of about the same design plus the app's Result view, built for darwin/linux/windows × amd64/arm64, replacing Go in one release.
**Architecture:** Cargo workspace at the repo root: `crates/rival-core` (library, one module per Go package) and `crates/rival` (binary: clap commands + `tui` module). CLI logic is a line-by-line port of the Go source with its tests; a fake-CLI scenario runner checks the Rust binary end to end against expected outputs written in the scenarios (no Go binary involved). Go stays the shipped binary until P6.
**Tech Stack:** Rust 1.98 · clap 4 (derive) · serde + serde_json · a maintained serde YAML crate (Task 1.4 picks it) · chrono · uuid · dotenvy · fd-lock · include_dir · sentry · ureq (update check) · notify (fs watch) · ratatui + crossterm · windows-sys (P5) · Python 3 for the scenario runner.

> For agentic workers: use superpowers:subagent-driven-development to implement task-by-task. Checkbox syntax for tracking.

**Tags:** `heavy` = Opus 5.5 implementer · `light` = small Opus task · `gate` = orchestrator only (runs the scenario runner, real reviews, CI, commits, merges).

**Implementer restriction (paste verbatim in every dispatch):** "You write the code and unit tests your task names and run the focused tests for that task (`cargo test -p <crate> <filter>`, `cargo build`, `go test ./internal/<pkg>/ -run <Name>` for the Go contract tests). You NEVER run the scenario runner, real reviewer CLIs (codex, claude, grok, opencode, glab, docker), network calls, e2e / integration / live / smoke tests; never publish, deploy, push, touch infra, or run anything money-bearing. You never touch the real ~/.rival: every test uses a temp HOME/RIVAL_HOME. Do not commit; the orchestrator commits. Do not set any test-DB env var. Never delete files with rm; move them to /tmp/trash/<name>.<timestamp>. CLI tasks: port the Go logic exactly (same branches, messages, ordering); list any Go bug you notice in your report instead of fixing it. TUI tasks: reuse Go logic, but layout and look may improve."

**Branches:** one per phase from `master` (`feature/rust-p1-core`, …), merged the same day its gate is green (merge hygiene). The Rust binary is built and tested in CI from P1 but not released or put on PATH until P6. Exec starts from a clean `master`.

## Contracts every task must keep

| Contract | Rule | Source |
|---|---|---|
| Session file | Same keys, same order, same `omitempty` behaviour as `session.Session`. Pointer fields (`exit_code *int`, `end_time *time.Time`) are `Option<_>` and omitted only when `None`: a finished run writes `exit_code: 0` (`session.go:55`, set at `:189`/`:201`; `wait` prints `exit=0`, not `exit=-`). RFC3339 times with Go's formatting (nanoseconds trimmed the way Go trims them). Atomic save: `<id>.json.tmp-*` then rename, mode 0600. | `rival/internal/session/session.go:152-190` |
| Queue ticket | Same keys/order as `queue.Ticket`; ticket files in `~/.rival/queue/`; all state changes inside one file-lock critical section. | `rival/internal/queue/{ticket,queue}.go` |
| stderr log | One JSON object per line: `level`, `app:"rival"`, per-call fields, `time` (RFC3339, seconds), `message`. Same field names as zerolog. | `rival/main.go:20` |
| `.env` | Loaded silently from the working directory at start (godotenv semantics). | `rival/main.go:17` |
| `wait` | Same exit codes 0 / 2 / 3 / 4 and the same stdout lines. | `rival/cmd/wait.go:21` |
| Flags | Same names, short forms, defaults and validation errors as `cli-surface.md`. `--help` text may differ (clap vs cobra). | `./cli-surface.md` |
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

**As-built layout, P1:** session is `src/session/{mod,summary,reaper,tests}.rs`; JSON compatibility helpers are in `src/gojson.rs`; Go standard-library compatibility helpers and Unicode tables are in `src/gostd{,_tables}.rs`. These share serialization and sorting behavior across later modules.

## Self-test sanity check (before P1)

- [x] Clean `master` **with `fix/go-review-findings` merged** (`066cb1a`); cargo 1.98.1; Go 1.27.1.
- [x] Baseline: 707 Go tests passed across 17 packages; golangci-lint v2.14.0 passed (2026-10-02).

---

## P1 — foundations + scenario runner (`feature/rust-p1-core`)

### Task 1.1 — workspace `light`
- [x] `Cargo.toml` workspace (`crates/rival-core`, `crates/rival`), `rust-toolchain.toml` 1.98, release profile lto/codegen-units 1/strip; binary name `rival`. `.gitignore` `target/`.
- [x] `cargo build` → 0 warnings; `cargo run -p rival -- version` prints `rival dev`.

### Task 1.2 — paths + logging `heavy`
Port: `main.go` (logger, .env), path helpers from `config`/`session`. Rust: `paths.rs`, `logging.rs`.
- [x] `logging::init()` → global JSON-lines writer to stderr with zerolog field names/order; `logging::info().str(k,v).int(k,v).msg(m)`-style builder (no `tracing`); levels debug/info/warn/error.
- [x] Test: a line produced by Rust and the same call in Go (golden string in the test) parse to equal maps.
- [x] Load `.env` from cwd, silent on missing. As built: direct godotenv parser port; dotenvy differed on duplicate keys and expansion.

### Task 1.3 — session model, save, summary, reaper `heavy`
Port: `internal/session/{session,summary,reaper}.go` + tests `{session,save,summary,reaper}_test.go`.
- [x] `Session` struct field order = Go; serde attrs reproduce `omitempty` exactly: value fields skip zero/empty, pointer fields are `Option` and skip only `None`; custom time serialiser matching Go `time.Time` JSON.
- [x] Writer tests for `exit_code` unset (key absent), `0` (present, 0) and nonzero.
- [x] Atomic save, load, list, summary (no prompt), reaper (orphan → failed) with `procinfo`.
- [x] All Go cases ported → `cargo test -p rival-core session summary reaper` green.

### Task 1.4 — config `heavy`
Port: `internal/config/config.go` (1,069 lines) + `{config,codex,kimi,security,antislop}_test.go`.
- [x] Pick the YAML crate: serde-saphyr 1.3.0, released 2026-09-16. Research and source links in `research.md`.
- [x] Same defaults, env overrides, Rival validation errors, model labels, public error/log scrubbing (`PublicRuntimeError`, `PublicRuntimeLog`, `replaceConcreteModelIDs`), `KimiAPIKeyFrom`. Compatibility limit: malformed YAML uses serde-saphyr's parser detail after the original `parse <path>:` prefix.
- [x] Ported config tests green: 64 tests, including signed duration overflow.

### Task 1.5 — logfmt + procinfo `light`
Port: `internal/logfmt`, `internal/procinfo` (darwin `sysctl kern.proc.pid`, linux `/proc/<pid>/stat`, other → None) + tests. As built: use Go's sysctl interface; proc_pidinfo could not inspect another user's process.
- [x] Green on macOS and Linux; native Linux process checks passed in CI run 37007239814.

### Task 1.6 — testdata + contract tests `heavy`
- [x] `testdata/sessions/*.json` + `logs/` = the fake fixtures in `app/Tests/Fixtures` (copy); `expected.json` lists decoded fields per file.
- [x] Go test `rival/internal/session/testdata_contract_test.go`: decode all → equals `expected.json`; plus "Go writes a session from expected values; bytes equal the Rust writer's output for the same input" (Rust golden files under `testdata/written/`).
- [x] Rust `crates/rival-core/tests/contract.rs`: same in reverse.
- [x] `go test ./internal/session/ -run Contract` + `cargo test -p rival-core --test contract` green.

### Task 1.7 — scenario runner (Rust only) `heavy`
- [x] `parity/run.py --bin <rival> [--scenario <glob>]`: per scenario, a temp HOME, PATH = `parity/fakes` first, run the Rust binary, collect stdout, **stderr split into plain lines and JSON log events**, exit code and `~/.rival/**`; normalise UUIDs/pids/times/temp paths; compare to the scenario's `expect:` block: exit code, stdout golden, `stderr_lines` (exact plain lines, e.g. `rival: detached pid=<PID>`, usage/validation errors from `root.go:144-149`, `Update available: …` from `update/check.go:108`), `log_events` (level + message + required fields, order-insensitive), session fields, files present; exit 1 on mismatch. Plain stderr lines are allowed alongside JSON; an unexpected plain line fails the scenario.
- [x] `parity/fakes/`: Python fakes `codex`, `claude`, `grok`, `opencode`, `glab`, `docker`, **`brew`** driven by `FAKE_<NAME>_SCRIPT=<file>` (canned output, exit code, delay, quota text); fake `brew --prefix rival` points into the scenario's temp dir, so `rival update` never touches the installed binary.
- [ ] Update endpoint: the Rust update client reads `RIVAL_UPDATE_API` (base URL override, honoured only in debug builds; scenarios run the debug binary) and the runner serves canned release JSON from a local HTTP server (`http.server` on 127.0.0.1). Scenarios run with no external network; a guard in the runner fails the scenario if the fake `brew` reports a non-temp prefix. Runner server/guard verified; client implementation remains Task 3.4.
- [x] Expected outputs are written from the Go source and Go test expectations (messages, formats), not from running Go.
- [ ] Self-test: 3 scenarios (`version`, `sessions` on empty home, `queue`) pass once Task 3.4 lands; until then the runner's own unit tests.

Runner validation: 55 unit tests pass locally, including SIGINT/SIGTERM cleanup, unrelated-process preservation, late detached output, inherited stdin after unlink, and bounded startup diagnostics. Hosted macOS/Linux verification passed in CI 37011767761. Initial CLI scenarios remain pending Task 3.4.

### Task 1.8 — CI `light`
- [x] `.github/workflows/ci.yml`: `cargo test --workspace` + `cargo clippy -D warnings` on macos-15 and ubuntu-latest; Go tests unchanged.
- [x] macos-15 job also runs `cd app && swift test --filter SessionDecodingTests` plus new Swift tests that decode every file in `testdata/written/` (the Rust writer's golden output, checked by the Rust contract test without regeneration). Required check from P1 on. Local Swift filter: 14 passed.

### Gate P1 `gate`
- [x] Orchestrator: workspace tests, Go contract, 55 runner tests, and Swift decoding passed on the required hosts. CI 37011767761 verified `50e3eea`; merged and pushed to `master` on 2026-10-02.

---

## P2 — execution (`feature/rust-p2a-queue`, then `feature/rust-p2b-review`)

### Task 2.1 — queue + tickets `heavy`
Port: `internal/queue/{queue,ticket}.go` + `{queue,crossproc}_test.go`.
- [x] fd-lock on the queue lock file; `WaitForSlot`, position callbacks, `ReapDead`, clear; ordering preserved. All 13 Go queue cases ported; 37 focused queue/cancellation tests pass locally.
- [x] Cross-process test spawns three helpers, matching Go A/B/C. Deterministic enqueue readiness and bounded cleanup verify FIFO and mutual exclusion.

### Task 2.2 — detach + wait `heavy`
Port: `cmd/{detach,detach_unix,detach_other,wait}.go` + `wait_test.go`. Rust: `crates/rival/src/{detach,wait}.rs`.
- [ ] Detach (`cmd/detach.go:30-57`): re-exec self with the same args and env `RIVAL_DETACHED=1` (same guard name as Go), new session (`setsid` via `pre_exec`), **inherit stdin, stdout and stderr as they are** (never reopen files; the caller's redirects are the contract), print `rival: detached pid=<n>` on the inherited stderr; if that print fails, kill the child and exit 1; `#[cfg(windows)]` stub until P5.
- [ ] Scenario: arbitrary redirect file names for stdin/stdout/stderr, input file unlinked right after the parent exits → the detached run still reads its prompt and writes to the caller's files.
- [x] `wait --log <file>` logic: starting markers, session polling, all six named Go test cases, exit codes and exact summary lines ported. CLI wiring and signal checks remain P3.

**P2a verification:** detach/wait functions implemented; 36 focused Rust tests pass. The full workspace has 257 passing tests and two ignored helper/generator entries. All 58 runner self-tests pass. Build, formatting and Clippy pass locally. The seven new detach/wait scenarios validate against source-derived sample output; actual command execution waits for P3. The scenario checks link short IDs to full UUID markers. Closed-before-startup stderr remains a P3 compatibility item (see `research.md`); the detach checkbox stays open until resolved. Redirect/unlink timing is stated explicitly in its scenario.

**P2a delivered:** `a073d03ced62500fd1ac9157b2e2bddfa72d9747` merged and pushed after [CI 37018189761](https://github.com/1905/rival/actions/runs/37018189761) passed on macOS/Linux, including Go contracts, runner self-tests and Swift decoding. P2b continues on `feature/rust-p2b-review`.

**Post-merge CI follow-up:** master run 37018628122 passed on macOS but failed an existing Go test on Linux. `TestCodexPlanUsesCodexRuntimeAndStructuredOutput` let its fake provider exit without consuming stdin, producing intermittent EPIPE. Fixture-only correction `734f1a3` passed the isolated Go suite and [CI 37021481661](https://github.com/1905/rival/actions/runs/37021481661), then merged separately. Runtime behavior is unchanged; P2b Rust work remains in progress.

### Task 2.3 — subprocess + quota `heavy`
Port: `internal/executor/{subprocess,quota}.go` + tests.
- [x] Process group per provider (Unix `CommandExt::process_group(0)`), timeout/cancel kills the **whole group**, five-second pipe drain, exact byte/line counts, and quota detection. Nonblocking pipe IO avoids cross-thread close assumptions.
- [x] Regression test ported from the Go fix: a fake launcher spawns a child that ignores SIGTERM and holds stdout open → the run returns within the bound and the child is dead. Both deadline and manual cancellation tested; an escaped helper also proves the stdin/stdout/stderr drain bound.

**Task 2.3 verification:** controller workspace check: 296 tests passed, three ignored helper/generator entries; formatting and Clippy passed. [CI 37022801563](https://github.com/1905/rival/actions/runs/37022801563) passed on macOS and Linux for `dcd9bd9`, including the Go contract, runner tests and Swift decoding on macOS. Tests use temporary session paths and injected child environments. `gitscope::repository_env` was pulled forward for subprocess filtering. `Config::environ()` preserves non-UTF-8 environment entries. Task2.4 also preserves Unix environment order through direct `execve`. Windows process operations remain explicit P5 placeholders.

### Task 2.4 — executors `heavy`
Port: `internal/executor/{codex,claude,claude_docker,grok,kimi,opencode}.go` + tests.
- [x] Exact argv per CLI and model, env, workdir handling; tests assert argv vectors (as Go tests do). All 35 named Go adapter tests are mapped in the task report, including registry tests and fake Docker preflight/build checks.

**Task 2.4 verification:** controller workspace check: 360 tests passed, three ignored helper/generator entries; formatting and Clippy passed. [CI 37026571060](https://github.com/1905/rival/actions/runs/37026571060) passed on macOS and Linux for `8e10ad1`, including Swift session decoding. A verified macOS shell-fallback mismatch is fixed with a prepared `execve` hook. Both executable-format regressions run without skips; byte-preserving environment, argument, process-group and drain checks pass. Twenty-two executor scenarios are authored and schema-checked; execution waits for P3. Windows preflight/temp-file/exit-status work remains P5.

### Task 2.5 — parser + gitscope `light`
Port: `internal/parser/{parser,review}.go`, `internal/gitscope/{env,gitscope}.go`, `cmd/gitscope_helper.go` + tests.
- [x] Argument grammar, source error strings, scope detection and diff preambles ported with the named Go tests. The small `lensPrompt` wrapper is complete with its prompt builder in Task 2.6.

**Task 2.5 verification:** controller workspace check: 407 tests passed, three ignored helper/generator entries; formatting and Clippy passed. [CI 37028459182](https://github.com/1905/rival/actions/runs/37028459182) passed on macOS and Linux for `6030678`, including Swift session decoding. Git tests use temporary homes and repositories. Duplicate-file and inherited-repository-override quirks are pinned. Invalid UTF-8 Git output remains a recorded compatibility limit; Windows PWD handling remains P5.

### Task 2.6 — review: types, prompt, parse, format `heavy`
Port: `internal/review/{types,prompt,parse,review_format,slots,security}.go` + tests.
- [x] Prompts byte-identical. Source comparisons and independent length/SHA-256 pins cover all six prompt pieces and both assembled review prompts.
- [x] Every parse of a provider log goes through `final_answer` first (code review, plan, antislop, security), as in the fixed Go code; port the regression tests (tool-printed assessment JSON + unstructured final answer → parse failure).
- [x] `FinalAnswer`, `jsonObjects`, `ParseReviewerOutput`, `ParsePlanOutput`, placeholder filters, severity order — ported with **Go** behavior. Tests pin both known gaps listed below.

**Task 2.6 verification:** controller workspace check: 490 tests passed, three ignored helper/generator entries; formatting and Clippy passed. [CI 37030778213](https://github.com/1905/rival/actions/runs/37030778213) passed on macOS and Linux for `b8f9bab`, including Swift session decoding. Eighty-two focused review tests cover Go JSON decoding, prompts, output, security validation and queue rollback/release. The Task 2.5 lens wrapper is complete. Pure plan parsing/formatting was pulled forward from Task 2.7. Provider-log helpers apply `final_answer`; runtime call sites remain Task 2.7/P3, so that checkbox stays open.

### Task 2.7 — review runs (plan/antislop/security/doc) `heavy`
Port: `internal/review/{plan,planrun}.go` + `{plan,planrun,antislop,selection}_test.go`.
- [x] Concurrent plan/document reviews, requested result order, per-model effort, final-answer parsing, timeout/quota handling and cleanup ported with all named Go cases.

**Task 2.7 verification:** controller workspace check: 532 tests passed, three ignored helper/generator entries; formatting and Clippy passed. [CI 37033031072](https://github.com/1905/rival/actions/runs/37033031072) passed on macOS and Linux for `86c3ee7`, including Swift session decoding. Forty-two focused orchestration tests include simultaneous reviewers, one batch ticket, persisted outcomes and panic cleanup. Four new review scenarios are authored; all 38 schemas validate. Scenario execution and code-review/security command parsing remain P3.

### Task 2.8 — merge requests `heavy`
Port: `internal/mergerequest/mergerequest.go` + test, `cmd/merge_request.go` + `mr_guard_test.go` (glab via `Command`, host-scoped credentials).
- [x] MR target validation, pinned snapshots, token filtering, patch framing/limit and cleanup ported with all seven Go core test cases. The command helper/guard is tested; its full command-path assertions remain Task 3.1.

**Task 2.8 verification:** controller workspace check: 570 tests passed, three ignored helper/generator entries; formatting and Clippy passed. Thirty-two MR tests and five command-helper tests cover identity, caller repository preservation, cancellation during fetch and symlink-safe cleanup. The URL helper is private and preserves Go's relevant parsing rules. Seven MR scenarios are authored; all 45 schemas validate without execution. CI 37037313215 passed Rust checks but failed a Linux runner diagnostic assertion after two fake stack dumps interleaved. The fixture now captures each process separately; all 58 runner tests pass locally. [CI 37038456086](https://github.com/1905/rival/actions/runs/37038456086) passed on macOS/Linux for `9f6f562`, including Swift decoding. Malformed JSON detail, invalid UTF-8 presentation and removal-error path detail retain the recorded compatibility limits.

### Gate P2 `gate`
- [x] Scenarios written for queue (2 concurrent runs), detach + wait (success, failure, crash, timeout), each public executor with fake success/failure/quota, plan/antislop/security JSON outputs, MR with fake glab. All 45 schemas validate. They run once P3 wires the commands; P2 is gated on the ported unit tests. The unused native Kimi adapter is covered by its ported unit tests.
- [x] Merge each P2 branch the day it is green. P2b `9f6f5626aa5c4d1330ee113fcb758582f89e10a7` merged and pushed after CI 37038456086 passed. Local `master` and `origin/master` match the tested revision. P3 continues on `feature/rust-p3-commands`.

---

## P3 — commands (`feature/rust-p3-commands`)

### Task 3.1 — root + model specs `heavy`
Port: `cmd/{root,model_specs,model_command,model_run,command,run,command_codex,command_claude,command_grok,command_k3,run_claude,run_grok,run_k3}.go` + `{model_spec,model_commands,grok_command}_test.go`.
- [x] clap tree with the exact command/flag names from `cli-surface.md`; `PersistentPreRunE` behaviour (config error, detach, reap, update check) in the same order.
- [x] `--workdir` resolved to an absolute, cleaned path once at command entry (one helper shared by every command that takes it), before preflight and session creation; same error text as the fixed Go helper; ported tests (relative subdir, `.`, absolute, missing; session stores absolute `work_dir`).

**Task 3.1 verification:** controller checks passed: 669 workspace tests, six ignored helper/generator entries, formatting and Clippy. Eleven fake-provider success/failure and detach/wait scenarios passed. Linked debug and LTO release binaries preserve closed stdin/stderr behavior on macOS. Closed or broken stderr leaves no task-owned child alive. [CI 37044718890](https://github.com/1905/rival/actions/runs/37044718890) passed both linked profiles on macOS/Linux at `3023181`, including Swift decoding. Model command/run MR tests cover snapshot lifetime and caller credentials. Update/telemetry production hooks remain Task 3.4; the root hook ordering is tested. A subprocess fixture now emits its output marker before publishing readiness, removing an observed cancellation race.

### Task 3.2 — plan / antislop / security commands `heavy`
(Use the shared `--workdir` helper from Task 3.1; parsing via `final_answer` per Task 2.6.)
Port: `cmd/{command_plan,command_antislop,command_security}.go` + tests, `review_output_test.go`.

- [x] Commands wired with the shared workdir helper, source validation order, final-answer parsing and security completion-save failure handling.

**Task 3.2 verification:** controller checks passed: 734 workspace tests, six ignored helper/generator entries, formatting, Clippy and build. All 12 plan/antislop/security scenarios passed, including the eight new validation and runtime-failure cases. All 53 scenario schemas validate. Sixty-five new command tests cover the named Go cases and security's distinct closed-stdin branch. A second subprocess fixture now emits its marker before starting the process that publishes readiness. [CI 37046983286](https://github.com/1905/rival/actions/runs/37046983286) passed on macOS/Linux at `daa123a`, including Swift decoding. Windows stdin/path handling remains P5; invalid UTF-8 input remains the recorded command-boundary limitation.

### Task 3.3 — install + skills `heavy`
Port: `internal/skills/{embed,codex}.go` + tests, `cmd/install.go` + test. `include_dir!` over `crates/rival-core/skills/` (a copy of `rival/internal/skills/*`; `scripts/bump-skill-versions.sh` updated to bump both until P6).

- [x] Embedded assets, Codex variants, target detection, prompt buffering, force/skip behavior and retired-skill cleanup ported. The version-bump script updates both trees.

**Task 3.3 verification:** controller checks passed: 774 workspace tests, six ignored helper/generator entries, formatting and Clippy. All four installer scenarios passed in temporary homes; all 57 scenario schemas validate. Ten embedded files match the Go assets byte for byte. The script updated 18 temporary copies without changing repository versions. Hosted CI is pending. The script now exits nonzero for missing skill files. Windows path behavior remains P5; invalid UTF-8 and failed-parent error-path limits remain recorded.

### Task 3.4 — queue, sessions, version, update, telemetry `light`
Port: `cmd/{queue,sessions,version,update}.go` + `update_{check_,}test.go`, `internal/update/check.go`, `internal/telemetry/telemetry.go` (same DSN, same opt-out).

### Gate P3 `gate`
- [ ] Full scenario set (every command and flag in `cli-surface.md`, success + failure) → all pass.
- [ ] Orchestrator manual check: one real `rival command codex review` and one `rival command plan` on this repo with the Rust binary (temp HOME, real CLIs); output reads right and the session opens in the Swift app.
- [ ] Merge.

---

## P4 — TUI (`feature/rust-p4-tui`)

### Task 4.1 — data side `heavy`
Port: `internal/sessionview/{cache,group}.go` + tests, `internal/dashboard/watcher.go` (notify crate).

### Task 4.2 — model, keys, layout, styles `heavy`
Use `internal/dashboard/{model,keys,layout,styles}.go` for logic (key map, loader, layout rules). Design: about the same as today (dim-phosphor palette from master `3989a88`, same panes), free to improve spacing and readability. Port only the logic tests from `{keys,layout,loader}_test.go`; style tests are rewritten for the Rust design.

### Task 4.3 — list, pagination `heavy`
Logic from `session_list.go` (filter, sections, pagination); port `{list_model,pagination}_test.go`; rendering tests written fresh.

### Task 4.4 — detail, preview, logview, viewport, kill `heavy`
Logic from `{detail_view,preview,logview,kill}.go` (follow, viewport, log tail, kill safety); port `{logview,viewport,kill_safety}_test.go`; rendering tests written fresh. `parity_test.go` (Go TUI vs app parity) is not ported.

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
- [ ] Orchestrator: vhs screenshots of every TUI state on the fake fixture; judged by eye for readability (not look-alike with Go); fix loop (max 3).
- [ ] Orchestrator: vhs screenshots of Go TUI vs Rust TUI on the same fake fixture; side by side by eye; fix loop (max 3).
- [ ] Merge.

---

## P5 — Windows (`feature/rust-p5-windows`)

### Task 5.1 — Windows process layer `heavy`
- [ ] `procinfo`: `GetProcessTimes`. Detach: `CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS | CREATE_NO_WINDOW`, stdio inherited as on Unix.
- [ ] Ownership: the `rival` process that runs a review creates one Job Object per provider with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` and assigns the provider at spawn (`CREATE_SUSPENDED` → assign → resume), so every descendant is in the Job. Timeout/cancel inside the owner = `TerminateJobObject`, then the same bounded pipe drain as Unix.
- [ ] Stop from another process (TUI, `rival` stop paths): console events cannot reach a detached process, so stop = `TerminateProcess` on the **owner `rival` pid** recorded in the session (checked against its start time like Unix). The owner's Job handle closes with it → kill-on-close ends the provider tree. The session is then reaped to `failed` and the queue ticket is freed by `ReapDead`, same path as a crashed owner on Unix. Wait up to the same grace period, then report.
- [ ] Native Windows tests (CI `windows-latest`): a fake launcher spawning a grandchild that holds stdout; (a) owner timeout → both dead, pipes closed, `RunSubprocess` returns; (b) a second process stops the detached owner → owner, launcher and grandchild dead, session `failed`, queue slot released.
- [ ] Paths: `%USERPROFILE%\.rival`; `.cmd`/`.exe` resolution for reviewer CLIs (`codex.cmd` from npm).

### Task 5.2 — Windows CI `light`
- [ ] `ci.yml` adds `windows-latest`: `cargo test --workspace` (cross-process queue test included); the scenario runner stays macOS/Linux (fake CLIs are Python scripts with Unix shebangs).

### Gate P5 `gate`
- [ ] CI green on all three; orchestrator reviews the Windows-only code paths. Merge.

---

## P6 — switch (`feature/rust-p6-switch`)

### Task 6.1 — release pipeline `heavy`
- [ ] `release.yml`: replace the goreleaser Go build with Rust builds for darwin/linux × amd64/arm64 and windows amd64/arm64; add a `workflow_dispatch` snapshot mode that builds and uploads artifacts to the run but publishes nothing. Option order: goreleaser's Rust builder (keeps the existing `brews:` formula config) → else cargo-dist. Verify the chosen tool's current docs before writing; same archive names `rival_<os>_<arch>.tar.gz` (zip on Windows), checksums, formula update.
- [ ] `Makefile`, README (Windows install: download zip, unsigned binary note), CHANGELOG.

### Task 6.2 — remove Go `light`
- [ ] Move `rival/` to `/tmp/trash/rival-go.<ts>`; `git add -A`; skills tree lives only under `crates/rival-core/skills/`; `bump-skill-versions.sh` points there; `testdata` contract test now Rust-only plus a Swift decode test in `app/Tests` reading `testdata/written/`.

### Gate P6 `gate`
- [ ] Full scenario set green on macOS and Linux.
- [ ] Swift decode contract job green on the P6 branch (required before merge and before any tag).
- [ ] Unpublished release build: run the new release workflow on the P6 branch via `workflow_dispatch` in snapshot mode (no publish, no formula push) for all six targets; download artifacts; check names `rival_<os>_<arch>.tar.gz`/`.zip`, checksums file, `rival version` output per archive (run where the host can), and the rendered formula diff against the current one.
- [ ] `/rival-codex review` on the whole Rust tree vs `master` before P1; verify findings, fix; `/simplify`.
- [ ] Merge; release is the user's call (version bump, SSH-alias push per project memory, `gh run watch`, brew upgrade, `rival version`).

---

## Known Go bugs (CLI: port as-is, fix after P6)

| Bug | Where | Note |
|---|---|---|
| No `codex` header → whole log scanned; a prompt example JSON can pass as the answer | `rival/internal/review/parse.go:165` | fixed in Swift only |
| ~~Timeout leaves the provider's children alive; run hangs, queue slot held~~ | `executor/subprocess.go` | fixed in Go before the port (`fix/go-review-findings`); ported fixed |
| ~~Relative `--workdir` applied twice; sessions store relative paths~~ | `cmd/*`, `executor/*` | fixed in Go before the port; ported fixed |
| ~~Plan/antislop/security parse the whole log, not the final answer~~ | `review/planrun.go`, `cmd/command_security.go` | fixed in Go before the port; ported fixed |
| Codex double-printed answer not deduped | same | Swift dedupes |
| CLI output parser (`review::parse`, Go port) and TUI Result parser (`result`, app port) are two implementations | `crates/rival-core/src/{review/parse,result}.rs` | unify after P6: CLI moves to `result` once parity no longer binds it |
| Force-clearing a running ticket permits another run before its holder finishes | `queue.Clear(true)` | Running holders do not self-heal. Preserved; only waiting tickets re-create themselves. |
| Git scope detection inherits repository overrides | `gitscope.gitCmd` | `Resolve`/`DiffStat` do not use `RepositoryEnv`; inherited `GIT_DIR` can redirect discovery. Preserve during the port. |
| File-list merging only removes duplicates across its two inputs | `gitscope.mergeFileLists` | Duplicates inside either input remain. Preserve during the port. |
| Claude Docker authentication is passed in process arguments | `executor.runClaudeDocker` | The existing `-e ANTHROPIC_AUTH_TOKEN=…` argument exposes the token to process inspection. Preserve during this port; fix separately. |
| Kimi adapter restricts tools only for the exact `review` mode | `executor.kimiRunOpts` | All other modes get full-auto options. Current callers use raw/review; task-mode behavior is a latent source bug. |
| Queue wait I/O errors are reported as cancellation | `review.WaitForGroupSlot` | Every non-timeout wait failure becomes `cancelled while queued`. Preserved in the port. |
| Plan output accepts an empty summary | `review.ParsePlanOutput` | A valid rating plus empty findings can render as a clean plan without a Summary line. Review/security validation is stricter. |
| An encoded MR marker is not detected | `mergerequest.Contains` | It checks the raw input; `%2F-%2Fmerge_requests%2F42` can fall through as an ordinary scope. Preserved. |
| MR command cancellation kills only the immediate Git/glab child | `mergerequest.git`, `Prepare` | Go uses `CommandContext` with no pipe-drain limit. A descendant holding a pipe can delay return after cancellation. This differs from the fixed provider executor and is preserved. |
| Codex antislop skill text names only Codex as the default | `skills.CodexSkill` | Runtime defaults to Codex plus Claude. Embedded instructions are copied unchanged. |
| Deprecated-skill cleanup leaves dangling symlinks | `installSkills` | Its initial `Stat` follows the link and fails. Preserved. |
| Partial retired-skill cleanup loses its removal count on error | `removeSkillDirsByHash` caller | The failure message is printed, but prior successful removals are not counted. Preserved. |
| (implementers append here) | | |
| Oversized timeout budgets wrap signed nanoseconds | `config.MaxRunWait`, `WithRunTimeout` | Rust preserves wrapping arithmetic; negative budgets must expire immediately. |
| Two maximum duration components can wrap the parser accumulator to zero | Go `time.ParseDuration` | Preserved with an explicit regression test. |
| Credential walk compares cleaned paths to raw HOME | `config` API-key lookup | A trailing slash or unclean HOME can allow walking above it; preserved. |

## Type-consistency check

- Module names mirror Go packages 1:1 (`session`, `queue`, `executor::*`, `review::*`, `sessionview`, `tui::*`), so every "Port:" line maps to one Rust file.
- `logging` builder used by every module; no `println!` to stderr outside it.
- Paths only via `paths.rs`; tests always pass a temp root.
- Binary name `rival` everywhere; scenario runner flag `--bin`.
- `RunResult` / `SeverityGroup` / `Finding` (Task 4.6) are the only types `result_view.rs` (Task 4.8) consumes; tab enum `DetailTab::{Result, Raw, Prompt, Info}` shared by `keys.rs`, `model.rs`, `detail_view.rs`.

## Self-review notes

- Every spec goal maps: G1 → 1.3, 1.6; G2 → 1.2, 2.2; G3 → ported Go tests in every CLI task + 1.7 scenarios at Gates P2/P3/P6; G4 → P4 (Tasks 4.1-4.8); G5 → P5; G6 → 6.1.
- Biggest risks: byte-identical prompts (Task 2.6 golden tests), Go `time.Time` JSON format (Task 1.3), cross-process queue ordering (Task 2.1), Windows Job Objects (Task 5.1).
- No Docker on this Mac: `claude_docker` is covered by argv tests and a fake `docker` in the scenario runner only.
