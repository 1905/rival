"""Shared Rival.app assembly: Info.plist, binary, icon, ad-hoc signature.

Used by dev_bundle.py (debug build, `.dev` bundle id) and bundle.py (release).
Stdlib only.
"""

from __future__ import annotations

import plistlib
import shutil
import subprocess
import sys
import time
from pathlib import Path

APP_DIR = Path(__file__).resolve().parent.parent
ICON = APP_DIR / "Resources" / "AppIcon.icns"

RELEASE_ID = "dev.1905.rival"
DEV_ID = "dev.1905.rival.dev"

# Where replaced outputs go; this repo never deletes with rm.
TRASH = Path("/tmp/trash")


def trash(path: Path, dest: Path = TRASH) -> Path:
    """Moves path to dest/<name>.<YYYYmmdd-HHMMSS>.<ns> and returns the new
    path. The nanosecond suffix keeps two moves in one second apart."""
    dest.mkdir(parents=True, exist_ok=True)
    moved = dest / f"{path.name}.{time.strftime('%Y%m%d-%H%M%S')}.{time.time_ns()}"
    path.rename(moved)
    return moved


def bin_path(cmd: list[str]) -> Path:
    """The RivalApp binary that `cmd` (a `swift build ...` line) produced,
    from `cmd --show-bin-path`. Exits when it is missing."""
    out = subprocess.run(
        cmd + ["--show-bin-path"], cwd=APP_DIR, check=True, capture_output=True, text=True
    ).stdout.strip()
    path = Path(out) / "RivalApp"
    if not path.is_file():
        sys.exit(f"missing {path}; build it first")
    return path


def info_plist(version: str, bundle_id: str, display_name: str = "Rival", build: str | None = None) -> dict:
    """The Info.plist keys. build defaults to version (CFBundleVersion)."""
    return {
        "CFBundleExecutable": "Rival",
        "CFBundleIconFile": "AppIcon",
        "CFBundleIdentifier": bundle_id,
        "CFBundleName": "Rival",
        "CFBundleDisplayName": display_name,
        "CFBundlePackageType": "APPL",
        "CFBundleShortVersionString": version,
        "CFBundleVersion": build if build is not None else version,
        "LSMinimumSystemVersion": "14.0",
        # A regular Dock app; the menu bar extra lives alongside the window.
        "LSUIElement": False,
        "NSHighResolutionCapable": True,
        "NSPrincipalClass": "NSApplication",
    }


def assemble(app: Path, binary: Path, info: dict, icon: Path = ICON) -> Path:
    """Writes app/Contents/{MacOS/Rival, Resources/AppIcon.icns, Info.plist}."""
    macos = app / "Contents" / "MacOS"
    resources = app / "Contents" / "Resources"
    macos.mkdir(parents=True, exist_ok=True)
    resources.mkdir(parents=True, exist_ok=True)
    shutil.copy2(binary, macos / "Rival")
    shutil.copy2(icon, resources / "AppIcon.icns")
    with open(app / "Contents" / "Info.plist", "wb") as f:
        plistlib.dump(info, f)
    return app


def adhoc_sign(app: Path, verify: bool = False) -> None:
    subprocess.run(["codesign", "--force", "--deep", "-s", "-", str(app)], check=True, capture_output=True)
    if verify:
        subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app)], check=True)
