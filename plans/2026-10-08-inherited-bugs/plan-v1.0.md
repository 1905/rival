# Inherited bugs Implementation Plan v1.0

**Date:** 2026-10-08
**Status:** in-progress
**Spec:** ./spec.md

**Goal:** fix the 15 problems in the spec on top of master `d66be1a` (rust-only merged), and replace the "Known Go bugs" table with a short "Known limits" list.

**Architecture:** four phases, one commit each, on branch `feat/inherited-bugs` in the worktree `worktrees/inherited-bugs`. The exit gate is one `/rival-codex review` of the branch, then one PR and the merge. Citations below are for master `d66be1a`.

**Tech Stack:** Rust 1.98, std, serde_json; Python 3 e2e harness; GitHub Actions.

> For agentic workers: implement task by task. Use the checkboxes to track.

## Implementer rules (paste in every dispatch)

"Write the code and the unit tests that your task names. Write the failing test first, then the fix. Run only focused tests (`cargo test -p <crate> <filter>`), `cargo build`, and at the end `cargo fmt --all`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked`. Never run real reviewer CLIs, network calls or releases. Never touch the real `~/.rival`. Do not commit. Do not stash. Delete files with `git rm`. Never make a `target` symlink. Keep every behaviour that the spec does not list as changed."

## File map

**Modify:** `crates/rival-core/src/executor/{claude_docker,kimi}.rs` (+ tests), `crates/rival-core/src/config.rs` (+ tests), `crates/rival-core/src/result.rs`, `crates/rival-core/src/review/{parse,plan,types,security,format,slots}.rs` (+ tests), `crates/rival/src/{model_command,command_security,command_plan,queue_sessions,install,root}.rs` (+ tests), `crates/rival-core/src/queue/mod.rs` (+ tests), `crates/rival-core/src/gitscope/mod.rs` (+ tests), `crates/rival-core/src/mergerequest/mod.rs` (+ tests), `crates/rival-core/Cargo.toml`, `Cargo.lock`, `crates/rival-core/src/lib.rs`, `docs/runtime-reference.md`, `README.md`, `CHANGELOG.md`, `e2e/scenarios/`.

**Delete:** `crates/rival-core/src/telemetry.rs`, `crates/rival-core/src/telemetry/tests.rs`.

## Task 0 — worktree and baseline

- [ ] `git worktree add worktrees/inherited-bugs -b feat/inherited-bugs master`.
- [ ] Workspace tests, e2e (`79/79`) and the grep gate pass. Record the counts.

## P1 — security

### Task 1.1 — Docker token by name only (Problem 1)
Files: `executor/claude_docker.rs:156` (+ tests).
- [ ] Failing test: the `docker` argv has `-e ANTHROPIC_AUTH_TOKEN` and no element that contains the token value. The request's `env` has `ANTHROPIC_AUTH_TOKEN=<token>`.
- [ ] Fix: pass the name only, and put the value in `Request.env`.
- [ ] Update the three Claude Docker e2e scenarios (argv and `expect.env`).

### Task 1.2 — Kimi review options except for raw (Problem 2)
Files: `executor/kimi.rs:113` (+ tests).
- [ ] Failing test: modes `review`, `plan`, `security` get the review options. Mode `raw` gets full-auto.
- [ ] Fix: full-auto only when the mode is `raw`.

### Task 1.3 — credential search stops at a cleaned HOME (Problem 3)
Files: `config.rs:1128` (+ tests).
- [ ] Failing test: with `HOME=/x/` and `HOME=/x`, a `.env` in `/` with the key is not read.
- [ ] Fix: compare with `paths::clean(home)`.
- [ ] Orchestrator: gate, commit `fix(security): Docker token, Kimi tool rule, credential search bound`.

## P2 — one review parser

### Task 2.1 — shared answer finder and decoder (Problems 4, 5)
Files: `result.rs:118`, `review/parse.rs:188`, `review/{plan,types,security,format}.rs`, `crates/rival/src/{model_command,command_security,command_plan}.rs` (+ tests).
- [ ] Before the change, record the CLI output of every log in `testdata/logs/` and every case in `review/parse/tests.rs` and `review/plan/tests.rs`.
- [ ] The CLI uses `result::final_answer`. Delete `review::parse::final_answer`. One decoder serves review, security and plan payloads.
- [ ] A blank plan summary is invalid (shows UNPARSED). Failing test first.
- [ ] Each recorded output stays the same, or the test documents the new output with a reason.
- [ ] Add a test: a codex log with no banner and no `exec` line.
- [ ] Orchestrator: gate, e2e (record new output only where the parser change explains it), commit `fix(review): one parser for the CLI and the TUI`.

