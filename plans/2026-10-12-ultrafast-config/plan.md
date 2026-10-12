# Plan: Ultrafast as a config switch

Spec: [spec.md](spec.md). Branch `feat/ultrafast-config`, one PR, then release v5.2.0.

## T1 — config (rival-core)

- `config.rs`: `pub struct UltrafastConfig { pub codex: bool, pub sol: bool }` (Default). `UserConfig.ultrafast`. `RawUserConfig.ultrafast: Option<RawUltrafast>` with `Option<bool>` fields. `Config::ultrafast(model)`.
- `config/write.rs`: `"ultrafast.codex" | "ultrafast.sol"` are `Kind::Bool`; add to `SETTABLE_KEYS` (11 → 13).
- Tests (`config/tests.rs`, `config/write/tests.rs`): parse on/off/missing/bad type; `ultrafast()` per model; set + round-trip.

## T2 — executor (rival-core)

- `executor/codex.rs`: `let ultrafast = cfg.ultrafast(model);` replaces the session check; when on, set `sess.service_tier = ULTRAFAST_TIER`. Keep `log_head`, `ultrafast_omitted`, `ultrafast_notice`.
- Tests: tier from config direct + proxy for codex and sol; off → byte-identical argv; another model never (k3/claude don't reach here; assert `cfg.ultrafast` false for them).

## T3 — remove the flag and the special specs (rival)

- `tree.rs`: drop `ultrafast()` arg; `root.rs`: back to `codex_spec()` / `sol_spec()`; `model_specs.rs`: drop `ultrafast` field, tier specs, the two usage constants; `model_command.rs`: drop the `spec.ultrafast` session line (executor sets it). Notice printing stays.
- Tests: tree (flag unknown on codex/sol too), model_specs, model_command (notice test sets the session flag through the fake run; drop the spec-driven part).

## T4 — skills (rival-core)

- Delete the two skill dirs; `NAMES` 9 → 7; the merged `codex_skill` arm goes; `DEPRECATED` += both names (13 → 15).
- `rival-codex` / `rival-sol` SKILL.md and their `codex_skill` input: one line on relaying `rival: ⚠ ultrafast not applied`.
- Tests: skills tests (names, golden text, deprecated), install test that an installed `rival-sol-ultrafast` dir is removed.

## T5 — plan and config check notices (rival)

- `review/plan.rs`: `PlanRunResult.notices: Vec<String>`. `planrun.rs` fills it from each session with `executor::ultrafast_notice` after the language repair, for completed and failed reviewers alike (plan review finding 1). `command_plan.rs` prints them after the plan output, stdout + stderr.
- `check.rs`: the runner's call result carries `service_tier` and `service_tier_omitted` from its local session (plan review finding 2). `CheckRow` gets them; the row shows `ultrafast` when the tier was asked, plus the notice when Codex omitted it. Test through `SpecRunner` with a warning log and an "ok" reply.
- `config_cmd.rs`: two rows in `show`.
- Tests: plan notice through `run_doc_review_with` with the warning in the repair log; check row through `SpecRunner`; show rows.

## T6 — TUI (rival)

- `config_form.rs`: `Field::CodexUltrafast`, `Field::SolUltrafast` (toggles) appended to the Models section; `toggle()`; patch builder emits `ultrafast.codex|sol` on change.
- `config_view.rs`: render two `field()` rows with `toggle()` after the table hint.
- Tests: `config_form/tests.rs` toggle + patch; `config_view/tests.rs` render contains the rows; any TUI snapshot that lists Models fields.

## T7 — e2e and docs

- e2e: rewrite `ultrafast-command.yaml` (config file with `ultrafast: {sol: true}`, plain `command sol`); install/update scenarios back to 7 skills; a scenario or case where installed `rival-astra-ultrafast` / `rival-sol-ultrafast` dirs are removed; coverage.md.
- Docs: command-reference (keys, TUI, no flag, skills table), README skills table + count, CHANGELOG `[v5.2.0]`.

## T8 — gates and release

- `scripts/check.sh` → `CHECK OK`.
- Live: `rival config set ultrafast.sol true` in a scratch `RIVAL_HOME`, `rival command sol` raw prompt → notice; off → no tier.
- `/rival-codex review` of the branch diff; fix; `/simplify`.
- PR, merge, `bump-skill-versions.sh 5.2.0`, release commit, snapshot + publish on the Air, verify, install on the Dell (`~/.local/bin/rival`, `rival install --force`).
