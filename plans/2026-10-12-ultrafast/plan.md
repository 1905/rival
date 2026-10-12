# Plan: Ultrafast skills

Spec: [spec.md](spec.md). One branch, `feat/ultrafast`, one PR. Each task ends with `cargo test` green for the touched crate.

## T1 — executor: tier in the codex argv (rival-core)

- `executor/codex.rs`: add `pub const ULTRAFAST_TIER: &str = "ultrafast";`.
- `codex_run_args(model, effort, workdir, tier: Option<&str>)`: when `tier` is `Some(t)`, insert after `-m <model>` (index 5): `-c`, `service_tier="<t>"`, `-c`, `features.ultrafast_mode=true`. `codex_proxy_run_args` passes `tier` through. Its provider splice stays at index 5, so the tier pairs come after the provider pairs. Fix the splice index to stay right.
- `run_codex_model` / `run_codex_model_with` take `tier: Option<&str>`. On `Some`, they set `sess.service_tier`.
- `pub fn ultrafast_dropped(log: &str) -> bool`: true when one trimmed line is exactly Codex's warning: prefix ``warning: Configured service tier `ultrafast` is not advertised as supported for model ` `` and suffix `` ` and will be omitted from requests.``. Negative tests: the words inside a prompt line and inside a reply.
- Callers: `review/planrun.rs:99` passes `None`. `model_specs.rs` passes the tier from `RunCall`.
- Tests (`executor/codex/tests.rs`): argv with and without the tier, direct and proxy; `ultrafast_dropped` true/false; the session field set. `review/planrun/tests.rs:257` gets the new `None` argument.

## T2 — session field (rival-core)

- `session/mod.rs`: `pub service_tier: String` with `#[serde(skip_serializing_if = "String::is_empty")]`, after `wire_model`. `#[serde(default)]` on the struct already covers old files.
- Test: a round trip with and without the field; old JSON without it loads.

## T3 — CLI flag and the warning (rival)

- `tree.rs`: a `bool_flag("ultrafast", "run on the Ultrafast service tier (gpt-6-astra, gpt-6.1-sol)")` only on `command codex` and `command sol`. A helper `ultrafast()` like `no_queue()`.
- `root.rs`: `CommandCodex` / `CommandSol` read `inv.bool("ultrafast")`. Other model commands pass `false`. `run_model_command` gets `ultrafast: bool` (or an options struct, if that reads better).
- `model_specs.rs`: `ModelSpec` gets `service_tier: Option<&'static str>` (None for every spec but `codex_spec_ultrafast` / `sol_spec_ultrafast`). The codex runtime closure passes it to `run_codex_model`. Because the tier is on the spec, the language-repair call gets it too (plan review finding 1).
- `model_command.rs`: after the run (and the language pass), when `spec.service_tier` is set and `ultrafast_dropped` is true for the session log or its `.repair.log`, write the warning line to stdout after the output (before the auth hint) and to stderr. Also add one `logging::warn()`.
- Tests: `tree/tests.rs` (flag on codex/sol only; error on claude), `model_command/tests.rs` (warning printed when the fake log has the line, not printed without `--ultrafast`), `model_specs/tests.rs` if the spec shape changed. Completion and help snapshots: update `e2e/scenarios/help-every-command.yaml`, `completion-scripts.yaml` and any test that prints the codex/sol flag list.

## T4 — skills (rival-core)

- `skills/rival-astra-ultrafast/SKILL.md` = `rival-codex` with: name, description ("Run GPT-6 Astra on the Ultrafast tier…"), heading, usage (`/rival-astra-ultrafast …`), launch `rival command codex --ultrafast --detach …`, one "Ultrafast" paragraph (speed, plan requirement, relay the `ultrafast not applied` line in the stats summary).
- `skills/rival-sol-ultrafast/SKILL.md`: the same from `rival-sol`.
- `skills.rs`: `NAMES` 7 → 9 (after `rival-sol`: `rival-astra-ultrafast`, `rival-sol-ultrafast`). `codex_skill` arms for both, `command` = `codex --ultrafast` / `sol --ultrafast`. Input text says to relay the warning.
- `skills/tests.rs`: names, frontmatter, the codex skill command line has `--ultrafast`, version line.
- `scripts/bump-skill-versions.sh` reads `NAMES`: check it still parses a 9-entry array.

## T5 — install/update e2e and unit tests

- `install/tests.rs`: counts and file lists.
- e2e scenarios that list installed skills (`install-*`, `update-*`): add the two files for `.claude` and `.agents`, and the stdout lines.
- New e2e scenario `ultrafast-command.yaml`, modelled on `executor-codex-success.yaml`: a fake codex that records its argv and prints the "not advertised" warning. Assert the argv has the two `-c` values and stdout has the `ultrafast not applied` line. Add it to `e2e/coverage.md`.

## T6 — docs

- README skills table: two rows. "seven review skills" → nine.
- `docs/command-reference.md`: the `--ultrafast` flag on `command codex|sol`; the two skills in the skills table.
- CHANGELOG `Unreleased`: Added.

## T7 — verify

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`, the e2e runner (see Makefile).
- Live: `printf 'Reply with just: ok' | target/debug/rival command sol --ultrafast --workdir /tmp` on this machine. Expect "ok" plus the `ultrafast not applied` line (this account has no Ultrafast). Same for `command codex --ultrafast`.
- Live: install the skills into a scratch HOME and diff them against the embedded files.

## Exit gates (fullsend)

1. `/rival-codex review` on the whole diff; verify every finding, fix what holds.
2. `/simplify`.
3. Push the branch, open the PR, link it, final report.
