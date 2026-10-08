# Review language pass Implementation Plan v1.0

**Date:** 2026-10-08
**Status:** done
**Spec:** ./spec.md

**Goal:** port the antislop-eng checker to a hidden Rust module with the full dictionary, put the STE rules in every reviewer prompt, and add one check plus one repair call after code, security and plan reviews.

**Architecture:** four phases, one commit each, on branch `feat/review-language` in `worktrees/review-language`, from master `0fbd7dc` (rust-only and inherited bugs merged). The exit gate is one `/rival-codex review` of the branch, then one PR and the merge. Citations are for `0fbd7dc`.

**Tech Stack:** Rust 1.98, serde_json, regex (already a dependency), std; the Go checker runs on Dell only, to make fixtures.

> For agentic workers: implement task by task. Use the checkboxes to track.

## Implementer rules (paste in every dispatch)

"Write the code and the unit tests that your task names. Write the failing test first. Run focused tests, `cargo build`, and at the end `cargo fmt --all`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked`. Never run real reviewer CLIs, network calls or releases. Never touch the real `~/.rival`. Do not commit. Do not stash. Never make symlinks. Never put Go code or a Go file in the repo. Keep every behaviour that the spec does not list as changed."

## File map

**Create:**
- `crates/rival-core/src/lang/{mod,dict,check,report,repair}.rs` and `lang/tests/` test modules
- `crates/rival-core/data/lang/dictionary.json`, `crates/rival-core/data/lang/software.txt` (copied unchanged from `~/.claude/skills/antislop-eng/scripts/`; record the skill version and SHA-256 in `lang/mod.rs`)
- `testdata/lang/*.md` (corpus) and `testdata/lang/*.json` (Go checker output)
- `e2e/scenarios/review-language-*.yaml`

**Modify:** `crates/rival-core/src/lib.rs`, `crates/rival-core/src/executor/subprocess.rs:93,478` and every adapter (`codex`, `claude`, `claude_docker`, `grok`, `opencode`, `kimi`), `crates/rival/src/model_specs.rs:66`, `crates/rival-core/src/review/prompt.rs:55` (+ tests: SHA-256 pins), `crates/rival-core/src/config.rs:450`, `crates/rival-core/skills/*/SKILL.md`, `crates/rival-core/skills/codex.md`, `crates/rival/src/{model_command,model_run,command_security}.rs`, `crates/rival-core/src/review/planrun.rs:126`, `README.md`, `CHANGELOG.md`.

## Task 0 — worktree and baseline

- [x] `git worktree add worktrees/review-language -b feat/review-language master`. Commit this plan.
- [x] Workspace tests, e2e (`82/82`) and the grep gate pass. Record the counts.

## P1 — the checker in Rust

### Task 1.1 — corpus and Go fixtures (orchestrator, on Dell)
- [x] Copy `dictionary.json` and `glossary/software.txt` to `crates/rival-core/data/lang/`. Record the skill version and both SHA-256 values.
- [x] Corpus `testdata/lang/`: the three specs in `plans/done/*/spec.md` and `plans/2026-10-08-review-language/spec.md`, each input text of `check_test.go`, and 6 review texts written for the test (no private code). One `.md` file each.
- [x] For each corpus file: `go run ~/.claude/skills/antislop-eng/scripts/check.go <file> --json > <file>.json` (default mode). Commit the `.json` files. Go and `check.go` stay out of the repo.

### Task 1.2 — the port
Files: `lang/{mod,dict,check,report}.rs`, `lib.rs`.
- [x] `dict`: parse the embedded `dictionary.json` once (`LazyLock`); load `software.txt` the same way. Every entry and every field stays.
- [x] `check`: port every check of `check.go` (words, part of speech, hedge hints, -ing, perfect tenses, been/being, passive, sentence length 20/25, semicolons, contractions, Latin abbreviations, noun clusters, he/she, paragraphs over 6 sentences, auto-glossary, code spans and fences). Keep the order of findings.
- [x] `report`: `Report { findings, words, auto }`, `fn hard(&self) -> usize`, `fn render(&self) -> String` (text for the repair prompt), and `fn to_json(&self) -> String` that prints exactly what `check.go --json` prints.
- [x] API: `pub(crate) fn check(text: &str) -> Report` (auto sentence mode, software glossary loaded). Not public outside the crate.
- [x] Tests: port the 27 tests of `check_test.go`. Golden test: for each corpus file, `to_json()` equals the committed Go output byte for byte.
- [x] Orchestrator: gate, commit `feat(lang): Rust port of the controlled-English checker`.

## P2 — a separate log for a second call

