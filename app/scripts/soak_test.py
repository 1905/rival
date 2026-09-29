#!/usr/bin/env python3
"""Launch soak test: runs the dev Rival.app and fails on memory growth or a
busy CPU in the launch, loading and idle states.

    python3 app/scripts/soak_test.py [--skip-build] [--seconds N] [--keep]

1. `swift build` (skipped with --skip-build), then the dev bundle is
   re-assembled from the last debug build (dev_bundle.build / bundle).
2. A fixture RIVAL_HOME with 300 extra runs, in a fresh temp dir.
3. Launches Contents/MacOS/Rival directly (so this script owns the PID) with
   RIVAL_DEBUG_SCAN_DELAY=30: the first scan waits 30s, so the loading state
   stays on screen. That is the state the pre-release menu bar spinner loop broke.
4. Samples `footprint -p PID` and `ps -o %cpu= -p PID` every 2s for N seconds.
5. Fails on: max footprint, footprint slope over the last samples, mean CPU
   while loading, median CPU when idle. Prints the sample table on failure.
6. Watchdog proof: relaunches with RIVAL_WATCHDOG_LIMIT_MB=20 and passes if
   the app exits with 70 within 15s and ~/Library/Logs/Rival/watchdog.log
   gained one line.

Needs a GUI session (it shows the app on your screen for about a minute) and
a debug build: both env hooks are compiled out of release builds. The fixture
dir is moved to /tmp/trash at the end (--keep leaves it), never deleted.

Exit codes: 0 pass, 1 a check failed, 2 cannot run here (no GUI, tool
missing, launch failed).
"""

import argparse
import os
import re
import signal
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import appbundle
import dev_bundle

# Thresholds, calibrated 2026-09-29 (plan Task 3.4): about 3x the worst of
# five healthy runs (49 MB, 0.076 MB/s, 8.9 % loading CPU, 13.1 % idle CPU).
# The pre-release spinner bug ran at ~38 MB/s and 100 % CPU.
MAX_FOOTPRINT_MB = 150.0
MAX_SLOPE_MB_PER_S = 0.3
SLOPE_SAMPLES = 10
MAX_LOADING_MEAN_CPU = 30.0
MAX_IDLE_MEDIAN_CPU = 40.0

SCAN_DELAY_S = 30
SAMPLE_EVERY_S = 2.0
# Loading window: from LOADING_FROM_S to the end of the scan delay. The idle
# window starts IDLE_AFTER_S after the delay, once the first scan published.
LOADING_FROM_S = 5.0
IDLE_AFTER_S = 4.0
# At least three idle samples.
MIN_SECONDS = SCAN_DELAY_S + IDLE_AFTER_S + 3 * SAMPLE_EVERY_S
FIXTURE_RUNS = 300

WATCHDOG_LIMIT_MB = 20
WATCHDOG_WAIT_S = 15.0
WATCHDOG_EXIT = 70
WATCHDOG_LOG = Path.home() / "Library" / "Logs" / "Rival" / "watchdog.log"

BINARY = dev_bundle.BUNDLE / "Contents" / "MacOS" / "Rival"

FOOTPRINT_RE = re.compile(r"Footprint:\s*([\d.]+)\s*(KB|MB|GB)")
UNIT_MB = {"KB": 1 / 1024, "MB": 1.0, "GB": 1024.0}


class CannotRun(Exception):
    """This machine cannot run the soak test (exit 2)."""


def parse_footprint_mb(text: str) -> float | None:
    """The first `Footprint: N KB|MB|GB` in footprint(1) output, in MB."""
    m = FOOTPRINT_RE.search(text)
    if not m:
        return None
    return float(m.group(1)) * UNIT_MB[m.group(2)]


def slope(points: list[tuple[float, float]]) -> float:
    """Least-squares slope of (x, y) points; 0 for fewer than two x values."""
    n = len(points)
    if n < 2:
        return 0.0
    mx = sum(x for x, _ in points) / n
    my = sum(y for _, y in points) / n
    den = sum((x - mx) ** 2 for x, _ in points)
    if den == 0:
        return 0.0
    return sum((x - mx) * (y - my) for x, y in points) / den


def require_gui() -> None:
    try:
        out = subprocess.run(["launchctl", "managername"], capture_output=True, text=True).stdout.strip()
    except FileNotFoundError:
        raise CannotRun("launchctl missing")
    if out != "Aqua":
        raise CannotRun(f"no GUI session (launchctl managername = {out!r}); run it locally, not over SSH")


