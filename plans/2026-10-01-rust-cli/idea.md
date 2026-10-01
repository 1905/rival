# Rust rewrite: CLI + TUI only

**Date:** 2026-10-01
**Status:** done

- User ask: another plan — rewrite the `rival` CLI and the TUI in Rust, no app work.
- Alternative to `plans/2026-10-01-rust-rewrite` (core + tray + Tauri window). That plan stays on file.
- Size: Go ~11.7k source + ~10.8k test lines; the TUI (`internal/dashboard`) is 3.3k of it. CLI ships for darwin/linux amd64/arm64 via goreleaser + brew formula.
- The CLI is the worker (git scope, prompts, reviewer subprocesses, queue, detach, sessions, MR reviews, skills, update); the app only reads what it writes. Most risk is exact behaviour parity with Go.
- Open: what happens to the Swift app's hand-copied logic once a Rust core exists.
