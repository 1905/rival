# Rust core + tray + on-demand window — Implementation Plan v1.0

**Date:** 2026-10-01
**Status:** draft
**Spec:** ./spec.md (P1 only)

**Goal:** replace the Swift app with a Rust `rival-core`, an always-on tray process and a Tauri window process that exits on close, at ≤ 30 MB idle and ≤ 150 MB open on 6000 runs.
**Architecture:** Cargo workspace at the repo root. `rival-core` owns all logic and its tests. `rival-tray` (tao + tray-icon + muda) and `rival-window` (Tauri 2, plain HTML/CSS/TS) only call it. One `Rival.app` holds both binaries; the tray is the main executable and spawns the window.
**Tech Stack:** Rust 1.98, tauri 2, tao 0.37, tray-icon 0.26, serde/serde_json, chrono, pulldown-cmark + ammonia (markdown → safe HTML), fd-lock (lock files), notify-rust (notifications), Vite + TypeScript (no UI framework), Python 3 scripts. Go side unchanged except one contract test.

> For agentic workers: use superpowers:subagent-driven-development to implement task-by-task. Checkbox syntax for tracking.

**Branch:** `feature/rust-core` from `master` in the main checkout (no worktree; user rule overrides the skill). Exec starts only from a clean `master`.

**Tags:** `heavy` = Opus 5.5 implementer · `light` = small, Opus or orchestrator · `gate` = orchestrator only (launches apps, measures, screenshots, commits).

**Implementer restriction (paste verbatim in every dispatch):** "You write the code and unit tests your task names and run the focused unit tests for that task (`cargo test -p <crate> <filter>`, `npm run build`, `npx vitest run`). You NEVER launch the app, run the soak test, run e2e / integration / live / smoke tests, publish, deploy, push, touch infra, or run anything money-bearing. Do not commit; the orchestrator commits. Do not set any test-DB env var. Never delete files with rm; move them to /tmp/trash/<name>.<timestamp>."

## File map

**Create**
- `Cargo.toml` (workspace), `Cargo.lock`, `rust-toolchain.toml` (1.98)
- `crates/rival-core/{Cargo.toml, src/lib.rs, src/paths.rs, src/sessions.rs, src/summary.rs, src/store.rs, src/view.rs, src/logs.rs, src/result.rs, src/markdown.rs, src/finish.rs, src/stop.rs, src/watchdog.rs}` + `crates/rival-core/tests/contract.rs`
- `crates/rival-tray/{Cargo.toml, src/main.rs, src/menu.rs, src/launcher.rs, src/settings.rs}`
- `desktop/src-tauri/{Cargo.toml, build.rs, tauri.conf.json, src/main.rs, src/commands.rs, src/single.rs}`, `desktop/ui/{index.html, src/main.ts, src/api.ts, src/views/*.ts, src/styles.css, package.json, vite.config.ts, tsconfig.json}`
- `testdata/sessions/*.json`, `testdata/logs/*.log`, `testdata/expected.json`
- `rival/internal/session/testdata_contract_test.go`
- `scripts/bundle.py`, `scripts/dev_bundle.py`, `scripts/soak_test.py`, `scripts/render_cask.py`, `scripts/appbundle.py` (moved from `app/scripts`, rewritten where noted)

**Modify**
- `Makefile`, `.github/workflows/release.yml`, `.gitignore` (`target/`, `desktop/ui/node_modules`, `desktop/ui/dist`), `README.md`, `CHANGELOG.md`, `assets/app.png`

**Remove (P1d, moved to /tmp/trash then `git add -A`)**
- `app/` (Swift package, its scripts after they move)

**Out of scope:** `rival/` Go CLI/TUI code (except the contract test), Linux/Windows packaging, notarization.

## Lock-down: rival-core public API (names every task must use)

