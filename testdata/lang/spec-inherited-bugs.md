# Inherited bugs

**Date:** 2026-10-08
**Scope:** /home/kass/dev/rival (starts after `plans/2026-10-08-rust-only/` lands and before `plans/2026-10-08-review-language/`. The file:line citations are from master `2809e58`.)
**Status:** approved

## TL;DR

**P1 — security fixes.**
- What: the Claude Docker run no longer puts the auth token in the process arguments. Kimi runs with review tools in every mode except raw. The `.env` credential search stops at HOME also when HOME ends in `/`.
- Why: other local users can read process arguments (`ps`). An incorrect stop point reads `.env` files above HOME.
- You decide or do: nothing.
- Not done: no change to how you supply the token.

**P2 — one review parser.**
- What: the CLI and the TUI use the same code to find and decode the review answer. A plan with an empty summary is not shown as a clean plan.
- Why: today the command output and the TUI can show different results for the same log.
- You decide or do: nothing.
- Not done: no change to the review JSON schema.

**P3 — queue, scope and merge-request fixes.**
- What: `rival queue clear --force` keeps live running tickets. Git scope ignores inherited `GIT_DIR` settings. An encoded MR link is found. MR cancel kills the whole git process group. A queue I/O error shows its real text.
- Why: today each one gives an incorrect result.
- You decide or do: to stop a hung live run, kill it (TUI stop or `kill`). `--force` no longer does it.
- Not done: nothing else in the queue changes.

**P4 — install, timeout, Windows and telemetry.**
- What: skill files are written safely. Dangling skill links are removed. Oversized timeouts saturate. Windows drive paths work for Claude Docker. The unused Sentry telemetry is deleted.
- Why: a cut-off skill write stays broken until `--force`. Today the telemetry code sends nothing.
- You decide or do: nothing.
- Not done: the update check keeps its version compare (documented).

## Problem(s)

The Rust port kept these Go bugs on purpose, to make its output match Go (`plans/2026-10-01-rust-cli/plan-v2.10.md`, table "Known Go bugs"). The rust-only spec removes the reason. Each problem below is still in the code at `2809e58`.

1. **Docker token in process arguments.** `run_claude_docker_with` passes `-e ANTHROPIC_AUTH_TOKEN=<token>` (`crates/rival-core/src/executor/claude_docker.rs:158-159`). Any local user can read it in `ps` or `/proc/*/cmdline` for the full run.
2. **Kimi tool rule depends on one mode string.** `kimi_run_opts` gives full-auto permission to every mode except `review` (`crates/rival-core/src/executor/kimi.rs:115`). Today only raw and review runs reach it. A plan or security run on Kimi would get full-auto tools.
3. **Credential search can go above HOME.** `dotenv_key_from` stops when the cleaned directory equals the raw HOME (`crates/rival-core/src/config.rs:1234`). With `HOME=/home/kass/` the two values never match, so the search reads `.env` files above HOME.
4. **Two review parsers.** The CLI uses `review::parse` and `review::plan` (`crates/rival-core/src/review/parse.rs:25`, `review/plan.rs:42`). The TUI and Rival.app use `result` (`crates/rival-core/src/result.rs:117,285`). The two parsers differ in footer removal, hook-line removal, duplicate answers and JSON in prose. The same log can give two different results.
5. **Empty plan summary looks clean.** `parse_plan_output` accepts a blank summary (`review/plan.rs:42`), and the formatter skips it (`review/format.rs:269`). A rating with no findings and no summary shows as a clean plan.
6. **Force clear breaks the run limit.** `Manager::clear(true)` deletes running tickets too (`crates/rival-core/src/queue/mod.rs:352`). A live run does not create its ticket again. Thus a second run can start at the same time as the live run.
7. **Git scope inherits repository overrides.** `git_cmd` passes the caller's `GIT_DIR` and related variables (`crates/rival-core/src/gitscope/mod.rs:119`). In a git hook, scope detection can read the incorrect repository.
8. **Encoded MR link missed.** `mergerequest::contains` checks the raw text (`crates/rival-core/src/mergerequest/mod.rs:45`). `%2F-%2Fmerge_requests%2F42` falls through to a plain review of the checkout.
9. **MR cancel kills one process only.** `output` and `reap` kill the direct git or glab child (`mergerequest/mod.rs:726,836`). A grandchild that holds the pipe delays the return.
10. **Queue I/O error shown as cancel.** `wait_with_manager` reports every non-timeout failure as `cancelled while queued` (`crates/rival-core/src/review/slots.rs:153-155`). The real error is lost.
11. **Cut-off skill write.** `write_file` truncates, then writes (`crates/rival/src/install.rs:350`). A crash leaves the new version header over cut-off text. Later installs skip that file as current.
12. **Dangling skill links stay.** Deprecated-skill cleanup uses `fs::metadata`, which follows the link and fails (`crates/rival/src/install.rs:233`). The rust-only spec adds `rival-antislop` to the deprecated list, so this path gets real use.
13. **Oversized timeouts wrap.** `max_run_wait` multiplies without a limit (`crates/rival-core/src/config.rs:1300`). A very large `RIVAL_RUN_TIMEOUT` gives a negative wait. Then `rival wait` stops immediately.
14. **Windows drive path in Claude Docker.** The absolute-path check is `starts_with('/')` (`executor/claude_docker.rs:134`). `C:\repo` becomes `<cwd>/C:\repo`.
15. **Dead telemetry.** `recover_panic` does nothing (`crates/rival-core/src/telemetry.rs:115`). No code calls the Sentry capture (`telemetry.rs:119`). The `sentry` dependency (`crates/rival-core/Cargo.toml:18`) and the flush in `root.rs:422` do no work.