def require_tools() -> None:
    for tool in ("footprint", "ps"):
        if subprocess.run(["which", tool], capture_output=True).returncode != 0:
            raise CannotRun(f"{tool} not found")


def launch(env_extra: dict[str, str]) -> subprocess.Popen:
    if not BINARY.is_file():
        raise CannotRun(f"missing {BINARY}; run dev_bundle.py --build")
    env = dict(os.environ)
    # Never inherit a hook from the caller's shell.
    for key in ("RIVAL_HOME", "RIVAL_DEBUG_SCAN_DELAY", "RIVAL_WATCHDOG_LIMIT_MB", "RIVAL_SELECT"):
        env.pop(key, None)
    env.update(env_extra)
    try:
        return subprocess.Popen([str(BINARY)], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    except OSError as e:
        raise CannotRun(f"launch failed: {e}")


def terminate(proc: subprocess.Popen) -> None:
    if proc.poll() is not None:
        return
    proc.send_signal(signal.SIGTERM)
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait()


def sample(pid: int) -> tuple[float | None, float | None]:
    """(footprint MB, %CPU) of pid; None for a value that could not be read."""
    fp = subprocess.run(["footprint", "-p", str(pid)], capture_output=True, text=True)
    cpu = subprocess.run(["ps", "-o", "%cpu=", "-p", str(pid)], capture_output=True, text=True)
    try:
        cpu_val = float(cpu.stdout.strip())
    except ValueError:
        cpu_val = None
    return parse_footprint_mb(fp.stdout), cpu_val


def soak(home: Path, seconds: float) -> tuple[list[Sample], str | None]:
    """Runs the app for `seconds`. Returns the samples (t, MB, %CPU) and an
    error when the process died early."""
    proc = launch({"RIVAL_HOME": str(home), "RIVAL_DEBUG_SCAN_DELAY": str(SCAN_DELAY_S)})
    samples = []
    t0 = time.monotonic()
    try:
        while True:
            next_at = t0 + (len(samples) + 1) * SAMPLE_EVERY_S
            if next_at - t0 > seconds:
                break
            time.sleep(max(0.0, next_at - time.monotonic()))
            if proc.poll() is not None:
                return samples, f"app exited early with code {proc.returncode} at t={time.monotonic() - t0:.1f}s"
            mb, cpu = sample(proc.pid)
            samples.append((round(time.monotonic() - t0, 1), mb, cpu))
    finally:
        terminate(proc)
    return samples, None


Sample = tuple[float, float | None, float | None]


def windows(samples: list[Sample], seconds: float) -> tuple[list[tuple[float, float]], list[float], list[float]]:
    """The readable footprints as (t, MB), and the readable %CPU values in
    the loading and idle windows. check() and summary() share these."""
    fps = [(t, mb) for t, mb, _ in samples if mb is not None]
    loading = [c for t, _, c in samples if c is not None and LOADING_FROM_S <= t <= SCAN_DELAY_S]
    idle = [c for t, _, c in samples if c is not None and SCAN_DELAY_S + IDLE_AFTER_S <= t <= seconds]
    return fps, loading, idle


def check(samples: list[Sample], seconds: float) -> list[str]:
    """Threshold breaches, as messages. Unreadable data is a breach too:
    never a silent pass."""
    bad = []
    fps, loading, idle = windows(samples, seconds)
    if len(fps) < len(samples) or not fps:
        bad.append(f"footprint unreadable in {len(samples) - len(fps)} of {len(samples)} samples")
    if fps:
        peak = max(mb for _, mb in fps)
        if peak > MAX_FOOTPRINT_MB:
            bad.append(f"max footprint {peak:.1f} MB > {MAX_FOOTPRINT_MB} MB")
        tail = fps[-SLOPE_SAMPLES:]
        s = slope(tail)
        if len(tail) < SLOPE_SAMPLES:
            bad.append(f"only {len(tail)} footprint samples for the slope, need {SLOPE_SAMPLES}")
        elif s > MAX_SLOPE_MB_PER_S:
            bad.append(f"footprint slope {s:.2f} MB/s over the last {len(tail)} samples > {MAX_SLOPE_MB_PER_S}")
    no_cpu = sum(1 for _, _, c in samples if c is None)
    if no_cpu:
        bad.append(f"CPU unreadable in {no_cpu} of {len(samples)} samples")
    if not loading:
        bad.append("no CPU samples in the loading window")
    elif statistics.mean(loading) > MAX_LOADING_MEAN_CPU:
        bad.append(f"mean CPU while loading {statistics.mean(loading):.1f}% > {MAX_LOADING_MEAN_CPU}%")
    if not idle:
        bad.append("no CPU samples in the idle window")
    elif statistics.median(idle) > MAX_IDLE_MEDIAN_CPU:
        bad.append(f"median CPU when idle {statistics.median(idle):.1f}% > {MAX_IDLE_MEDIAN_CPU}%")
    return bad


def summary(samples: list[Sample], seconds: float) -> str:
    fps, loading, idle = windows(samples, seconds)
    parts = []
    if fps:
        parts.append(f"max {max(mb for _, mb in fps):.1f} MB")
        parts.append(f"slope {slope(fps[-SLOPE_SAMPLES:]):.3f} MB/s")
    if loading:
        parts.append(f"loading CPU mean {statistics.mean(loading):.1f}%")
    if idle:
        parts.append(f"idle CPU median {statistics.median(idle):.1f}%")
    return ", ".join(parts)


def table(samples: list[Sample]) -> str:
    rows = ["     t   footprint MB   %CPU"]
    for t, mb, cpu in samples:
        mb_s = f"{mb:12.1f}" if mb is not None else f"{'?':>12}"
        cpu_s = f"{cpu:6.1f}" if cpu is not None else f"{'?':>6}"
        rows.append(f"{t:6.1f}   {mb_s}   {cpu_s}")
    return "\n".join(rows)


def log_lines() -> int:
    try:
        return len(WATCHDOG_LOG.read_text().splitlines())
    except FileNotFoundError:
        return 0


def watchdog_proof(home: Path) -> str | None:
    """None when the watchdog quit the app as expected, else the reason."""
    before = log_lines()
    proc = launch({"RIVAL_HOME": str(home), "RIVAL_WATCHDOG_LIMIT_MB": str(WATCHDOG_LIMIT_MB)})
    try:
        code = proc.wait(timeout=WATCHDOG_WAIT_S)
    except subprocess.TimeoutExpired:
        terminate(proc)
        return f"app still running {WATCHDOG_WAIT_S:.0f}s after launch with a {WATCHDOG_LIMIT_MB} MB limit"
    if code != WATCHDOG_EXIT:
        return f"app exited with {code}, want {WATCHDOG_EXIT}"
    grew = log_lines() - before
    if grew != 1:
        return f"{WATCHDOG_LOG} gained {grew} lines, want 1"
    return None


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--skip-build", action="store_true", help="don't run swift build first")
    ap.add_argument("--seconds", type=float, default=50.0, help="soak length (default 50)")
    ap.add_argument("--keep", action="store_true", help="leave the fixture dir in place")
    args = ap.parse_args()
    if args.seconds < MIN_SECONDS:
        ap.error(f"--seconds must be at least {MIN_SECONDS:.0f} to cover loading and idle")

    try:
        require_gui()
        require_tools()
        if not args.skip_build:
            dev_bundle.build()
        dev_bundle.bundle()
    except CannotRun as e:
        print(f"soak: cannot run: {e}", file=sys.stderr)
        return 2
    # dev_bundle and appbundle report a missing binary through sys.exit.
    except (subprocess.CalledProcessError, OSError, SystemExit) as e:
        print(f"soak: build failed: {e}", file=sys.stderr)
        return 2

    tmp = Path(tempfile.mkdtemp(prefix="rival_soak_"))
    home = tmp / "home"
    try:
        try:
            dev_bundle.make_fixture(home, FIXTURE_RUNS)
        # make_fixture reports an existing dir through sys.exit.
        except (OSError, SystemExit) as e:
            raise CannotRun(f"fixture failed: {e}")
        print(f"soak: {args.seconds:.0f}s, first scan held {SCAN_DELAY_S}s")
        samples, died = soak(home, args.seconds)
        bad = [died] if died else check(samples, args.seconds)
        if bad:
            print(table(samples))
            for b in bad:
                print(f"soak: FAIL: {b}", file=sys.stderr)
            return 1
        print(f"soak: ok ({summary(samples, args.seconds)})")
        print(f"soak: watchdog proof, limit {WATCHDOG_LIMIT_MB} MB")
        err = watchdog_proof(home)
        if err:
            print(f"soak: FAIL: watchdog: {err}", file=sys.stderr)
            return 1
        print("soak: watchdog ok")
        return 0
    except CannotRun as e:
        print(f"soak: cannot run: {e}", file=sys.stderr)
        return 2
    finally:
        if args.keep:
            print(f"soak: fixture kept at {tmp}")
        else:
            appbundle.trash(tmp)


if __name__ == "__main__":
    sys.exit(main())
