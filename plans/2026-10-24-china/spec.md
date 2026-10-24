# `/china`: reviews from Kimi K3.1 Pro through opencode v2

**Date:** 2026-10-24
**Scope:** /wrk/dev/rival, branch `feat/china` from master `1c61e9c` (v6.0.0), worktree `/wrk/tmp/rival-china`.
**Status:** approved (fullsend; self-review below)

## TL;DR

- New skill `china` (Claude Code `/china`, Codex `$china`). "review with china" runs a review with the configured China model and returns its findings.
- New command `rival command china`. Same input as every model: `[-re level] review [scope]` or `[-re level] <prompt>`.
- New config key `china.model`: an opencode `provider/model` selector. Default `moonshot/kimi-k3.1-pro`.
- Runtime: opencode **v2** (`opencode run --standalone`). The key comes from opencode itself (`opencode auth login` or the opencode config). Rival injects no key.

## Facts checked on the Dell (2026-10-24)

- opencode `v2.1.3` is installed (the user said v2.1.2; same major).
- v2 `run` has no `--pure`, `--variant`, `--dir`. It takes `-m provider/model[#variant]`, the prompt on stdin, the cwd as workdir.
- v2 has no `OPENCODE_PERMISSION` env. A `permission` block inside `OPENCODE_CONFIG_CONTENT` works: with `bash`/`edit` denied the model had no bash or edit tool.
- `OPENCODE_CONFIG_CONTENT` is merged over the global config with `--standalone`. A model must be declared under its provider or the run fails with `Model unavailable`.
- A model `options.reasoningEffort` arrives at the API as `reasoning_effort` (captured with a local fake server).
- Moonshot `/v1/models` lists `kimi-k3.1-pro`: 1M context, thinking only, efforts `low`, `high`, `max`, default `max`.
- The user's opencode `moonshot` provider (openai-compatible, `https://api.moonshot.ai/v1`, key in opencode auth) answered with `kimi-k3.1-pro`.
- ⚠ Rival's existing `k3` and security runs pass `--pure --variant --dir`, so they fail on opencode v2. Out of scope here; reported.

## Decisions

1. **Config:** `china.model`, a string `provider/model`. Split at the first `/`: provider non-empty, no `/`; model non-empty (it may contain `/`, for example `openrouter/moonshotai/kimi-k3.1-pro`). No whitespace or control characters. A bad value is a config error naming the key. `rival config set china.model <value>` writes it; `rival config show` lists it with `default`/`file`.
2. **Effort:** `-re` takes the shared ladder. It maps onto Moonshot's menu: `low`→`low`, `medium`/`high`→`high`, `xhigh`/`ultra`→`max`. Omitted → `max`. The value goes to the model as `options.reasoningEffort`. No `efforts.china` key.
3. **Runtime check (preflight):** `opencode` on PATH, and `opencode --version` reports major ≥ 2. Otherwise a clear error. No key check: auth lives in opencode.
4. **Argv:** `opencode run --standalone -m <china.model>`. Prompt on stdin. The child cwd is the workdir (already set from the session).
5. **Child env:**
   - `OPENCODE_CONFIG_CONTENT` = `{"permission": <profile>, "provider": {<provider>: {"models": {<model>: {"options": {"reasoningEffort": <effort>}}}}}}`.
   - Profile: review (and every non-raw mode) = the existing read-only reviewer profile; raw = the existing full-auto profile.
   - `OPENCODE_DISABLE_PROJECT_CONFIG=1`, so a reviewed repo's `opencode.json` cannot re-enable tools (replaces v1 `--pure`).
   - `OPENCODE_DB=rival-<session>.db`, like the other opencode runs.
   - Raw mode also drops the K3 credential list (`KIMI_DROP_ENV`).
6. **Session:** `cli = "china"`, `model = china.model`. Display label = the part after the last `/` (`kimi-k3.1-pro`). The review header and dashboards show it.
7. **Scope out:** `rival run china`, `skill.<host>` values, TUI config pickers, `rival config check`, fixing `k3` for v2.
8. **Skill:** `skills/china/SKILL.md` = the `/rival` skill flow with `rival command china`, no security branch. Description triggers on `/china` and on "review with china" / "ask china". Codex skill from `codex_skill("china")`.

## Requirements

- R1 `rival command china` runs `china.model` through opencode v2 and prints the review like every other model.
- R2 Default model `moonshot/kimi-k3.1-pro`; `china.model` overrides it.
- R3 Reviews run read-only (no bash, no edit, no out-of-workdir reads); project opencode config is ignored.
- R4 `rival install` installs `china` for Claude Code and Codex.
- R5 Docs: README section, CHANGELOG entry.
- R6 Tests for argv, env/config JSON, effort map, label, config parse/set/show, version check, skill embed.

## Self-review

- Risk: `OPENCODE_DISABLE_PROJECT_CONFIG` is read from the binary's strings, not docs. Verify with a repo-level `opencode.json` that allows bash.
- Risk: a provider that does not know `max` (for example OpenAI) rejects the effort. Accepted: the default target is Moonshot; documented.
- Risk: a 5xx from the provider makes opencode retry with no end. The rival run timeout (30 min) bounds it.
- `ModelSpec.model` is `&'static str`; the configured model is leaked once per process. One small string per CLI run: fine.