```rust
// paths
pub fn root(env: &HashMap<String, String>) -> PathBuf;     // RIVAL_HOME or ~/.rival
pub fn sessions_dir(root: &Path) -> PathBuf;                // <root>/sessions
pub fn app_dir(root: &Path) -> PathBuf;                     // <root>/app (locks, socket, settings)

// sessions + summary
pub struct Session { id, group_id: Option<String>, cli, mode, model, effort, review_scope: Option<String>,
    prompt_preview: Option<String>, status: String, start_time: DateTime<Utc>, queued_at: Option<..>,
    queue_position: Option<i64>, end_time: Option<..>, exit_code: Option<i64>, duration: Option<String>,
    work_dir: String, log_file: String, output_bytes: i64, output_lines: i64, error: Option<String>,
    account: Option<String>, pid: i32, pid_start: Option<i64>, owner_pid: Option<i32>, owner_pid_start: Option<i64> }
pub fn load_summary(path: &Path) -> Option<Session>;        // no prompt; head/tail read for big files (Swift SessionSummary)
pub fn load_prompt(path: &Path) -> Option<String>;
pub enum RunStatus { Running, Queued, Completed, Failed, Unknown }   // from_str, is_live()

// store
pub struct Store;  impl Store {
  pub fn new(root: PathBuf) -> Self;
  pub fn scan(&mut self, progress: Option<&dyn Fn(usize, usize)>) -> ScanOutcome; // mtime+size cache; skips .json.tmp*
  pub fn runs(&self) -> &[Run];  pub fn counts(&self) -> Counts;  pub fn directory_exists(&self) -> bool;
}
pub struct ScanOutcome { pub changed: bool, pub file_count: usize }

// view
pub struct Run { pub id: String /* "solo:<id>" | "group:<gid>" */, pub sessions: Vec<Session> }
impl Run { pub fn primary(&self) -> &Session; pub fn status(&self) -> RunStatus; pub fn is_group(&self) -> bool; }
pub fn group_runs(sessions: Vec<Session>) -> Vec<Run>;      // newest first
pub struct Counts { pub all: usize, pub live: usize, pub failed: usize, pub done: usize, pub running: usize, pub queued: usize }
pub enum StatusTab { All, Live, Failed, Done }
pub fn filter_runs<'a>(runs: &'a [Run], tab: StatusTab, query: &str) -> Vec<&'a Run>;
pub enum Section { Today, Yesterday, ThisWeek, Older }  pub fn section(run: &Run, now: DateTime<Local>) -> Section;
pub struct Page { pub page: usize, pub pages: usize, pub start: usize, pub end: usize }
pub fn page(total: usize, page: usize, per: usize /* 50 */) -> Page;
pub fn page_of(index: usize, per: usize) -> usize;
pub fn run_elapsed(run: &Run, now: DateTime<Utc>) -> String;  pub fn session_elapsed(s: &Session, now: DateTime<Utc>) -> String;
pub fn model_name(s: &Session) -> String;  pub fn kind_label(run: &Run) -> String;  pub fn project(s: &Session) -> String;
pub fn info_rows(s: &Session) -> Vec<(String, String)>;

// logs
pub const MAX_TAIL_BYTES: usize = 256 * 1024;
pub struct Tail { pub text: String, pub truncated: bool }
pub fn read_tail(path: &Path, max: usize) -> io::Result<Tail>;   // aligned past first newline when truncated, invalid UTF-8 dropped
pub fn sanitize(raw: &str) -> String;                              // CR frames, ANSI strip, C0 drop, tabs → 4 spaces

// result + markdown
pub fn final_answer(raw: &str) -> &str;          // Swift rules: last "codex" line, tokens-used footer + hook lines dropped, double copy deduped
pub fn json_objects(s: &str) -> Vec<&str>;       // Go brace-stack scan
pub enum RunResult { Findings { summary: String, rating: Option<u8>, groups: Vec<SeverityGroup> },
                     Markdown { html: String }, Failed { reason: String } }
pub struct Finding { file, line: u32, severity, category, title, body, failure_scenario: Option<String>, suggestion: Option<String>, confidence: u8 }
pub struct SeverityGroup { pub severity: String /* critical|high|medium|low|other */, pub findings: Vec<Finding> }
pub fn parse_run_result(raw: &str) -> RunResult; // unanswered codex transcript → Failed("no answer in the log") BEFORE the JSON scan
pub fn markdown_html(text: &str) -> String;      // pulldown-cmark → ammonia; no raw HTML, only http(s)/mailto links, no images

// finish, stop, watchdog
pub struct FinishDetector;  impl FinishDetector { pub fn new() -> Self; pub fn observe(&mut self, runs: &[Run]) -> Vec<String /* run ids that went live → done */>; }
pub trait ProcessInspector { fn start_time(&self, pid: i32) -> Option<i64>; fn signal(&self, pid: i32, sig: i32) -> io::Result<()>; }
pub struct StopTarget { pub session_id: String, pub pid: i32 }
pub fn stop_candidates(run: &Run, insp: &dyn ProcessInspector) -> Vec<StopTarget>;
pub fn perform_stop(root: &Path, run: &Run, confirmed: &[String], insp: &dyn ProcessInspector) -> String /* toast text */;
pub fn current_footprint() -> Option<u64>;     // macOS proc_pid_rusage ri_phys_footprint; other OS: None until P4
pub struct Watchdog;  impl Watchdog { pub fn start(limit: u64, log: PathBuf, version: String, exit: fn(i32)) -> Self; }
```

