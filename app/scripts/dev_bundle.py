#!/usr/bin/env python3
"""Wrap the debug RivalApp binary in app/.build/Rival.app so `open` works.

Development only; the release bundle is bundle.py (P5).

    python3 app/scripts/dev_bundle.py [--build] [--fixture DIR [--many N]] [--select RUNID]
    python3 app/scripts/dev_bundle.py --finish DIR [--id ID] [--as failed]

--build          run `swift build` first
--fixture DIR    also create a RIVAL_HOME at DIR from app/Tests/Fixtures:
                 sessions/*.json with log_file pointing at generated logs, and
                 the live fixtures moved to "now" so the list looks current.
                 DIR must not exist yet, so a real ~/.rival is never touched.
--many N         with --fixture: add N finished (completed or failed) runs
                 spread over the last 40 days, for the paged list. Same N,
                 same runs: ids, models, statuses and offsets are fixed.
--select RUNID   print a launch line that preselects run RUNID ("solo:<id>"
                 or "group:<group id>") through RIVAL_SELECT, for
                 screenshots. Debug builds only; a release bundle ignores it.
--finish DIR     flip one live session in fixture DIR to completed (or
                 --as failed) and exit, to fire a finish notification in the
                 running app. Default --id is the running orbit-web
                 gpt-6-astra review. Refuses a DIR not made by --fixture.

Launch against the fixture (never the real ~/.rival):

    open -n --env RIVAL_HOME=DIR app/.build/Rival.app

Always launch through `open`. Running Contents/MacOS/Rival directly makes
macOS refuse notification permission (UNErrorDomain 1, "Notifications are not
allowed for this application").
"""

import argparse
import datetime as dt
import json
import subprocess
import sys
from pathlib import Path

import appbundle

APP_DIR = appbundle.APP_DIR
BUNDLE = APP_DIR / ".build" / "Rival.app"
FIXTURES = APP_DIR / "Tests" / "Fixtures"
FAKE_LOGS = FIXTURES / "logs"
# Invented transcripts for finished fixture runs, so the Result tab (and a
# README screenshot) shows parsed output with no real data. Other fixtures get
# SAMPLE_LOG.
FIXTURE_LOGS = {
    "completed-plan.json": "plan-codex.log",
    "mega-reviewer-a.json": "review-markdown.log",
    "failed.json": "broken-json.log",
}
# --many runs: completed ones alternate these, failed ones get broken-json.log.
MANY_LOGS = ("plan-codex.log", "review-markdown.log")
# Written by --fixture; --finish refuses a directory without it.
FIXTURE_MARK = ".rival-app-fixture"
RUNNING_FIXTURE = "a1b2c3d4-0001-4000-8000-000000000001"

INFO = appbundle.info_plist("0.0.0-dev", appbundle.DEV_ID, display_name="Rival (dev)", build="0")

SAMPLE_LOG = (
    "\x1b[1;32m==> rival review\x1b[0m  scope internal/core/\n"
    "reading 14 files…\n"
    "\tinternal/core/fingerprint.go\n"
    "progress 10%\rprogress 55%\rprogress 100%\n"
    "\x1b[33mwarning:\x1b[0m re-key drops the legacy prefix\n"
)


def build() -> None:
    subprocess.run(["swift", "build"], cwd=APP_DIR, check=True)


def bundle() -> Path:
    appbundle.assemble(BUNDLE, appbundle.bin_path(["swift", "build"]), INFO)
    appbundle.adhoc_sign(BUNDLE)
    return BUNDLE


