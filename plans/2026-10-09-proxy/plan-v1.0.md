# Proxy support, config window, Fable and Sol Implementation Plan v1.0

**Date:** 2026-10-09
**Status:** planned
**Spec:** ./spec.md

**Goal:** send Claude (Opus, Fable) and Codex (Codex, Sol 6.1) runs through CLIProxyAPI with one URL, one key file and a model prefix for each provider. Add `rival config` with a live model check, a TUI config window and a Rival.app settings pane. Bring Fable and Sol 6.1 back as reviewers.

**Architecture:** eight phases (P1–P8), one commit each, on branch `feat/proxy` in `worktrees/proxy`, from master `71835d3` plus the spec commit. P0 is done (spec, "P0 findings"). The exit gate is one `/rival-codex review` of the branch, then one PR and the merge. Citations are for `71835d3`.

**Tech Stack:** Rust 1.98, serde_json, serde-saphyr (read) plus a YAML tree writer, ureq 3 (already a dependency, `crates/rival-core/Cargo.toml:20`), ratatui 0.30, crossterm 0.29; SwiftUI (macOS 14) for the app.

> For agentic workers: implement task by task. Use the checkboxes to track.

## Implementer rules (paste in every dispatch)

"Write the code and the unit tests that your task names. Write the failing test first. Run focused tests, `cargo build`, and at the end `cargo fmt --all`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked`. Never run real reviewer CLIs, network calls to a real proxy, or releases. Never touch the real `~/.rival`. Never put a real key in a test, a fixture or a log. Do not commit. Do not stash. Never make symlinks. Keep every behaviour that the spec does not list as changed: a user with no `proxy:` block sees no change."

## File map

**Create:**
- `crates/rival-core/src/proxy.rs` + `proxy/tests.rs`: URL normalize, wire id, key load, `/v1/models` call, preflight, 429 classify.
- `crates/rival-core/src/config/write.rs` + tests: YAML tree writer, key file writer.
- `crates/rival-core/src/check.rs` + `check/tests.rs`: the model check engine.
- `crates/rival/src/config_cmd.rs` + `config_cmd/tests.rs`: `rival config` subcommands.
- `crates/rival/src/tui/{config_view,config_form,config_check}.rs` + tests.
- `crates/rival-core/skills/rival-fable/`, `crates/rival-core/skills/rival-sol/`.
- `app/Sources/RivalApp/Settings/{SettingsView,ProxyPane,ModelsPane,CheckPane}.swift`, `app/Sources/RivalKit/RivalCLI.swift`, `app/Tests/RivalKitTests/RivalCLITests.swift`, fixtures in `app/Tests/Fixtures/`.
- `e2e/fakes/fake_proxy.py` (local HTTP: `/v1/models`, 401 mode, 429 marker), `e2e/scenarios/proxy-*.yaml`, `config-*.yaml`, `fable-*.yaml`, `sol-*.yaml`, `plan-three-models.yaml`.

**Modify:** `crates/rival-core/src/config.rs` (`:21-45` ids, `:104` `engine_label`, `:703` `UserConfig`, `:741` `RawUserConfig`, `:779` `load_user_config`, `:807` Sol drop, `:1184` `claude_auth`), `crates/rival-core/src/executor/{claude,claude_docker,codex}.rs`, `executor/subprocess.rs:93,526`, `session/mod.rs:50`, `review/planrun.rs:88`, `skills.rs:30`, `crates/rival/src/{model_specs,root,tree,command_plan}.rs`, `crates/rival/src/tui/{keys,model,runtime,styles}.rs`, `app/Sources/RivalApp/RivalApp.swift:125,141`, `app/Sources/RivalKit/Grouping.swift:93`, `README.md`, `docs/runtime-reference.md`, `CHANGELOG.md`.

## Task 0 — worktree and baseline

- [ ] `git worktree add worktrees/proxy feat/proxy`. Commit this plan.
- [ ] Workspace tests, e2e and the app tests pass. Record the counts.

## P1 — config schema, writer, `rival config` (no wire change)

### Task 1.1 — schema and validation
Files: `config.rs` (+ `config/tests.rs`).
- [ ] Failing tests first: parse a full `proxy:` and `plan:` block; defaults (no block = proxy off for both); `RIVAL_PROXY_URL`, `RIVAL_PROXY_KEY`, `RIVAL_PROXY=off`; URL normalize (`/`, `/v1` removed, scheme and host required); prefix normalize (trailing `/`); a `key:` under `proxy:` is an error naming `rival config key set`; an unknown name in `plan.models` is an error; `efforts.fable` and `efforts.sol` are valid; `efforts.sol` is no longer dropped.
- [ ] `ProxyConfig { url, key_file, claude: ProxyRoute, codex: ProxyRoute }`, `ProxyRoute { enabled, model_prefix }`, `PlanConfig { models }`. Raw types follow the `GoString` pattern (`config.rs:716`).
- [ ] `Config::proxy_route(provider) -> Option<Route>` where `Route { url, key, prefix }`. `None` when off or `RIVAL_PROXY=off`. The key is loaded lazily, at the first call.
- [ ] Key load order: `RIVAL_PROXY_KEY`, `proxy.key_file`, `~/.rival/proxy.key` (via `paths.rs`). Trim. Unix: a mode with group or other bits is an error.
- [ ] Remove the `cfg.efforts.remove(SOL_LABEL)` drop (`:807`). Extend `known_effort_model` and the error text (`:811`).