## Self-test sanity check (before Task 1)

- [ ] `git status` clean on `master`; `git checkout -b feature/rust-core master`
- [ ] `cargo --version` → 1.98.x; `node --version` → v26.x
- [ ] `cd rival && go test ./...` green; `cd app && swift test` → 187 tests, 0 failures (baseline for parity)

---

## P1a — core read side, skeletons, RAM gate

### Task 1 — workspace scaffold `light`
Files: Create `Cargo.toml`, `rust-toolchain.toml`, `crates/rival-core/{Cargo.toml,src/lib.rs}`, `crates/rival-tray/{Cargo.toml,src/main.rs}`; Modify `.gitignore`.
- [ ] Workspace members `crates/rival-core`, `crates/rival-tray`, `desktop/src-tauri`; `[profile.release]` lto = true, codegen-units = 1, opt-level = "s", panic = "abort", strip = true.
- [ ] `cargo build` (core + tray only; `desktop` member added in Task 9) → success, 0 warnings.

### Task 2 — shared testdata + contract tests `heavy`
Files: Create `testdata/sessions/*.json` (copy of `app/Tests/Fixtures/*.json`), `testdata/logs/*.log` (copy of `app/Tests/Fixtures/logs/*`), `testdata/expected.json`, `rival/internal/session/testdata_contract_test.go`, `crates/rival-core/tests/contract.rs`.
- [ ] `expected.json`: per fixture file → `{id, group_id, cli, mode, model, effort, status, start_time (RFC3339), end_time, work_dir, log_file, exit_code, pid}`; `unknown-key.json` included (unknown keys ignored).
- [ ] Go test decodes each file with the Go `session` loader and compares to `expected.json` → `go test ./internal/session/ -run Contract` passes.
- [ ] Rust test (written failing first, passes after Task 3) does the same with `load_summary`.

### Task 3 — paths, Session, summary loader `heavy`
Files: `crates/rival-core/src/{paths,sessions,summary}.rs`.
- [ ] Port `app/Sources/RivalKit/{Session,SessionSummary}.swift` behaviour exactly: field decoding, `RunStatus`, prompt skipped, big-file head/tail read.
- [ ] Port all 12 `SessionDecodingTests` + 5 `SessionSummaryTests` cases as table-driven Rust tests → `cargo test -p rival-core sessions summary` green; `contract.rs` green.

### Task 4 — store `heavy`
Files: `crates/rival-core/src/store.rs`.
- [ ] Port `SessionStore.swift` `SessionScanner` semantics: mtime+size cache, temp files ignored, failed decode keeps last good copy and retries, removed files dropped, `changed` only when something changed, unchanged scans do not rebuild the run list, progress every 100 files. No threads inside the store (callers own timing).
- [ ] Port the applicable `SessionStoreTests` (scan/cache/temp/missing dir; skip SwiftUI/actor timing ones, list them in the report) → `cargo test -p rival-core store` green.
- [ ] Bench test (ignored): 6000-session fixture cold scan < 1.5 s release.

### Task 5 — view `heavy`
Files: `crates/rival-core/src/view.rs`.
- [ ] Port `Grouping.swift`, `Filter.swift`, `Pagination.swift`, and the label/elapsed/info-row helpers from `RunDetailModel.swift`.
- [ ] Port 24 `GroupingTests` + 10 `FilterTests` + 17 `PaginationTests` → `cargo test -p rival-core view` green.

### Task 6 — logs `heavy`
Files: `crates/rival-core/src/logs.rs`.
- [ ] Port `LogReader.swift` (`readTail`, `sanitizeLog`, ANSI state machine, tab expand).
- [ ] Port 11 `LogTests` → green.

