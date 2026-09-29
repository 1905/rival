# TUI Redesign Implementation Plan v1.0

**Date:** 2026-09-26
**Status:** done
**Spec:** ./spec.md

**Goal:** Rebuild `rival tui` as a phosphor-green, filterable list + preview + tabbed detail on bubbles components, showing real model ids.
**Architecture:** One root bubbletea `Model` routes keys by mode (list / filter / detail / search / confirm) through a `bubbles/key` keyMap. It owns three panes: `listPane` (custom cell-width renderer over `[]row`), `previewPane`, and `detailPane` (`viewport` + tabs). Data flow is unchanged: `WatchSessions` → `SessionEvent` → `groupSessions`.
**Tech Stack:** Go, charm.land/bubbletea/v2 v2.0.8, charm.land/bubbles/v2 v2.1.1 (key, help, textinput, viewport, spinner), charm.land/lipgloss/v2 v2.0.5 (`Blend2D`), github.com/charmbracelet/x/ansi.

> For agentic workers: use superpowers:subagent-driven-development to implement task-by-task. Checkbox syntax for tracking.

**Implementer rules (every dispatch states these verbatim):**
- Model: Opus 5.5 (`model: "opus"`).
- Scope: write the code and unit tests the task names, and run only `go test ./internal/dashboard/...` plus `go build ./...` and `go vet ./internal/dashboard/...`. NEVER run e2e / integration / live / smoke tests. Never rent a GPU or pod, never call a provider API, never publish, deploy or touch infra, never run anything money-bearing. Never run `rival tui` against the real `~/.rival`.
- Tests that create sessions MUST `t.Setenv("HOME", t.TempDir())` first. Never read or write `~/.rival`. Never set any test-DB env var.
- Don't commit. The orchestrator commits per phase.

## File map

**Modify**
- `rival/internal/dashboard/styles.go` — palette, styles, gradient logo, header.
- `rival/internal/dashboard/model.go` — root model, mode routing, layout, header, View.
- `rival/internal/dashboard/session_list.go` — `listPane`, rows, sections, filter, tabs, columns.
- `rival/internal/dashboard/detail_view.go` — `detailPane`: tabs, members, follow, search, kill confirm.
- `rival/internal/dashboard/logview.go` — drop the Public* rewrite and group concatenation.
- `rival/internal/dashboard/{viewport,session_list,detail_view,logview,parity}_test.go` — port or replace.

**Create**
- `rival/internal/dashboard/keys.go`, `keys_test.go`
- `rival/internal/dashboard/preview.go`, `preview_test.go`
- `rival/internal/dashboard/layout.go`, `layout_test.go`
- `rival/internal/dashboard/styles_test.go`

**Unchanged:** `kill.go`, `watcher.go`, `cmd/tui.go` (`dashboard.New()` keeps its signature), everything outside `internal/dashboard`.

## Locked interfaces

These names are fixed. Every task uses exactly these.

