# Rust-only rival Implementation Plan v1.0

**Date:** 2026-10-08
**Status:** in-progress
**Spec:** ./spec.md

**Goal:** remove `rival-antislop`, the first word check, all Go-compat layers and all Go names. Rename the scenario harness. Do not change behavior if the spec does not identify the change.

**Architecture:** removal first (P1, P2), then the Go-compat modules one group at a time (P3a–P3d), then names and the harness (P4). Each phase is one commit on the branch `feat/rust-only` in the worktree `.claude/worktrees/rust-only`. The exit gate for the full branch is one `/rival-codex review`, then one PR, then the merge.

**Tech Stack:** Rust 1.98, serde / serde_json, chrono and std. Python 3 for the scenario harness. GitHub Actions.

> For AI agent workers: do the tasks one at a time, in sequence. Use the checkboxes to monitor the work.

## Implementer rules (paste in every dispatch)

"Write the code and the unit tests that your task names. Do only the tests for your task (`cargo test -p <crate> <filter>`) and `cargo build`. Do not operate e2e scenarios, real reviewer CLIs, network calls or releases. Do not touch the `~/.rival` of the user. Each test must use a temporary HOME.

Do not commit. The orchestrator commits. If a file is not in `target/`, do not remove it with `rm`. Use `git rm`. Keep all behavior that the spec does not identify as changed."

## File map

**Remove:**
- `crates/rival/src/command_antislop.rs`, `crates/rival/src/command_antislop/tests.rs`
- `crates/rival-core/skills/rival-antislop/`
- `parity/scenarios/antislop-{claude-structured,default-dual,effort-conflict,model-conflict,no-queue-workdir}.yaml`
- `crates/rival/src/ste_fix.rs`, `crates/rival/src/ste_fix/tests.rs`
- `crates/rival-core/src/review/ste.rs`, `crates/rival-core/src/review/ste/tests.rs`, `crates/rival-core/data/ste.json`
- `crates/rival-core/src/gostd.rs`, `crates/rival-core/src/gostd_tables.rs`, `crates/rival-core/src/gojson.rs`, `crates/rival-core/src/winpath.rs` and their test modules
- `licenses/Go-LICENSE`

**Add:**
- `crates/rival-core/src/duration.rs` (+ `duration/tests.rs`)

**Change:** the 55 files that use `gostd`, `gojson` or `winpath` (Task 3a.1 has the list). Also change these files in `crates/`: `crates/rival/src/{tree,root,main,model_command,model_run,queue_sessions}.rs`, `crates/rival-core/src/{lib,config,skills}.rs`, `crates/rival-core/src/session/mod.rs`, `crates/rival-core/src/sessionview/group.rs`, `crates/rival-core/src/review/{mod,plan,planrun}.rs`, `crates/rival/src/tui/session_list.rs`, `crates/rival-core/tests/contract.rs`. Also change these other files: `app/Sources/RivalKit/{Grouping,Session}.swift`, `app/Tests/RivalKitTests/*`, `testdata/written/*.json`, `.goreleaser.yaml`, `scripts/check_release_archives.py`, `scripts/test_check_release_archives.py`, `.github/workflows/ci.yml`, `README.md`, `CHANGELOG.md`, `docs/*.md`, `.claude/skills/rival-release/SKILL.md`.

**Rename:** `parity/` → `e2e/`.

**Out of scope:** the inherited bugs (except the duration overflow), the pass for the review language, telemetry.

## Task 0 — baseline

- [ ] In the worktree: `cargo build --workspace --locked` → `Finished`.
- [ ] `cargo test --workspace --locked` → no test failure (record the counts).
- [ ] `cargo build -p rival --locked && python3 parity/run.py --bin target/debug/rival` → `84/84 scenarios passed`.
- [ ] Commit `plans/2026-10-08-*` (the three specs and this plan) with the message `docs(plans): rust-only, inherited bugs, review language`.

## P1 — remove rival-antislop

