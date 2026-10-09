# Proxy support, config window, Fable and Sol reviewers

**Date:** 2026-10-09
**Scope:** /wrk/dev/rival. The file:line citations are from master `71835d3`.
**Status:** spec

## TL;DR

**P0 — proxy facts (no code).**
- What: log Codex in on the proxy (`-codex-device-login`). Read `/v1/models` to get the Sol 6.1 id and the Codex prefix. Run one Codex CLI call by hand through the proxy.
- Why: the Codex wiring and the Sol id depend on these facts.
- You decide or do: approve the device login on your phone or browser.
- Not done: no change to the proxy config. T3 keeps working as it does today.

**P1 — config schema and writer.**
- What: a `proxy:` block, a `plan:` block, and `efforts.fable` / `efforts.sol` in `~/.rival/config.yaml`. The first writer for that file. A new `rival config` command: `show`, `set`, `key`, `check`.
- Why: today nothing writes the file (`config.rs:779` only reads it). The TUI and the app need one shared writer.
- You decide or do: nothing.
- Not done: the file keeps no comments after rival rewrites it. rival makes a `.bak` copy one time.

**P2 — Claude through the proxy.**
- What: when `proxy.claude.enabled` is set, native and Docker Claude runs get the proxy URL, the key and `--model <prefix>/<model>`.
- Why: today the model id is a constant (`config.rs:31`). `--model` takes `codex` or `claude` only. The proxy rejects a model name without a prefix.
- You decide or do: nothing.
- Not done: no proxy-side alias.

**P3 — Codex through the proxy.**
- What: when `proxy.codex.enabled` is set, Codex runs get a custom Codex provider (`-c model_provider=…`) that points to `<url>/v1`, with the key in an environment variable.
- Why: the Codex arguments are fixed today (`executor/codex.rs:105`).
- You decide or do: nothing after P0.
- Not done: no proxy route for the opencode models (K3, Grok).

**P4 — Fable and Sol 6.1 come back.**
- What: `fable` (`claude-fable-5-1`, Claude Code runtime) and `sol` (Sol 6.1, Codex runtime) are full reviewers: commands, skills, plan review, efforts, labels. `opus` is a new name for the existing `claude` model.
- Why: you want reviews from Opus, Fable and Sol.
- You decide or do: nothing.
- Not done: no fan-out command that runs one code review on several models (see Open questions).

**P5 — model check.**
- What: `rival config check` sends a short "hi" prompt to each model through the same code path that reviews use. It shows a check mark, the latency and the reply, or the error and a hint.
- Why: you want to see that each request goes to the model you expect.
- You decide or do: nothing.

**P6 — TUI config window.** A full-screen config window with sections for Proxy, Models, Review and Check. Opens with `c` from the run list or with `rival config`.

**P7 — Rival.app settings.** A macOS Settings window (⌘,) with the same sections. It calls `rival config … --json`.

## Problem(s)

1. **rival cannot use the proxy for Claude.**
   - The proxy URL works today: `ANTHROPIC_BASE_URL` gets through to `claude`.
   - The key works today: `RIVAL_CLAUDE_AUTH=api` with `ANTHROPIC_API_KEY` (tested on 2026-10-09: `apikey ok`).
   - The model name does not work. `CLAUDE_MODEL` is a constant (`config.rs:31`), and `run_claude_model` rejects every other id (`executor/claude.rs:73`). The proxy has `force-model-prefix: true` and wants `emcd2_/claude-opus-5-5`.
   - Subscription mode drops `ANTHROPIC_AUTH_TOKEN` (`executor/claude.rs:130`). A user who sets the proxy token in the shell sees the run go to the subscription instead.
2. **rival cannot use the proxy for Codex.**
   - `codex_run_args` is fixed (`executor/codex.rs:105`). It has no base URL and no provider.
   - The preflight runs `codex login status` (`executor/codex.rs:23`). Through the proxy, a local Codex login is not necessary and the check is wrong.
3. **The proxy has no Codex account yet.** `auths/` holds the two Claude accounts only.
4. **No user can set any of this.**
   - Nothing writes `~/.rival/config.yaml`. `UserConfig` has no `Serialize` (`config.rs:703`).
   - The TUI has no settings screen (`tui/keys.rs:14` modes: List, Filter, Detail, Search, Confirm).
   - Rival.app has no Settings scene (`RivalApp.swift:125`, `:141`) and never calls the CLI.
