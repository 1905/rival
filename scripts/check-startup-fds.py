#!/usr/bin/env python3
"""Check that rival's startup fd constructor survives the final link.

Rust reopens a closed fd 0/1/2 on /dev/null before main. A loader
constructor (crates/rival/src/startup_fds.rs) records the original state so
`rival` fails on a closed descriptor instead of using /dev/null. The Rust Reference keeps `#[used]` statics in object
files only, so this script checks the linked debug and LTO release binaries
by behavior:

1. fd 2 closed, `command codex --detach`: `rival` fails to start the child
   (EBADF) and exits 1. Without the constructor, Rust would spawn a detached
   child that prints the usage on stdout.
2. fd 0 closed, `command codex`: `rival` fails the stdin read and exits 1 with
   `read stdin: read /dev/stdin: Bad file descriptor (os error 9)`. Without the
   constructor, Rust would read /dev/null as a terminal and print usage.

Both cases stop before any provider, queue or network work. Each run gets a
temporary HOME and RIVAL_HOME and a minimal environment.
"""

import os
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def build(release):
    cmd = ["cargo", "build", "-p", "rival", "--locked"]
    if release:
        cmd.append("--release")
    subprocess.run(cmd, cwd=ROOT, check=True)
    return os.path.join(ROOT, "target", "release" if release else "debug", "rival")


def run(binary, args, close_fd):
    with tempfile.TemporaryDirectory() as home:
        env = {
            "HOME": home,
            "RIVAL_HOME": os.path.join(home, ".rival"),
            "PATH": "/usr/bin:/bin",
            "RIVAL_NO_UPDATE_CHECK": "1",
        }
        proc = subprocess.run(
            [binary, *args],
            cwd=home,
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            preexec_fn=lambda: os.close(close_fd),
            timeout=60,
        )
        return proc.returncode, proc.stdout, proc.stderr


def check(binary):
    problems = []
    code, out, _ = run(binary, ["command", "codex", "--detach"], 2)
    if code != 1 or out:
        problems.append(f"fd 2 closed + --detach: exit {code}, stdout {out!r} (want exit 1, no stdout)")
    code, out, err = run(binary, ["command", "codex"], 0)
    want = b"read stdin: read /dev/stdin: Bad file descriptor (os error 9)\n"
    if code != 1 or out or err != want:
        problems.append(f"fd 0 closed: exit {code}, stdout {out!r}, stderr {err!r} (want exit 1, {want!r})")
    return problems


def main():
    failed = False
    for release in (False, True):
        binary = build(release)
        label = "release (LTO)" if release else "debug"
        problems = check(binary)
        for p in problems:
            print(f"FAIL {label}: {p}")
        if not problems:
            print(f"ok   {label}: startup fd state recorded before std reopened the fds")
        failed |= bool(problems)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