### Task 1.1 — command, prompt and effort
Files: remove `command_antislop.rs` (+ tests). Change `crates/rival/src/{tree,root,main}.rs`, `crates/rival/src/tree/tests.rs`, `crates/rival-core/src/config.rs`, `crates/rival-core/src/config/tests.rs`, `crates/rival-core/src/executor/{claude,kimi}.rs` (+ tests: remove the antislop-only test cases), `crates/rival-core/src/parser/{review.rs,tests.rs}`.
- [ ] Remove `CommandId::CommandAntislop`, its help entry and its dispatch.
- [ ] Remove `ANTISLOP_CODE_PROMPT`, `DEFAULT_ANTISLOP_EFFORT` and the antislop arm of the effort lookup.
- [ ] `cargo test -p rival tree` and `cargo test -p rival-core config` give no test failure. `rival command antislop` gives the unknown-command error.

### Task 1.2 — session mode and doc review path
Files: `crates/rival-core/src/session/mod.rs` (+ tests), `crates/rival-core/src/sessionview/group.rs`, `crates/rival-core/src/review/{plan,planrun,mod}.rs` (+ tests), `crates/rival-core/src/result.rs` (+ tests: rename the antislop-only test cases to plan), `crates/rival/src/tui/session_list.rs` (+ tests), `crates/rival/src/tui/model/list_tests.rs`, `crates/rival/src/queue_sessions/tests.rs`, `crates/rival/src/gitscope_helper.rs` (+ tests).
- [ ] Remove `MODE_ANTISLOP`, its arm in `is_task_mode`, the antislop group type, the `slop` label, the antislop `DocReview` branch and `format_antislop_result`.
- [ ] Add one test in `session/tests.rs`. In the test, rival reads a session file with `"mode": "antislop"` and shows it in the list as a usual run.
- [ ] `cargo test -p rival-core session sessionview review result` and `cargo test -p rival tui` give no test failure.

### Task 1.3 — skill, scenarios, app, docs
Files: `crates/rival-core/skills/rival-antislop/` (remove), `crates/rival-core/src/skills.rs` (+ tests), five antislop scenarios (remove), `parity/scenarios/{help-every-command,install-all-prompts,install-claude-fresh-then-current,install-codex-force,root-help-and-errors,update-already-latest,update-brew-reinstall-fallback,update-brew-upgrade,workdir-missing}.yaml`, `parity/coverage.md`, `app/Sources/RivalKit/Grouping.swift`, `app/Tests/RivalKitTests/GroupingTests.swift`, `app/Sources/RivalKit/ResultParser.swift` (doc comment), `README.md`, `docs/{codex-skills,runtime-reference,ai-code-review-patterns}.md`.
- [ ] Move `rival-antislop` from `NAMES` to `DEPRECATED`. Add a skills test that examines this move.
- [ ] Remove the antislop lines from the help and install scenarios.
- [ ] Remove the antislop mode from `Grouping.swift` and from its test.
- [ ] Orchestrator: do the full workspace tests and the scenarios (`79/79`). Then commit with the message `refactor: remove the rival-antislop code-slop review`.

## P2 — remove the first word check

### Task 2.1 — code
Files: remove `ste_fix.rs` (+ tests), `review/ste.rs` (+ tests), `data/ste.json`. Change `crates/rival/src/{main,model_command,model_run}.rs`, `crates/rival-core/src/review/{mod,prompt}.rs`, `crates/rival-core/src/config.rs` (+ tests).
- [ ] Remove `mod ste_fix`, the `refine` calls and the `ste_*` exports. If `WRITING_RULES` has no other user, remove it. Remove the `ste_rewrite` key, its getter and its test.
- [ ] Keep the `writing_rules!` block in the reviewer contract. Do not change the SHA-256 pins of the contract. Without a pin change, `cargo test -p rival-core prompt` must give no test failure.
- [ ] `cargo test --workspace` gives no test failure.

### Task 2.2 — docs
Files: `README.md` (lines about `ste_rewrite`), `CHANGELOG.md`.
- [ ] Remove the `ste_rewrite` README lines and the Unreleased CHANGELOG entry. In the v4.2.0 entry, replace "ASD-STE100" with "plain-English writing rules".
- [ ] `git grep -n -i "ste_\|ste100\|\bSTE\b" -- . ':!plans'` → no match. The orchestrator commits with the message `refactor: remove the first word check`.

## P3a — JSON and file formats

