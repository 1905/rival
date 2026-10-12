# Ultrafast as a config switch

**Date:** 2026-10-12
**Scope:** /wrk/dev/rival, branch `feat/ultrafast-config` from master `d1f3daf` (v5.1.0 + CI removal).
**Status:** approved (Codex plan review 7/10: 1 HIGH, 1 MEDIUM, both folded into plan T5)
**Release:** v5.2.0 from the Air with `scripts/release-local.sh`, then install on the Dell.

## TL;DR

- What: Ultrafast becomes an on/off setting per Codex-runtime model: `ultrafast.codex` (GPT-6 Astra) and `ultrafast.sol` (Sol 6.1) in `~/.rival/config.yaml`. Both are off by default. The TUI config window gets two toggles. `rival config set ultrafast.codex true` works too.
- Effect: every Codex-runtime call for a model with the switch on asks for the Ultrafast tier: `/rival-codex`, `/rival-sol`, `rival command plan -m codex|sol`, the language-repair pass and `rival config check`.
- Removed: the `--ultrafast` flag on `rival command codex|sol` and the skills `rival-astra-ultrafast` and `rival-sol-ultrafast`. Install and update delete installed copies of the two skills.
- Kept: the tier argv (`-c service_tier="ultrafast"`, `-c features.ultrafast_mode=true`), the session fields `service_tier` / `service_tier_omitted`, the prompt-anchored warning check, and the `rival: ⚠ ultrafast not applied` notice.

## Decisions

1. **Config, not a flag.** One mechanism. The user asked for the switch and for the special skills to go. A flag that also forces the tier would be a second way to do the same thing; it goes too. (Assumption, stated: no per-run override.)
2. **Per model.** Astra and Sol are priced and rate-limited apart, so each has its own switch. Key names follow the command names: `ultrafast.codex`, `ultrafast.sol`.
3. **The executor reads the config.** `run_codex_model_with` asks `cfg.ultrafast(model)`. When it is on, it sets `sess.service_tier = "ultrafast"`, adds the tier pairs, and checks the log after the call. Every caller of the codex runtime gets it with no caller change. The session stays the record of what was asked.
4. **Notice in every command that runs a codex model and prints output.** `rival command codex|sol` already prints it. `rival command plan` prints it too, once per omitted session, after the plan output. `rival config check` shows the tier on the model row when the switch is on, and the notice text when Codex omitted it.
5. **Removed skills go to `skills::DEPRECATED`**, so `rival install` and `rival update` delete installed copies, like other renamed skills.
6. **The base skills relay the notice.** `rival-codex` and `rival-sol` (Claude and Codex variants) get one line: when the output has `rival: ⚠ ultrafast not applied`, put that line in the stats summary.
7. **Rival.app:** no change. It reads `rival config show --json`, which gets the two new rows; the app ignores keys it does not know. Not verified on a Mac (no Xcode on the Air).

## Requirements

- R1. `UserConfig.ultrafast: UltrafastConfig { codex: bool, sol: bool }`, parsed from an optional `ultrafast:` block. Missing block or key: false. A non-bool value is a config error naming the key (same path as `auto_fix_critical_high`).
- R2. `Config::ultrafast(model: &str) -> bool`: `CODEX_MODEL` → `ultrafast.codex`, `SOL_MODEL` → `ultrafast.sol`, every other model → false.
- R3. `rival config set ultrafast.codex|ultrafast.sol true|false`; both keys in `SETTABLE_KEYS`; `rival config show` (text and `--json`) lists them with source `file` or `default`.
- R4. TUI config window, Models section: two toggle rows after the model table, labels `Codex Ultrafast` and `Sol Ultrafast`, with a dim hint (`Pro $500 or Enterprise/Edu; falls back to standard`). `space`/`enter` toggles; save writes only changed keys.
- R5. Executor: tier from `cfg.ultrafast(model)` (R2), on the direct and the proxy route. The argv without the switch is byte-identical to v5.1.0 without `--ultrafast`.
- R6. Remove `--ultrafast` from the tree, `ModelSpec.ultrafast`, `codex_tier_spec` / `sol_tier_spec`, `ASTRA_ULTRAFAST_USAGE` / `SOL_ULTRAFAST_USAGE`. `rival command codex --ultrafast` is `unknown flag: --ultrafast`.
- R7. Remove `skills/rival-astra-ultrafast` and `skills/rival-sol-ultrafast`, their `codex_skill` arm and `NAMES` entries (9 → 7); add both names to `DEPRECATED`.
- R8. `rival command plan`: after the plan output, one notice per session with `service_tier_omitted`.
- R9. `rival config check`: a row whose model has the switch on shows `ultrafast`; an omitted tier adds the notice line under the row.
- R10. Tests: config parse/show/set, TUI toggle + save patch, executor tier from config (both routes, both models, other models never), plan notice, skill removal on install, e2e `ultrafast-command` rewritten to use a config file, install/update scenarios back to 7 skills plus a removal scenario for the two retired skill dirs.
- R11. Docs: command reference (config keys, no flag, skills table back to 7), README, CHANGELOG `[v5.2.0]`, e2e coverage.

## Risks

- ⚠ Turning the switch on for an account without Ultrafast costs nothing extra: Codex drops the tier and rival prints the notice on every run. That is noisy but honest.
- ⚠ Plan review and the model check now also use the tier when the switch is on. That is the point of "same skills use it", but a user who wants Ultrafast only for code review cannot get that split.
