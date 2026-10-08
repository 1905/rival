# Inherited bugs

**Date:** 2026-10-08
**Status:** done

The Rust port kept 23 Go bugs on purpose, so that its output matched Go (table "Known Go bugs" in `plans/2026-10-01-rust-cli/plan-v2.10.md`). Go is gone, so the reason is gone. The user wants them triaged and fixed in their own spec, after the rust-only spec and before the review-language spec. The most important one: the Claude Docker run puts the auth token in the process arguments.
