# Rust-only rival

**Date:** 2026-10-08
**Scope:** /home/kass/dev/rival (branch `feat/rust-only` from master `2809e58`)
**Status:** approved

## TL;DR

**P1 — remove `rival-antislop`.**
- What: delete the code-slop review: command, skill, prompts, five scenarios and the `antislop` session mode.
- Why: the language pass in the next spec replaces it, and two "antislop" names would confuse users.
- You decide or do: nothing.
- Not done: no replacement in this spec. Old `antislop` sessions show as plain runs.

**P2 — remove the first word check.**
- What: delete `ste_rewrite`, the word list `ste.json`, `review/ste.rs` and `ste_fix.rs`. Remove every STE mention from code and docs.
- Why: the next spec replaces it with a full port of the antislop-eng checker.
- You decide or do: nothing.
- Not done: reviews get no word check until the next spec ships. The reviewer prompt keeps its plain-writing rules.

**P3 — drop Go compatibility.**
- What: replace Go-compat code with plain Rust: Go JSON encoding, Go error text, Go Unicode tables, Go Windows path rules. Remove `licenses/Go-LICENSE`, Go comments and `go_*` test names.
- Why: rival is Rust-only, and the Go layers cost about 2,400 lines plus 212 call sites.
- You decide or do: accept that error messages and session-file bytes change. Old session files still load.
- Not done: the other preserved Go bugs stay. They get their own spec. One is fixed here: a duration overflow is now an error.

**P4 — rename the scenario harness.**
- What: rename `parity/` to `e2e/`, remove the Go-parity text, and record new expected output from the new binary.
- Why: no Go source is left to be "in parity" with.
- You decide or do: nothing.
- Not done: the harness stays Python.

## Problem(s)

1. **A second "antislop" feature blocks the language work.**
   - The code-slop review is a full command (`crates/rival/src/command_antislop.rs`, 172 lines), a skill (`crates/rival-core/skills/rival-antislop/SKILL.md`) and a prompt (`crates/rival-core/src/config.rs:527`).
   - It also has its own session mode (`crates/rival-core/src/session/mod.rs:35`) with branches in the TUI, sessionview and Rival.app (`app/Sources/RivalKit/Grouping.swift:171`).
   - The user wants it removed.
2. **The first word check is a dead end.**
   - `ste_rewrite` (`crates/rival-core/src/config.rs:745`) drives `ste_fix::refine` (`crates/rival/src/ste_fix.rs:36`). It uses a word list cut down to 75 KB (`crates/rival-core/src/review/ste.rs:21`).
   - The user wants a full port of the antislop-eng checker instead, and no STE names in code.
3. **Go behavior is copied on purpose.** About 2,400 lines exist only to match Go:
   - `gostd.rs`: 977 lines.
   - `gojson.rs`: 895 lines.
   - `gostd_tables.rs`: 545 lines.
   - `winpath.rs`: Go Windows path rules.
   
   They have 212 call sites in 55 files. Session files use Go's HTML-safe JSON escapes (`crates/rival-core/src/gojson.rs:19`). The copied Unicode tables force `licenses/Go-LICENSE` into every release archive (`.goreleaser.yaml:68`).
4. **Go names are everywhere.**
   - 1,617 "Go `x`" doc references and 288 "as Go does" comments in `crates/`.
   - `go_*` test names.
   - A scenario harness that says its expected output "is written from the Go source and Go tests" (`parity/README.md:3`).

## Goals

1. Remove the code-slop review and its session mode fully (Problem 1).
2. Remove the first word check and every STE mention outside prompts (Problem 2).
3. Remove every Go-compat module and every Go-only behavior, except the preserved Go bugs (Problem 3).
4. Continue to read session, queue and cache files that the Go and Rust v4 binaries wrote (Problem 3).
5. Remove every Go reference from code, tests, scripts, CI, release config and current docs (Problem 4).
6. Keep all 84 end-to-end scenarios, renamed, with expected output from the new binary (Problem 4).

## Non-goals

- Fix the 22 bugs in the "Known Go bugs" table (`plans/2026-10-01-rust-cli/plan-v2.10.md`). They get their own spec.
- Add the language pass. That is the next spec.
- Port the Python scenario harness to Rust.
- Rewrite history: released CHANGELOG entries for v4.x and older keep the word "Go" where they describe what shipped.
- Change the user-facing duration syntax (`30m`, `1h30m`, `500ms`) of `RIVAL_RUN_TIMEOUT` and the `--timeout` flags.

