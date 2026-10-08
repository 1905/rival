# Rust-only rival

**Date:** 2026-10-08
**Status:** done

The user wants to finish the Rust work before the review-language feature. The Rust CLI is on master (PR #16, merged 2026-10-08, not released). The code still copies Go behaviour on purpose: Go JSON encoding, Go error text, Go Unicode tables, Go Windows path rules and about 1,600 "Go `x`" comments. The user wants no Go left.

Also remove two features that the language work replaces: the `rival-antislop` code-slop review and the first word-check attempt (`ste_rewrite`).

First-guess scope: remove rival-antislop, remove the word-check code, remove the Go-compat layers, rename the scenario harness and re-baseline its expected output. The 22 preserved Go bugs and the language feature are separate specs.