### Task 7 — result + markdown `heavy`
Files: `crates/rival-core/src/{result,markdown}.rs`; Cargo deps `pulldown-cmark`, `ammonia`.
- [ ] Port `ResultParser.swift` incl. the review fix (unanswered transcript fails before JSON scan) and severity groups; `markdown_html` replaces Swift `MarkdownBlocks` (CommonMark lazy list continuation comes free).
- [ ] Port 41 `ResultParserTests` (markdown-block cases become HTML assertions) + new: `<script>` stripped, `javascript:` link dropped, `<img>` removed, the three fake logs in `testdata/logs` → findings(6, rating 6) / markdown / failed.
- [ ] `cargo test -p rival-core result markdown` green.

### Task 8 — tray skeleton `heavy`
Files: `crates/rival-tray/src/{main,menu,launcher}.rs`; deps `tao`, `tray-icon`, `fd-lock`.
- [ ] `main`: take `app_dir/tray.lock` (held → exit 0); `Store::new(root)`, first scan on a background thread, rescan every 2 s; tray title `r N` (N = live) or `r`; menu: "Open Rival", separator, "Quit".
- [ ] `menu::menu_model(runs: &[Run], now) -> MenuModel` pure (live ≤ 10, recent ≤ 5, labels) — unit tests with a fixture store.
- [ ] `launcher::open(select: Option<&str>)`: connect `app_dir/window.sock`, send `select <id>\n`; on failure spawn `<current_exe dir>/rival-window [--select <id>]` detached. Unit test with a fake socket path and a fake spawner trait.
- [ ] Debug-only env hooks: `RIVAL_TEST_OPEN_AFTER=<s>` opens the window after s seconds; `RIVAL_DEBUG_SCAN_DELAY=<s>` delays the first scan.
- [ ] `cargo test -p rival-tray` green; `cargo build -p rival-tray` 0 warnings.

### Task 9 — window skeleton with prototype UI `heavy`
Files: `desktop/src-tauri/*`, `desktop/ui/*` (start from the throwaway prototype at `/private/tmp/claude-501/-Users-kass-dev-rival/73c68d01-003b-4c38-8719-c648947619b2/scratchpad/gui/proto-a`, copy UI files, then rewire); add `desktop/src-tauri` to workspace.
- [ ] Binary name `rival-window`; one window 1180×760 created at start; app exits when it closes (no tray in this process); activation policy Regular.
- [ ] `single.rs`: take `app_dir/window.lock`; held → send `select <id>` to `window.sock` and exit 0; else listen on `window.sock` and emit `select-run`. `--select <id>` CLI arg handled.
- [ ] Commands now: `list_runs` only, backed by `rival-core` (Store owned in Tauri state, rescan thread every 2 s emitting `runs-changed`). Other tabs may show prototype placeholders until Task 11.
- [ ] Remove `marked` from `package.json` (markdown comes from core in Task 11).
- [ ] Debug-only `RIVAL_TEST_CLOSE_AFTER=<s>` closes the window after s seconds.
- [ ] `npm run build` + `cargo build -p rival-window` 0 errors; `cargo test -p rival-window` green.

### Task 10 — RAM + latency gate `gate`
- [ ] Orchestrator: `scripts/dev_bundle.py --build` (Task 19 early minimal version: assemble `Rival (dev).app` with both binaries, LSUIElement=true) — or run binaries directly.
- [ ] Fixture: `dev_bundle.py --fixture <tmp> --many 6000`.
- [ ] Measure (python, process set = tray + window + every `com.apple.WebKit.*` started after launch): tray idle t=10 s; window open (+8 s and +30 s); 10 s and 40 s after close; window open latency (spawn → first `runs-changed` handled, logged by the window to stderr with a timestamp).
- [ ] Pass: idle ≤ 30 MB before and after close; open ≤ 150 MB; window process gone after close; latency recorded. Fail → STOP, notify user with the table.
- [ ] Commit P1a: `feat(rust): rival-core read side, tray and window skeletons`.

## P1b — window feature parity

