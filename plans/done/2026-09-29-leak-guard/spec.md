# Leak guard

**Date:** 2026-09-29
**Scope:** /Users/kass/dev/rival/app, /Users/kass/dev/rival/Makefile
**Status:** done

## TL;DR

**P1 — Ship the spinner fix.**
- What: commit the static menu bar label (already in the working tree, verified flat at ~80-85 MB) and cut a patch release.
- Why: the 4.0.0 app grows ~38 MB/s from launch until it dies. 4.0.0 was never pushed (origin is at v3.34.0), so only this Mac ran it.
- You decide: the release version (4.0.1 proposed).
- Does NOT: add any guard. P2 and P3 do that.

**P2 — In-app memory watchdog.**
- What: a background timer checks the app's own memory every 5s. Above 1 GB it logs the numbers and quits the app.
- Why: the next leak we don't foresee then stops at 1 GB, not 200 GB. It works even when the main thread is stuck, like in this bug.
- You decide: nothing (1 GB limit, fixed in code).
- Does NOT: restart the app, send telemetry, or show a dialog.

**P3 — Launch soak test in `make test`.**
- What: `make test` also builds the dev app, launches it against a fixture, keeps it in the loading state for 30s, then idle for 20s, and fails if memory grows or CPU stays high.
- Why: the unit tests passed because fixtures load in milliseconds, so the loading-state loop never ran.
- You decide: nothing. `make test` gets ~2 min slower.
- Does NOT: run in GitHub CI (not verified that a macOS runner can show a menu bar app).

## Problem(s)

1. **A render loop shipped.** `MenuBarLabel` put a `TimelineView` spinner in the `MenuBarExtra` label (`app/Sources/RivalApp/MenuBar.swift:14`, commit `4c6fdb9`). Each frame set the status item image, which forced a relayout and another render. The loop was synchronous on the main thread. The first scan publishes on the main actor (`SessionStore.swift` `refresh()`), so `isLoading` never became false and the loop never ended. Measured: 124 MB → 2233 MB in 60s. The macOS CPU report `/Library/Logs/DiagnosticReports/Rival_2026-09-29-112206_MacBook-Pro.cpu_resource.diag` shows 89 MB → 3495 MB in 85s, with `-[NSStatusBarButton setImage:]` in the heaviest stack.
2. **Nothing caps memory.** The app has no self-limit, so the leak ran until the Mac ran out.
3. **No test runs the real app.** `make test` runs `swift test` only (`Makefile`). Fixtures load in milliseconds, so loading-state UI is never on screen long enough to misbehave.

## Goals

1. (P1) Users on 4.0.0 get the fix.
2. (P2) Any runaway memory growth stops the app at 1 GB footprint.
3. (P3) A loop or leak in the launch or loading state fails `make test` before a release.

## Non-goals

- Finding every possible leak. The soak test covers launch, loading and idle only.
- Memory optimization (options D/E, not chosen).
- Crash reporting or telemetry.

## Watchdog

```
DispatchSourceTimer (queue: global .utility, every 5s, leeway 1s)
  └─ footprint = task_info(TASK_VM_INFO).phys_footprint
       ├─ < 1 GiB  → nothing
       └─ ≥ 1 GiB  → Logger(subsystem "dev.1905.rival", category "watchdog").fault(…)
                     append one line to ~/Library/Logs/Rival/watchdog.log:
                       2026-09-29T12:36:30+08:00 footprint=1073741824 limit=1073741824 version=4.0.1
                     exit(70)
```

- `phys_footprint` is the number Activity Monitor and `footprint(1)` show. Checked: `footprint -p` reported it during the incident.
- It runs on a global queue, not the main thread. The incident's main thread never returned, so a main-thread check would never have fired.
- Exit, not `abort()`: no crash dialog. The log line is the record.
- Code in `RivalKit/MemoryWatchdog.swift`. The footprint reader and the limit check are injected, so the unit test drives it without allocating 1 GB.
- `RIVAL_WATCHDOG_LIMIT_MB` env override, DEBUG builds only. The soak test uses it to prove the watchdog fires.

## Soak test

