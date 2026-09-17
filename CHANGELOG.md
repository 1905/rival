# Changelog

All notable changes to **rival** are documented here. Versions follow [semver](https://semver.org/); every release is git-tagged.

## [Unreleased]

### Added — pinned GitLab MR reviews

- Review GitLab MR URLs at the API's exact base/head commits in a temporary checkout.
- Keep the caller's files and index intact, including runs with inherited Git variables.
- Include the patch for reviewers without shell access. Reject patches over 512 KiB.
- Keep K3 credential lookup in the caller's project. Support custom SSH ports.
- Reject MR URLs on commands that cannot resolve them. Document MR usage in both skill hosts.
- Preserve the current Codex installer and model defaults while merging PR #12.
- Validation: 637 tests passed with the race detector; build, vet, and lint passed. Live GitLab and paid model calls were not tested.

### Added — /rival-astra

A single-model runner for Astra (`gpt-6-astra`), verified against the codex
CLI before wiring. Astra is Sol's sibling on the same runtime, so it is also
selectable in a megareview with `-m astra`.

Default effort is **xhigh**, and two collisions specific to sharing a runtime
with Sol are covered by tests: `EngineLabel`'s `codex` adapter fallback would
otherwise label every Astra run "sol", and the command path's non-empty
effort fallback silently overrode the xhigh default until it was fixed.

A model that pins its own effort now outranks any surface-specific fallback,
so `-m astra` in a megareview reaches xhigh instead of the shared `high` —
verified against the session record. Explicit `-re` and `~/.rival/config.yaml`
still win over the pin. Codex preflight errors and the runtime banner take the
resolved label, so an Astra failure no longer reports Sol as broken.

### Changed — Fable reviews run 5.1

`FableModel` moves from `claude-fable-5` to `claude-fable-5-1`, verified
against the CLI before the constant changed. The `fable` label is unchanged,
so dashboards, session records, and `-m fable` all keep working.

### Removed — /rival-antislop-plan

Antislop no longer reviews plan documents. The skill, the `plan <path>` stdin
mode, and the plan prompt are gone; `rival command antislop` now only takes a
code scope. The skill is marked deprecated, so `rival install` removes copies
already on disk.

`/rival-antislop` on changed code is unaffected.

### Added — /rival-security, a dedicated security review

A new skill and command that hunt exploitable vulnerabilities across twelve
classes: injection, authorization and IDOR, authentication, crypto misuse,
path traversal, SSRF, deserialization, secret exposure, input validation,
CSRF, open redirect, and resource exhaustion. It does not report style or
ordinary logic bugs, which the existing reviews already cover.

The model is a config choice, not a fixed part of the skill:

```yaml
security:
  reviewer: k3      # or grok — k3 is the default
```

`k3` runs Kimi K3 on Moonshot; `grok` runs Grok 4.6 through OpenRouter with
`OPENROUTER_API_KEY`. Adding a model is now a registry entry rather than new
code, because the OpenCode adapter reads the provider, credential, selector,
and reasoning variant from data.

Two behaviors worth knowing. The run fails when the selected model has no
key, instead of falling back, because a security review that silently skips
is worse than one that refuses to start. And output that does not parse into
usable findings is reported as unusable rather than as a clean review, so
automation cannot mistake it for a pass.

### Changed — the default review roster is Sol alone

K3 left the default `/rival-review` roster. It stays selectable with `-m k3`,
where it now always carries the security lens rather than a second bug hunt.
So `-m sol,k3` gives you two different lenses, and the consilium judge is
told which reviewer used which, so a finding only the security reviewer could
have made is not discounted for lacking a second vote.

### Changed — the xAI Grok model is now grok-4.6

Verified against the CLI before shipping. Because its id textually overlaps
the OpenRouter model's label, runtime log normalization was rewritten to
substitute placeholders before expanding labels; previously it could turn
`grok-4.6-openrouter` into `grok-openrouter`.

### Changed — plan reviews default to xhigh, antislop runs at xhigh

`/rival-plan` and `/rival-plan-sol` now pin **xhigh** instead of ultra, and
antislop's default effort is **xhigh** for every model (explicit `-re` and
per-model `~/.rival/config.yaml` efforts still win; `RunDocReview` gained a
surface fallback-effort parameter). README moved the antislop section to the
top of the skills list.

