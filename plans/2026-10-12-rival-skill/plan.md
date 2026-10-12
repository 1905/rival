# Plan: one `/rival` skill

Spec: [spec.md](spec.md). Branch `feat/rival-skill`.

## T1 — config (rival-core)
- `config.rs`: `SkillConfig { claude: String, codex: String }`, `RawSkill`, `UserConfig.skill`; validation in the same pass as `security.reviewer` (allowed: `codex sol claude opus fable k3 grok`, case-insensitive, `opus` → `claude`). `Config::skill_model(host: SkillHost) -> &'static str` returns a command label (`sol` default for Claude, `claude` default for Codex). `pub enum SkillHost { Claude, Codex }` with `parse`.
- `UltrafastConfig` default true: `RawUltrafast` `unwrap_or(true)`; `Default` impl gives true/true; a user config without the block → on. `Config::ultrafast` with no user file → true.
- `write.rs`: settable `skill.<host>` (Str, validated by load), keys list.
- Tests: defaults, set values, opus alias, bad value error, ultrafast default true (parse + no file), writer round trip.

## T2 — command (rival)
- `tree.rs`: `rival command skill` with `--host` (required string), `--workdir`, `--no-queue`.
- `root.rs`: `CommandSkill` → `skill_command::run(env, inv)`.
- New `skill_command.rs`: read stdin once; if the first word is `security`, call the security action with the rest as input; else resolve `cfg.skill_model(host)` → spec via a label→spec fn (`codex_spec`, `sol_spec`, `claude_spec`, `fable_spec`, `k3_spec`, `grok_spec`), then `run_model_command` with the input re-fed (refactor `run_model_command` to take the raw input string, or wrap stdin). Empty input → `RIVAL_USAGE`. Preflight failure → append the `rival config set skill.<host> <model>` hint.
- `model_specs.rs`: `RIVAL_USAGE` (no model names), `spec_for_label(label) -> ModelSpec`.
- Tests: host routing both ways with configured and default models, security route, usage, bad host, preflight hint.

## T3 — skills (rival-core)
- `skills/rival/SKILL.md` from `rival-codex` (name `rival`, no model names, launch `rival command skill --host claude --detach …`, usage incl. `security`, relay the ultrafast line). Delete the seven model skill dirs.
- `skills.rs`: `NAMES = ["rival"]`; `DEPRECATED` += 7; `codex_skill("rival")` with command `skill --host codex` and model-free input text.
- `scripts/bump-skill-versions.sh` still parses `NAMES` (one entry).
- Tests: skills tests rewritten (one name, no model names in any embedded text: grep for `Opus`, `Sol 6.1`, `GPT-6`, `Fable`, `Kimi`, `Grok`, `codex` as a model word), install removes all seven.

## T4 — TUI config window (rival)
- `config_form.rs`: drop the PLAN toggle path (`toggle_plan`, `plan_default`, `plan_set`, the `plan.models` patch) — keep `ModelInfo.plan` only if still used elsewhere, else remove. Add `Field::Skill(SkillHost)` pickers (cycle the 6 model labels), patch `skill.<host>`.
- `config_view.rs`: model table without PLAN; rows `/rival in Claude Code` and `/rival in Codex` with picker + hint; Ultrafast toggles stay; key bar without `space plan`.
- Regenerate goldens; tests for picker/patch/render.

## T5 — e2e + docs
- e2e: install/update scenarios → one skill `rival` (Claude + Codex) and the deleted ones; new `skill-command.yaml` (host claude → sol argv with ultrafast on by default; host codex → claude argv; `security` route); config-check/show scenarios for new rows/defaults.
- Docs: README, command-reference (skills, config, TUI, ultrafast default), e2e/coverage.md, CHANGELOG `[v6.0.0]`.

## T6 — gates and release
- `scripts/check.sh` → `CHECK OK`; live: `printf 'Reply with just: ok' | rival command skill --host claude` and `--host codex` in a scratch RIVAL_HOME.
- `/rival-codex review` equivalent: `rival command codex` review of the branch (the old skill is gone after install — call the binary directly); fix; `/simplify`.
- PR, merge, bump 6.0.0, release from the Air, install on the Dell (`rival install --force` removes the old skills, installs `rival`).

## Plan review (Codex, 7/10) — folded in

1. [high] `scripts/bump-skill-versions.sh` parses `NAMES` only as a multi-line array; rustfmt writes `pub const NAMES: [&str; 1] = ["rival"];` on one line. → T3: make the parser accept the one-line form too; add a regression test (script test or a small shell check in `scripts/test_*.py`).
2. [med] Ultrafast on by default changes the argv of every existing codex/sol e2e fixture (e.g. `command-codex-no-queue-workdir.yaml`, `executor-codex-*`, `sol-review`, `plan-*`, `proxy-codex-*`, `mr-*`, `review-language-*`, `config-check-*`). → T5: update them: add the two tier `-c` pairs where the default applies (or set `ultrafast: {codex: false, sol: false}` in a fixture whose subject is not the tier). Spec R8 "unchanged" is wrong; corrected here.
3. [med] `config show` rows for `skill.claude` / `skill.codex` are built in `crates/rival/src/config_cmd.rs` `rows()`. → T1/T2: add both rows (value, `default`/`file`), tests for text and `--json`.
4. [med] `-re high` with no prompt sets `is_empty` and `run_model_command` prints `spec.usage` of the resolved model (names a removed skill). → T2: the skill route sets `spec.usage = RIVAL_USAGE`; test `-re high` alone.