## P3 — queue, scope, merge request

### Task 3.1 — force clear keeps live running tickets (Problem 6)
Files: `queue/mod.rs:349` (+ tests), `crates/rival/src/queue_sessions.rs` (+ tests).
- [ ] Failing test: `clear(true)` keeps a running ticket whose process is alive and removes a dead one.
- [ ] The command prints how many live running tickets it kept.
- [ ] New e2e scenario: `queue clear --force` with one live running ticket.

### Task 3.2 — git scope ignores repository overrides (Problem 7)
Files: `gitscope/mod.rs:119` and `merge_file_lists` (+ tests).
- [ ] Failing test: an inherited `GIT_DIR` does not change scope detection.
- [ ] Fix: `git_cmd` uses `gitscope::repository_env(cfg.environ())`. Add the `seen` line in `merge_file_lists`.

### Task 3.3 — encoded MR link and group cancel (Problems 8, 9)
Files: `mergerequest/mod.rs:42,672,781` (+ tests).
- [ ] Failing test: `%2F-%2Fmerge_requests%2F42` is found.
- [ ] Failing test: cancel kills a grandchild that holds the pipe, within the drain bound.
- [ ] Fix: percent-decode before `contains`; run git and glab in their own process group with the provider executor's kill and drain.
- [ ] New e2e scenario: the encoded MR link.

### Task 3.4 — queue I/O error text (Problem 10)
Files: `review/slots.rs:152` (+ tests).
- [ ] Failing test: a queue I/O error prints `queue wait failed: <error>`.
- [ ] Orchestrator: gate, e2e, commit `fix: force clear, git scope, MR link and cancel, queue error text`.

## P4 — install, timeout, Windows, telemetry

### Task 4.1 — safe skill write and dangling links (Problems 11, 12)
Files: `crates/rival/src/install.rs:344,229` (+ tests).
- [ ] Failing test: a write that fails before the rename leaves the old file.
- [ ] Failing test: a dangling deprecated skill link is removed.
- [ ] Fix: temp file in the same directory, then rename; `symlink_metadata` for the deprecated cleanup.

### Task 4.2 — saturating timeouts (Problem 13)
Files: `config.rs:1192,1218` (+ tests).
- [ ] Failing test: a huge `RIVAL_RUN_TIMEOUT` gives the maximum wait, not a negative one.

### Task 4.3 — Windows drive path in Claude Docker (Problem 14)
Files: `executor/claude_docker.rs:131` (+ tests).
- [ ] Windows test: `C:\repo` mounts as `C:\repo`. Fix: `Path::is_absolute`.

### Task 4.4 — delete telemetry (Problem 15)
Files: delete `telemetry.rs` and `telemetry/tests.rs`; modify `lib.rs`, `crates/rival/src/root.rs` (+ tests), `crates/rival-core/Cargo.toml`, `Cargo.lock`, `README.md`.
- [ ] Remove the module, its call sites and the `sentry` dependency. `cargo tree -i sentry` prints nothing.

### Task 4.5 — Known limits
Files: `docs/runtime-reference.md`, `CHANGELOG.md`.
- [ ] Add "Known limits" with rows 16 (removal count lost on partial cleanup) and 20 (version compare).
- [ ] One CHANGELOG Unreleased line for each user-visible fix.
- [ ] Orchestrator: gate, three-OS CI, commit `fix: safe skill install, bounded timeouts, Windows Docker path; remove telemetry`.

## Exit gate

- [ ] fmt, clippy, workspace tests, e2e, release-script tests, the grep gate, three-OS CI.
- [ ] Manual on Dell: a Claude Docker run if Docker and a token are present (else say so), `ps` shows no token; `queue clear --force` with a live run.
- [ ] `/rival-codex review` of the branch against master. Verify every finding. Fix the confirmed ones.
- [ ] PR, link it, CI green, merge.
- [ ] As-built notes in `spec.md`, plan `done`, `git mv` the dir to `plans/done/`.

## Name check

- `result::final_answer` is the only answer finder after Task 2.1.
- `Manager::clear(force: bool) -> anyhow::Result<ClearReport>` with `removed` and `kept_live` counts (Task 3.1). `queue_sessions.rs` prints both.
- No new public names in other tasks.
