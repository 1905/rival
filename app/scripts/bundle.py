#!/usr/bin/env python3
"""Build the release Rival.app, ad-hoc sign it and zip it for the cask.

    python3 app/scripts/bundle.py --version X.Y.Z [--out DIR] [--arch universal|native]

Steps: `swift build -c release --product RivalApp` (arm64 + x86_64 for
universal), assemble DIR/Rival.app (bundle id dev.1905.rival), codesign -s -,
codesign --verify, ditto-zip to DIR/Rival-app.zip (for the cask), and a
drag-to-Applications DIR/Rival-X.Y.Z.dmg (for direct downloads). Prints
`dmg=<path>`, then `sha256=<hex>` (of the zip) and `zip=<path>` on the last
two lines.
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


def make_dmg(app: Path, dmg: Path) -> None:
    """A compressed DMG holding the app and an /Applications shortcut, the
    usual drag-to-install window. Not notarized: a browser download is
    quarantined and macOS asks the user to allow it once."""
    stage = dmg.parent / "dmg-stage"
    if stage.exists():
        appbundle.trash(stage)
    stage.mkdir()
    subprocess.run(["ditto", str(app), str(stage / app.name)], check=True)
    (stage / "Applications").symlink_to("/Applications")
    subprocess.run(["hdiutil", "create", "-volname", "Rival", "-srcfolder", str(stage),
                    "-ov", "-format", "UDZO", str(dmg)], check=True, stdout=subprocess.DEVNULL)
    appbundle.trash(stage)


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
    dmg_path = out / f"Rival-{version}.dmg"
    for stale in (app, zip_path, dmg_path):
        if stale.exists():
            appbundle.trash(stale)

    binary = swift_build(args.arch)
    appbundle.assemble(app, binary, appbundle.info_plist(version, appbundle.RELEASE_ID))
    appbundle.adhoc_sign(app, verify=True)
    subprocess.run(["ditto", "-c", "-k", "--keepParent", str(app), str(zip_path)], check=True)
    make_dmg(app, dmg_path)

    print(f"dmg={dmg_path}")
    print(f"sha256={sha256(zip_path)}")
    print(f"zip={zip_path}")


if __name__ == "__main__":
    main()