## Goals

1. Close the three security problems (Problems 1–3).
2. One parser for review answers in the CLI, the TUI and Rival.app input (Problems 4, 5).
3. Correct results for queue, scope and MR input (Problems 6–10).
4. Safe skill install, bounded timeouts, Windows Docker paths, no dead telemetry (Problems 11–15).
5. Remove the "Known Go bugs" table. Each row is fixed, dropped with a reason, or moved to a short "Known limits" list in `docs/runtime-reference.md`.

## Non-goals

- New features in the queue, the MR flow or the installer.
- A new telemetry system.
- Changes to the review JSON schema.
- The review-language pass.

## Triage of the 23 rows

| # | Row in the old table | Decision | Problem |
|---|---|---|---|
| 1 | No codex header: whole log scanned | Fixed by `8985d82`. Drop the row. A test covers a codex log with no banner. | 4 |
| 2 | Codex double answer not deduped | No effect on the result today. Closed by the one parser. | 4 |
| 3 | Two parsers | Fix. | 4 |
| 4 | Force clear of a running ticket | Fix: skip live running tickets. | 6 |
| 5 | Git scope inherits overrides | Fix. | 7 |
| 6 | File-list merge keeps duplicates inside one input | Fix with one line. Today git prints each path once. One `seen` line removes the doubt. | — |
| 7 | Docker token in process arguments | Fix first. | 1 |
| 8 | Windows drive path in Claude Docker | Fix. | 14 |
| 9 | Kimi tool rule only for `review` | Fix. | 2 |
| 10 | Queue I/O error shown as cancel | Fix. | 10 |
| 11 | Empty plan summary accepted | Fix. | 5 |
| 12 | Encoded MR marker missed | Fix. | 8 |
| 13 | MR cancel kills one process | Fix. | 9 |
| 14 | Codex antislop skill text | Gone with the rust-only spec P1. | — |
| 15 | Dangling deprecated-skill links | Fix. | 12 |
| 16 | Removal count lost on partial cleanup | Keep. Document in Known limits. | — |
| 17 | Cut-off skill write | Fix. | 11 |
| 18 | Panic wrapper never captures | Delete telemetry. | 15 |
| 19 | Errors skip the telemetry flush | Delete telemetry. | 15 |
| 20 | Padded version compare | Keep. Document in Known limits. A part of more than 3 digits compares incorrectly. A numeric compare would show update notices to `dev` builds. | — |
| 21 | Oversized timeout wraps | Fix. | 13 |
| 22 | Duration parser wrap | Fixed in the rust-only spec P3b. | — |
| 23 | Credential walk vs raw HOME | Fix. | 3 |

