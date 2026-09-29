# Leak guard

**Date:** 2026-09-29
**Status:** done

- Incident: Rival.app 4.0.0 grew ~38 MB/s from launch (124 MB -> 2233 MB in 60s), 100% CPU on the main thread, reached ~200 GB before it died.
- Cause: TimelineView spinner in the MenuBarExtra label (fixed, uncommitted: static label).
- User picked option A: a launch soak test in `make test` (slow first-scan fixture, sample footprint ~30s, fail on steady growth) plus an in-app background watchdog that exits above 1 GB.
- Goal: catch this class of bug before release, and cap the damage at 1 GB if one ships.