### Task 3a.1 — serde for session, ticket and cache files
Files: `crates/rival-core/src/session/{mod,summary}.rs` (+ tests), `crates/rival-core/src/queue/{mod,ticket}.rs` (+ tests), `crates/rival-core/src/update.rs` (+ tests), `crates/rival-core/src/review/{types,parse,format,security}.rs`, `crates/rival-core/src/sessionview/group.rs`, `crates/rival-core/src/executor/opencode.rs`, `crates/rival-core/src/mergerequest/mod.rs`, `crates/rival/src/{wait,queue_sessions}.rs`, `crates/rival/src/tui/{detail_view,preview,session_list}.rs` (+ tests).
- [ ] Write the files with `serde_json::to_vec_pretty` / `to_vec`. Keep the times that are not set as `Option<DateTime<FixedOffset>>`. If the value is `None`, do not write the field.
- [ ] Read the files with serde `Deserialize`, `#[serde(default)]` and a time deserializer. The time deserializer changes `0001-01-01T00:00:00Z` to `None`.
- [ ] First, write a test that gives a failure before the code change. Before the change, record the field values of each file in `testdata/sessions/`. The test must read each file and get the same field values.
- [ ] Do a round-trip test: write the file, read it, then write it again. The two written files must have equal bytes.
- [ ] `cargo test -p rival-core session queue update review` gives no test failure.

### Task 3a.2 — golden files and Swift
Files: `crates/rival-core/tests/contract.rs`, `testdata/written/*.json`, `testdata/expected.json`, `app/Sources/RivalKit/Session.swift`, `app/Tests/RivalKitTests/*`.
- [ ] Regenerate `testdata/written/*.json` with the ignored generator test (`cargo test -p rival-core --test contract regenerate_written_golden -- --ignored`). Examine each diff manually. Each diff must contain only escapes, zero times that are not written, and the change of the time format.
- [ ] Swift: add a decode test for a file that has no time keys and has a `<` without an escape. Keep the test for zero times in files from previous versions.
- [ ] Orchestrator: do the workspace tests and the scenarios. Then do the macOS CI for Swift. Commit with the message `refactor: plain serde JSON for session, queue and cache files`.

## P3b — errors, quotes, case, duration

### Task 3b.1 — the duration module
Files: add `crates/rival-core/src/duration.rs` (+ `duration/tests.rs`). Change `crates/rival-core/src/{lib,config}.rs`, `crates/rival/src/{tree,wait,queue_sessions}.rs`, `crates/rival-core/src/{review/planrun,review/slots,queue/mod,session/mod,sessionview/group}.rs`.
- [ ] Move `parse_duration` and `format_duration` to `duration::{parse, format}` with the same grammar and output.
- [ ] First, write a test that gives a failure before the code change. In the test, a duration with two maximum parts must give an invalid-duration error, not `0`. Change the regression test for the overflow into this test.
- [ ] Move the other parse and format test cases without a change. `cargo test -p rival-core duration` gives no test failure.

### Task 3b.2 — std error text, quotes and case
Files: all users of `gostd::{quote,os_error_text,errtext,to_lower,equal_fold,slice_stable,open_file,is_not_exist,is_not_exist_code}` (see the grep in the Task 0 notes), and their tests.
- [ ] `quote(s)` → `format!("{s:?}")`. `os_error_text(e)` → `e.to_string()`. Go `fork/exec <path>: <errno>` → `start <path>: <io error>`. Go `exit status N` → `ExitStatus` `Display`.
- [ ] `to_lower` → `str::to_lowercase`. `equal_fold` → `eq_ignore_ascii_case`. `slice_stable` → `sort_by`. `open_file` → `File::open`. `is_not_exist*` → `ErrorKind::NotFound`.
- [ ] If a test has a Go text as its expected value, change the value to the new text. Do not remove a test.
- [ ] Remove `gostd.rs` and its tests. Remove `pub mod gostd` from `lib.rs`.
- [ ] `cargo test --workspace` gives no test failure. Orchestrator: record new scenario output only for the error lines that changed. Examine each changed line. Commit with the message `refactor: std error text, quotes and case; duration module`.

## P3c — Windows paths

### Task 3c.1 — std::path for Windows
Files: `crates/rival-core/src/winpath.rs` (remove) and its 21 users. These users include `crates/rival-core/src/{paths,executor/process,executor/process/windows,executor/oscmd}.rs` and `crates/rival/src/{detach,workdir}.rs`.
- [ ] Replace each `winpath` call with `std::path::Path` / `PathBuf` operations.
- [ ] Keep the Windows test cases. Change an expected value only if `std::path` gives a different lexical result. For each such change, add a comment that names the test case.
- [ ] On Dell, `cargo test --workspace` gives no test failure. Orchestrator: push the branch. The Windows CI must give no failure. Commit with the message `refactor: std::path replaces the Windows path rules`.

