# Parsed output view

**Date:** 2026-09-29
**Status:** done

- User ask: the right-hand Output pane shows the raw log. Parse it and show the result as a good-looking view.
- Keep the raw log as its own tab. If parsing fails, show the error and point to Raw.
- README: new screenshot of one selected, opened run. All data fake (no real project names, paths or findings).
- Go already has the parse rules: `review.FinalAnswer`, `ParseReviewerOutput`, `ParsePlanOutput` (rival/internal/review). Port them to Swift, like `LogReader.swift` ports `logfmt`.
- Log formats seen: codex transcript (header, `exec`/`codex` blocks, JSON or markdown answer after the last `codex` line), plain JSON (fable plan), plain markdown (claude native).