def rfc3339(t: dt.datetime) -> str:
    return t.astimezone(dt.timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")


MANY_MODELS = (
    ("codex", "gpt-6-astra", "high"),
    ("claude", "claude-opus-5-5", "medium"),
    ("codex", "gpt-5.5", "xhigh"),
    ("opencode", "kimi-k3", "max"),
)
MANY_MODES = ("review", "plan", "review", "security")
MANY_PROJECTS = ("orbit-web", "ledger", "rival", "atlas-cli", "pixel-lab", "acme-api")
MANY_SPAN = dt.timedelta(days=40)


def many_sessions(n: int, now: dt.datetime) -> list[dict]:
    """N finished solo sessions, newest first, evenly spread over the 40 days
    before `now`. Everything but the absolute times is fixed by N and the
    index, so two runs of the script produce the same list."""
    out = []
    for i in range(n):
        cli, model, effort = MANY_MODELS[i % len(MANY_MODELS)]
        failed = i % 7 == 3
        # Evenly spaced, at least a minute old, with a fixed per-index wobble.
        start = now - MANY_SPAN * (i + 1) / (n + 1) - dt.timedelta(minutes=1 + (i * 37) % 53)
        secs = 20 + (i * 97) % 900
        data = {
            "id": f"f0000000-{i // 10000:04d}-4000-8000-{i:012d}",
            "cli": cli,
            "mode": MANY_MODES[i % len(MANY_MODES)],
            "model": model,
            "effort": effort,
            "prompt_preview": f"fixture run {i + 1} of {n}",
            "status": "failed" if failed else "completed",
            "start_time": rfc3339(start),
            "end_time": rfc3339(start + dt.timedelta(seconds=secs)),
            "exit_code": 1 if failed else 0,
            "duration": f"{secs // 60}m{secs % 60}s" if secs >= 60 else f"{secs}s",
            "work_dir": f"/Users/dev/src/{MANY_PROJECTS[i % len(MANY_PROJECTS)]}",
            "pid": 999_999,
            "pid_start": 1,
        }
        if failed:
            data["error"] = "codex exited 1: fixture failure"
        out.append(data)
    return out


def make_fixture(root: Path, many: int = 0) -> None:
    if root.exists():
        sys.exit(f"{root} exists; pick a new directory")
    sessions = root / "sessions"
    sessions.mkdir(parents=True)
    (root / FIXTURE_MARK).write_text("made by app/scripts/dev_bundle.py --fixture\n")
    now = dt.datetime.now(dt.timezone.utc)
    for i, src in enumerate(sorted(FIXTURES.glob("*.json"))):
        data = json.loads(src.read_text())
        sid = data["id"]
        log = sessions / f"{sid}.log"
        if src.name in FIXTURE_LOGS:
            log.write_text((FAKE_LOGS / FIXTURE_LOGS[src.name]).read_text())
        else:
            log.write_text(SAMPLE_LOG * (40 if data.get("status") == "running" else 3))
        data["log_file"] = str(log)
        if data.get("status") in ("running", "queued"):
            data["start_time"] = rfc3339(now - dt.timedelta(minutes=3 + i))
            data["queued_at"] = rfc3339(now - dt.timedelta(minutes=4 + i))
            # A dead PID, so Stop only ever marks the fixture failed.
            data["pid"] = 999_999
            data["pid_start"] = 1
        (sessions / f"{sid}.json").write_text(json.dumps(data, indent=2))
    for i, data in enumerate(many_sessions(many, now)):
        log = sessions / f"{data['id']}.log"
        name = "broken-json.log" if data["status"] == "failed" else MANY_LOGS[i % len(MANY_LOGS)]
        log.write_text((FAKE_LOGS / name).read_text())
        data["log_file"] = str(log)
        (sessions / f"{data['id']}.json").write_text(json.dumps(data, indent=2))
    print(f"fixture RIVAL_HOME: {root}" + (f" (+{many} generated runs)" if many else ""))


def finish(root: Path, sid: str, status: str) -> None:
    """Rewrites one session like the Go CLI's Save: tmp file, then rename."""
    if not (root / FIXTURE_MARK).is_file():
        sys.exit(f"{root} is not a dev_bundle.py fixture; refusing to touch it")
    path = root / "sessions" / f"{sid}.json"
    data = json.loads(path.read_text())
    if data.get("status") not in ("running", "queued"):
        sys.exit(f"{sid} is {data.get('status')!r}, not live; pick another --id")
    now = dt.datetime.now(dt.timezone.utc)
    data["status"] = status
    data["end_time"] = rfc3339(now)
    data["exit_code"] = 0 if status == "completed" else 1
    if status == "failed":
        data["error"] = "codex exited 1: fixture failure"
    tmp = path.with_suffix(".json.tmp")
    tmp.write_text(json.dumps(data, indent=2))
    tmp.replace(path)
    print(f"{sid}: {status}")


def launch_line(bundle_path: Path, env: list[str]) -> str:
    """The `open` line for bundle_path with each KEY=VALUE in env."""
    return "launch: open -n " + " ".join(f"--env {e}" for e in env) + f" {bundle_path}"


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--build", action="store_true")
    ap.add_argument("--fixture", type=Path)
    ap.add_argument("--many", type=int, default=0, metavar="N")
    ap.add_argument("--select", metavar="RUNID")
    ap.add_argument("--finish", type=Path, metavar="DIR")
    ap.add_argument("--id", default=RUNNING_FIXTURE)
    ap.add_argument("--as", dest="status", choices=("completed", "failed"), default="completed")
    args = ap.parse_args()
    if args.many < 0 or (args.many and not args.fixture):
        ap.error("--many N needs --fixture DIR and N >= 0")
    if args.finish:
        finish(args.finish.expanduser().resolve(), args.id, args.status)
        return
    if args.build:
        build()
    path = bundle()
    print(f"bundle: {path}")
    env = []
    if args.fixture:
        root = args.fixture.expanduser().resolve()
        make_fixture(root, args.many)
        env.append(f"RIVAL_HOME={root}")
    if args.select:
        env.append(f"RIVAL_SELECT={args.select}")
    if env:
        print(launch_line(path, env))


if __name__ == "__main__":
    main()
