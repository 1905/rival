#!/usr/bin/env python3
"""Build the release Rival.app, ad-hoc sign it and zip it for the cask.

    python3 app/scripts/bundle.py --version X.Y.Z [--out DIR] [--arch universal|native]

Steps: `swift build -c release --product RivalApp` (arm64 + x86_64 for
universal), assemble DIR/Rival.app (bundle id dev.1905.rival), codesign -s -,
codesign --verify, ditto-zip to DIR/Rival-app.zip. Prints `sha256=<hex>` and
`zip=<path>` on the last two lines.
"""

import argparse
import hashlib
import subprocess
from pathlib import Path

import appbundle

APP_DIR = appbundle.APP_DIR


def swift_build(arch: str) -> Path:
    cmd = ["swift", "build", "-c", "release", "--product", "RivalApp"]
    if arch == "universal":
        cmd += ["--arch", "arm64", "--arch", "x86_64"]
    subprocess.run(cmd, cwd=APP_DIR, check=True)
    return appbundle.bin_path(cmd)


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--version", required=True, help="X.Y.Z, no leading v")
    ap.add_argument("--out", type=Path, default=APP_DIR / "dist")
    ap.add_argument("--arch", choices=("universal", "native"), default="universal")
    args = ap.parse_args()
    version = args.version.removeprefix("v")

    out = args.out.expanduser().resolve()
    out.mkdir(parents=True, exist_ok=True)
    app = out / "Rival.app"
    zip_path = out / "Rival-app.zip"
    for stale in (app, zip_path):
        if stale.exists():
            appbundle.trash(stale)

    binary = swift_build(args.arch)
    appbundle.assemble(app, binary, appbundle.info_plist(version, appbundle.RELEASE_ID))
    appbundle.adhoc_sign(app, verify=True)
    subprocess.run(["ditto", "-c", "-k", "--keepParent", str(app), str(zip_path)], check=True)

    print(f"sha256={sha256(zip_path)}")
    print(f"zip={zip_path}")


if __name__ == "__main__":
    main()
