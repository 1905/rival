# Rust rewrite + Tauri app

**Date:** 2026-10-01
**Status:** done

- User ask: rewrite rival in Rust and share as much code as possible between the CLI and the app.
- App: Tauri + Mantine UI, dark theme, as little custom CSS as possible. Drop the hacker theme in the app for now.
- Why: zero code is shared today. The Swift app hand-ports ~1,600 of its 4,602 lines from Go, and the copies already drifted twice (codex double answer, the "no answer → whole log" fallback fixed in Swift only).
- Size today: Go ~11.7k source + ~10.8k test lines in 16 packages (TUI `internal/dashboard` 3.3k). Swift app 4.6k.
- First-guess shape: a Rust workspace — one core crate (sessions, parsing, queue, config, executors), a CLI crate, and a Tauri app whose backend calls the core crate directly. The Mantine frontend only renders.