### Task 2.1 — `Request.log`
Files: `executor/subprocess.rs:93,478` (+ tests), the six adapters, `crates/rival/src/model_specs.rs:66`.
- [x] Failing test: a request with `log: Some(path)` writes the child output to `path` and leaves the session log unchanged.
- [x] Add `log: Option<&str>` to `Request`; `None` keeps today's behaviour. Pass it through `RunCall` and every adapter. Every existing caller passes `None`.
- [x] Orchestrator: gate, e2e, three-OS CI, commit `feat(executor): optional log file per provider call`.

## P3 — STE rules in every prompt

### Task 3.1 — reviewer and plan prompts
Files: `review/prompt.rs:55` (+ tests), `config.rs:450` (+ tests).
- [x] Rewrite the `writing_rules!` block: name ASD-STE100 Simplified Technical English, and give its core rules and the hedge table for review text (from the antislop-eng `SKILL.md`, adapted to findings). No word list.
- [x] Add the same block to `PLAN_REVIEW_PROMPT`.
- [x] Update the SHA-256 pins. A test checks that no prompt contains a dictionary entry line (for example `MAKE SURE (v)`).

### Task 3.2 — skill text
Files: `crates/rival-core/skills/*/SKILL.md` (step 5 rule), `skills/codex.md`.
- [x] The rule for host-written lines names STE and its rules. No word list.
- [x] Orchestrator: gate, e2e (prompt-echo scenarios may need new expected text), commit `feat(prompts): STE writing rules in every reviewer prompt`.

## P4 — check and one repair after each review

### Task 4.1 — the repair module
Files: `lang/repair.rs` (+ tests).
- [x] `pub(crate) fn needs_repair(r: &Report) -> bool` (any hard finding or word hit).
- [x] `pub(crate) fn review_text(out: &ReviewerOutput) -> String` and the same for `PlanOutput`: summary, then each finding's title, body, failure_scenario, suggestion.
- [x] `pub(crate) fn repair_prompt(json: &str, report: &Report) -> String`: STE strict task, hedge table, untrusted-data rule, the report, the review JSON, "reply with JSON only in the same shape".
- [x] `pub(crate) fn guard(old, new) -> Guarded` (review and plan): shape check (count, order, file, line, severity, category, confidence, plan rating) reverts all; facts check per text field (code spans, fences, `file:line`, paths, numbers as multisets) and the empty check revert one field.
- [x] Unit tests for each guard rule, with a fact change in one field and a shape change.

### Task 4.2 — wiring
Files: `crates/rival/src/{model_command,model_run,command_security}.rs`, `review/planrun.rs:126` (+ tests), `crates/rival-core/src/review/mod.rs` (export one entry point).
- [x] One entry point: `review::repair_language(ctx, cfg, sess, raw_log, run_again) -> Option<String>` returns the JSON line to append, or `None`. `run_again` is a closure that runs the same model at `low` (clamped to the provider; K3 stays `max`), read-only, with `log: Some(<session log>.repair.log)`, and the run timeout.
- [x] On `Some(line)`: append it to the session log, and build the printed result from the log plus the line. On `None`: nothing changes, nothing is printed about the pass.
- [x] Code review: `model_command.rs:253` path and `model_run.rs`. Security: `command_security.rs:361`. Plan: inside the per-model run of `planrun.rs`, so each model repairs its own block in parallel.
- [x] Tests with a fake provider: no hits → no call; hits → one call with low effort, read-only and the repair log; bad reply, shape change, provider error → original kept; one field with a changed number → only that field reverts; the session log ends with the repaired JSON and `result::parse_run_result` shows it; the session log never holds the repair transcript.

### Task 4.3 — e2e and docs
Files: `e2e/scenarios/review-language-{code,security,plan,clean}.yaml`, `README.md`, `CHANGELOG.md`.
- [x] Four scenarios: a code review, a security review and a dual plan review whose fakes return a flagged review and then a repaired one (two calls each); one clean review (one call).
- [x] README and CHANGELOG: one line each — reviews are edited into controlled technical English.
- [x] Orchestrator: gate, e2e, three-OS CI, commit `feat(review): one check and one repair call for review language`.

## Exit gate

- [x] fmt, clippy, workspace tests, e2e, the grep gate (the pattern must still pass: no `ste_` identifiers, the module is `lang`), three-OS CI.
- [x] Manual on Dell: one real Codex review and one real Claude review with the branch binary. Read the result, the session log and `<session>.repair.log`.
- [x] `/rival-codex review` of the branch against master. Verify every finding. Fix the confirmed ones.
- [x] PR, link it, CI green, merge.
- [x] As-built notes in `spec.md`, plan `done`, `git mv` the dir to `plans/done/`.

## Name check

- `lang::check`, `lang::Report`, `Report::{hard, render, to_json}` (Task 1.2) are used by `lang::repair` (Task 4.1).
- `Request.log: Option<&str>` (Task 2.1) is set only by the `run_again` closure (Task 4.2).
- `review::repair_language` is the only entry point the commands call (Task 4.2).
