#!/usr/bin/env python3
"""Build a fresh release Rival.app and install it into /Applications.

    python3 app/scripts/install_local.py [--dest /Applications] [--no-open]

Steps: bundle.py --arch native (version from `git describe`), quit a running
Rival, move the old /Applications/Rival.app to /tmp/trash (never deleted),
copy the new bundle in with ditto, strip quarantine (a no-op for a local
build, kept so the result matches the cask), then open it.
"""
import argparse
import subprocess
import sys
import time
from pathlib import Path

import appbundle

REPO = appbundle.APP_DIR.parent
DIST = appbundle.APP_DIR / "dist" / "local"


def run(*cmd: str, check: bool = True) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, check=check, text=True, capture_output=True)


def version() -> str:
    # "v3.34.0-12-gd6ca69c" -> "3.34.0-12-gd6ca69c"; no tags -> "0.0.0-<sha>".
    out = run("git", "-C", str(REPO), "describe", "--tags", "--always", check=False).stdout.strip()
    if not out:
        return "0.0.0-dev"
    return out[1:] if out.startswith("v") else f"0.0.0-{out}"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--dest", default="/Applications")
    ap.add_argument("--no-open", action="store_true")
    args = ap.parse_args()

    ver = version()
    print(f"building Rival.app {ver} (native)…")
    build = subprocess.run(
        [sys.executable, str(appbundle.APP_DIR / "scripts" / "bundle.py"), "--version", ver,
         "--out", str(DIST), "--arch", "native"],
        text=True,
    )
    if build.returncode != 0:
        return build.returncode
    fresh = DIST / "Rival.app"

    # Quit a running copy so the bundle can be replaced cleanly.
    run("osascript", "-e", f'tell application id "{appbundle.RELEASE_ID}" to quit', check=False)
    time.sleep(1)

    target = Path(args.dest) / "Rival.app"
    if target.exists():
        print(f"old copy moved to {appbundle.trash(target)}")

    run("ditto", str(fresh), str(target))
    run("xattr", "-dr", "com.apple.quarantine", str(target), check=False)
    print(f"installed {target} ({ver})")

    if not args.no_open:
        run("open", str(target))
    return 0


if __name__ == "__main__":
    sys.exit(main())