### Task 1.2 — writer
Files: `config/write.rs` (+ tests).
- [ ] Tests: round trip keeps unknown keys and key order; `set("proxy.claude.model_prefix", "emcd_")` makes the missing maps; a value that fails `load_user_config` leaves the old file unchanged; `.bak` is made only on the first rewrite of a file without the header; the key file has mode 0600 (Unix) and is written by temp file + rename; `key clear` removes it.
- [ ] Use a generic YAML value tree for edit and emit. If serde-saphyr cannot emit, use `serde_yaml_ng` for the writer only, and record the new dependency in the commit message.

### Task 1.3 — `rival config` command
Files: `config_cmd.rs` (+ tests), `tree.rs`, `root.rs:554`.
- [ ] `config show [--json]`: each value, its source (`default`, `file`, `env`), and the key as `set (…a91f)` or `missing`. The full key never prints.
- [ ] `config set KEY VALUE`, `config set --json` (patch from stdin, one write), `config key set` (stdin only; an argument is an error), `config key clear`.
- [ ] `config models [--json]`: stub in P1 that errors `needs P2`; real in Task 2.1.
- [ ] `rival config` with no subcommand: opens the TUI on the config window (stub to `tui` until P6).
- [ ] Shell completions include the new subtree.
- [ ] Orchestrator: gate, three-OS CI, commit `feat(config): proxy and plan settings, a writer, rival config`.

## P2 — Claude through the proxy

### Task 2.1 — proxy module
Files: `proxy.rs` (+ tests).
- [ ] `wire_id(prefix, model)`, `models(route) -> Result<Vec<String>>` (ureq GET `<url>/v1/models`, `Authorization: Bearer <key>`, 5 s timeout), `preflight(route, provider, model) -> Result<(), ProxyError>` with the exact error texts of the spec table (unreachable, 401/403, wire id missing with the prefixes that serve it, empty Claude prefix, no Codex account).
- [ ] One `/v1/models` call per process: a `OnceLock` cache keyed by URL.
- [ ] `classify_limit(output) -> Option<String>`: 429 with `cooling down`, `monthly spend limit` or `rate_limit_error`. Returns the hint with the other prefixes.
- [ ] Tests against an in-process `TcpListener` fake. No real network.
- [ ] Wire `config models` (Task 1.3) to `proxy::models`, grouped by prefix.

### Task 2.2 — Claude wiring
Files: `executor/claude.rs` (`:73`, `:130`, `:153`, `:213`), `executor/claude_docker.rs` (`:30`, `:150-171`), `executor/subprocess.rs`, `session/mod.rs:50`.
- [ ] Failing tests on `claude_args` and the `Request` from `run_claude_model`: on the proxy route `--model emcd_/claude-opus-5-5`; `env` has `ANTHROPIC_BASE_URL` and `ANTHROPIC_API_KEY`; `drop_env` has the seven variables of the spec; the key is not in `args`; `sess.account == "proxy"`; direct route args are byte-identical to today.
- [ ] Docker: the same names with `-e NAME` only; a loopback host becomes `host.docker.internal` plus `--add-host=host.docker.internal:host-gateway`; the preflight does not need `RIVAL_CLAUDE_TOKEN` on the proxy route.
- [ ] `claude_preflight` runs `proxy::preflight` on the proxy route.
- [ ] `claude_auth_hint`: proxy branch (key, prefix, 429 hint from `classify_limit`).
- [ ] `Session`: optional `route`, `wire_model` (serde `default`, `skip_serializing_if` empty). Old JSON loads.
- [ ] Leak guard: the proxy key is a secret in the scrub list used for logs, errors and session files. A test runs a fake whose output echoes the key and checks that the session log and the error do not have it.

### Task 2.3 — e2e
- [ ] `e2e/fakes/fake_proxy.py` and a fake `claude` that records its argv and the names of its env (never values, except a fixed test key).
- [ ] Scenarios: `proxy-claude-review` (args and env as above), `proxy-claude-key-rejected`, `proxy-claude-model-missing`, `proxy-claude-limit-429`, `proxy-off-env` (`RIVAL_PROXY=off` runs direct).
- [ ] Orchestrator: gate, e2e, three-OS CI, manual: one `rival command claude review` through the real proxy on `emcd_`. Commit `feat(proxy): Claude runs through CLIProxyAPI`.