### Task 11 — window commands `heavy`
Files: `desktop/src-tauri/src/commands.rs`.
- [ ] `run_detail(run_id)`, `run_result(session_id)` (non-live: `read_tail` + `parse_run_result`; live → `{kind:"live"}`), `log_tail(session_id)` (sanitized), `prompt(session_id)` (`load_prompt`), `stop_preview(run_id)`, `stop_run(run_id, session_ids)`; JSON shapes as spec "Window boundary".
- [ ] Each command has a test against a fixture `RIVAL_HOME` copied from `testdata` into a temp dir → `cargo test -p rival-window` green.

### Task 12 — UI parity `heavy`
Files: `desktop/ui/src/**`, `desktop/ui/index.html`, `styles.css`.
- [ ] From prototype A, cleaned into `views/{list,detail,result,raw,info,stop}.ts` + `api.ts` (typed wrappers). Fixed: header block fixed, only the list scrolls; `[hidden]{display:none!important}`; exactly one of empty-state/detail visible; first run auto-selected; finished → Result, live → Raw, auto-switch on finish only when on Raw with follow.
- [ ] Markdown: render `html` from core with `innerHTML` (already sanitized); everything else via `textContent`.
- [ ] States: loading progress, empty dir, parse failure + "→ Open Raw", live note, Stop modal, member pills, keyboard ↑/↓/←/→, ⌘F in Raw (native find not available → simple filter-highlight in Raw).
- [ ] Vitest for pure helpers (formatting, tab rules) → `npx vitest run` green; `npm run build` 0 errors.

### Task 13 — screenshot QA `gate`
- [ ] Orchestrator screenshots on fake fixture: list, Result findings, markdown, parse failure, live Raw, Info, Stop modal, loading, empty dir. Look at each; fix loop via Task 12 implementer (max 3 rounds).
- [ ] Commit P1b: `feat(rust): window UI parity on rival-core`.

## P1c — tray parity, notifications, stop, watchdog, soak

### Task 14 — finish + stop in core `heavy`
Files: `crates/rival-core/src/{finish,stop}.rs`.
- [ ] Port `FinishDetector.swift`, `ProcessGuard.swift`, `StopMark.swift`; `SystemInspector` (libc kill, proc start time via `proc_pidinfo` on macOS).
- [ ] Port 9 `FinishDetectorTests` + 12 `StopTests` + 8 `StopMarkTests` → green.

