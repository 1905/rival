# Ultrafast skills: rival-astra-ultrafast and rival-sol-ultrafast

**Date:** 2026-10-12
**Scope:** /wrk/dev/rival, branch `feat/ultrafast` from master `9ae4d3d`.
**Status:** approved (Codex plan review 7/10, 3 MEDIUM findings folded in: R5, R5a, Decision 3)

## TL;DR

- What: two new skills, `/rival-astra-ultrafast` and `/rival-sol-ultrafast`. They run GPT-6 Astra and GPT-6.1 Sol through Codex with the Ultrafast service tier.
- How: a new `--ultrafast` flag on `rival command codex` and `rival command sol`. The flag adds `-c service_tier="ultrafast"` to the `codex exec` call.
- Guard: Codex drops an Ultrafast tier that the account does not have, with only a warning. Rival finds that warning and reports "ran at standard speed".
- Not done: no Ultrafast for `rival run`, plan review, security review, or the TUI/app settings.

## Facts (checked 2026-10-12)

1. OpenAI ships Ultrafast for `gpt-6-astra` and `gpt-6.1-sol` in Codex, ChatGPT Work and the API. Plans: Pro $500, and Enterprise/Edu when the workspace owner turns it on. Speed: up to 8x Standard, about 300 tokens/s.
2. API: `service_tier: "ultrafast"` on the request (developers.openai.com/api/docs/guides/ultrafast-mode).
3. Codex CLI (openai/codex `a9b0815`, 2026-10-11; installed 0.162.1, the latest on npm):
   - Config key `service_tier`. The value `ultrafast` is kept only when the feature `ultrafast_mode` is on (`core/src/config/mod.rs:4033`). That feature is stable and on by default (`features/src/lib.rs:1968`).
   - The TUI has `/ultrafast`. `codex exec` has no flag for it. `-c service_tier="ultrafast"` sets it.
   - When the model catalog for the account does not list the tier, Codex prints this warning and sends the request without the tier (`core/src/session/mod.rs:1214`):
     `Configured service tier `ultrafast` is not advertised as supported for model `<slug>` and will be omitted from requests.`
4. Live run on this machine (Dell, `codex exec -m <model> -c service_tier='"ultrafast"'`): both models answered, and both printed the warning above. This account has only the `priority` ("Fast") tier in `~/.codex/models_cache.json`. So the skills will run here at standard speed until the account gets Ultrafast.
5. In rival, `command codex` runs `gpt-6-astra` (`config::CODEX_MODEL`) and `command sol` runs `gpt-6.1-sol` (`config::SOL_MODEL`). Both go through `executor::run_codex_model` and `codex_run_args` (`executor/codex.rs`).

## Decisions

1. **A flag, not two new models.** Labels, effort keys (`efforts.codex`, `efforts.sol`), session grouping and the app all key on the model id. Ultrafast is the same model with a different service tier. A flag keeps every one of those paths unchanged. The Ultrafast skills use the same effort default as their base model.
2. **Skill names follow the user's request:** `rival-astra-ultrafast` (it runs `command codex`) and `rival-sol-ultrafast`. `rival-astra` stays in `DEPRECATED`. The new name is a different directory, so install does not remove it.
3. **Warn, do not fail, when the tier is dropped.** Codex sends the request anyway, so the tokens are already spent when the warning shows. Rival keeps the result. It adds one line to stdout after the output and one to stderr:
   `rival: ⚠ ultrafast not applied — Codex omitted service_tier=ultrafast for <model> because its model catalog does not list the tier; the run used the standard tier.`
   On the direct route only, a second line: `rival: Ultrafast needs a Codex account on Pro $500, or an Enterprise/Edu workspace with Ultrafast on.` On the proxy route the catalog and the account are the proxy's, so rival names no cause (plan review finding 3).
   The skill text tells Claude to repeat that line in the summary.
4. **Force the local feature on:** also pass `-c features.ultrafast_mode=true`. Then a local config that turned the feature off cannot drop the tier without a trace. A server-side requirement can still turn it off. Codex then omits the tier and prints no warning. Rival cannot see that case (see Risks).
5. **The proxy route gets the same two `-c` values.** Whether the proxy forwards `service_tier` is unknown and not verified. The docs say so.
6. **The session records the tier:** a new `service_tier` string field in the session JSON, empty when not set and then left out of the JSON. Older readers ignore it. The app does not show it yet.

## Requirements

- R1. `rival command codex --ultrafast` and `rival command sol --ultrafast` add, after `-m <model>`: `-c` `service_tier="ultrafast"` `-c` `features.ultrafast_mode=true`. The direct route and the proxy route both do this.
- R2. Without the flag, the argv is the same as today, byte for byte. The existing tests must stay green.
- R3. `--ultrafast` on `command claude|fable|grok|k3|plan|security` is a usage error: "--ultrafast works only with command codex and command sol". Simplest form: register the flag only on codex and sol, so clap rejects it everywhere else.
- R4. `--detach` passes `--ultrafast` through to the child. Detach re-runs the same argv, so R4 needs only a test.
- R5. After each codex call (the run and its language pass), the executor reads that call's log. Codex prints its config warnings once, right after it echoes the prompt. Only the `warning:` lines directly after the echo of the exact prompt rival sent count, and one of them must be exactly Codex's warning (`warning: Configured service tier \`ultrafast\` is not advertised as supported for model \`<slug>\` and will be omitted from requests.`). The same line inside the prompt or the reply does not count (branch review finding). A hit sets `service_tier_omitted` on the session, and rival prints the warning line from Decision 3. Exit codes do not change.
- R5a. The session carries the tier: `command codex|sol --ultrafast` sets `service_tier` on the session, and the codex executor reads it. Every codex call of the command gets it: the review and its language-repair pass. Other callers (plan review, the model check) leave it empty, so their argv does not change.
- R6. The session JSON gets `"service_tier":"ultrafast"` for flagged runs.
- R7. Two Claude skills under `crates/rival-core/skills/`: `rival-astra-ultrafast`, `rival-sol-ultrafast`. Each is a copy of its base skill (`rival-codex`, `rival-sol`) with the name, the description, the usage text and the launch line changed (`rival command codex --ultrafast …`). Plus one paragraph: what Ultrafast is, and how to relay the "not applied" line.
- R8. Two Codex skills from `skills::codex_skill`. The `{{COMMAND}}` substitution must carry the flag: `command` becomes `codex --ultrafast` / `sol --ultrafast`. Check `codex.md` for every place `{{COMMAND}}` goes.
- R9. `skills::NAMES` grows from 7 to 9, after `rival-sol`. Install, update and bump-skill-version tests and e2e scenarios that list the skills are updated.
- R10. Docs: README skill list, `docs/command-reference.md` (`--ultrafast`), CHANGELOG `Unreleased`.

## Risks

- ⚠ The account on this machine has no Ultrafast, so the real speed-up cannot be shown here. The tests use a fake codex. The live check shows only that the flag reaches Codex and that the warning is found.
- ⚠ When a server requirement turns `ultrafast_mode` off, Codex drops the tier and prints nothing. Rival then cannot tell that the run was at standard speed.
- ⚠ The warning text is not an API. If a later Codex changes the text, R5 stops firing, without an error. The match uses only the stable part: `` `ultrafast` is not advertised``.

## Out of scope

- `rival run codex|sol` (no `run` surface exists for them today).
- Plan review (`command plan -m`), security review, `config check`.
- TUI and Rival.app display of the tier.
- A release. The user can run `/rival-release` later.