## P3d — Unicode tables and license

### Task 3d.1 — tables and license
Files: `crates/rival-core/src/gostd_tables.rs` (remove it if Task 3b.2 did not remove it), `licenses/Go-LICENSE` (remove), `.goreleaser.yaml`, `scripts/check_release_archives.py`, `scripts/test_check_release_archives.py`, `README.md:203`.
- [ ] Remove `licenses/Go-LICENSE` from the archive file list and from `BUNDLED`.
- [ ] Change the tests of the release archives: the archives contain `rival`, `LICENSE` and `README.md`.
- [ ] Orchestrator: do the tests of the release scripts and `goreleaser check`. Make one snapshot without a release (`gh workflow run release.yml --ref feat/rust-only`). All six `snapshot-native` jobs must give no failure. Commit with the message `build: drop the Go license and tables`.

## P4 — names and harness

### Task 4.1 — rename the harness
Files: `parity/` → `e2e/` (`git mv`), `e2e/README.md` and the description in each `e2e/scenarios/*.yaml`. Also `.github/workflows/ci.yml`, `README.md`, the text in `scripts/` that refers to the harness, `.claude/skills/rival-release/SKILL.md`, `docs/releasing.md`.
- [ ] Do `git mv parity e2e`. Change the paths in the CI and in the docs.
- [ ] Remove "Go" from the README of the harness and from each scenario description.
- [ ] `python3 -m unittest discover -s e2e -p 'test_*.py'` gives no test failure. `python3 e2e/run.py --bin target/debug/rival` → `79/79`.

### Task 4.2 — Go names in code
Files: all files in `crates/` that have a doc text that refers to "Go", or a `go_` test name (approximately 133 files).
- [ ] Remove each text of the type "Go `x`" and each text of the type "as Go does". If a comment gives a cause, keep the cause, and write it in clear English.
- [ ] Rename each `go_*` test. Use a name that tells the behavior that the test examines.
- [ ] `cargo fmt --check`, `clippy -D warnings` and `cargo test --workspace` give no failure.

### Task 4.3 — the grep gate
Files: `.github/workflows/ci.yml`, `CHANGELOG.md`.
- [ ] Add a CI step: `git grep -n -I -E '\bGo\b|gostd|gojson|winpath|ste_|ste100|antislop' -- . ':!plans' ':!CHANGELOG.md'` must give no output. The pattern is case-sensitive. Thus, the pattern does not find the English verb "go". If a match is incorrect, add an explicit pathspec exclusion for it, with a comment.
- [ ] Add one Unreleased CHANGELOG entry. In the entry, tell that the change removes rival-antislop and the Go license. Tell that the error texts and the escapes in session files changed. Tell that a duration overflow is an error.
- [ ] The orchestrator commits with the message `chore: remove Go names; rename the scenario harness to e2e`.

## Exit gate

- [ ] Orchestrator: on the branch head, do fmt, clippy, the workspace tests, e2e and the tests of the release scripts. Do the CI on the three OS.
- [ ] Manual test on Dell: operate `rival tui` on the `~/.rival/sessions` of the user. Do one real Codex code review with the new binary. Read the code review in the TUI.
- [ ] Do `/rival-codex review` on the full branch against master. Examine each result of the code review. If a result is correct, repair the problem. Then do the gates again.
- [ ] Open the PR and link it. Wait for the CI. Merge the PR.
- [ ] Write the As-built notes in `spec.md`. The spec must agree with the code. Set the status of this plan to `done`. Do `git mv plans/2026-10-08-rust-only plans/done/`.

## Type and name check

- `duration::parse(&str) -> Result<i64 /* nanos */, String>` and `duration::format(i64) -> String` are the only new public names. Tasks 3b.1 and 3b.2 use these names.
- `MODE_ANTISLOP`, `CommandAntislop`, `ANTISLOP_CODE_PROMPT`, `DEFAULT_ANTISLOP_EFFORT`, `format_antislop_result`, `ste_rewrite`, `refine`, `gostd`, `gojson`, `winpath` have no user after their task.