```go
// styles.go
var colFg, colDim, colAccent, colRunning, colQueued, colOK, colFail color.Color // hex values per spec palette
var logoStops = []color.Color{"#7C3AED", "#22D3EE", "#39FF14"}
func renderLogo() string                          // bannerLines with Blend2D gradient, 45°, cached (sync.Once)
type headerStats struct{ Running, Queued, Completed, Failed, Total int; Version string }
func renderHeader(width int, compact bool, st headerStats, spin string) string
const compactHeaderBelowHeight = 30

// keys.go
type mode int
const ( modeList mode = iota; modeFilter; modeDetail; modeSearch; modeConfirm )
type keyMap struct {
    Up, Down, Top, Bottom, Open, Back, Filter, NextTab, PrevTab, Help, Quit,
    Follow, Search, NextMatch, PrevMatch, NextMember, PrevMember,
    TabOutput, TabPrompt, TabInfo, OpenLog, Stop, Yes, No key.Binding
}
func defaultKeys() keyMap
func (k keyMap) help(m mode) help.KeyMap           // returns a modeHelp{short, full [][]key.Binding}

// layout.go
type layout struct{ Width, Height, HeaderH, BodyH, ListW, PreviewW int; ShowPreview, Compact, TooSmall bool }
func computeLayout(width, height int) layout
const ( minWidth = 60; minHeight = 16; previewMinWidth = 120 )

// session_list.go
type statusTab int
const ( tabAll statusTab = iota; tabRunning; tabFailed; tabDone )
type row struct{ Section string; Item *displayItem }   // Section != "" → non-selectable header row
func modelName(s *session.Session) string              // s.Model verbatim; s.CLI if empty
func groupModelName(item *displayItem) string          // first member model, + " +N" when N>0 other distinct models
func kindLabel(item *displayItem) string               // review|plan|mega|sec|slop|raw, "/dk" suffix for docker claude
func projectName(workdir string) string                // filepath.Base; "-" if empty
func sectionFor(t, now time.Time) string               // TODAY|YESTERDAY|THIS WEEK|OLDER (local time)
func matchesFilter(item *displayItem, terms []string) bool
func buildRows(items []displayItem, tab statusTab, filter string, now time.Time) []row
func statusGlyph(status, spin string) string           // spin for running, "◌" queued, "✓", "✗", "·"
func fitCell(s string, w int) string                   // ansi.Truncate(s, w, "…") then right-pad to exactly w cells
type columns struct{ Status, Kind, Model, Effort, Time, Project, Prompt int }
func layoutColumns(width int) columns                  // drops Effort then Project below 90 cols
type listPane struct{ rows []row; cursor, offset int; tab statusTab; filter textinput.Model }
func (l *listPane) setItems(items []displayItem, now time.Time)
func (l *listPane) move(delta int)                     // skips section rows
func (l *listPane) top(); func (l *listPane) bottom()
func (l *listPane) selected() *displayItem
func (l *listPane) view(width, height int, spin string, focused bool) string

// preview.go
type previewPane struct{ key string; body string }
func (p *previewPane) refresh(item *displayItem, width, height int)
func (p previewPane) view(width, height int) string
const previewTailLines = 200

// detail_view.go
type detailTab int
const ( tabOutput detailTab = iota; tabPrompt; tabInfo )
type killConfirm struct{ targets []*session.Session }
type detailPane struct {
    tab detailTab; member int; vp viewport.Model; follow bool
    search textinput.Model; query string; matches []int; matchIdx int
    confirm *killConfirm; prompts map[string]string
}
func (d *detailPane) open(item *displayItem)            // loads prompts, member=0, tab=Output, follow=true
func (d *detailPane) sync(item *displayItem, width, height int, resetBottom bool)
func (d *detailPane) view(item *displayItem, width, height int, spin string) string
func findMatches(lines []string, query string) []int    // case-insensitive, line indexes
func infoLines(s *session.Session, width int) []string

// model.go
type Model struct { /* existing fields minus pagination */ keys keyMap; help help.Model; mode mode;
    list listPane; preview previewPane; detail detailPane; spin spinner.Model; lay layout;
    kill func(pid int, sig syscall.Signal) error }  // New() sets kill = syscall.Kill
```

## Self-test sanity check (before Task 1)

- [ ] `git status` clean on the feature branch. `cd rival && go build ./... && go test ./internal/dashboard/...` → `ok`.

## Phase P1 — look, list, model fix

### Task 1 — palette + gradient logo + header
**Files:** Modify `styles.go`. Create `styles_test.go`.
- [ ] Test: `renderLogo()` has 5 lines, and each line's `lipgloss.Width` equals the widest `bannerLines` entry. The output contains ≥3 distinct `38;2;` colour sequences (the gradient is applied). A second call returns the same string (cached).
- [ ] Test: `renderHeader(200, false, st, "⠋")` has 5 lines, each ≤200 cells, and contains the version and the four counts. `renderHeader(80, true, …)` is 1 line ≤80 cells. `renderHeader(59, …)` never exceeds 59 cells.
- [ ] Implement the palette tokens, styles (`selectedStyle` = accent bg + black fg bold, `dimStyle`, `borderStyle` with a dim rounded border), `statusStyle(status)`, `renderLogo`, `renderHeader`. Compact form: `rival` with the gradient per rune + stats on one line.
- [ ] `go test ./internal/dashboard/ -run 'Logo|Header'` → PASS.

