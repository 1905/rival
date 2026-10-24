# Plan: `/china`

Spec: `spec.md` (same folder). One branch, `feat/china`.

## Tasks

1. **Config** (`crates/rival-core/src/config.rs`, `config/write.rs`)
   - `CHINA_LABEL = "china"`, `CHINA_DEFAULT_MODEL = "moonshot/kimi-k3.1-pro"`.
   - `ChinaConfig { model }` in `UserConfig`, `RawChina` in `RawUserConfig`.
   - `parse_china_model(raw) -> Option<(provider, model)>`; load validation error names `china.model`.
   - `Config::china_model() -> &str` (configured or default), `china_model_configured()`.
   - `china_display_label(selector)` = text after the last `/`.
   - `engine_label`: `cli == CHINA_LABEL` → display label.
   - `write.rs`: `china.model` settable (Str).
2. **Executor** (`crates/rival-core/src/executor/china.rs` + `china/tests.rs`)
   - `china_effort(requested) -> Result<&'static str>` (map in spec decision 2).
   - `china_preflight(cfg)`: look_path + `opencode --version` major ≥ 2.
   - `china_run_args(selector)`, `china_config_content(selector, effort, mode)`, `china_run_env(...)`.
   - `run_china` / `run_china_with` (spawn injected), raw drops `KIMI_DROP_ENV`.
   - Export from `executor/mod.rs`.
3. **CLI** (`crates/rival`)
   - `model_specs.rs`: `CHINA_USAGE`, `china_spec(cfg)`; `resolve_effort` branch for china.
   - `tree.rs` + `root.rs`: `rival command china`.
   - `config_cmd.rs`: `china.model` row in `config show`.
4. **Skill** (`crates/rival-core/skills/china/SKILL.md`, `skills.rs`)
   - `NAMES = ["rival", "china"]`; `codex_skill` handles `china` (command `china`).
5. **Docs**: README section, CHANGELOG `Unreleased`.
6. **Tests**: unit tests per task; `cargo test`, `cargo clippy`, `make` gate if present.
7. **Live check**: `echo 'review' | rival command china --workdir <repo>` on the Dell; project-config bypass check.
8. **Exit gate**: `/rival review` of the diff, fix, `/simplify`, report.