## Fixes

| Problem | Fix |
|---|---|
| 1 | Pass only the name: `-e ANTHROPIC_AUTH_TOKEN`. Put the value in the child environment (`Request.env`). Docker copies it from there. |
| 2 | Full-auto only for `raw` mode. All other modes get the review options. This includes task modes. |
| 3 | Compare the cleaned directory with the cleaned HOME (`paths::clean`). |
| 4 | The CLI uses `result::final_answer` and one shared decoder for review, security and plan payloads. `review::parse::final_answer` and the second decoder go away. The CLI keeps its own formatter. |
| 5 | A blank summary makes a plan payload invalid, as it does for a review payload. The output shows UNPARSED with the raw text. |
| 6 | `clear(true)` removes dead tickets and live waiting tickets. It keeps a running ticket if the process of that ticket is alive. The command prints how many it kept. |
| 7 | `git_cmd` uses `gitscope::repository_env(cfg.environ())`. |
| 8 | Percent-decode the input once before `contains`. |
| 9 | Run git and glab in their own process group. Kill the group on cancel, with the bounded drain of the provider executor. |
| 10 | Match the I/O error and print it: `queue wait failed: <error>`. |
| 11 | Write to a temp file in the same directory, then rename. |
| 12 | Use `symlink_metadata`. |
| 13 | Use saturating arithmetic in `max_run_wait` and `run_timeout_budget`. |
| 14 | Use `Path::is_absolute` for the mount source. |
| 15 | Delete `telemetry.rs`, its tests, its call sites in `root.rs` and the `sentry` dependency. Remove telemetry text from the README. |

## File-level changes

| File | Change |
|---|---|
| `crates/rival-core/src/executor/claude_docker.rs` | Token by name only. `Path::is_absolute` for the mount. |
| `crates/rival-core/src/executor/kimi.rs` | Full-auto only for `raw`. |
| `crates/rival-core/src/config.rs` | Compare with cleaned HOME. Saturating timeout math. |
| `crates/rival-core/src/result.rs`, `review/{parse,plan,types,security,format}.rs` | One answer finder and one decoder. Blank plan summary is invalid. |
| `crates/rival/src/model_command.rs`, `command_security.rs`, `command_plan.rs` | Call the shared parser. |
| `crates/rival-core/src/queue/mod.rs`, `crates/rival/src/queue_sessions.rs` | `clear(true)` keeps live running tickets and reports the count. |
| `crates/rival-core/src/gitscope/mod.rs` | `repository_env` in `git_cmd`. One `seen` line in `merge_file_lists`. |
| `crates/rival-core/src/mergerequest/mod.rs` | Percent-decode in `contains`. Process-group kill and bounded drain. |
| `crates/rival-core/src/review/slots.rs` | Print the queue I/O error. |
| `crates/rival/src/install.rs` | Temp file and rename. `symlink_metadata`. |
| `crates/rival-core/src/telemetry.rs`, `telemetry/tests.rs`, `crates/rival-core/Cargo.toml`, `crates/rival/src/root.rs`, `Cargo.lock` | Delete telemetry and the `sentry` dependency. |
| `docs/runtime-reference.md` | New "Known limits" list with rows 16 and 20. |
| `plans/2026-10-01-rust-cli/plan-v2.10.md` | No edit: it is a closed plan. The "Known Go bugs" table stays as history. |
| `e2e/scenarios/` | New scenarios for force clear, the encoded MR link and a blank plan summary. Record new output where the parser change affects it. |
| `README.md`, `CHANGELOG.md` | One CHANGELOG line per user-visible fix. Remove telemetry text from the README. |