## Removal: rival-antislop

These parts go:

| Part | Where |
|---|---|
| Command | `CommandAntislop` (`crates/rival/src/tree.rs:28`). `command_antislop.rs` and its tests. Dispatch in `root.rs`. |
| Prompt and effort | `ANTISLOP_CODE_PROMPT` (`config.rs:527`). `DEFAULT_ANTISLOP_EFFORT` (`config.rs:49`). The antislop arm of the effort lookup (`config.rs:1124`). |
| Session mode | `MODE_ANTISLOP` (`session/mod.rs:35`) and its arm in `is_task_mode` (`session/mod.rs:42`). The sessionview group kind, TUI label `slop` (`tui/session_list.rs:110`) and Rival.app `Grouping.swift:171,181`. |
| Doc review path | The antislop branch of `DocReview` in `review/planrun.rs`. `format_antislop_result` in `review/plan.rs`. |
| Skill | `crates/rival-core/skills/rival-antislop/`. `rival-antislop` moves from `NAMES` to `DEPRECATED` (`skills.rs:19,34`), so `rival install` deletes old copies. |
| Scenarios | five `antislop-*.yaml` files. Their lines in help and install scenarios go too. |
| Docs | README, `docs/codex-skills.md`, `docs/runtime-reference.md`, `docs/ai-code-review-patterns.md`. |

An old session file with `"mode": "antislop"` still loads. It shows as a plain finished run with no special label or group.

## Removal: first word check

- Delete `crates/rival-core/data/ste.json`, `review/ste.rs` and its tests, `crates/rival/src/ste_fix.rs` and its tests.
- Delete the `ste_rewrite` config key and its getter and test (`config.rs:745`).
- Delete the `ste_*` exports in `review/mod.rs`.
- Delete the `refine` calls in `model_command.rs:170` and `model_run.rs:197`.
- Remove the README lines 507 and 518 and CHANGELOG line 21.
- Rename "ASD-STE100" in CHANGELOG line 38 to "plain-English writing rules".
- Keep the reviewer prompt's writing-rules block (`review/prompt.rs:56`) and its SHA-256 pins. The next spec rewrites that text.
- Keep the fixes that came with the word check, because they are not word-check code:
  - Claude takes `read_only` from its caller (`executor/claude.rs:39`).
  - The `rival run` stdout mirror (`crates/rival/src/mirror.rs`).
  - Docker container cleanup.
  - The rule that a codex transcript with no answer has no review (`review/parse.rs:192`).

## Go-compat replacement

Each Go-compat item gets a plain Rust replacement:

| Go-compat item | Replacement | Visible change |
|---|---|---|
| `gojson::marshal_indent` / `marshal` (session, ticket, update cache) | `serde_json::to_vec_pretty` / `to_vec` | `<`, `>`, `&`, U+2028, U+2029 are written as raw characters. They are not `\u003c`-style escapes. |
| `gojson` decode helpers (`decode_object`, `match_field`, `decode_string`, `decode_int`, `GoString`) | serde `Deserialize` with `#[serde(default)]` and field aliases | Keys match exactly. Go's case-insensitive key match is removed. |
| Go zero time `0001-01-01T00:00:00Z` (`gojson::zero_time`) | `Option<DateTime>`: Unset times are omitted on write. | The reader still maps the zero-time string to "unset". |
| `gojson::format_time` / `parse_time` | chrono RFC 3339 | Fractional seconds can print differently. Readers accept both forms. |
| `gostd::quote` (41 call sites) | Rust `{:?}` | Output changes. Non-ASCII and control characters get Rust escapes. |
| `gostd::os_error_text` (37 call sites), Go error forms such as `fork/exec <path>: <errno>` and `exit status 1` | `std::io::Error` and `ExitStatus` `Display` | Error text changes. This is true on every OS. |
| `gostd::to_lower`, `equal_fold` | `str::to_lowercase`, `eq_ignore_ascii_case` | Unicode edge cases only. |
| `gostd::parse_duration`, `format_duration` | a rival module `duration` with the same grammar | The syntax is kept on purpose (see Non-goals). One fix: an overflow is an invalid duration. Today two maximum parts wrap to 0, and `RIVAL_RUN_TIMEOUT=0` turns the run timeout off. |
| `gostd::slice_stable`, `open_file`, `is_not_exist*` | `slice::sort_by` (stable), `std::fs::File::open`, `ErrorKind::NotFound` | None. |
| `gostd_tables.rs` | `char` methods from std | Rare code points only. Some of them print differently. |
| `winpath.rs` (21 call sites) | `std::path::Path` on Windows | Windows lexical path edge cases change. Windows CI is the gate. |
| `licenses/Go-LICENSE` | deleted, with its entries in `.goreleaser.yaml:68-73`, `scripts/check_release_archives.py:35`, its test and README line 203 | Archives ship `LICENSE` and `README.md` only. |