5. **No user can see that a model works before a review.** A wrong prefix, a bad key or a missing login shows only as a failed review, after the queue wait.
6. **Fable and Sol are gone.**
   - Fable was removed in 3.34 (CHANGELOG `:123`). Sol (`gpt-5.6-sol`) was removed on 2026-09-26 (CHANGELOG `:103`).
   - `rival install` deletes the `rival-fable` and `rival-sol` skills (`skills.rs:30`).
   - Old Fable sessions show as `retired-model` (`config.rs:125`).
   - `efforts.sol` is dropped when the config loads (`config.rs:807`).

## Goals

1. Send Claude runs (Opus, Fable) and Codex runs (Codex, Sol) through the proxy, each provider on its own switch (Problems 1, 2).
2. Keep one URL, one key and one model prefix for each provider. The key is never in `config.yaml`, in process arguments or in logs (Problems 1, 2).
3. Check before each proxied run that the proxy answers, accepts the key and serves the exact model name. The error names the fix (Problems 1, 2, 5).
4. A `rival config` command that the TUI and the app share: show, set, store the key, check (Problem 4).
5. A model check that uses the real review code path and shows a result for each model (Problem 5).
6. A TUI config window and a Rival.app settings pane, both easy to read and fast to use (Problem 4).
7. Fable and Sol 6.1 as reviewers in every place that Codex and Claude are (Problem 6).
8. Direct runs (no proxy) work as they do today. A user with no `proxy:` block sees no change.

## Non-goals

- A change to the proxy config, or proxy-side model aliases.
- rival logs in to the proxy, or manages proxy accounts.
- A proxy route for opencode (K3, Grok).
- Load balance between the two Claude prefixes. One prefix for each provider. The proxy does its own round-robin inside a prefix.
- The megareview fan-out command (removed on 2026-09-26) comes back.

## Configuration

```yaml
proxy:
  url: http://127.0.0.1:8317      # base URL, no /v1
  key_file: ~/.rival/proxy.key    # default; mode 0600
  claude:
    enabled: true
    model_prefix: emcd2_          # wire id: emcd2_/claude-opus-5-5
  codex:
    enabled: true
    model_prefix: ""              # from P0; empty = no prefix
plan:
  models: [codex, claude]         # default --model for `rival command plan`
efforts:
  claude: medium
  fable: medium
  codex: xhigh
  sol: xhigh
```

Rules:
- **Key.** The order is `RIVAL_PROXY_KEY`, then `proxy.key_file`, then `~/.rival/proxy.key`. A key in `config.yaml` is an error that names `rival config key set`. The key file is trimmed. On Unix, a key file that other users can read is an error, as `ssh` does.
- **URL.** `RIVAL_PROXY_URL` overrides `proxy.url`. The URL must be `http` or `https` with a host. A trailing `/` or `/v1` is removed.
- **Prefix.** A trailing `/` is removed. The wire id is `<prefix>/<model>`, or `<model>` when the prefix is empty.
- **Switch for one run.** `RIVAL_PROXY=off` sends that run direct, also when the config enables the proxy.
- **Validation.** `load_user_config` validates the new keys the same way as `efforts` today. A typo fails every command before queue work.
- **Old keys.** `efforts.sol` is valid again. The drop at `config.rs:807` goes away.

### Writer

- New module `crates/rival-core/src/config/write.rs`.
- It loads the file as a generic YAML tree, sets the given key paths, and keeps unknown keys.
- It validates the new text with `load_user_config`, writes a temp file in the same directory, and renames it over the old one.
- The first time rival rewrites a file that it did not write, it copies the old file to `config.yaml.bak`. The file starts with a header line `# written by rival config; comments are not kept`.
- The key file is written with mode 0600, also through a temp file and a rename.

### `rival config`

