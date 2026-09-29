# TUI redesign — `rival tui`

**Date:** 2026-09-26
**Scope:** rival/internal/dashboard (+ rival/cmd/tui.go)
**Status:** done

## TL;DR

**P1 — Look, list and model fix.**
- **What:** rebuild the list screen on bubbles components: filterable table, status tabs, spinner, `?` help.
- **Look:** phosphor-green theme. The same ASCII logo gets a violet→cyan→green gradient.
- **Model column:** shows the real model id (`gpt-6-astra`, `claude-opus-5-5`, `gpt-5.5`), not "codex"/"retired-model".
- **Why:** duplicate and wrong model columns, misaligned rows, 2982 runs in one unsearchable list.
- **You do:** nothing.
- **Not done:** the web dashboard keeps its public labels. Only the TUI shows real ids, in both the list and the logs.

**P2 — Preview pane.**
- **What:** a split view like k9s/lazygit, with the list on the left and the selected run on the right (meta + live log tail).
- **Layout:** the preview hides below 120 columns.
- **Why:** today you must open every run to see anything.
- **You do:** nothing.
- **Not done:** no mouse support.

**P3 — Detail screen.**
- **What:** Enter opens a full screen with tabs: Output · Prompt · Info. Group runs get one sub-tab per model plus the judge.
- **Output tab:** live follow, `/` search with `n`/`N`.
- **Stop:** `x` asks y/n before it kills a run.
- **Why:** the current detail screen buries the result under 20+ lines of meta and prompt, and `x` kills a run instantly.
- **You do:** nothing.
- **Not done:** no parsed "Findings" tab. You still read the raw review text.

## Problem(s)

1. **Model column is wrong and duplicated.** Both REVIEWER and MODEL render `config.EngineLabel` (`internal/dashboard/session_list.go:244-245`). That gives "codex" twice for `gpt-6-astra`. `EngineLabel` also falls back to `"retired-model"` for every claude/fable/opencode run with an old id (`internal/config/config.go:136-137`). Measured on this machine: 245 `claude-fable-5` runs and 132 `claude-fable-5-1` runs are unreadable. The 533 `gpt-5.5` runs are mislabelled "sol" by the codex fallback (`config.go:132-133`).
2. **Rows misalign.** Columns are padded with `fmt %-*s` (`session_list.go:153,242`), which counts bytes, not display cells. Icons like `◈`/`⬡` are 3 bytes. Any label longer than its slot (`⬡ retired-model` in a 10-wide column) shifts every later column right. Seen in the 160×45 capture.
3. **The list is unusable at scale.** 2982 sessions (5948 files, 1.7 GB) sit in one flat list, paginated 100 at a time with `l` (`model.go:62,404-408`). There is no search and no status filter. WORKDIR shows a truncated absolute path (`/Users/kass/…`), so the one useful part, the project name, gets cut. There is no start time, so you can't tell today's run from last month's.
4. **Running, completed and failed rows share one icon.** `statusIcon` returns `●` for all three (`session_list.go:287-294`). Only colour tells them apart.
5. **The detail screen buries the result.** Meta (8-10 lines) plus a 10-line prompt plus headings take ~25 rows before the log starts (`detail_view.go:52-91`). On a 45-row terminal that leaves ~12 rows of log. Group runs concatenate every member's log into one stream (`logview.go:73-106`), so the judge verdict sits after thousands of reviewer lines.
6. **`x` kills without confirmation** (`model.go:329-350`), one key away from `p`/`o`.
7. **Help is a hand-built string** (`model.go:662-677`). There is no `?`, and key handling is a hand-ordered string switch with a documented ordering trap (`model.go:287-290`).

## Goals

1. Show the real model id in the TUI list, preview and detail. Show kind and runtime separately (Problem 1).
2. Keep every row aligned at any width by measuring display cells (Problem 2).
3. Add instant filtering (`/`), status tabs, rows grouped by recency, and a project column instead of an absolute path. No manual pagination (Problem 3).
4. Give each status its own glyph plus colour, and use an animated spinner for running rows (Problem 4).
5. Add a preview pane beside the list, plus a tabbed full detail screen that opens on Output. Group runs get per-member tabs (Problem 5).
6. `x` asks for confirmation (Problem 6).
7. Use `bubbles/key` + `bubbles/help` for every binding. `?` shows the full help (Problem 7).
8. Hacker look: phosphor-green theme, and the same ASCII logo with a gradient (user request, not a bug).

## Non-goals

- Changing the web dashboard (`internal/server`), its labels or its API.
- Parsing review findings into structured data.
- Mouse support, themes, or a config file for colours.
- Changing session storage, the reaper or the watcher protocol.