File compatibility on read, with no migration step:

```
old file (Go or Rust v4)          new file (this spec)
"start_time": "0001-01-01T…Z"  →  key omitted
"group_id": "a\u003cb"         →  "group_id": "a<b"
both load with the same reader; the next save writes the new form
```

Rival.app decodes both forms. It already maps the zero time to "unset" (`app/Sources/RivalKit/Session.swift:25`). It needs a test for a file with the time keys omitted.

## Go-name cleanup

- Remove "Go `x`" doc references and "as Go does" comments. Where a comment explains a behavior, keep the reason in plain words.
- Rename the `go_*` tests by the behavior they check.
- Rename `gostd` and `gojson`. If a helper survives, give it a name for what it does, for example `duration`.
- Remove the Go text from CI, `docs/releasing.md`, the release skill and the README install text.

## Scenario harness

- Rename `parity/` to `e2e/`. Update the CI step names in `.github/workflows/ci.yml` and the README.
- Remove the Go-parity text from `e2e/README.md` and from the scenario `description` fields.
- Record new expected output from the new binary.
- Review each change of expected output by hand in the P3 commit. A change must trace to a row of the replacement table.

## File-level changes

| File | Change |
|---|---|
| `crates/rival/src/command_antislop.rs`, `command_antislop/tests.rs` | Delete. |
| `crates/rival/src/tree.rs`, `root.rs`, `main.rs` | Remove the `CommandAntislop` id, its help and dispatch. |
| `crates/rival-core/src/config.rs` | Remove the antislop prompt and effort, and `ste_rewrite`. Use `duration` instead of `gostd`. |
| `crates/rival-core/src/session/mod.rs` | Remove `MODE_ANTISLOP`. Use serde for read and write. Store times as `Option`. Read the old zero-time form. |
| `crates/rival-core/src/session/summary.rs`, `queue/ticket.rs`, `queue/mod.rs`, `update.rs` | Use serde instead of `gojson`. Continue to read old files. |
| `crates/rival-core/src/sessionview/group.rs`, `crates/rival/src/tui/session_list.rs` | Remove the antislop group kind and label. |
| `crates/rival-core/src/review/{plan,planrun,mod,parse,types}.rs` | Remove the antislop doc path and the `ste_*` exports. Use serde decoding. |
| `crates/rival-core/src/review/ste.rs`, `ste/tests.rs`, `crates/rival-core/data/ste.json` | Delete. |
| `crates/rival/src/ste_fix.rs`, `ste_fix/tests.rs` | Delete. Remove the calls in `model_command.rs` and `model_run.rs`. |
| `crates/rival-core/src/gostd.rs`, `gostd_tables.rs`, `gojson.rs`, `winpath.rs` | Delete. Callers use std, serde, chrono and the new `duration` module. |
| `crates/rival-core/src/duration.rs` | New. Parse and format the duration syntax, moved from `gostd.rs`. |
| 55 files that call `gostd`/`gojson` | Change the call sites (the replacement table gives each one). |
| `crates/rival-core/skills/rival-antislop/` | Delete. Add `rival-antislop` to `DEPRECATED` in `skills.rs`. |
| `crates/rival-core/tests/contract.rs`, `testdata/written/*.json` | Record new golden files. Keep `testdata/sessions/*` (old format) as read-compatibility input. |
| `app/Sources/RivalKit/Grouping.swift`, `Session.swift`, `app/Tests/RivalKitTests/*` | Remove the antislop mode. Add a decode test for omitted time keys and raw `<`. |
| `licenses/Go-LICENSE`, `.goreleaser.yaml`, `scripts/check_release_archives.py`, `scripts/test_check_release_archives.py` | Delete the license. Delete its archive entry and its check. |
| `parity/` → `e2e/` | Rename. Remove the Go-parity text. Record new expected output. Delete the five antislop scenarios. |
| `.github/workflows/ci.yml`, `README.md`, `CHANGELOG.md`, `docs/*.md`, `.claude/skills/rival-release/SKILL.md` | Update paths and names. Remove antislop, STE and Go text. Add one Unreleased CHANGELOG entry. |

