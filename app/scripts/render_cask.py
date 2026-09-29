#!/usr/bin/env python3
"""Render the rival-app cask from cask.rb.tmpl to stdout.

    python3 app/scripts/render_cask.py --version X.Y.Z --sha256 HEX \
        --url-base https://github.com/1905/rival/releases/download [--local-zip PATH]

The release url is <url-base>/v#{version}/Rival-app.zip. --local-zip swaps it
for file://PATH (local-tap install test).
"""

from __future__ import annotations

import argparse
import re
from pathlib import Path

TEMPLATE = Path(__file__).resolve().parent / "cask.rb.tmpl"


def render(version: str, sha256: str, url_base: str, local_zip: str | None = None, template: str | None = None) -> str:
    version = version.removeprefix("v")
    if not re.fullmatch(r"[0-9A-Za-z.\-]+", version):
        raise ValueError(f"bad version {version!r}")
    if not re.fullmatch(r"[0-9a-f]{64}", sha256):
        raise ValueError(f"bad sha256 {sha256!r}")
    if local_zip:
        url = "file://" + str(Path(local_zip).expanduser().resolve())
    else:
        url = url_base.rstrip("/") + "/v#{version}/Rival-app.zip"
    text = template if template is not None else TEMPLATE.read_text()
    out = text.replace("{{VERSION}}", version).replace("{{SHA256}}", sha256).replace("{{URL}}", url)
    if "{{" in out:
        raise ValueError("unrendered placeholder left in cask")
    return out


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--version", required=True)
    ap.add_argument("--sha256", required=True)
    ap.add_argument("--url-base", required=True)
    ap.add_argument("--local-zip")
    args = ap.parse_args()
    print(render(args.version, args.sha256, args.url_base, args.local_zip), end="")


if __name__ == "__main__":
    main()