## Visual design

**Palette** (truecolor; lipgloss degrades it automatically on 256/16-colour terminals):

| token | hex | use |
|---|---|---|
| `fg` | `#B8FFB8` | body text |
| `dim` | `#3E6B4A` | secondary text, borders, help |
| `accent` | `#39FF14` | selection bar, active tab, focus border |
| `running` | `#FFB000` | amber: spinner, running status |
| `queued` | `#6B8F7A` | grey-green |
| `ok` | `#39FF14` | ✓ completed |
| `fail` | `#FF3B3B` | ✗ failed |
| `logo` | gradient `#7C3AED → #22D3EE → #39FF14` | `lipgloss.Blend2D`, 45°, one colour per cell |

**Glyphs:** running = spinner (`spinner.MiniDot`), queued `◌ #N`, completed `✓`, failed `✗`, killed/unknown `·`.

**Screen, wide (≥120 cols):**

```
   _____(_)   ______ _/ /         (gradient logo, 5 rows)        ⠋ 1 running  ◌ 0 queued
  / ___/ / | / / __ `/ /                                           ✓ 2684  ✗ 296   3.34.0
 / /  / /| |/ / /_/ / /
/_/  /_/ |___/\__,_/_/
 ALL 2982   RUNNING 1   FAILED 296   DONE 2684                / orbit█
┌────────────────────────────────────────────────────┬──────────────────────────────────┐
│ ST KIND    MODEL            EFF    TIME  PROJECT   │ orbit-web · review · codex        │
│ TODAY                                              │ gpt-6-astra · xhigh · 1m31s       │
│ ⠋  review  gpt-6-astra      xhigh  1m31s orbit-we… │ started 11:40 · pid 81233         │
│ ✓  review  gpt-6-astra      xhigh  6m24s harbor-o… │ ─ scope ─                         │
│ ✓  plan    claude-opus-5-5  medium 2m36s ledger    │ plans/2026-09-26-service-iden…    │
│ YESTERDAY                                          │ ─ output (tail) ─                 │
│ ✗  mega    gpt-5.5 +2       high   23s   quartz-…  │ VERDICT: 6/10                     │
│                                                    │ 1. HIGH  fingerprint re-key …     │
└────────────────────────────────────────────────────┴──────────────────────────────────┘
 ↑/k up · ↓/j down · enter open · / filter · tab status · ? more · q quit
```

- The header collapses to one line (`rival ⠋1 ✓2684 ✗296 3.34.0`, logo gradient applied to the word) when height < 30.
- Below 120 columns the preview hides. Below 90, EFF and PROJECT drop in that order.
- Section rows (`TODAY`, `YESTERDAY`, `THIS WEEK`, `OLDER`) are not selectable. The cursor skips them.
- Group row MODEL: first member's model plus `+N` (`gpt-5.5 +2`). The preview lists all members.
- KIND values: `review`, `plan`, `mega`, `sec`, `slop`, `raw` (from `Mode` / `sessionview.Kind`). Docker claude runs append `/dk`.

**Detail screen (enter):**

```
 rival › orbit-web › review 94334ac6          ⠋ running 1m31s   f follow ●
 [ Output ]  Prompt  Info                       (group:  [ gpt-5.5 ] gemini-3.1  judge )
────────────────────────────────────────────────────────────────────────────────────────
 <viewport: full log, tail-following while running>
────────────────────────────────────────────────────────────────────────────────────────
 /fingerprint  3/7 matches · n next · N prev · esc clear
 tab/1-3 tab · [/] member · f follow · / search · o open · x stop · esc back · ? more
```

- **Output:** the full log. The tail is read with `logfmt.ReadTail` as today, and the "earlier output omitted — o opens full log" marker stays. Follow is on while running and pauses when you scroll up. `f` or `G` resumes it. Search highlights matches in the viewport, and `n`/`N` jump between them.
- **Prompt:** the full prompt, word-wrapped and scrollable. It is loaded on entry via `session.Load` (the list only has summaries).
- **Info:** all fields (id, group id, cli, model, effort, mode, status, exit, started, ended, duration, queued-at, workdir, scope, account, pid, output bytes/lines, log path) and the error message in full.
- **Group:** `[`/`]` switch member. Order follows `session.SortGroupMembers`, so the judge comes last.
- **`x`:** an inline confirm bar `stop 2 running sessions? y/n`. `y` runs the existing `failSessionForKill` path. Anything else cancels.

## Architecture

bubbletea v2 (`charm.land/bubbletea/v2`), bubbles v2.1.1, lipgloss v2.0.5. All are already in `go.mod`.

```
Model (root)
 ├─ keys   keyMap (bubbles/key) ─── help.Model (bubbles/help, ? toggles ShowAll)
 ├─ list   listPane
 │    ├─ filter  textinput.Model   (/ focuses; esc clears; enter keeps + blurs)
 │    ├─ tab     statusTab         (all|running|failed|done; tab / shift+tab)
 │    ├─ rows    []row             (built from allItems → filtered → sectioned)
 │    └─ cursor/offset             (own render; see "why not bubbles/table")
 ├─ preview previewPane            (meta block + last N wrapped log lines)
 ├─ detail  detailPane
 │    ├─ tab     output|prompt|info
 │    ├─ member  int               (group member index)
 │    ├─ vp      viewport.Model
 │    ├─ search  textinput.Model + []matchLine + cur
 │    └─ confirm *killConfirm
 ├─ spin   spinner.Model           (ticks only while something runs)
 └─ theme  (styles.go)             (palette, gradient logo cached per width)
