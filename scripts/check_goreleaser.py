#!/usr/bin/env python3
"""Validate the release config, allowing only the documented brew-formula warning.

GoReleaser 2.18.2 check returns 1 for invalid configuration and 2 for valid
configuration with deprecations. Rival intentionally retains `brews` to ship
a Homebrew formula. Any other warning or validation error must still fail.
"""

import re
import subprocess
import sys


def accepted_check(returncode, output):
    if returncode == 0:
        return True
    warnings = re.findall(r"DEPRECATED:\s+(\S+)", output)
    return returncode == 2 and warnings == ["brews"]


def main():
    result = subprocess.run(
        ["goreleaser", "check"], stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        text=True, check=False,
    )
    print(result.stdout, end="")
    if not accepted_check(result.returncode, result.stdout):
        return result.returncode or 1
    if result.returncode == 2:
        print("Accepted the documented brews deprecation; the Homebrew formula is retained.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