## Tests

**Unit**
- `session`: load every file in `testdata/sessions/` (old Go format) and get the same field values as before.
- `session`: save, load and save again. Compare bytes.
- `session`: a zero-time string and an omitted key both give "unset".
- `queue/ticket`, `update`: the same checks for old ticket and cache files.
- `duration`: move the parse and format cases from `gostd`. Change the wrap regression test: an overflow now gives an invalid-duration error.
- Error text: each place that used `os_error_text` has one test for the new `io::Error` text.
- `skills`: `rival-antislop` is in `DEPRECATED` and not in `NAMES`. `rival install` removes an old copy.

**Integration**
- `crates/rival-core/tests/contract.rs` passes on the new golden files and on the old input files.
- All e2e scenarios pass (84 minus the 5 antislop ones).

**Swift (macOS CI)**
- Rival.app decodes the new golden files, the old ones, and a file with omitted time keys.

**Manual**
- On Dell, run `rival tui` over the real `~/.rival/sessions` (old files) without errors.
- On Dell, run one real Codex review with the new binary and read the session in the TUI.

**Gates on every phase**
- `cargo fmt --check`, `clippy -D warnings`, `cargo test --workspace`.
- e2e scenarios, release-script tests.
- Three-OS CI.
- No `go`/`Go`/`gostd`/`gojson`/`ste`/`antislop` match outside prompts, CHANGELOG history and `plans/`. One `grep` command in CI checks this.

## Failure modes & decisions

| Failure or choice | Behavior |
|---|---|
| Old session file with Go escapes or zero times | Loads. The next save writes the new form. |
| Old session file with `"mode": "antislop"` | Loads. It shows as a plain finished run. |
| Old file with a key in a different letter case (Go matched it) | The key is ignored. Its field gets the default. Accepted: rival never wrote such keys. |
| Old `rival-antislop` skill installed on a machine | `rival install` deletes it. It is in `DEPRECATED`. |
| A user script parses an error message | The text changes. Accepted. The CHANGELOG lists it. |
| A Windows path edge case changes with `std::path` | Windows CI must pass. A regression is fixed in P3, not hidden. |
| Rival.app is older than the CLI and reads a new file | Accepted. It decodes omitted keys as nil, and raw `<` is ordinary JSON. A test covers this. |
| An expected-output change in e2e that the replacement table does not explain | Treat it as a bug. Do not treat it as a new baseline. |

## Out of scope

- The 22 preserved Go bugs.
- The review-language pass (antislop-eng port, repair call, prompt rewrite).
- A Rust port of the e2e harness.
- A release or tag. The user releases when they decide.

## Rollout

- **P1** — remove rival-antislop. One commit. Gate: the checks above.
- **P2** — remove the first word check and the STE names. One commit. Gate: the checks above.
- **P3a** — JSON and file formats: serde for session, ticket and cache files, old-file reads, new golden files, Swift decode tests. One commit. Gate: the checks above and the manual Dell checks.
- **P3b** — errors, quotes and lower case: std error text, `{:?}` quotes, std case folding, the `duration` module. One commit. Gate: the checks above.
- **P3c** — Windows paths: `std::path` replaces `winpath.rs`. One commit. Gate: the checks above, Windows CI in particular.
- **P3d** — Unicode tables and license: delete `gostd_tables.rs` and `Go-LICENSE`, update the archive check. One commit. Gate: the checks above and one unpublished release snapshot.
- **P4** — rename `parity/` to `e2e/`, remove Go names and comments, add the CI grep gate. One commit. Gate: the checks above.
