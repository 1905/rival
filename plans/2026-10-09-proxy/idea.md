# Proxy support, config window, Fable and Sol reviewers

**Date:** 2026-10-09
**Status:** spec

The user runs CLIProxyAPI (`cliproxyapi` container, `http://127.0.0.1:8317`). It holds two Claude accounts with the model prefixes `emcd_/` and `emcd2_/` (`force-model-prefix: true`). It will hold a Codex (ChatGPT) account after a one-time `-codex-device-login`.

rival must send Claude and Codex runs through that proxy: one URL, one key from a file, a model prefix for each provider. Users set this in a TUI config window and in a Rival.app settings pane. A "Check" button sends "hi" to each model that rival uses and shows a check mark or the error.

The same spec brings back two reviewers next to Codex and Claude (Opus 5.5): **Fable** (`claude-fable-5-1`, Claude Code runtime) and **Sol 6.1** (Codex runtime). Both run direct or through the proxy.
