# Parity scenario coverage (Gate P3)

Every command and flag of `plans/2026-10-01-rust-cli/cli-surface.md`, mapped to the scenarios in `parity/scenarios/` that exercise it.

- ✓ = success path, ✗ = failure path.
- "help" = structural check of clap help (`help-every-command`, `root-help-and-errors`). clap help and completion text may differ from cobra under the approved contract, so these checks match the usage line and every flag spelling, not the full text.
- Every other output is compared exactly.

## Phase boundary

- `tui`: only the parser and `tui --help` are covered here. The interactive dashboard is Task 4.5 and Gate P4. Nothing at Gate P3 is TUI acceptance.
- Telemetry: the runner always sets `RIVAL_NO_TELEMETRY=1`, so no scenario initializes Sentry. Opt-out rules, client options and the no-automatic-capture rule are unit-tested (`rival_core::telemetry::tests`, `root::tests::only_a_normal_return_reaches_the_telemetry_flush`).

## Root and help

| Command / flag | ✓ | ✗ |
|---|---|---|
| `rival` (no args: banner + usage) | root-help-and-errors | — |
| `rival -h, --help` | root-help-and-errors (help) | — |
| unknown command / unknown flag | — | root-help-and-errors |
| `help [command]` | root-help-and-errors (help) | unknown topic: unit test `root::tests::help_command_prints_help_or_an_unknown_topic` |
| `version` | version, update-check-* | — |

## command

| Command / flag | ✓ | ✗ |
|---|---|---|
| `command` (prints help), `-h` | root-help-and-errors (help) | root-help-and-errors (`command extra`) |
| `--detach` | detach-wait-success, detach-wait-failure, detach-wait-crash, detach-wait-timeout, detach-redirect-unlink | detach-wait-crash |
| `command codex` | executor-codex-success, mr-review-success | executor-codex-failure, executor-codex-quota, executor-codex-missing-runtime, executor-codex-auth-unavailable, mr-* refusals, mr-checkout-fetch-failure |
| `command codex --no-queue`, `--workdir` | command-codex-no-queue-workdir | workdir-missing |
| `command claude` | executor-claude-success, executor-claude-docker-success | executor-claude-failure, executor-claude-quota, executor-claude-missing-runtime, executor-claude-docker-failure, executor-claude-docker-quota |
| `command claude --no-queue`, `--workdir` | command-claude-no-queue-workdir | workdir-missing |
| `command grok` | executor-grok-success | executor-grok-failure, executor-grok-quota, executor-grok-missing-runtime, executor-grok-missing-auth |
| `command grok --no-queue`, `--workdir` | command-grok-no-queue-workdir | workdir-missing |
| `command k3` | executor-k3-success | executor-k3-failure, executor-k3-quota, executor-k3-missing-runtime, executor-k3-missing-key |
| `command k3 --no-queue`, `--workdir` | command-k3-no-queue-workdir | workdir-missing |
| `command antislop` (default codex + claude) | antislop-default-dual | — |
| `command antislop -m, --model` | antislop-claude-structured | antislop-model-conflict |
| `command antislop --effort` | antislop-no-queue-workdir (native `--effort low` reaches claude and the session) | antislop-effort-conflict |
| `command antislop --no-queue`, `--workdir` | antislop-no-queue-workdir | workdir-missing |
| `command plan` (default codex) | plan-codex-structured | plan-codex-quota-final-answer, plan-missing-file |
| `command plan -m, --model` (codex,claude) | plan-dual-models | — |
| `command plan --effort` | plan-dual-models | plan-effort-conflict |
| `command plan --no-queue`, `--workdir` | plan-no-queue-workdir | workdir-missing |
| `command security` | security-k3-structured | security-k3-nonzero-exit, security-missing-key |
| `command security --which` | security-which-ready | security-which-missing-key |
| `command security --no-queue`, `--workdir` | security-no-queue-workdir | workdir-missing |
| every `command *` leaf `--help` | help-every-command (help) | — |

Concurrent reviewers (antislop-default-dual, plan-dual-models) bind each session file by its `cli` field (`expect.files` glob/where/bind). The checks tie the start event, the file name, the model, the result and the shared group id to the same reviewer, whichever starts first. `home_files` proves the queue ticket was released (only `.rival/queue/.lock` remains).

## run

| Command / flag | ✓ | ✗ |
|---|---|---|
| `run` (prints help) | root-help-and-errors (help) | root-help-and-errors (`run extra`) |
| `run claude --prompt-stdin` | queue-contention, run-claude-flags, sessions-list | run-claude-flags (missing mode, empty prompt) |
| `run claude --effort` | run-claude-flags | run-claude-flags (invalid effort) |
| `run claude --review` | run-claude-flags | — |
| `run claude --no-queue`, `--workdir` | run-claude-flags, sessions-list | workdir-missing |
| `run grok --prompt-stdin`, `--effort`, `--no-queue`, `--workdir` | run-grok-flags | workdir-missing |
| `run grok --review` (ultra clamps to high, read-only sandbox) | run-grok-flags | — |
| `run k3 --prompt-stdin`, `--no-queue`, `--workdir` | run-k3-flags | workdir-missing |
| `run k3 --review` | run-k3-flags | run-k3-flags (`--effort` is not a k3 flag) |
| every `run *` leaf `--help` | help-every-command (help) | — |