## P3 — Codex through the proxy

### Task 3.1 — Codex wiring
Files: `executor/codex.rs` (`:17`, `:72`, `:105`).
- [ ] Failing test pins the two `-c` strings exactly as P0 ran them (`model_provider="rival_proxy"` and the inline table with `env_key = "RIVAL_PROXY_KEY"`, `wire_api = "responses"`, `base_url = "<url>/v1"`). Direct args are byte-identical to today.
- [ ] `RIVAL_PROXY_KEY=<key>` in `Request.env`. A TOML-unsafe URL (quote, backslash, newline) is a config error, not an escape.
- [ ] `codex_preflight_for` on the proxy route: binary check, then `proxy::preflight`; no `codex login status`.
- [ ] e2e: fake `codex` records argv; scenarios `proxy-codex-review`, `proxy-codex-key-rejected` (the preflight fails before the fake runs).
- [ ] Orchestrator: gate, e2e, manual: one `rival command codex review` through the real proxy. Commit `feat(proxy): Codex runs through CLIProxyAPI`.

## P4 — Fable, Sol 6.1, `opus`

### Task 4.1 — ids and labels
Files: `config.rs:21-45,104`, `app/Sources/RivalKit/Grouping.swift:93` (+ app tests).
- [ ] `SOL_MODEL = "gpt-6.1-sol"`, `FABLE_MODEL = "claude-fable-5-1"`, `FABLE_LABEL = "fable"`, `OPUS_ALIAS = "opus"`. `SOL_LABEL` is no longer read-compat only.
- [ ] `engine_label`: `gpt-6.1-sol` → `sol`, `gpt-5.6-sol` → `sol` (old), `claude-fable-5-1` → `fable` (was `retired-model`). Table test over every old and new id. The same table in Swift.

### Task 4.2 — executors and specs
Files: `executor/claude.rs:73`, `executor/codex.rs:72`, `model_specs.rs`, `root.rs:567-588`, `tree.rs`.
- [ ] Allow-lists: Claude runtime takes `CLAUDE_MODEL` and `FABLE_MODEL`; Codex runtime takes `CODEX_MODEL` and `SOL_MODEL`. `run_claude` takes the model id instead of the constant.
- [ ] `fable_spec()`, `sol_spec()`, usage texts. `resolve_effort`: Fable as Claude, Sol as Codex (`model_specs.rs:118`).
- [ ] Commands: `command fable`, `command sol`, `run fable`; `opus` is an alias of `claude` for `command` and `run`. Completions.

### Task 4.3 — plan review
Files: `command_plan.rs:20,163`, `review/planrun.rs:88` and its callers.
- [ ] `parse_plan_models` accepts `codex, sol, claude, opus, fable`; de-duplicates by model id. Default from `plan.models`, then `codex`. Usage text.
- [ ] The plan runner keys on the model id. `codex,sol` gives two blocks on one runtime. The language repair pass runs per block with its own model (test with a fake for each).

### Task 4.4 — skills
Files: `skills.rs:20-45`, `skills/rival-fable/`, `skills/rival-sol/`, `skills/rival-plan/SKILL.md`.
- [ ] New skills from the `rival-claude` and `rival-codex` templates. Remove `rival-fable` and `rival-sol` from `DEPRECATED`; keep `rival-plan-fable` and `rival-plan-sol` there. `rival-plan` documents `-m opus,fable,sol`. Version bumps.
- [ ] e2e: `fable-review`, `sol-review`, `plan-three-models` (`-m opus,fable,sol`, both runtimes through the fake proxy).
- [ ] Orchestrator: gate, e2e, app tests, commit `feat(models): Fable 5.1 and Sol 6.1 reviewers, opus alias`.

## P5 — model check

### Task 5.1 — engine
Files: `check.rs` (+ tests), `executor/subprocess.rs:526`.
- [ ] `pub struct CheckRow { name, runtime, route, wire_model, ok, ms, reply, error, hint, limit }`. `pub fn run_check(cfg, names, on_row: impl Fn(CheckRow))`.
- [ ] Order: static (binary, URL, key) → one shared `proxy::models` → live call through `ModelSpec.run` with `Reply with exactly: ok`, effort `low` (K3 `max`), read-only, 90 s timeout, no queue slot, log `~/.rival/check/<name>.log`.
- [ ] `run_subprocess` must not save a check session. Add `Request.persist: bool` (default `true`); the check sets `false`. Test: no file in `sessions/` after a check.
- [ ] At most 3 live calls at once. A 429 row sets `limit: true` with the `classify_limit` hint.
- [ ] Tests with fake runners: all pass, one fail, binary missing, proxy down, key rejected, model not listed, 429, timeout, reply not `ok` (pass, flagged).

