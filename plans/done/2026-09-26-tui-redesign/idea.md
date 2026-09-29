# TUI redesign — idea

**Date:** 2026-09-26
**Status:** done

User (2026-09-26): "this tui is broken as shit… model is wrong. inside each screen is broken shit too" →
"fully redesign it. logo i like, hacker style feel. but make great UX" → "use bubbletea stuff".

Observed on `rival tui` (160x45 capture, 2982 sessions):
- REVIEWER and MODEL columns print the same label ("codex"); older sessions show "retired-model" in both.
- Columns misalign when a label is wider than its slot ("⬡ retired-model" pushes every later column right).
- 2982 sessions in one flat list, paginated 100 at a time with `l`; no search, no filter.
- Detail screen: 8+ lines of meta, a 10-line prompt block, then a raw log tail. The review result is buried.
- `x` kills a run with no confirmation.

First-guess scope: rebuild `rival/internal/dashboard` on bubbles components (table, viewport, help, key,
spinner, textinput), keep the ASCII logo, green-on-black terminal look, fix model naming and the detail view.