### Added — antislop: quality-only slop & over-engineering review

New `rival command antislop` plus two skills, `/rival-antislop` (changed code
or an explicit scope) and `/rival-antislop-plan` (a plan/spec markdown
document). The model — Sol by default, `-m` accepts `sol`/`fable` — hunts
reuse/DRY violations, needless complexity, wrong-altitude fixes,
backward-compat hoarding, library reinvention, and AI-slop signatures
(comment slop, silent fallbacks, pass-through wrappers, speculative
generality; scope creep and ceremony padding for plans), then returns a
leanness rating (1-10) and a numbered cut list. Report-only, never hunts
bugs. Derived from Claude Code's built-in `/simplify` skill, adapted to
rival's detached, queued, read-only reviewer architecture. Internals:
plan-review runner generalized to `RunDocReview`, shared `buildDiffPreamble`
helper replaces two hand-rolled gitscope preamble copies, and
`isPlaceholderFinding` now strips any pipe-delimited echoed category enum.

### Breaking — model surface reduced to Sol, Fable, and K3

This change is queued for the next major release.

Rival now supports three curated models — Sol, Fable, and Kimi K3 — plus the
opt-in Grok provider added later in this release. The default review roster is
Sol + K3, both using the `bug_hunter` role, with successful consilium-judge
priority Sol → K3. Fable remains available for standalone review and Sol/Fable
plan review through its Claude Code runtime. Grok never joins the default
roster and participates only when `--model` names it.

Commands, selectors, executors, configuration, and dashboard presentation for
all other models were removed. This is a breaking command/configuration change:
scripts must use `sol` or `k3` for the review roster (or `grok` to opt into it),
and Fable's public terminal command is `rival run fable`.

Before upgrading, remove stale entries from `~/.rival/config.yaml`. The
`efforts` map accepts only these keys:

```yaml
efforts:
  sol: high
  kimi-k3: max
  fable: medium
  grok: high
```

Any key outside this set is invalid and stops Rival before it creates sessions,
touches the queue, or invokes a provider.

### Added — public Fable terminal runner

Fable now has a first-class `rival run fable` command for prompt-stdin and
single-model review workflows. It uses Claude Code as its runtime, shares the
same native-or-Docker authentication path as the Fable skills, and honors the
configured `fable` effort default.

### Added — Grok CLI provider (`/rival-grok`, opt-in megareview selector)

New provider backed by xAI's `grok` CLI running `grok-4.5`: `rival command grok`,
`rival run grok`, and the `/rival-grok` skill (detached plus background watcher,
the same async pattern as `/rival-k3`). Sessions record `cli: grok`,
`model: grok-4.5`; the TUI and web dashboard label them `𝕏 grok`.

Grok is an **opt-in** review model. The default roster is unchanged — Sol plus
Kimi K3 — and grok joins a review only when selected explicitly:

```bash
rival review --model grok src/api/
rival review --model grok,sol src/api/
```

As with every selector, an explicit `--model` list replaces the complete roster
rather than adding to it. When grok is the only successful reviewer it also
judges the consilium.

Effort: grok-4.5 exposes `low`, `medium`, and `high`, and `high` is its own
default. Rival's wider ladder clamps onto that menu — `xhigh`, `ultra`, and
`max` become `high`; `minimal` and `none` become `low` — and the clamped value
is what the session records, so `rival sessions` never reports an effort that
was not actually sent. An unrecognized level is still an error rather than a
silent downgrade. The configurable key is `grok` in the `efforts` map of
`~/.rival/config.yaml`.

Auth is the grok CLI's own `grok login` browser OAuth, verified by a preflight
that requires `grok` on `PATH` and `~/.grok/auth.json` to exist. **`XAI_API_KEY`
is deliberately not supported**: runs use the credentials you logged in with.