### Task 5.2 — CLI
Files: `config_cmd.rs`.
- [ ] `config check [-m LIST|all] [--json] [--config-stdin]`. Text output as in the spec. `--json`: one line per row as it finishes, then `{"summary":{"ok":4,"total":5}}`. Exit 1 if a row fails.
- [ ] e2e: `config-check-json`, `config-check-fail-exit`, `config-check-draft` (`--config-stdin` with an unsaved prefix).
- [ ] Orchestrator: gate, e2e, manual: `rival config check` on the real proxy shows Opus, Fable, Codex and Sol. Commit `feat(config): rival config check sends hi to each model`.

## P6 — TUI config window

### Task 6.1 — form state
Files: `tui/config_form.rs` (+ tests).
- [ ] `ConfigForm { saved: UserConfig, draft: UserConfig, section, field, edit: Option<TextInput>, errors }`. Field list per section from the spec. Pure functions: `apply(key)`, `validate()`, `patch() -> json`. Masked key input never stored in the draft; it goes straight to `config key set`.
- [ ] Prefix picker values from `proxy::models` (on a job thread), plus `none` and free text.

### Task 6.2 — view
Files: `tui/config_view.rs` (+ snapshot tests), `tui/styles.rs`.
- [ ] Layout of the spec mockup: section list left, section body right, check panel bottom, key bar. Below 100 columns the section list folds into a tab row. No overflow at 80×24.
- [ ] "Requests go to" list from the draft. Online dot from the debounced (600 ms) `/v1/models` call.
- [ ] Row styles: green ✓, red ✗, red `limit`, yellow reply, spinner.
- [ ] Snapshot tests at 80×24 and 140×40: each section, unsaved draft, an invalid field, check rows in each state.

### Task 6.3 — wiring
Files: `tui/keys.rs:14`, `tui/model.rs`, `tui/runtime.rs`, `tui/config_check.rs`.
- [ ] `Mode::Config`, `Mode::ConfigEdit`. `c` in the list opens it; `rival config` starts on it; `esc` goes back, with the `save? y/n/cancel` confirm bar when the draft is dirty.
- [ ] Keys: `tab` section, `↑↓` field, `enter` edit, `space` toggle, `←→` choose, `c` check, `a` check all, `s` save (blocked while invalid), `u` undo, `?` help.
- [ ] `config_check` runs `check::run_check` on the draft as a `Cmd::Job`; each row is a `Msg`.
- [ ] Orchestrator: gate, manual use at 80×24 and full screen. Commit `feat(tui): config window with a live model check`.

## P7 — Rival.app settings

### Task 7.1 — CLI bridge
Files: `RivalKit/RivalCLI.swift` (+ tests and JSON fixtures from P1 and P5 output).
- [ ] Find `rival`: `RIVAL_BIN`, `/opt/homebrew/bin`, `/usr/local/bin`, `~/.local/bin`, then the login shell `PATH`.
- [ ] `show()`, `models()`, `set(patch:)`, `setKey(_:)` (stdin), `check(models:) -> AsyncStream<CheckRow>` from the JSON lines.

### Task 7.2 — views
Files: `Settings/*.swift`, `RivalApp.swift:125,141`.
- [ ] `Settings` scene (⌘,) with Proxy, Models, Check tabs as in the spec. "Settings…" and "Check models" in the menu-bar menu; the menu check posts a notification with `N of M ok`.
- [ ] `rival CLI not found` state with the install command.
- [ ] Orchestrator: app build and tests on the Mac, manual use. Commit `feat(app): settings window with proxy, models and check`.

## P8 — docs and release

- [ ] `README.md`: proxy setup in five lines (`rival config key set`, `rival config set proxy.url …`, enable, prefix, `rival config check`). Fable and Sol in the model list.
- [ ] `docs/runtime-reference.md`: the wire changes, the env names, the preflight errors, `RIVAL_PROXY*`.
- [ ] `CHANGELOG.md`: proxy, `rival config`, the config window, Settings, Fable, Sol 6.1, `opus`.
- [ ] Spec: as-built notes. Move the plan to `plans/done/`.
- [ ] Exit gate: `/rival-codex review` of the branch (through the proxy), fix findings, PR, merge, `rival-release`.

## Risks

| Risk | Plan |
|---|---|
| serde-saphyr has no emitter. | Task 1.2 allows one writer-only YAML crate. |
| `run_subprocess` saves the session in more than one place. | Task 5.1 finds every save behind `persist`; a test checks `sessions/` is empty. |
| The Codex CLI changes the `-c` provider keys. | The pinned test plus `rival config check` show it at once. |
| A 429 on one prefix during the manual gates. | Use the prefix that the check shows green. |
| The app cannot find `rival` when started from Finder. | The login shell `PATH` step and `RIVAL_BIN`. |