## install

| Command / flag | ✓ | ✗ |
|---|---|---|
| `install --target claude` | install-claude-fresh-then-current | — |
| `install --target codex --force` | install-codex-force | — |
| `install --target all` (prompts) | install-all-prompts | — |
| `install --target <bad>` | — | install-unknown-target |
| `install --target auto` (default) | update-already-latest, update-brew-upgrade (child `install --force --target auto`) | — |

`install` with the host's own `/Applications/Codex.app` cannot be isolated by the runner. Every scenario that reaches `auto` puts the `codex` fake on `PATH`, so detection is deterministic. The injected-applications-dir cases are unit-tested.

## queue and sessions

| Command / flag | ✓ | ✗ |
|---|---|---|
| `queue` (empty) | queue-empty, queue-clear, queue-contention | root-help-and-errors (unknown flag) |
| `queue` (live ticket listing) | queue-contention | — |
| `queue clear` | queue-clear | — |
| `queue clear --force` | queue-clear, queue-contention | — |
| `sessions` (empty) | sessions-empty | — |
| `sessions` (listing, newest first, engine labels, `running...`) | sessions-list | — |
| `sessions --active` | sessions-list | — |
| `sessions --recent` | sessions-list (2, 3, 0, combined with `--active`) | sessions-list (`--recent x`) |

Queue read and clear errors (`read queue: …`, `clear queue: …`) are unit-tested (`queue_sessions::tests::queue_errors_are_wrapped`).

## update and the release check

| Command / flag | ✓ | ✗ |
|---|---|---|
| `update` (already latest: forced skill refresh) | update-already-latest | — |
| `update` (brew upgrade + skills from the upgraded binary) | update-brew-upgrade | — |
| `update` (upgrade fails, reinstall works) | update-brew-reinstall-fallback | — |
| `update` (release API error) | — | update-fetch-failure |
| `update` (no brew) | — | update-brew-missing |
| `update` (reinstall fails) | — | update-brew-reinstall-failure |
| `update` (`brew --prefix` fails, prefix without `bin/rival`) | — | unit test `update_cmd::tests::update_error_branches` (the runner's brew guard refuses unsafe prefixes by design) |
| background release check: notice | update-check-notice-source-compat (canned `vzzz` tag, labelled source-compatibility) | — |
| background release check: numeric release on a dev build, fresh cache | update-check-numeric-release-quiet | — |
| background release check: stale cache, HTTP error, opt-outs | update-check-stale-cache-and-errors | update-check-stale-cache-and-errors |

Numeric ordering (`1.2.3` < `1.10.0`, `v` prefixes, two-part versions) is unit-tested in `rival_core::update::tests`. A release build ignores `RIVAL_UPDATE_API`: `update::tests::release_builds_ignore_the_api_override` (run it with `cargo test --release`).

## wait

| Command / flag | ✓ | ✗ |
|---|---|---|
| `wait <session-id>...` | wait-session-ids | wait-invalid, wait-session-ids |
| `wait --log` | detach-wait-success, detach-wait-failure, detach-redirect-unlink | detach-wait-crash, wait-invalid |
| `wait --poll` | detach-wait-timeout, wait-session-ids | wait-invalid |
| `wait --timeout` | detach-wait-timeout, wait-session-ids | wait-invalid |

## completion

| Command / flag | ✓ | ✗ |
|---|---|---|
| `completion` (help) | completion-scripts (also `completion tcsh`: cobra prints help, exit 0, because the non-runnable check precedes NoArgs) | — |
| `completion bash`, `zsh`, `fish`, `powershell` | completion-scripts (structural) | — |
| `--no-descriptions` | completion-scripts (structural) | — |

## MR checkout cleanup

`expect.dirs` asserts that `<ROOT>/tmp` (TMPDIR) exists and is empty after the run:

- mr-review-success: the checkout was created, used and removed.
- mr-checkout-fetch-failure: the checkout was created, the fetch failed, and Prepare removed it.
- mr-glab-wrong-mr, mr-glab-wrong-project, mr-glab-stale-diff-refs, mr-glab-api-failure, mr-no-matching-remote, mr-raw-prompt-rejected: no checkout was ever created; tmp stays empty.
- run-grok-flags: the grok prompt temp file is removed.

No scenario covers cancellation (SIGINT/SIGTERM) during an MR fetch. It is unit-tested (`cancel_during_fetch_removes_the_checkout`, Task 2.8).