## Tests

**Unit**
- Docker: the argv has `-e ANTHROPIC_AUTH_TOKEN` and no token value. The child environment has the value.
- Kimi: raw gets full-auto. Review, plan and security modes get review options.
- Credential walk: `HOME=/x/` and `HOME=/x` both stop at `/x`.
- Parser: every existing case in `result/tests.rs` and `review/parse/tests.rs` gives the same answer through the shared parser, or a documented new one. Add a codex log with no banner and no `exec` line.
- Plan: a blank summary is invalid.
- Queue: `clear(true)` keeps a live running ticket and removes a dead one.
- Git scope: an inherited `GIT_DIR` does not change scope detection.
- MR: the encoded link is found. Cancel kills a grandchild that holds the pipe, within the drain bound.
- Slots: a queue I/O error prints its text.
- Install: a write that fails before the rename leaves the old file. A dangling deprecated link is removed.
- Timeout: a very large `RIVAL_RUN_TIMEOUT` gives the maximum wait, not a negative one.
- Docker on Windows: `C:\repo` mounts as `C:\repo`.

**Integration**
- All e2e scenarios pass, with the new ones.
- Three-OS CI.

**Manual on Dell**
- `rival queue clear --force` while a real run is live: the run's ticket stays.
- A Claude Docker run: `ps` shows no token.

## Failure modes & decisions

| Failure or choice | Behavior |
|---|---|
| A hung live run blocks the queue | `--force` keeps it. Kill the process (TUI stop or `kill`). Then `clear` removes the dead ticket. |
| A log that the old CLI parser accepted and the shared parser rejects | Shows UNPARSED with the raw text. The CHANGELOG lists the change. |
| A plan with a rating but a blank summary | Shows UNPARSED, not a clean plan. |
| A `.env` file above HOME held a key that someone used | It is no longer read. The key must be in the environment or in a `.env` at or below HOME. |
| Git scope inside a hook depended on `GIT_DIR` | Scope uses the working directory's repository. |
| Telemetry removal | Nothing is lost: nothing was sent. |

## Out of scope

- Rows 16 and 20: kept and documented.
- The review-language pass.
- A queue redesign.

## Rollout

- **P1** — Problems 1–3. One commit. Gate: fmt, clippy, workspace tests, e2e, three-OS CI, the manual Docker check.
- **P2** — Problems 4–5. One commit. Gate: the checks above, the parser cases from both test sets.
- **P3** — Problems 6–10. One commit. Gate: the checks above, the manual force-clear check.
- **P4** — Problems 11–15 and the Known limits list. One commit. Gate: the checks above.

## As-built notes

Delivered on branch `feat/inherited-bugs` (2026-10-08). Deviations from the text above:

- P2: the shared decoder changes 22 bare-JSON test inputs to the TUI's result; every fixture log gives the same output. Three of them are judgement calls: in a review run a payload with a `rating` key counts as a plan payload; a plan payload before a review payload wins; an unrelated `1e999999` number makes the object invalid JSON for the scan.
- P2: the blank-summary rule lives in the shared decoder, so the TUI and the CLI both reject it. The Codex review of the branch found the first version (CLI only).
- P3: `queue clear --force` help text changed to say that live running tickets stay. `merge_file_lists` lost its early returns to trim and dedupe a single list the same way.
- P3: MR git and glab now start through the provider executor. A failed kill reports `error sending signal to Cmd: …`.
- P4: a rewritten skill file gets mode 0644 minus the umask; it no longer keeps its old mode. `RIVAL_NO_TELEMETRY` and `DO_NOT_TRACK` are gone with telemetry. Claude Docker keeps the leading-`/` check, because `/repo` is not absolute on Windows.
- Manual Dell checks not run: no Claude Docker token on Dell. The e2e scenario `queue-clear-force-keeps-live` and the argv tests cover the two behaviours.