```

**Why not `bubbles/table`:** it pads cells with `runewidth` and can't render non-selectable section rows or per-cell styles (status colour, dim project). It also re-renders every row on every `SetRows`. A 60-line custom renderer over `[]row` with `ansi.Truncate`/`lipgloss.Width` per cell handles both, and still uses `key`/`help`/`viewport`/`textinput`/`spinner` from bubbles.

**Filter:** case-insensitive substring over `status kind model effort project promptPreview reviewScope id[:8]`. Space-separated terms are ANDed. It is recomputed on every keystroke. 3000 rows × ~200 bytes stays under 1 ms (verify with a benchmark in P1).

**Selection anchoring:** keep today's `itemKey` + `reanchorSelection` semantics (`model.go:153-193`). A refresh never swaps the selected run, and a vanished run returns detail to the list.

**Data flow** is unchanged: `WatchSessions` → `SessionEvent` → `groupSessions` → `allItems`. Filtering and sectioning are derived views, rebuilt on event, filter change and tab change.

**Model naming in the TUI:** `modelName(s)` returns `s.Model` verbatim, or `s.CLI` when it is empty. Logs and errors in the TUI skip `config.PublicRuntimeLog`/`PublicRuntimeError` and only get `sanitizeLog`. `o` (open in editor) writes the raw log too, so the id you see and the id in the file match.

## File-level changes

| file | change |
|---|---|
| `internal/dashboard/styles.go` | Replace with the phosphor palette tokens and styles above, plus `renderLogo(width)`, which applies a `Blend2D` gradient per cell (cached until width changes). |
| `internal/dashboard/keys.go` (new) | `keyMap` with `key.Binding`s for list, detail, filter and confirm modes. Implements `help.KeyMap` (`ShortHelp`/`FullHelp`) per mode. |
| `internal/dashboard/model.go` | Root model: sub-panes, mode routing (list / filter-focused / detail / search-focused / confirm), `key.Matches` routing, spinner lifecycle, header + collapse rule, layout math (one `layout` struct computed on WindowSizeMsg and shared by View and Update). Drop `pageSize` pagination. Keep `openPublicLog*` minus the Public* rewrite. |
| `internal/dashboard/session_list.go` | Rewrite as `listPane`: rows, sections, filter, status tabs, cell-width columns, `modelName`, KIND mapping, relative project name (`filepath.Base(WorkDir)`). |
| `internal/dashboard/preview.go` (new) | `previewPane`: meta block + last N wrapped log lines of the selected run. Re-read on selection change, on SessionEvent and on tick while that run is running. |
| `internal/dashboard/detail_view.go` | Rewrite as `detailPane`: tabs, member switching, per-tab viewport content, search with highlight, follow state, kill confirm. |
| `internal/dashboard/logview.go` | Keep `wrapLogLines` tail reading and wrapping. Remove the `PublicRuntimeLog` call. Remove group concatenation (members are separate tabs now). |
| `internal/dashboard/kill.go` | Unchanged. |
| `internal/dashboard/watcher.go` | Unchanged. |
| `internal/dashboard/*_test.go` | Update `viewport_test`, `session_list_test`, `detail_view_test`, `logview_test`, `parity_test`. The model column now asserts the raw id, not `EngineLabel`. Status, effort, kind and elapsed parity with `sessionview` stays. |
| `cmd/tui.go` | Unchanged apart from any constructor signature change. |

## Tests

**Unit (`internal/dashboard`)**
- `modelName`: `gpt-6-astra` → `gpt-6-astra`, `claude-fable-5` → `claude-fable-5`, empty model → cli.
- Row render: every row's `lipgloss.Width` equals the pane width for widths 60/90/120/200, with labels containing multi-byte glyphs (regression for Problem 2).
- Filter: AND of terms. It matches project, model and prompt preview. An empty filter returns all rows. The status tab composes with the filter.
- Sections: the cursor skips section rows (j/k/g/G). Sections disappear when empty.
- Anchoring: a new running session inserted above keeps the selected run selected. A removed run in detail mode returns to the list (port the existing tests).
- Detail: the group member order matches `SortGroupMembers`, and `[`/`]` wrap. Follow pauses on scroll-up and resumes on `G`/`f`. Search finds and cycles matches. `x` then `n` sends no signal (inject the killer func).
- Layout: `contentHeight` is shared by View and viewport sizing, and no frame line exceeds the terminal width or height (port the existing `viewport_test` guarantees).
- Logo: each rendered logo line's `lipgloss.Width` equals `bannerWidth`, and the collapsed header renders at height 29.
- Benchmark: filtering 3000 rows.

**Manual (tmux capture, 200×50, 120×40, 90×30, 70×24)**
- List, filter `orbit`, tab to FAILED, open a single run, open a group run and switch members, search, `x` → `n`.
- Compare against the real `~/.rival/sessions` (read-only; never press `y` on a confirm).

## Failure modes & decisions

| failure | behaviour |
|---|---|
| Terminal < 60×16 | Show `terminal too small (need 60×16)` instead of a broken frame. |
| Truecolor unsupported | lipgloss downsamples. The gradient becomes banded, which is acceptable. |
| Log file missing/unreadable | Output tab shows `(log unavailable: <err>)`. Preview shows the same, dimmed. |
| Log > `logfmt.MaxTailBytes` | Tail only, plus the "earlier output omitted — o opens full log" line (as today). |
| Filter matches nothing | Empty pane with `no runs match "<filter>" · esc clears`. |
| Selected run vanishes (deleted) | List: cursor clamps. Detail: back to list (as today). |
| `x` on a run that already finished | Confirm bar says `nothing running` and closes. No signal is sent. |
| `session.Load` fails for prompt | Prompt tab shows the 100-char preview plus `(full prompt unavailable)`. |
| Spinner with nothing running | The tick stops, so there is no idle CPU. It restarts on the next SessionEvent with a running row. |
| 3000+ rows | Only the visible window is rendered. Filter is O(n) per keystroke and benchmarked. |

## Out of scope

- Web dashboard naming and layout.
- Findings parser / structured verdicts.
- Deleting or archiving old sessions from the TUI.
- Mouse, custom themes, colour config.
- Re-running a review from the TUI.

## Rollout

- **P1** — theme + gradient logo + `keys.go`/help + list rewrite (model column, cell-width columns, filter, status tabs, sections, spinner). Old detail view still reachable. Commit.
- **P2** — preview pane + responsive layout + small-terminal guard. Commit.
- **P3** — detail screen rewrite (tabs, members, follow, search, kill confirm), log naming without the Public* rewrite. Commit.

## As-built notes

Shipped 2026-09-26 on `feat/tui-redesign` (P1 66d1ac3, P2 13e1468, P3 eb0016e, review fixes b497d86, simplify 416405a). Drift from the text above:

- **Split widths.** The list never drops below its fixed columns (`listFixedWidth` 68 + border). A bare 55% split dropped EFF and PROJECT at 120-145 cols. Column thresholds are the fixed sums: EFF drops below 68 inner cols, PROJECT below 60, and PROMPT shrinks first.
- **Queued rows** show `#N <wait>` in TIME. The 3-cell status column can't hold `◌ #N`.
- **Detail header.** The logo header stays above the detail screen (the drawing omitted it). `backspace` also closes detail.
- **Member bar** labels the judge `judge`, not its model id. The Info tab shows the judge's model.
- **Kill safety.** `x` only offers runs whose process still matches `PIDStart` (`procinfo.Alive`). A dead "running" record gets `nothing running`. `y` re-checks status and PIDStart again before SIGTERM. Found by the branch review; the same gap existed on master.
- **Search** uses a custom `lipgloss.StyleRanges` highlight over pre-wrapped lines, not the viewport's built-in highlights. It never turns follow on.
- **Log reads** are cached by path/size/mtime/width. The preview reads at most 32 KB and cuts raw lines before sanitizing.
- **Refresh.** One 1s tick chain and one spinner chain, both started only while something is live.
- **Deliberately not done** (simplify pass): migrating search to `viewport.SetHighlights`/`SoftWrap` (a search rewrite that changes match placement and line numbering), and `ansi.Wrap` for prompt wrapping (it can drop leading spaces).