### Task 2 — keymap + help
**Files:** Create `keys.go`, `keys_test.go`.
- [ ] Test: every binding in `defaultKeys()` has non-empty keys and a help text. `help(modeList).ShortHelp()` includes Up, Down, Open, Filter, NextTab, Help and Quit. `help(modeDetail).FullHelp()` includes Follow, Search, NextMember, OpenLog and Stop. `help(modeConfirm).ShortHelp()` is exactly Yes, No.
- [ ] Bindings: Up `k/↑`, Down `j/↓`, Top `g/home`, Bottom `G/end`, Open `enter`, Back `esc`, Filter `/`, NextTab `tab`, PrevTab `shift+tab`, Help `?`, Quit `q/ctrl+c`, Follow `f`, Search `/` (detail), NextMatch `n`, PrevMatch `N`, NextMember `]`, PrevMember `[`, TabOutput `1`, TabPrompt `2`, TabInfo `3`, OpenLog `o`, Stop `x`, Yes `y`, No `n/esc`.
- [ ] PASS.

### Task 3 — row data: model, kind, project, sections, filter
**Files:** Modify `session_list.go` (add the funcs, keep the old render until Task 5). Test in `session_list_test.go`.
- [ ] Tests (table-driven):
  - `modelName`: `gpt-6-astra`→`gpt-6-astra`, `claude-fable-5`→`claude-fable-5`, `{CLI:"codex",Model:""}`→`codex`.
  - `groupModelName`: 3 members with 3 distinct models → `first +2`.
  - `kindLabel`: review, plan, megareview→mega, security→sec, antislop→slop, raw, native→review, docker claude→`review/dk`.
  - `projectName("/a/b/orbit-web")`→`orbit-web`.
  - `sectionFor`: today, yesterday, 3 days ago → THIS WEEK, 30 days → OLDER.
  - `matchesFilter`: terms ANDed, case-insensitive over status/kind/model/effort/project/PromptPreview/ReviewScope/ID[:8].
  - `buildRows`: tab filter composes with the text filter, empty sections are omitted, and each section header comes before its items.
- [ ] Implement. For a group, `kindLabel` uses `sessionview.Kind` and maps `megareview`→`mega`. A solo run maps its Mode (`native`/`docker`/`review`/"" → `review`).
- [ ] Add `BenchmarkBuildRows3000`. `go test -bench BuildRows -run ^$ ./internal/dashboard/` < 1ms/op. Record the number in the task report.
- [ ] PASS.

### Task 4 — cell-width columns
**Files:** `session_list.go`, `session_list_test.go`.
- [ ] Test: for widths 60/90/120/200 and rows with `✓`, a spinner frame, `claude-opus-4-6[1m]`, CJK in the project and a 300-char prompt, every rendered row's `lipgloss.Width` equals exactly the pane width. `layoutColumns(89)` has Effort=0. `layoutColumns(70)` has Project=0.
- [ ] Implement `fitCell`, `statusGlyph`, `layoutColumns` (Status 3, Kind 8, Model 20, Effort 7, Time 7, Project 16, Prompt = rest ≥0; single-space gaps).
- [ ] PASS.

### Task 5 — listPane + root model wiring (list mode)
**Files:** `session_list.go`, `model.go`, `viewport_test.go`.
- [ ] Tests:
  - `move` skips section rows, and `top`/`bottom` land on items.
  - The cursor stays on the same `itemKey` after `setItems` with a new row inserted above (port `TestDetailSelectionSurvivesReorder` to the list).
  - `/` focuses the filter. Typing narrows the rows, `esc` clears and blurs, and `enter` keeps the filter and blurs.
  - `tab`/`shift+tab` cycle the status tabs, and the tab bar shows counts.
  - An empty result shows `no runs match "<q>" · esc clears`.
  - `q` inside the filter types `q` and does not quit.
  - Port `TestViewNeverExceedsWidth` for widths 60/90/120/200.
