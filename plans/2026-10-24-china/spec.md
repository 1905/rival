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

## Amendment 2026-10-25: provider and model pickers in the TUI

- Models section, `/CHINA RUNS`: `provider` picker (moonshot, openrouter, plus a configured other provider) and `model` picker.
- The list comes from `opencode models --standalone` (v2 takes no provider argument; about 2.5 s). The window asks once when it opens. It respects the user's opencode config, which can disable models.
- Model order: Chinese labs first (assumption: "china" means Chinese-lab models), then the rest, each by name. OpenRouter lists 363 models here.
- A provider change keeps the model name if the new provider lists it, else takes the first model. No listed model: the change is refused with a notice.
- `e` on the model: exact name wins, then a unique substring, then any full `provider/model` (saved with a warning when unlisted).
- Found while testing: opencode v2 keeps `opencode auth login` credentials in its DB. The per-run `OPENCODE_DB` hid them (OpenRouter: "No cookie auth credentials found") and left ~8 MB per run. China runs now use opencode's own DB; two concurrent runs passed.

## Amendment 2026-10-25: china runs the security review; K3 removed

User request: "remove 'security.reviewer: k3' stuff. china will do this role now. remove legacy stuff".

- `rival command security` (and `/rival security`) runs `china.model` through the china runner, mode `security`, at `max`; the language pass at `low`. Session `cli = "china"`.
- `--which` prints the china model, `china.model` with its source, the effort, the auth note, and the preflight result (opencode v2).
- Removed (legacy, opencode v1 path, broken on v2): `rival command k3`, `rival run k3`, `skill.<host>: k3`, `efforts.kimi-k3`, `security.reviewer` (`k3`, `grok`), the OpenRouter Grok security model, `executor/kimi.rs`, `executor/opencode.rs`, the Moonshot/OpenRouter key lookups (`MOONSHOT_API_KEY`, `KIMI_API`, `OPENROUTER_API_KEY`, the `.env` walk).
- Old configs: a `security:` block and `efforts.kimi-k3` are ignored at load (no error); `rival config set` rejects both keys as unknown. Old K3/Grok-OpenRouter sessions label as `retired-model`.
- `rival config check`: the `china` row replaces `k3` and `grok-4.6-openrouter`, and is in the default set.
- TUI: the Kimi K3 effort row and the `Security reviewer` picker are gone; the Review section names the security model.
- Assumption: the Grok CLI model (`grok`) stays. Only its OpenRouter security variant is removed.