Environment hardening: `GROK_` and `XAI_` join `blockedEnvPrefixes`, so a
reviewed repository's `.env` (loaded by godotenv) cannot reconfigure the
authenticated grok runtime through proxy or base URLs, `GROK_HOME`, auth
helpers, or an injected API key. The preflight reads the real home directory
for the same reason — honoring `GROK_HOME` would check a location the run can
never use. The prompt is handed over in a temporary file rather than argv,
which also keeps it out of the process table.

Review sandbox: review-mode runs add `--sandbox read-only`, derived from the
session mode so the two can never disagree; raw prompts stay writable in the
workdir. Two caveats worth knowing:

- The grok CLI's built-in profiles **fail open** if a kernel sandbox is
  unavailable on the host. Rival passes the flag; it cannot guarantee the
  runtime enforced it. On macOS the applied profile is recorded in
  `~/.grok/sandbox-events.jsonl` (`"enforced": true` when seatbelt applied).
- The read-only profile still grants write access to a fixed allowlist that
  includes `~/.grok` and the system temp directories (`/tmp`, `/private/tmp`,
  `/var/folders/...`). A workdir located inside one of those paths is therefore
  writable during a review.
- Child-process network access is not blocked on macOS.

## [v3.23.0] — 2026-07-20

### Added — Kimi K3 via OpenCode (`/rival-k3`)

New standalone runner for Moonshot's Kimi K3, served through OpenCode's
built-in Moonshot AI provider (`moonshotai/kimi-k3`, 1M context):
`rival command k3`, `rival run k3`, and the `/rival-k3` skill (detached +
background watcher, same async pattern as `/rival-sol`). K3 is a thinking-only
model whose API accepts a single reasoning level, so every run pins
`--variant max` regardless of `-re`; sessions record `cli: opencode`,
`model: moonshotai/kimi-k3`, `effort: max` truthfully. `k3`/`kimi-k3` are also
selectable review models (`/rival-review -m k3`).

