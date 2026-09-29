# Leak guard — plan v1.0

**Date:** 2026-09-29
**Spec:** spec.md (approved)
**Status:** done (as built: the scan delay lives in AppDelegate, which starts the store late under DEBUG; SessionStore has no delay code). Final review: Codex + /simplify applied 2026-09-29 (P1: 62668c4 + release a03a9f9, local only, push held)

Calibration 2026-09-29 (dev build, fixture with 300 runs, scan held 30s):
- Green, 5 runs: max 48-49 MB, slope 0.000-0.076 MB/s, loading CPU 6.9-8.9 %, idle CPU 10.8-13.1 %.
- Thresholds set: 150 MB, 0.3 MB/s, 30 % loading, 40 % idle.
- Red (4.0.0 MenuBarLabel restored): FAIL, the watchdog quit the app at t=22s, footprint=1080610752.
- Watchdog proof limit lowered 40 -> 20 MB: the soak fixture app sits at ~27-49 MB, 40 MB was not reliably exceeded.
- `proc_pid_rusage` footprint 27.7 MB vs `footprint -p` 27 MB: same counter.
- P2 and P3 landed as one commit: RivalApp.swift wires both hooks.
**Branch:** `feature/leak-guard` from `master`

## P2 — Memory watchdog

### Task 2.1 — `MemoryWatchdog` in RivalKit
File: `app/Sources/RivalKit/MemoryWatchdog.swift` (new)

```swift
public enum WatchdogVerdict: Equatable { case ok, unknown, over(bytes: UInt64) }
public func judgeFootprint(_ bytes: UInt64?, limit: UInt64) -> WatchdogVerdict
public func currentFootprint() -> UInt64?          // task_info(TASK_VM_INFO).phys_footprint, nil on failure
public final class MemoryWatchdog: @unchecked Sendable {
    public static let defaultLimit: UInt64 = 1 << 30
    public init(limit: UInt64 = defaultLimit, interval: DispatchTimeInterval = .seconds(5),
                read: @escaping @Sendable () -> UInt64? = currentFootprint,
                logURL: URL = <~/Library/Logs/Rival/watchdog.log>,
                version: String,
                exit: @escaping @Sendable (Int32) -> Void = { Darwin.exit($0) })
    public func start()      // DispatchSourceTimer on DispatchQueue.global(qos: .utility), leeway 1s
    public func check()      // one tick; internal for tests
}
```

- `check()`: verdict `.over` → `Logger(subsystem: "dev.1905.rival", category: "watchdog").fault(...)`, append one line `"<ISO8601 local> footprint=<n> limit=<n> version=<v>\n"` to `logURL` (create dir, append, ignore write errors), then `exit(70)`. Fires at most once (flag).
- `.unknown` and `.ok` → nothing.
- Limit override: `RIVAL_WATCHDOG_LIMIT_MB`, read in the app (Task 2.2) only under `#if DEBUG`.

### Task 2.2 — start it first
File: `app/Sources/RivalApp/RivalApp.swift`
- `AppDelegate` gets `let watchdog: MemoryWatchdog`, created and started first thing in `init()`, before `SessionStore`. Version from `Bundle.main.infoDictionary?["CFBundleShortVersionString"]`, fallback `"dev"`.
- DEBUG: `RIVAL_WATCHDOG_LIMIT_MB` (positive int) sets the limit.

### Task 2.3 — tests
File: `app/Tests/RivalKitTests/MemoryWatchdogTests.swift` (new), XCTest like the others.
- `judgeFootprint`: nil → unknown; limit-1 → ok; limit → over; limit+1 → over.
- `check()` with injected read over the limit, temp `logURL`, exit hook that records codes: one line written containing `footprint=` `limit=` `version=`, exit called once with 70. A second `check()` does not call exit again.
- `check()` under the limit: no file, no exit.
- `currentFootprint()` returns non-nil and > 1 MB in the test process.

Gate: `cd app && swift build && swift test`. Commit: `feat(app): memory watchdog quits above 1 GB`.

## P3 — Launch soak test

### Task 3.1 — first-scan delay hook
File: `app/Sources/RivalKit/SessionStore.swift`
- `SessionStore.init` gains `firstScanDelay: Duration = .zero`. In `start()`, the first `refresh()` Task sleeps that long first. The poll loop does not start its first tick until after that delay either (else it scans at 2s and ends loading).
- File: `RivalApp.swift`: under `#if DEBUG`, `RIVAL_DEBUG_SCAN_DELAY` (seconds, int) → `firstScanDelay`.
- Test in `SessionStoreTests`: with delay 0.3s, `isLoading` is still true after 0.15s and false after 1.0s (fixture dir from existing helpers).

### Task 3.2 — `soak_test.py`
File: `app/scripts/soak_test.py` (new), Python 3 stdlib only, style of `dev_bundle.py`.
Steps:
1. `python3 app/scripts/dev_bundle.py --build`.
2. Fixture home: `tempfile.mkdtemp(prefix="rival_soak_")` + `/home`, `dev_bundle.py --fixture <home> --many 300`.
3. Refuse (exit 2, clear message) if no GUI session: `launchctl managername` != `Aqua`.
4. Launch the bundle's binary directly with `subprocess.Popen([<bundle>/Contents/MacOS/Rival], env=… RIVAL_HOME, RIVAL_DEBUG_SCAN_DELAY=30)` so we own the PID. No `open`.
5. Sample every 2s for 50s: `footprint -p PID` (parse `Footprint: N MB|GB|KB`), `ps -o %cpu= -p PID`. Process died early → fail with its exit code.
6. Terminate (SIGTERM, 5s, SIGKILL). Move the temp dir to `/tmp/trash/rival_soak_<ts>` (house rule: no rm).
7. Checks (constants at top, set in Task 3.4): max footprint, slope of the last 10 samples (least squares, MB/s), mean CPU during loading (t 5-30s), median CPU idle (t 34-50s). Any breach → print the sample table + which check, exit 1.
8. Watchdog proof: relaunch with `RIVAL_WATCHDOG_LIMIT_MB=20` (no scan delay), wait up to 15s for exit; pass if exit code 70 and `~/Library/Logs/Rival/watchdog.log` grew by one line.
Flags: `--skip-build`, `--seconds N` (default 50), `--keep` (don't move the fixture).

### Task 3.3 — Makefile
File: `Makefile`: `test:` → `cd app && swift test` then `python3 app/scripts/soak_test.py --skip-build` after a `python3 app/scripts/dev_bundle.py --build`. New `soak:` target runs the soak alone. Keep `.PHONY` in sync.

### Task 3.4 — calibrate + red/green (orchestrator, not the implementer)
- Green: 5 runs on the fixed build. Record max footprint, slope, CPU. Set thresholds at about 2-3× the worst healthy value, not below the spec's first guesses unless the data says so.
- Red: temporarily restore the 4.0.0 `MenuBarLabel` (TimelineView) in the working tree, run once → must fail. Restore with `git checkout`.
- Watchdog step passes on the fixed build.
- Write the numbers into this plan's Status section.

Commit: `test(app): launch soak test in make test; first-scan delay hook`.

## Implementer rules (stated verbatim in every dispatch)

"You write the code and unit tests your task names and run the focused unit tests for that task (`swift build`, `swift test --filter …`). You NEVER run e2e / integration / live / smoke tests, never launch the app, never run soak_test.py, never publish, deploy, push or touch infra, never run anything money-bearing. Do not commit; the orchestrator commits. Do not set any test-DB env var."