| Command | Output | Use |
|---|---|---|
| `rival config` | Opens the TUI on the config window. | Users. |
| `rival config show [--json]` | The resolved config: each value and its source (default, file, env). The key shows as `set (…a91f)` or `missing`. | Users, the app. |
| `rival config set KEY VALUE` | Sets one key path, for example `proxy.claude.model_prefix emcd2_`. | Users, scripts. |
| `rival config set --json` | Reads a JSON patch from stdin and applies it in one write. | The app. |
| `rival config key set` | Reads the key from stdin, never from an argument. `key clear` removes the file. | Users, TUI, app. |
| `rival config models [--json]` | `GET <url>/v1/models`, grouped by prefix. | The prefix picker. |
| `rival config check [-m LIST] [--json] [--config-stdin]` | See [Model check](#model-check). `--config-stdin` checks a draft YAML that is not saved. | Users, TUI, app. |

`--json` output is one object. `check --json` writes one JSON line for each model as it finishes, then a summary line. The app fills its rows live from these lines.

## Wire changes

### Claude (native)

When `proxy.claude.enabled` is set and `RIVAL_PROXY` is not `off`:

| Item | Value |
|---|---|
| `--model` | `<prefix>/<model>`, for example `emcd2_/claude-fable-5-1` |
| Environment set | `ANTHROPIC_BASE_URL=<url>`, `ANTHROPIC_API_KEY=<key>` |
| Environment dropped | `ANTHROPIC_AUTH_TOKEN`, `ANTHROPIC_DEFAULT_OPUS_MODEL`, `ANTHROPIC_DEFAULT_SONNET_MODEL`, `ANTHROPIC_DEFAULT_HAIKU_MODEL`, `CLAUDE_CODE_USE_BEDROCK`, `CLAUDE_CODE_USE_VERTEX`, `CLAUDECODE` |
| `RIVAL_CLAUDE_AUTH` | Not used. A debug log line says that the proxy route wins. |
| `sess.account` | `proxy` |

- `ANTHROPIC_API_KEY` is the form tested on 2026-10-09. The variables go in `Request.env` (`subprocess.rs`), never in the arguments.
- `run_claude_model` accepts `CLAUDE_MODEL` and `FABLE_MODEL` (`executor/claude.rs:73`). The prefix goes on only at the `claude_args` call. The session keeps the bare id.

### Claude (Docker)

- The same environment, passed by name with `-e` as `ANTHROPIC_AUTH_TOKEN` is today (`claude_docker.rs:158`). Values never show in `ps`.
- A loopback proxy host (`127.0.0.1`, `localhost`, `::1`) changes to `host.docker.internal`, and the run adds `--add-host=host.docker.internal:host-gateway`.
- The Docker preflight does not ask for `RIVAL_CLAUDE_TOKEN` on the proxy route.

### Codex

When `proxy.codex.enabled` is set:

```
codex exec -C <workdir> -m <prefix>/<model>
  -c model_provider="rival_proxy"
  -c model_providers.rival_proxy={ name = "rival proxy", base_url = "<url>/v1", env_key = "RIVAL_PROXY_KEY", wire_api = "responses" }
  -c model_reasoning_effort=<effort> --sandbox read-only --ephemeral --skip-git-repo-check --color never -
```

- `RIVAL_PROXY_KEY=<key>` goes in `Request.env`. The `-c` value holds only the name of the variable.
- The preflight on the proxy route does not run `codex login status` (`executor/codex.rs:23`). It checks the binary and runs the [proxy preflight](#proxy-preflight).
- `run_codex_model_with` accepts `CODEX_MODEL` and `SOL_MODEL` (`executor/codex.rs:72`).
- P0 confirms the `-c` form and `wire_api` against Codex CLI 0.161.0 and the proxy. If the inline table form fails, the fallback is a generated `CODEX_HOME` overlay with the same provider block, as `opencode` gets a generated config today.

### Session record

- Two new optional fields, `route` (`direct` or `proxy`) and `wire_model`. Old readers ignore them. The TUI Info tab and the app detail view show them.
- `model` stays the bare id, so labels, efforts and the language repair pass do not change.

### Proxy preflight

The proxy preflight runs before the queue slot, for each proxied model:

1. URL and key are present and valid. Else: `proxy key missing — run rival config key set`.
2. `GET <url>/v1/models` with the key, timeout 5 s. One call per process, shared by the models of a plan review.
3. Results:

| Result | Error text |
|---|---|
| Connection refused or timeout | `proxy unreachable at <url>: <reason>` |
| 401 or 403 | `proxy rejected the key (<status>) — run rival config key set` |
| Wire id not in the list | `proxy does not serve emcd2_/claude-fable-5-1; it serves claude-fable-5-1 as: emcd_/… — set proxy.claude.model_prefix` |
| List has no model of that provider | `proxy has no codex account — log in on the proxy (-codex-device-login)` |

- `claude_auth_hint` (`executor/claude.rs:213`) gets a third branch for the proxy route. It names the key and the prefix, not `/login`.
- The leak guard scrubs the key from logs, errors and session files, the same as other secrets.

## Models

| Name | Aliases | Runtime | Model id | Label | Effort default | Commands |
|---|---|---|---|---|---|---|
| codex | — | codex | `gpt-6-astra` | codex | xhigh | `command codex`, plan |
| sol | — | codex | `gpt-6.1-sol` (P0 confirms) | sol | xhigh | `command sol`, plan |
| claude | opus | Claude Code | `claude-opus-5-5` | claude | medium | `command claude`, `run claude`, plan |
| fable | — | Claude Code | `claude-fable-5-1` | fable | medium | `command fable`, `run fable`, plan |

Changes:
- `config.rs`: `SOL_MODEL`, `FABLE_MODEL`, `FABLE_LABEL`. `engine_label` matches the new ids before the adapter fallback. `claude-fable-5-1` sessions now show as `fable`, not `retired-model`. `gpt-5.6-sol` sessions keep `sol`.
- `known_effort_model` and the error text at `config.rs:811` accept `fable` and `sol`. Fable uses the Claude ladder, and Sol uses the Codex ladder. P0 confirms that Sol accepts `ultra`. If it does not, Sol clamps `ultra` to `xhigh` the same way Grok clamps.
- `model_specs.rs`: `fable_spec()`, `sol_spec()`. The `ModelSpec` closures pass the model id. `resolve_effort` treats Fable like Claude and Sol like Codex.
- `root.rs:573`: `CommandFable`, `CommandSol`, `RunFable`, and `opus` as an alias of `command claude` / `run claude`. `tree.rs` and the shell completions follow.
- `command_plan.rs:163`: `parse_plan_models` accepts `codex, sol, claude, opus, fable`. It removes duplicates by model id (`opus` and `claude` are one model). With no `--model`, the default comes from `plan.models`, then `codex`.
- `review/planrun.rs:88`: the plan runner keys on the model id, not on the CLI word. Two models on one runtime (Codex and Sol) run as two blocks.
- The language repair pass (`review::repair_language`) already uses the same model and the same route. It needs no change. A test covers a Fable block and a Sol block.
- Skills: new `rival-fable` and `rival-sol`, from the `rival-claude` and `rival-codex` templates. Remove them from `DEPRECATED` (`skills.rs:30`). `rival-plan` documents `-m fable,sol`. `rival-plan-fable` and `rival-plan-sol` stay deprecated.
- `app/Sources/RivalKit/Grouping.swift:93` (the Swift port of `engine_label`) gets the same ids.

## Model check

One engine in `crates/rival-core/src/check.rs`. The CLI, the TUI and the app all use it.

Models checked:
- By default: `codex`, `sol`, `claude`, `fable`, and the configured security reviewer.
- With `-m LIST`: the models in the list. `-m all` adds K3 and Grok.

The steps for each model:
1. **Static.** The runtime binary is on `PATH` (for Claude, `claude` or Docker). On the proxy route: URL and key present.
2. **Proxy.** One shared `/v1/models` call. The wire id is in the list.
3. **Live.** The real adapter call, through the `ModelSpec.run` closure:
   - Prompt: `Reply with exactly: ok`.
   - Effort `low`. K3 is pinned to `max`.
   - Read-only review permissions.
   - Timeout 90 s.
   - No queue slot. A `Session` that is never saved.
   - The log goes to `~/.rival/check/<name>.log`, through `RunCall.log`.

   No session JSON is written, so the run list and the app do not show check runs. If `run_subprocess` saves the session, P5 adds a flag that turns this off.
4. **Pass.** Exit code 0 and a reply that is not empty. The row shows the latency and the first 40 characters of the reply. If the reply is not `ok`, the row still passes and shows the reply in yellow.
5. **Fail.** The row shows the first error line from the preflight table, `auth_hint`, or the provider error after scrubbing.

Concurrency is at most 3 live calls. The exit code is 0 when all checked models pass, and 1 if one or more fail.

Output (text):

```
proxy  http://127.0.0.1:8317  ✓ 200  38 models  key …a91f

  ✓ claude  proxy   emcd2_/claude-opus-5-5   2.1s  ok
  ✓ fable   proxy   emcd2_/claude-fable-5-1  1.8s  ok
  ✓ codex   proxy   gpt-6-astra              3.4s  ok
  ✗ sol     proxy   gpt-6.1-sol              —     proxy does not serve gpt-6.1-sol
  ✓ k3      direct  moonshotai/kimi-k3       4.0s  ok

4 of 5 ok
```

JSON line for each model:

```json
{"name":"fable","runtime":"claude","route":"proxy","wire_model":"emcd2_/claude-fable-5-1","ok":true,"ms":1812,"reply":"ok","error":"","hint":""}
```

## TUI config window

New modules `tui/config_view.rs` (layout and render), `tui/config_form.rs` (draft state and edit), `tui/config_check.rs` (runs the check engine on a job thread). New `keys::Mode::Config` and `Mode::ConfigEdit`. `c` in the run list opens it. `rival config` starts the TUI on it. `esc` goes back.

```
╭─ rival · config ─────────────────────────────────────── ~/.rival/config.yaml ● unsaved ─╮
│                    │                                                                     │
│  ▸ Proxy           │  PROXY                                                              │
│    Models          │                                                                     │
│    Review          │  Claude through proxy   [■] on        Codex through proxy  [■] on   │
│    Check           │                                                                     │
│                    │  URL            http://127.0.0.1:8317                     ● online  │
│                    │  Key            ••••••••••••a91f   ~/.rival/proxy.key     0600 ✓    │
│                    │  Claude prefix  ‹ emcd2_ ›      also: emcd_                         │
│                    │  Codex prefix   ‹ none ›                                            │
│                    │                                                                     │
│                    │  REQUESTS GO TO                                                     │
│                    │    claude  Opus 5.5    → emcd2_/claude-opus-5-5                     │
│                    │    fable   Fable 5.1   → emcd2_/claude-fable-5-1                    │
│                    │    codex   Codex       → gpt-6-astra                                │
│                    │    sol     Sol 6.1     → gpt-6.1-sol                                │
│                    │                                                                     │
├────────────────────┴─────────────────────────────────────────────────────────────────────┤
│  CHECK                                                         last run 14:02 · 4 of 5 ok │
│  ✓ claude   proxy   emcd2_/claude-opus-5-5    2.1s   ok                                   │
│  ✓ fable    proxy   emcd2_/claude-fable-5-1   1.8s   ok                                   │
│  ⠼ codex    proxy   gpt-6-astra               …                                           │
│  ✗ sol      proxy   gpt-6.1-sol               proxy does not serve gpt-6.1-sol — set the  │
│                                               Codex prefix or log in Codex on the proxy   │
├───────────────────────────────────────────────────────────────────────────────────────────┤
│ tab section  ↑↓ field  enter edit  space toggle  ←→ choose  c check  s save  u undo  esc │
╰───────────────────────────────────────────────────────────────────────────────────────────╯
```

Sections:
- **Proxy.**
  - The two route switches.
  - URL. `d` imports `ANTHROPIC_BASE_URL` when it is set.
  - Key: masked, with the last 4 characters. `enter` opens a masked input. Paste works.
  - The prefix for each provider is a picker. Its values come from `rival config models`: the prefixes that serve that provider's models, plus `none`. A free-text edit is also possible.
  - "Requests go to" shows the wire id of each model, live from the draft.
  - The online dot comes from the `/v1/models` call. It refreshes when the URL or the key changes, after a 600 ms pause in typing.
- **Models.** One row for each model: name, runtime, route (proxy or direct, from the switches), effort picker (`←→` steps through the model's ladder), plan default (`space`). Runtimes that are not installed show dim, with `not installed`.
- **Review.** Security reviewer (`k3` / `grok`), `auto_fix_critical_high`, and the role prompt overrides (read-only, with the file path to edit them).
- **Check.** The full check table, with a `c` button and `-m all` (`a`). The check panel at the bottom shows on every section.

Behavior:
- Edits change a draft. The title shows `● unsaved`. `s` writes through the [writer](#writer). `u` drops the draft. `esc` with unsaved changes asks `save? y/n/cancel` in the confirm bar.
- **The check uses the draft.** A user can test a new prefix before saving.
- An invalid value shows red under its field with the validation text. `s` is not possible until all values are valid.
- Colors come from `tui/styles.rs`: green check mark, red cross, yellow for an unexpected reply, a spinner while a row runs. The layout does not overflow at 80×24. Below 100 columns the section list folds into a tab row at the top.
- `?` help lists the window keys.

## Rival.app settings

- A SwiftUI `Settings` scene (⌘, and the menu-bar menu item "Settings…"). Toolbar tabs: **Proxy**, **Models**, **Check**.
- **Proxy tab.** Two toggles, a URL field with a status dot, a `SecureField` for the key with a "Saved …a91f" label, and a prefix `Picker` for each provider, filled from `rival config models --json`. A "Requests go to" list.
- **Models tab.** A `Table` with columns: model, runtime, route, effort (`Picker`), plan default (`Toggle`).
- **Check tab.** A "Check models" button and a `Table` that fills one row at a time from the `check --json` lines, with SF Symbols `checkmark.circle.fill` (green) and `xmark.octagon.fill` (red), latency, and reply or error. The button checks the saved config. "Apply & check" saves first.
- The menu-bar menu gets "Check models". It shows a notification with `4 of 5 ok`.
- **CLI calls.** The app finds `rival` in this order: `RIVAL_BIN`, `/opt/homebrew/bin`, `/usr/local/bin`, `~/.local/bin`, the login shell `PATH`. If none is found, the pane shows `rival CLI not found` and the install command. All writes use `rival config set --json` and `rival config key set` (the key goes through stdin). The app never writes the YAML file or the key file itself.
- New files: `app/Sources/RivalApp/Settings/{SettingsView,ProxyPane,ModelsPane,CheckPane,RivalCLI}.swift`.

## File-level changes

| File | Change |
|---|---|
| `crates/rival-core/src/config.rs` | `ProxyConfig`, `PlanConfig`, new model ids and labels, validation, `proxy_route(provider)`, key lookup. |
| `crates/rival-core/src/config/write.rs` | New. YAML writer, key file writer. |
| `crates/rival-core/src/proxy.rs` | New. URL normalize, wire id, `/v1/models` call (ureq, already a dependency), the preflight and its errors. |
| `crates/rival-core/src/check.rs` | New. The check engine, with callbacks for each row. |
| `crates/rival-core/src/executor/claude.rs`, `claude_docker.rs` | Proxy env, model allow-list with Fable, wire id, auth hint branch. |
| `crates/rival-core/src/executor/codex.rs` | Provider `-c` args, key env, preflight split, Sol allowed. |
| `crates/rival-core/src/session/…` | `route`, `wire_model` fields. |
| `crates/rival-core/src/review/planrun.rs` | Key on the model id. Default list from `plan.models`. |
| `crates/rival-core/src/skills.rs`, `skills/rival-fable/`, `skills/rival-sol/` | New skills, `DEPRECATED` change, version bump. |
| `crates/rival/src/model_specs.rs` | `fable_spec`, `sol_spec`. Usage texts. |
| `crates/rival/src/root.rs`, `tree.rs` | New commands, `opus` alias, `config` subtree. |
| `crates/rival/src/command_plan.rs` | Model parse, default from config. |
| `crates/rival/src/config_cmd.rs` | New. `rival config` subcommands. |
| `crates/rival/src/tui/{config_view,config_form,config_check}.rs`, `keys.rs`, `model.rs`, `runtime.rs` | Config window. |
| `app/Sources/RivalApp/Settings/*.swift`, `RivalApp.swift`, `RivalKit/Grouping.swift` | Settings scene, CLI bridge, labels. |
| `e2e/scenarios/` | New scenarios (see Tests). |
| `README.md`, `docs/runtime-reference.md`, `CHANGELOG.md` | Proxy setup, `rival config`, Fable and Sol. |

## Tests

**Unit**
- Config: parse, defaults, env overrides, `RIVAL_PROXY=off`, prefix and URL normalize, a key in YAML is an error, a key file that other users can read is an error, an unknown model in `plan.models` is an error, `efforts.sol` and `efforts.fable` are valid.
- Writer: round trip keeps unknown keys, `.bak` made one time only, the validation failure leaves the old file, mode 0600 on the key file.
- Wire args: Claude native and Docker, proxy and direct, for Opus and Fable. The key is never in `args`. The dropped variables are not in the child environment. Codex `-c` args for Codex and Sol.
- Labels: `engine_label` for every old and new id.
- Plan models: `opus,claude` makes one block. `codex,sol` makes two blocks.
- Preflight: each row of the error table, against a local fake HTTP server.

**Check engine (fake provider)**
- All pass, one fail, binary missing, proxy down, key rejected, model not listed, timeout, an unexpected reply (pass, shown yellow). No session JSON written. Logs in `~/.rival/check/`. Concurrency at most 3.

**e2e**
- A fake proxy (local HTTP: `/v1/models`, 401 mode) plus a fake `claude` and a fake `codex` that record their environment and arguments.
- `command fable review` through the proxy: the fake saw `--model emcd2_/claude-fable-5-1`, `ANTHROPIC_BASE_URL`, the key in the environment only.
- `command sol` through the proxy: the fake saw the provider `-c` args and `RIVAL_PROXY_KEY`.
- `command plan -m opus,fable,sol`: three blocks, each repaired by its own model.
- `config check --json`: one line for each model, then the summary line. Exit code 1 on one failure.
- `config set` and `config key set` from stdin. `config show --json` hides the key.

**TUI**
- Render snapshots at 80×24 and 140×40 for each section, with an unsaved draft and with check rows in each state.
- Key flow: edit, invalid value, save blocked, save, unsaved-exit prompt.

**App**
- `RivalCLI` parses the `show` and `check` JSON fixtures. Settings views build in the existing app test target.

**Manual on Dell (and on the Mac for the app)**
- `rival config check` against the real proxy: Opus, Fable, Codex and Sol all show a check mark.
- One real code review and one real plan review (`-m opus,fable,sol`) through the proxy.
- The T3 sessions on the same proxy still work.

## Failure modes & decisions

| Failure or choice | Behavior |
|---|---|
| Proxy down during a review, after the preflight. | The run fails with the provider error and the proxy hint. No fallback to direct: a silent switch could bill a different account. |
| Key file missing or empty on the proxy route. | Preflight error that names `rival config key set`. |
| Prefix points at an account without the model. | Preflight error that lists the prefixes that serve it. |
| `RIVAL_CLAUDE_AUTH=api` and the proxy is on. | Proxy wins. Debug log line. |
| Docker Claude with a loopback proxy URL. | Host is changed to `host.docker.internal` with `--add-host`. |
| Hand-written `config.yaml` with comments. | `.bak` copy one time, then rival rewrites the file. |
| The proxy changes the model list between preflight and run. | Normal provider error. |
| Sol 6.1 id differs from `gpt-6.1-sol`. | P0 sets the constant. It is the only place. |
| Codex CLI rejects the inline provider table. | Generated `CODEX_HOME` overlay (P3 fallback). |
| Check reply is not `ok`. | Pass, shown yellow: the model answered. |
| Check on a model whose runtime is not installed. | Fail row `not installed`, no call. |

## Open questions

1. **One code review on several models.** This spec makes Opus, Fable and Sol reviewers in each place Codex and Claude are. The plan review runs any set of them in one run. A command that runs one *code* review on several models and joins the findings is the removed megareview. It is not in this spec. Say if you want it, as a later spec.
2. **Default plan models.** This spec keeps `[codex, claude]`. Change `plan.models` in the window to make `[claude, fable, sol]` the default.

## Rollout

- **P0** — Codex device login on the proxy (you approve). Read `/v1/models`. Set the Sol id and the Codex prefix. Run one `codex exec` through the proxy with the `-c` provider by hand. No commit, notes in this file.
- **P1** — config schema, writer, `rival config show/set/key/models`. Gate: fmt, clippy, workspace tests, three-OS CI.
- **P2** — Claude proxy wiring, proxy preflight, session fields, leak-guard scrub. Gate: the checks above, e2e with fake proxy, manual Opus run on Dell.
- **P3** — Codex proxy wiring. Gate: the checks above, manual Codex run on Dell.
- **P4** — Fable, Sol, `opus` alias, plan models, skills, labels, Swift labels. Gate: the checks above, e2e plan with three models.
- **P5** — check engine and `rival config check`. Gate: the checks above, manual check on Dell.
- **P6** — TUI config window. Gate: the checks above, render snapshots, manual use at 80×24.
- **P7** — Rival.app settings. Gate: app build and tests, manual use on the Mac.
- **P8** — README, runtime reference, CHANGELOG. Release with `rival-release`.