### Task 15 — tray complete `heavy`
Files: `crates/rival-tray/src/{main,menu,settings}.rs`; dep `notify-rust`.
- [ ] Menu: Open Rival, up to 10 live runs (click → open with select), "+N more — open Rival", up to 5 recent, separator, "Notify on finish" checkbox (persisted in `app_dir/settings.json`), Quit.
- [ ] On each changed scan: `FinishDetector::observe` → notification "Rival: <kind> <model> finished|failed" when enabled. Click-to-open is best effort (record in report if the crate can't route clicks on macOS).
- [ ] Unit tests: settings round-trip; menu model with >10 live.

### Task 16 — watchdog `heavy`
Files: `crates/rival-core/src/watchdog.rs`; use in tray and window `main`.
- [ ] Port `MemoryWatchdog.swift` (5 s timer thread, 1 GiB, one log line to `~/Library/Logs/Rival/watchdog.log`, exit 70, fires once; DEBUG env `RIVAL_WATCHDOG_LIMIT_MB`).
- [ ] Port 5 `MemoryWatchdogTests` → green.

### Task 17 — soak test port `heavy`
Files: `scripts/soak_test.py` (from `app/scripts/soak_test.py`).
- [ ] Launch the dev bundle's tray with `RIVAL_DEBUG_SCAN_DELAY=30`, `RIVAL_TEST_OPEN_AFTER=40`, window `RIVAL_TEST_CLOSE_AFTER=30`; sample every 2 s for 110 s over the process set (tray, window, WebKit children).
- [ ] Checks: tray loading CPU mean < 30 %, tray idle ≤ 30 MB, window phase ≤ 150 MB, window process gone ≤ 10 s after close, idle after close ≤ 30 MB, no steady growth (slope over the last 10 idle samples ≤ 0.3 MB/s); watchdog proof at `RIVAL_WATCHDOG_LIMIT_MB=10` on the tray.
- [ ] `python3 -m py_compile` + `--help` only (implementer); runs are Task 18.

### Task 18 — soak gate `gate`
- [ ] Orchestrator runs `make soak` 3× on the fixture → green; records numbers in this plan's status.
- [ ] Commit P1c: `feat(rust): tray menu, notifications, stop, watchdog, soak test`.

## P1d — packaging, docs, Swift removal, release

### Task 19 — bundle scripts `heavy`
Files: `scripts/{appbundle,bundle,dev_bundle,render_cask}.py` (moved from `app/scripts`, rewritten).
- [ ] `bundle.py --version X.Y.Z [--arch universal|native]`: `npm ci && npm run build` in `desktop/ui`; `cargo build --release -p rival-tray -p rival-window` for aarch64 + x86_64, `lipo` both; assemble `Rival.app` (CFBundleExecutable = rival-tray, `rival-window` beside it, LSUIElement = true, bundle id `dev.1905.rival`, icon from `assets`); ad-hoc sign; zip `Rival-app.zip`; DMG `Rival-X.Y.Z.dmg` (same `make_dmg` as today). Prints `dmg=`, `sha256=`, `zip=`.
- [ ] `dev_bundle.py`: same, debug, `Rival (dev).app`, `--fixture/--many/--select` kept.
- [ ] `test_appbundle.py` updated → `python3 scripts/test_appbundle.py` OK.

### Task 20 — release workflow + Makefile `light`
- [ ] `release.yml` `app` job: macos-15, setup Node 26 + Rust 1.98 (both targets), `cargo test --workspace`, `python3 scripts/bundle.py`, upload zip + DMG, cask update unchanged.
- [ ] `Makefile`: `run` (dev bundle), `install` (release native into /Applications, old copy to /tmp/trash), `test` (`cargo test --workspace` + `go test` + `make soak`), `soak`.

### Task 21 — docs + screenshot `light` + `gate`
- [ ] README Rival.app section: two processes, RAM numbers measured in Task 18, same install commands; formal register. CHANGELOG `[Unreleased]` entry.
- [ ] Orchestrator: `assets/app.png` from the fake fixture (Result tab of a finished plan run).

### Task 22 — remove the Swift app `light`
- [ ] Move `app/` to `/tmp/trash/app.<ts>`, `git add -A`; grep the repo for `app/Sources`, `swift test`, `app/scripts` → none left outside `plans/done`.
- [ ] `cargo test --workspace`, `go test ./...` (in `rival/`) green.
- [ ] Commit P1d: `feat(rust): Rival.app from rival-core + tray + window; Swift app removed`.

### Task 23 — exit gate `gate`
- [ ] `/rival-codex review` on `master..feature/rust-core`; verify each finding; fix what holds; `/simplify`; soak once more.
- [ ] Merge to `master` the same day; release is the user's call (version bump, SSH-alias push per project memory).
- [ ] Reconcile spec.md (As-built notes), plan status `done`, move the feature dir to `plans/done/`.

## Type-consistency check

- `Run.id` format `solo:<id>` / `group:<gid>` used by Tasks 5, 8, 9, 11, 15 — same as the Swift app's `RunItem.id`.
- `RunResult` variants (Task 7) match the window JSON `kind` values (Task 11): `findings`, `markdown`, `failed`, plus window-side `live`.
- `app_dir` files: `tray.lock`, `window.lock`, `window.sock`, `settings.json` (Tasks 8, 9, 15).
- Env hooks: `RIVAL_HOME`, `RIVAL_DEBUG_SCAN_DELAY`, `RIVAL_TEST_OPEN_AFTER`, `RIVAL_TEST_CLOSE_AFTER`, `RIVAL_WATCHDOG_LIMIT_MB` (Tasks 8, 9, 16, 17) — debug builds only.
- Binary names `rival-tray`, `rival-window` (Tasks 8, 9, 19).

## Self-review notes

- Every spec goal maps: G1 → Tasks 3-7, 14, 16; G2/G3 → Tasks 10, 17, 18; G4 → Tasks 11-15; G5 → Task 2; G6 → Tasks 19-20; G7 → API locks (`current_footprint` returns None off macOS, sockets via `cfg(unix)`; Windows named pipe deferred to P4 with a compile-time stub).
- Known risk: two processes in one bundle with one bundle id. Notifications come from the tray (main executable), so the identity is the bundle's. The window is spawned as a helper binary, not via LaunchServices; Task 10 verifies it gets a Dock icon and focus.
- Known risk: notification click routing on macOS via `notify-rust` may not deliver clicks; Task 15 reports it rather than adding a native workaround.