- [ ] Implement: remove `pageSize`, `visibleCount`, `paginateItems`, `hasMore` and `l`. Root routing uses `key.Matches` per mode. The spinner (`spinner.MiniDot`) ticks only while `hasRunning`. The `?` toggle flips `help.ShowAll`. The layout comes from one `computeLayout` (Task 7 extends it; for now: header + tab bar + list + help). Keep the old detail view reachable via `enter` (it's replaced in P3).
- [ ] `go build ./... && go test ./internal/dashboard/...` → PASS.

### Task 6 — P1 cleanup + parity
**Files:** `session_list_test.go`, `parity_test.go`.
- [ ] Remove `cliLabel`, `groupIcon`, the icon consts and their tests (`TestCLILabelUsesPublicModelNames`, `TestCLIIconsAreSingleCellWide`, `TestGroupIconReflectsSelectedReviewerCount`, `TestPairedPlanGroupUsesPublicModelsAndPlanIcon`). Replace them with `modelName`/`kindLabel` assertions for the same fixtures.
- [ ] `parity_test`: keep the status/effort/kind/elapsed parity with `sessionview`. Drop the models-vs-EngineLabels assertion and assert `groupModelName` instead.
- [ ] `go vet ./internal/dashboard/... && go test ./internal/dashboard/...` → PASS.

**P1 gate (orchestrator):** build + tests green. tmux capture at 200×50 and 90×30 against a temp HOME seeded with fixture sessions (never the real `~/.rival`). Commit `feat(tui): phosphor theme, gradient logo, filterable list with real model ids`.

## Phase P2 — preview + layout

### Task 7 — layout
**Files:** Create `layout.go`, `layout_test.go`.
- [ ] Test:
  - `computeLayout(200,50)` → ShowPreview, ListW+PreviewW+1 == 200, !Compact.
  - `(119,50)` → !ShowPreview.
  - `(120,29)` → Compact, HeaderH==1.
  - `(59,30)` and `(80,15)` → TooSmall.
  - HeaderH + 1 (tab bar) + BodyH + 1 (help) == Height whenever !TooSmall.
- [ ] Implement. ListW = 55% of width, min 60. The preview takes the rest minus a 1-col border.
- [ ] PASS.

### Task 8 — previewPane
**Files:** Create `preview.go`, `preview_test.go`.
- [ ] Test:
  - Single run: the body contains the project, kind, `modelName`, effort, elapsed, started, the scope if set, and the last lines of the log.
  - Group: one line per member with glyph + model + status.
  - Missing log → `(log unavailable: …)`.
  - Every line ≤ width.
  - `refresh` with the same key and a non-running item does not re-read the file (inject via a counter on a package-level `readTail` var = `logfmt.ReadTail`).
- [ ] Implement with `wrapLogLines` (Task 10 changes its naming, not its signature). Keep the last `height-metaLines` lines.
- [ ] PASS.

### Task 9 — wire preview + too-small guard
**Files:** `model.go`, `viewport_test.go`.
- [ ] Test:
  - At 200×50 the frame has the list and the preview side by side inside borders, and no line exceeds 200 cells.
  - At 100×40 there's no preview.
  - At 50×20 the frame is exactly `terminal too small (need 60×16)`.
  - Moving the cursor refreshes the preview.
  - A tick refreshes the preview only when the selected run is running.
- [ ] Implement: `lipgloss.JoinHorizontal` of the bordered list and preview. The focused pane border uses the accent colour.
- [ ] PASS.

**P2 gate:** same as P1, plus captures at 120×40 and 70×24. Commit `feat(tui): live preview pane and responsive layout`.

## Phase P3 — detail screen

### Task 10 — log text without the public rewrite
**Files:** `logview.go`, `logview_test.go`, `viewport_test.go`, `model.go` (open-log helpers).
- [ ] Test: a log containing `gpt-6-astra` renders `gpt-6-astra` in `wrapLogLines`. ANSI is still stripped and tabs are still expanded. `createLogView` writes the raw log (no Public* rewrite).
- [ ] Remove `publicLogText`, `buildGroupLogContent`, `TestDetailLogRedactsModelIDSplitByANSI` and `TestCreatePublicLogViewSanitizesRuntimeMetadata`. Rename `openPublicLog`→`openLog`, `openPublicGroupLogs`→`openGroupLogs`, `createPublicLogView`→`createLogView`, `createPublicGroupLogView`→`createGroupLogView`, `createPublicTextView`→`createTextView`. Group errors use the raw `ErrorMsg`.
- [ ] PASS.

### Task 11 — detailPane: tabs + members + follow
**Files:** `detail_view.go`, `detail_view_test.go`.
- [ ] Tests:
  - `open` → tab Output, member 0, follow on.
  - `1/2/3` switch tabs, and `tab` cycles them in detail.
  - `]`/`[` wrap across members ordered by `session.SortGroupMembers` (judge last).
  - The member bar shows each `modelName`, and the judge is labelled `judge`.
  - Prompt tab shows the full prompt from `session.Load` (temp HOME), or the preview + `(full prompt unavailable)`.
  - Info tab lists every field from the spec plus the full error.
  - Scrolling up turns follow off. `G` and `f` turn it on and jump to the bottom. A tick while following keeps the bottom visible (port `TestDetailViewportStickyBottom`).
  - Header breadcrumb: `rival › <project> › <kind> <id8>` + glyph + status + elapsed.
- [ ] Implement. Remove `renderDetailMeta`, `clampMeta` and `renderSingleDetailMeta`/`renderGroupDetailMeta`, with their tests `TestGroupDetailReservesSpaceForEveryPlanLog`, `TestSingleDetailStillShowsItsEffort`, `TestGroupLogsDistinguishJudgeAndIncludeAllMembers` and `TestDetailViewHandlesTinyTerminal`. Replace them with the tests above plus a tiny-terminal test (TooSmall guard).
- [ ] PASS.

### Task 12 — search
**Files:** `detail_view.go`, `detail_view_test.go`.
- [ ] Tests:
  - `findMatches` is case-insensitive and returns line indexes.
  - `/` in Output focuses search. `enter` runs it, and the viewport jumps to the first match.
  - `n`/`N` cycle and wrap. The status line reads `<i>/<n> matches`.
  - Zero matches → `no matches`.
  - `esc` clears.
  - Matched substrings are rendered with the accent background, and line widths don't change.
- [ ] PASS.

### Task 13 — kill confirm
**Files:** `detail_view.go`, `model.go`, `detail_view_test.go`, `kill_safety_test.go`.
- [ ] Tests (inject `Model.kill` with a recorder):
  - `x` with 2 running members → the confirm bar reads `stop 2 running sessions? y/n`, and nothing has been signalled yet.
  - `n`/`esc` → no signal, and the bar closes.
  - `y` → SIGTERM to both PIDs and `failSessionForKill` is called (temp HOME).
  - `x` on a finished run → `nothing running`, no signal.
  - The existing `TestKillReloadsBeforeFail` still passes.
- [ ] PASS.

### Task 14 — wire detail into the root model
**Files:** `model.go`, `viewport_test.go`.
- [ ] Tests:
  - `enter` opens detail, `esc` returns to the list with the same cursor.
  - Port `TestDetailScrollKeysReachTheViewport` and `TestDetailExitsWhenSelectionDisappears`.
  - Detail at 200/120/90/60 widths never exceeds the width or height.
  - `q` in the search input types `q`.
- [ ] Remove the old detail path left from P1.
- [ ] `go build ./... && go vet ./internal/dashboard/... && go test ./internal/dashboard/...` → PASS.

**P3 gate:** manual tmux run of the full spec checklist at 200×50, 120×40, 90×30 and 70×24 against a fixture HOME. One read-only look against the real `~/.rival` (open a single run and a group run, search, `x`→`n`; never `y`). Commit `feat(tui): tabbed detail with members, follow, search and kill confirm`.

## Exit gates (orchestrator)

1. `/rival-codex review` on the whole branch (codex is the current reviewer name after the 3.34 rename). Verify each finding, fix what holds. If it fails, fall back to a self-review with `feature-dev:code-reviewer`.
2. `/simplify` on the diff.
3. Reconcile spec.md (`## As-built notes`), plan → done, move the dir to `plans/done/`.
4. Merge to `master` the same day it's green (merge-hygiene rule), then `/notify`.

## Type-consistency check

`modelName`, `groupModelName`, `kindLabel`, `projectName`, `buildRows`, `row`, `statusTab`, `listPane`, `previewPane`, `detailPane`, `detailTab`, `killConfirm`, `computeLayout`, `layout`, `keyMap`, `mode`, `renderLogo` and `renderHeader` are used with the same names and signatures in Tasks 1-14. `wrapLogLines(s, width)` keeps its signature across Tasks 8 and 10. `failSessionForKill` is unchanged.