Auth: the Moonshot API key is read from `MOONSHOT_API_KEY` — process env first
(godotenv loads the invocation directory's `.env`), then a `.env` found by
walking up from `--workdir` toward root — and injected per run into the
`moonshotai` provider via `OPENCODE_CONFIG_CONTENT`, never written to any
on-disk config. Inherited Moonshot credentials stay blocked from child
environments.

Permissions: `review` runs under the same mechanical read-only
`OPENCODE_PERMISSION` sandbox as the megareview reviewers. Raw prompts run
full auto (edit/bash/web allowed; native file tools stay inside the workdir
via `external_directory: deny`) with known credential env vars stripped
(`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, the whole `AWS_*` family via prefix
matching, `GOOGLE_APPLICATION_CREDENTIALS`, GitHub/GitLab tokens, rival's own
provider keys) — `RunSubprocess` dropEnv entries ending in `_` match as
prefixes. Prompts travel via stdin like every other opencode run (no argv
size cap). TUI/web show `❯ kimi-k3`.

### Added — configurable per-model effort defaults

This release introduced the validated `efforts` map in
`~/.rival/config.yaml`. Explicit command or skill effort flags take precedence,
then the per-model setting, then Rival's built-in default. Invalid model names
or effort values fail before queue, session, or provider work begins. K3
remains pinned to its only supported value, `max`. The accepted model keys have
since changed; the current set is documented under Unreleased and in the
README.

### Documentation — Claude Code onboarding and project rationale

The README now opens with a concise explanation of why Rival is useful and adds
a dedicated Claude Code setup guide. It covers installing Rival, Codex, and
OpenCode; preferring Codex subscription login while documenting API-key auth;
obtaining a Moonshot API key; and placing `MOONSHOT_API_KEY` in a project
`.env` or the shell environment.

### Changed — rebuilt web dashboard

The local `rival server` dashboard is now self-contained, responsive, and
bounded for large histories. Session metadata is cached incrementally, list
responses are paginated and exclude prompts and process internals, prompt
loading is deferred, and log details return a UTF-8-safe 256 KiB tail instead
of entire multi-megabyte files.

The UI restores the canonical Rival logo and opens details in an accessible
side drawer. Queued, running, completed, failed, grouped, empty-log,
missing-log, and live-transition states render explicitly. Per-member status,
queue position, duration, exit code, errors, old-run deep links, focus
containment, mobile layout, and sequential group wall time are covered.

### Changed — review results are presented before follow-up work

All seven shipped skills now present a severity-count summary and the full
review output before making any follow-up tool calls. This prevents background
results from being dropped by the harness and keeps findings visible before
fixes or triage begin.

### Removed — retired legacy reviewer integrations

Obsolete reviewer commands, executors, selectors, dashboard icons, tests, and
documentation were removed, narrowing the maintained integration surface. The
current exact model roster is documented under Unreleased.

### Fixed — skill input heredoc was shell-injectable (all 7 skills)

Every shipped SKILL.md piped arguments through `cat <<"$DELIM"` with a
randomly generated `$DELIM` variable — but the shell performs no expansion on
a heredoc delimiter, so the real terminator was the literal string `$DELIM`
and the randomization was dead code: argument content containing a `$DELIM`
line would break out of the heredoc and execute as shell. Skills now create
the input file with the executing agent's **Write tool** instead of any shell
construct — file content never passes through the shell, eliminating the
entire delimiter/injection class rather than re-randomizing it.

### Fixed — reaper race stomping successful runs to failed

Sessions now record the owning rival process (`owner_pid`/`owner_pid_start`)
alongside the provider child PID, and `ReapOrphans` only reaps a session when
BOTH are dead. Previously, in the window between the provider CLI exiting and
rival writing the final status, a concurrent rival invocation's startup reap
saw "running + dead PID" and marked a successful run `failed: orphaned
(process dead)` (observed live with a detached K3 run). A live owner always
finalizes its own sessions; sessions from older releases keep the old
provider-only check.

## [v3.22.0] — 2026-07-16

### Changed — two concurrent queue slots by default

The cross-process FIFO queue now runs up to two logical reviews concurrently,
reducing long waits for independent single-model jobs while retaining bounded
fan-out and crash-safe slot recovery. Set `RIVAL_MAX_CONCURRENT=1` to restore
strict serialization; `RIVAL_NO_QUEUE` remains unnecessary for normal use.

## [v3.21.0] — 2026-07-12

### Added — paired `/rival-plan` skill

`/rival-plan <path>` runs Sol and Fable plan reviews in parallel at `ultra`
effort and shows both 1-10 ratings and finding sets. If one model is unavailable,
the other result is still returned and the missing model is reported as skipped.
The TUI and web dashboard group paired runs as plan reviews, show public model
names for every curated reviewer, and render matching model icons. Group members
stay in requested order with stable IDs; detail views reserve space for every
log, distinguish judge output, and surface per-model failures.

### Removed — obsolete standalone reviewer skill

An obsolete standalone reviewer skill is no longer embedded or installed.
`rival install` removes existing installed copies during an upgrade.

## [v3.20.0] — 2026-07-11

### Added — first-class per-review model selection

`/rival-review` and both binary entry points now accept `-m/--model` with an exact,
comma-separated or repeated model list:

- `/rival-review -m sol src/api/`
- `rival review --model sol src/api/`

An explicit list replaces the complete roster; no reviewer is added implicitly. Model-only
invocations still auto-detect the git scope, options can be combined in either order, and `--`
escapes a scope beginning with a dash. The skill version was advanced so existing installs
receive the new argument contract.

### Changed — expanded curated megareview roster

The curated roster expanded at the time with independent bug-hunting,
architecture/security, and code-quality lenses. Those experimental reviewers
were later retired; per-run selection remains limited to Rival's maintained
model set.

Code review and the native Sol plan command default to `high` effort and accept `ultra`.
The `/rival-plan-sol` skill pins Sol to `ultra`; compatible OpenCode reviewers
map `ultra` to their maximum variant. A Fable-only plan retains its low default.

The process-wide roster override is retired; use the per-invocation
`-m/--model` interface instead.

Judge selection now follows the requested model order after reviewer-phase failures, and console output shows
the concrete judge model. Empty
`/rival-review` arguments now run the default roster with git scope detection; use `--help` for
usage.

### Added — Sol plan review

`/rival-plan-sol` reviews a plan/spec with Sol at `ultra` effort. The plan binary now
selects reviewers by model name via `--model`; output, dashboard labels,
skipped-reviewer messages, and consilium attribution all show concrete model names.
The skill is bumped to v3.20.0 so existing installs receive the ultra default.
Superseded provider-named skills are removed during `rival install`.

### Changed — short public model names

The standalone skills are now `/rival-sol` and `/rival-fable`; plan review uses
`/rival-plan`, `/rival-plan-sol`, and `/rival-plan-fable`. Binary commands, selectors, review attribution,
session lists, and dashboards expose stable public model names rather than
concrete runtime identifiers.

### Added — `/rival-fable` code-review skill

A new skill runs a **code review with Fable** via `rival command fable`, at **medium**
reasoning effort by default (`/rival-fable` reviews git-detected changes;
`/rival-fable src/api/` reviews a scope). The disabled `rival-fable-only` skill is
superseded by it.

### Fixed — opencode "database is locked" under parallel reviewers

The megareview runs several opencode processes at once; they otherwise share one
SQLite session DB and one intermittently lost the write lock, failing with
`database is locked` (exit 1, observed as a skipped reviewer).
Each opencode reviewer now gets its own `OPENCODE_DB` (keyed on the session ID), so
the parallel processes never contend.

### Fixed — clear preflight error for an experimental provider

An experimental provider path began failing before session creation when its
credential was unavailable, instead of letting every selected reviewer fail
mid-run with an opaque error. That provider path was later retired.

## [v3.16.0] — 2026-07-03

### Added — experimental OpenCode provider path

An early OpenCode provider integration injected its credential into the child
configuration per run and stripped inherited provider configuration so a
reviewed repository could not weaken it. That provider and credential path were
later retired; the current OpenCode integration is K3's Moonshot provider.

### Added — experimental parallel provider roster

An early release generalized OpenCode review execution to multiple parallel
models, deterministic judge selection, model-specific labels, and a
workdir-scoped read-only permission profile. The experimental roster and its
configuration surface were later retired; the maintained OpenCode path is now
K3 with Moonshot authentication.

## [v3.15.0] — 2026-07-02

### Added — OpenCode as an additional megareview reviewer

Megareview now runs an expanded set of reviewers in parallel, including an
OpenCode model, and merges all results through the consilium judge. OpenCode is
wired with a preflight, reviewer session, and run adapter.

- New `internal/executor/opencode.go`: `OpencodePreflight` (LookPath `opencode`) +
  `RunOpencode` → `opencode run -m <configured-model> --variant <effort> --dir
  <workdir> --dangerously-skip-permissions` (the skip-permissions flag is required —
  without it opencode auto-rejects file reads and produces nothing).
- A configured OpenCode model and `OpencodeVariantLevel` effort-to-variant map
  (low/medium→minimal, high→high, xhigh→max).
- Runner: `opencodeOK` preflight, a reviewer plan, and `case "opencode"` in the run and
  judge switches + `modelForCLI`. Any reviewer that is unavailable is skipped, not fatal.
  The consilium judge follows the successful reviewer preference order.
- OpenCode receives the `arch_security` lens, diversifying the reviewer roster.
- TUI/web add an `❯ opencode` icon and label; grouped megareviews derive their
  visible engines from the participating sessions.

opencode runs **read-only** (via `OPENCODE_PERMISSION` — read/grep/glob/list allowed,
edit/bash/task/web denied) rather than `--dangerously-skip-permissions`, matching codex's
`--sandbox read-only` posture so a prompt-injected repo cannot make the reviewer write files
or run commands. rival loads the reviewed repo's `.env`, so `OPENCODE_*` is now stripped from
the child environment (`safeEnv` blocklist + a targeted `dropEnv`) — a malicious repo cannot
ship a permissive `OPENCODE_PERMISSION`/`OPENCODE_CONFIG` in its `.env` to escape the sandbox.

### Fixed — megareview robustness (from the review of this change)

- **Judge is re-selected from reviewers that actually produced output.** The consilium judge
  was chosen from preflight availability, so a reviewer that preflighted OK but then 429'd at
  runtime could fail the whole review; the judge is now picked from the
  successful reviewers in preference order.
- **Consilium judge session is marked complete only after its output parses** — an empty or
  unparseable verdict now fails the session instead of showing "completed" in the dashboard.
- **Reviewer/judge session cleanup is registered before creation**, so a mid-creation failure
  no longer orphans the sessions already created.

## [v3.14.4] — 2026-07-02

### Fixed — megareview reviewer that produces no output

A reviewer CLI can exit 0 while writing nothing at all (empty stdout + empty log,
no 429 envelope), such as on a silent auth/session failure. rival previously
marked that session `completed` and fed the empty result to the consilium, so the
TUI detail view showed only a blank `(empty log)` block ("no results"). An
empty-output reviewer is now marked **failed** with the reason
"produced no output (empty result) — likely an auth/session failure" and reported
in the review's `Skipped:` list, so the TUI shows why and the consilium isn't fed
a no-op input.

### Fixed — test no longer writes to the real session store

`TestRunPlanCLI_RestoresPlanMode` created sessions via `session.NewQueued`, which
persists to `~/.rival/sessions`; it now isolates `$HOME` to a temp dir so the
suite never pollutes the user's real session history.

## [v3.14.3] — 2026-07-02

### Added — dual-engine plan review

`/rival-plan` now reviews a plan/spec with **Codex and claude-fable in parallel** and prints
each engine's independent 1-10 rating + findings side by side (no judge/merge). Two
single-engine variants ship alongside it:

- `/rival-plan` — codex + claude-fable, both results shown; an unavailable engine is
  skipped, not fatal.
- `/rival-plan-codex` — codex only (the previous `/rival-plan` behavior).
- `/rival-plan-fable` — claude-fable only.

All three back the same command via `rival command plan --cli codex,fable` (default both);
the skills pass the engine set. Implemented as `review.RunPlanReview` (mirrors megareview's
reviewer phase — one queue ticket, parallel runners — with no consilium), split into an
injectable executor plus a pure, unit-tested `assemblePlanResults`.

### Changed — TUI/web group labels derived from sessions

Grouped rows/detail panels previously hardcoded one fixed set of megareview
labels for **any** multi-session group. They now derive the kind, engines, and mode
from the sessions, so a dual plan group renders as `Plan Review` / `codex+claude-fable` /
mode `plan`. Megareview output is unchanged. The web API group object gains a `kind` field.

### Fixed — review findings on the new code

A `/rival-codex` review found 0 critical/high and 4 medium issues, all fixed:

- **Fable plan sessions kept their `plan` mode.** `executor.RunClaudeModel` overwrites
  `sess.Mode` with the transport (`native`/`docker`); `runPlanCLI` now restores
  `sess.Mode = "plan"` before the session is persisted, so the TUI/web (which classify on
  `Mode == "plan"`) render fable plan reviews correctly.
- **Run-timeout diagnostics preserved.** A timed-out engine now reports a
  `RIVAL_RUN_TIMEOUT` reason (via `runTimeoutReason`) in the skipped list instead of a bare
  exit code.
- **Partial session-creation cleanup.** The queued-session cleanup defer is registered
  before the creation loop, so a mid-loop failure still fails the sessions already created.
- **Web detail Mode field** uses `group.kind` for grouped rows instead of the primary
  session's transport mode.

## [v3.14.2] — 2026-06-13

### Fixed — `rival wait` / `detach` review findings
A megareview of the v3.14.0 wait/detach code surfaced four real issues, all fixed:

- **`rival wait --log` no longer tracks stale sessions.** `parseLogFile` matched
  every `"session"` field in the stderr, including ones that startup maintenance
  (`ReapOrphans`, queue reaper) logs there — so wait could watch old sessions and
  mis-report a healthy run as failed/crashed. It now only collects IDs from this
  run's `"message":"starting …"` lines.
- **No false "crashed" on a finalize race.** When the rival process is detected
  dead, wait now re-reads the session JSON once before declaring a crash — the
  process can finalize the session and exit between the status read and the
  liveness check.
- **`rival wait` default timeout derived from config.** Was a fixed 75m, below a
  legitimate worst-case megareview (queue wait + 2× run). New `config.MaxRunWait()`
  = `RIVAL_QUEUE_TIMEOUT + 2×RIVAL_RUN_TIMEOUT + 5m` (95m by default), used as the
  default and inherited by the skills (they no longer hardcode `--timeout`).
- **Windows build fixed.** `detach.go`'s `syscall.SysProcAttr{Setsid}` is Unix-only;
  split into build-tagged `detach_unix.go` / `detach_other.go` so the package
  compiles for `GOOS=windows`. (Releases remain darwin+linux.)

## [v3.14.1] — 2026-06-13

### Changed — `/rival-fable` skill disabled
The `rival-fable-only` skill is removed on install (added to `Deprecated`, no
longer embedded) — Claude Fable 5 is currently unavailable upstream. The
`rival command fable` executor and `FableModel` stay in the binary, so the skill
can be re-enabled later by re-adding it to `Names` + the embed list.

## [v3.14.0] — 2026-06-13

### Fixed — reviews no longer die when the launching context ends
The v3.13.0 background-shell skill pattern had a fatal flaw: when a Claude Code
skill's forked context completed, Claude Code tore down the fork's background
shells with a **process-group kill**, SIGTERM-ing the rival process underneath —
mid-review or while waiting in the queue (`cancelled while queued`). The review
died even though the queue itself behaved correctly.

### Added — `rival command … --detach`
- New `--detach` flag on all `rival command` subcommands: rival **re-execs
  itself into its own process session** (`setsid`), prints
  `rival: detached pid=N` to stderr, and the parent exits immediately. The
  launching shell returns at once; no process-group kill can reach the review.
- Stdin/stdout/stderr redirections are inherited by the detached child, so the
  `< prompt > out 2> err` contract is unchanged.
- `RIVAL_DETACHED=1` guards against re-exec loops.

### Added — `RIVAL_RUN_TIMEOUT` (no run can hang forever)
Previously nothing bounded the provider run itself: a hung CLI (network stall,
deadlock) meant the detached rival never exited and any waiter blocked forever.
Now every run is bounded by `RIVAL_RUN_TIMEOUT` (default **30m**), and the clock
starts **after** the queue slot is acquired so queue wait never eats the budget.
On deadline the provider child is killed, the session fails with
`run timeout after <d> (RIVAL_RUN_TIMEOUT) — provider CLI did not finish`, and
the queue slot is freed. `RIVAL_RUN_TIMEOUT=0` disables it. The megareview
pipeline gets 2× the budget (reviewers phase + judge phase).

### Added — `rival wait` subcommand
One clean primitive that blocks until a run reaches a terminal state, used by
the skills' background watcher and available to any script/CI:

- `rival wait --log <stderr-file>` — parse the detached rival PID + session IDs
  from a run's stderr, poll the **rival process** for liveness (PID-reuse-proof
  via `procinfo` start-time pinning), then summarize the sessions when it exits.
  Detects a crashed rival (process dead, sessions not finalized).
- `rival wait <session-id>...` — terminal-status-only mode.
- Exit codes: `0` all completed · `2` some failed · `3` rival crashed · `4`
  timed out. Default `--timeout` 75m.

### Changed — skills are now async (launch + background watch, never block)
All active skills were rewritten to **not block the session**. They
drop `context: fork`, launch rival `--detach` (foreground, returns in seconds),
arm a **background `rival wait --log …` watcher** (`run_in_background: true`),
then hand control back immediately and end the turn. When the watcher exits, the
harness wakes the session and the skill presents the output verbatim — possibly
several turns later. This removes the previous design's two hang paths (no
provider-run timeout, bare-PID polling with "never stop polling") and frees the
session for the whole review. Cancel = `kill <pid>`; status on demand =
`tail <err>`. The detached run + result files survive the context ending, so a
lost watcher can be resumed with `rival wait --log <err>`.

### Fixed — claude/fable bills the subscription, not API credits
The claude CLI silently prefers an inherited `ANTHROPIC_API_KEY` over its own
`/login` (Pro/Max) auth, so rival runs were billed to API credits whenever the
shell exported the key. Auth is now **explicit**:

- Default (`RIVAL_CLAUDE_AUTH` unset / `subscription`): `ANTHROPIC_API_KEY` and
  `ANTHROPIC_AUTH_TOKEN` are stripped from the subprocess env — the CLI uses
  its subscription login.
- `RIVAL_CLAUDE_AUTH=api`: opt-in API billing; requires `ANTHROPIC_API_KEY`,
  hard error when empty. Any other value is a hard error.
- Auth mode is logged per run and shown in the TUI detail view (Account field).
- Auth/billing failures (`Credit balance is too low`, `Please run /login`,
  invalid key, expired OAuth) now append an actionable, mode-specific hint to
  the output instead of a bare CLI error.

### TUI
- Detail view shows the full error message word-wrapped (was a single truncated
  line); applied to single and group views.
- Fable sessions store `cli: "claude"` (fable is a model inside the Claude Code
  CLI, not a CLI) — TUI/web/`rival sessions` show `⬡ claude` + model
  `claude-fable-5`. Old sessions with `cli: "fable"` still render as claude via
  a read-compat fallback.

## [v3.13.0] — 2026-06-11

### Added — cross-process review queue
Multiple Claude Code clients (or terminals) invoke `rival` as independent processes. They now serialize through a **cross-process FIFO queue** instead of all firing their provider CLIs at once and hitting rate limits.

- **One review at a time by default.** Coordination is via ticket files in `~/.rival/queue/` guarded by an `flock` — no daemon. Each waiting process scans, reaps dead tickets, and promotes itself in a single locked critical section.
- **Position reported to the waiting client.** A queued review prints `rival queue: position 2/3 (1 running), waiting 1m12s` to stderr; skills relay it to the user.
- **Visible in TUI and web dashboard.** Queued sessions show a `◌ queued #N` row with a live wait timer, plus a queued counter. The TUI `x` key cancels a queued session too.
- **New `rival queue` subcommand** — `rival queue` (list position/state/mode/wait/workdir), `rival queue clear` (remove dead tickets), `rival queue clear --force` (remove all; live waiters re-queue at the tail).
- **Config:** `RIVAL_MAX_CONCURRENT` (default 1), `RIVAL_QUEUE_TIMEOUT` (default 30m, wait-only), `RIVAL_NO_QUEUE` / `--no-queue` to bypass.
- A megareview holds **one** slot for its whole pipeline (both reviewers + the consilium judge).

### Changed — skills run rival in the background
A queued-then-run sequence can exceed the 600s foreground Bash cap. All skills now launch rival with `run_in_background: true`, redirect stdout/stderr to separate temp files, poll the stderr file for queue-position lines, and present stdout verbatim on exit. Skill `allowed-tools` updated to `Bash, Read, KillShell`. All active skills bumped to v3.13.0; `rival-fable-only` added.

### Reliability
- **PID-reuse-safe liveness.** Tickets and sessions record the owning process's start time (new `internal/procinfo` package: macOS `sysctl KERN_PROC`, Linux boot-relative `/proc/<pid>/stat`). A recycled PID — a different process with a different start time — is correctly treated as dead, so a crashed holder's slot is reclaimed and never wedges. Degrades to a bare existence check on unsupported platforms.
- **SIGKILL-survivor handling.** If rival is killed mid-review but its provider-CLI child survives, the slot stays held (the child still consumes quota) until that child exits — covers reviewers and the consilium judge.
- No heartbeats by design: laptop sleep/wake can never false-reap a live holder.

## [v3.12.0] — 2026-06-10
- The Claude Code runtime dependency was updated; CI is now the sole release
  publisher.

## [v3.11.0] — 2026-06-09
- Web dashboard server, `/rival-plan`, provider quota/429 detection, and a
  revised default megareview roster.

## [v3.10.0] — 2026-03-21
- Rival skills became directly Skill-tool invocable (the old Agent-tool/binary workaround is no longer needed).

---

Older releases (v3.9.0 and earlier) predate this changelog; see `git log` and tags for history.