`app/scripts/soak_test.py` (Python, per the house rule), called from `make test` after `swift test`.

```
1. dev_bundle.py --build
2. dev_bundle.py --fixture $TMP/home --many 300
3. launch: open -n --env RIVAL_HOME=$TMP/home --env RIVAL_DEBUG_SCAN_DELAY=30 app/.build/Rival.app
4. every 2s for 50s: footprint -p PID, ps -o %cpu= -p PID
5. kill PID, move $TMP/home to /tmp/trash
6. fail if any:
     max footprint                       > 300 MB
     footprint slope over the last 20s   > 0.5 MB/s
     median CPU over seconds 30-50       > 20 %
     mean CPU over seconds 5-30          > 50 %   (loading state; the incident was 100 %)
7. watchdog proof: relaunch with RIVAL_WATCHDOG_LIMIT_MB=20; pass if the process exits
   within 15s and watchdog.log gained a line.
```

- `RIVAL_DEBUG_SCAN_DELAY` (DEBUG builds only): the first scan sleeps N seconds before it reads, so `isLoading` stays true and loading-state UI stays on screen. This is the state that broke.
- Thresholds are first guesses. Measured healthy numbers so far: 77-83 MB debug, 85 MB release, CPU 0-8 % idle with one 38 % sample. The plan runs the test 5 times on the fixed build and sets the final thresholds with a clear margin, and runs it once on the broken commit to prove it fails.
- The test launches a GUI app on the user's screen for ~1 min. It closes it at the end.

## File-level changes

| File | Change |
|---|---|
| `app/Sources/RivalApp/MenuBar.swift` | P1: the static label (already done, uncommitted). |
| `app/Sources/RivalKit/MemoryWatchdog.swift` (new) | Timer, footprint reader, limit check, log line, exit hook (injected). |
| `app/Sources/RivalApp/RivalApp.swift` | `AppDelegate.init` starts the watchdog before the store. |
| `app/Sources/RivalApp/RivalApp.swift` | DEBUG-only `RIVAL_DEBUG_SCAN_DELAY`: `AppDelegate` starts the store after the delay (as built; SessionStore unchanged). |
| `app/scripts/soak_test.py` (new) | The steps above. Prints the samples table on failure. |
| `Makefile` | `test:` runs `swift test`, then `python3 app/scripts/soak_test.py`. New `soak:` target for the soak alone. |
| `app/Tests/RivalKitTests/MemoryWatchdogTests.swift` (new) | See Tests. |
| `CHANGELOG.md` | P1 entry. |

## Tests

- **Unit:** watchdog below the limit does nothing. At or above it, it writes one log line and calls the exit hook once. A failed `task_info` call reads as "unknown" and does nothing.
- **Soak, red:** check out `4c6fdb9`'s `MenuBar.swift` into a scratch copy, build, run the soak test → must fail on footprint. Restore.
- **Soak, green:** 5 runs on the fixed build → all pass. Record the numbers in the plan.
- **Watchdog, live:** step 7 of the soak test.

## Failure modes & decisions

| Failure | Behaviour |
|---|---|
| A real large `~/.rival` (6100 sessions, 1.7 GB) pushes a healthy app over 1 GB | Not expected: measured 118 MB peak on exactly that. If it happens, the watchdog log shows it and the limit gets raised. |
| `footprint` or `ps` missing or the app fails to launch in the soak test | Test fails with that reason. Never a silent pass. |
| Soak test run over SSH with no GUI session | Fails with "no GUI session". Run it locally. |
| Watchdog quits during a user action | Accepted. The app is past 1 GB, so something already broke. |
| Log dir not writable | `Logger.fault` still records it. Exit anyway. |

## Out of scope

- GitHub CI soak run.
- Auto-restart after a watchdog exit.
- Memory optimization (options D/E).
- Leak checks for the main window's detail views over long sessions.

## Rollout

- **P1** — commit the spinner fix, CHANGELOG, release 4.0.1 via `rival-release`. One commit.
- **P2** — watchdog + unit tests. One commit.
- **P3** — scan-delay hook, soak test, Makefile, red/green runs. One commit.
