#!/usr/bin/env python3
"""Check GoReleaser's rival archives in a dist directory.

For each archive: its checksums.txt entry, safe member names, the binary's
format and CPU, the executable bit, and the bundled LICENSE and README.md
(byte-equal to this checkout, line endings normalised).

--formula also checks dist/homebrew/rival.rb: one URL and hash per
darwin/linux archive. --run executes the archive built for this host as
`rival version` with a private HOME/USERPROFILE/RIVAL_HOME and update checks
and telemetry off. With --only, a host that does not match the archive fails.

Usage (release.yml):
  check_release_archives.py dist --formula --run      # build host, all six
  check_release_archives.py dist --only linux_arm64 --run
"""

import argparse
import hashlib
import json
import os
import platform
import re
import stat
import struct
import subprocess
import sys
import tarfile
import tempfile
import zipfile
from pathlib import Path, PurePosixPath

ROOT = Path(__file__).resolve().parent.parent
TARGETS = [(system, arch) for system in ("darwin", "linux", "windows") for arch in ("amd64", "arm64")]
BUNDLED = ("LICENSE", "README.md")
FORMULA_URL = "https://github.com/1905/rival/releases/download/"

MACHO_CPU = {0x1000007: "amd64", 0x100000C: "arm64"}
ELF_MACHINE = {62: "amd64", 183: "arm64"}
PE_MACHINE = {0x8664: "amd64", 0xAA64: "arm64"}


class CheckError(Exception):
    pass


def require(condition, message):
    if not condition:
        raise CheckError(message)


def archive_name(system, arch):
    return f"rival_{system}_{arch}.{'zip' if system == 'windows' else 'tar.gz'}"


def parse_checksums(text):
    """checksums.txt lines `<sha256>  <name>` → {name: sha256}."""
    sums = {}
    for line in text.splitlines():
        if not line.strip():
            continue
        parts = line.split()
        require(len(parts) == 2, f"checksums.txt: malformed line {line!r}")
        digest, name = parts[0], parts[1].lstrip("*")
        require(len(digest) == 64 and all(c in "0123456789abcdef" for c in digest),
                f"checksums.txt: bad digest in {line!r}")
        require(name not in sums, f"checksums.txt: {name} listed twice")
        sums[name] = digest
    return sums


def check_member_name(name):
    path = PurePosixPath(name)
    require("\\" not in name and not path.is_absolute() and ".." not in path.parts,
            f"unsafe archive member {name!r}")


def read_members(path):
    """Regular files in a .zip or .tar.gz → {name: (bytes, mode)}."""
    members = {}
    if path.name.endswith(".zip"):
        with zipfile.ZipFile(path) as archive:
            # read() checks each member's CRC and raises BadZipFile.
            for info in archive.infolist():
                check_member_name(info.filename)
                if info.is_dir():
                    continue
                mode = info.external_attr >> 16
                require(not stat.S_ISLNK(mode), f"{path.name}: symlink {info.filename}")
                require(info.filename not in members, f"{path.name}: duplicate {info.filename}")
                members[info.filename] = (archive.read(info), mode)
    else:
        with tarfile.open(path, "r:gz") as archive:
            for info in archive:
                check_member_name(info.name)
                if info.isdir():
                    continue
                require(info.isfile(), f"{path.name}: {info.name} is not a regular file")
                require(info.name not in members, f"{path.name}: duplicate {info.name}")
                members[info.name] = (archive.extractfile(info).read(), info.mode)
    return members


def binary_target(data):
    """(system, arch) from a Mach-O 64, ELF64 or PE32+ header."""
    if data[:4] == b"\xcf\xfa\xed\xfe":
        cpu = struct.unpack_from("<I", data, 4)[0]
        require(cpu in MACHO_CPU, f"Mach-O: unknown CPU type {cpu:#x}")
        return "darwin", MACHO_CPU[cpu]
    if data[:4] == b"\x7fELF":
        require(data[4:6] == b"\x02\x01", "ELF: not 64-bit little-endian")
        machine = struct.unpack_from("<H", data, 18)[0]
        require(machine in ELF_MACHINE, f"ELF: unknown machine {machine}")
        return "linux", ELF_MACHINE[machine]
    if data[:2] == b"MZ":
        pe = struct.unpack_from("<I", data, 0x3C)[0]
        require(data[pe:pe + 4] == b"PE\0\0", "PE: missing signature")
        machine = struct.unpack_from("<H", data, pe + 4)[0]
        require(struct.unpack_from("<H", data, pe + 24)[0] == 0x20B, "PE: not PE32+")
        require(machine in PE_MACHINE, f"PE: unknown machine {machine:#x}")
        return "windows", PE_MACHINE[machine]
    raise CheckError(f"unknown executable header {data[:8].hex()}")


def one_member(members, name, archive):
    """The member at `name` or ending in `/name` (GoReleaser may wrap it)."""
    found = [key for key in members if key == name or key.endswith("/" + name)]
    require(len(found) == 1, f"{archive}: expected one {name}, found {found}")
    return found[0]


def lf(data):
    return data.replace(b"\r\n", b"\n")


def check_archive(dist, sums, system, arch):
    """Checks one archive; returns (binary bytes, binary file name)."""
    name = archive_name(system, arch)
    paths = list(dist.rglob(name))
    require(len(paths) == 1, f"{name}: expected one file under {dist}, found {len(paths)}")
    path = paths[0]
    require(name in sums, f"{name}: missing from checksums.txt")
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    require(digest == sums[name], f"{name}: sha256 {digest} != checksums.txt {sums[name]}")

    members = read_members(path)
    exe = "rival.exe" if system == "windows" else "rival"
    binary_path = one_member(members, exe, name)
    binary, mode = members[binary_path]
    got = binary_target(binary)
    require(got == (system, arch), f"{name}: binary is {got[0]}/{got[1]}")
    if system != "windows":
        require(mode & 0o111, f"{name}: {binary_path} is not executable ({oct(mode)})")
    for bundled in BUNDLED:
        member = one_member(members, bundled, name)
        require(lf(members[member][0]) == lf((ROOT / bundled).read_bytes()),
                f"{name}: {member} differs from {bundled}")
    return binary, exe


def check_formula(text, sums):
    require("class Rival < Formula" in text, "rival.rb: not the Rival formula")
    require('bin.install "rival"' in text, 'rival.rb: no bin.install "rival"')
    for system, arch in TARGETS:
        name = archive_name(system, arch)
        if system == "windows":
            require(name not in text, f"rival.rb: lists {name}")
            continue
        url = re.escape(FORMULA_URL) + r'[^"/\s]+/' + re.escape(name) + '"'
        require(re.search(url, text), f"rival.rb: no release URL for {name}")
        require(sums[name] in text, f"rival.rb: no sha256 for {name}")


def host_target():
    system = platform.system().lower()
    arch = {"x86_64": "amd64", "amd64": "amd64", "arm64": "arm64", "aarch64": "arm64"}.get(
        platform.machine().lower())
    return system, arch


def run_version(binary, exe):
    """`rival version` stdout from the packaged bytes, in a private home."""
    with tempfile.TemporaryDirectory(prefix="rival-packaged-") as scratch:
        scratch = Path(scratch)
        path = scratch / exe
        path.write_bytes(binary)
        path.chmod(0o755)
        home = scratch / "home"
        home.mkdir()
        env = {key: value for key, value in os.environ.items()
               if key.upper() in ("PATH", "SYSTEMROOT", "WINDIR", "TEMP", "TMP", "LANG")}
        env.update(HOME=str(home), USERPROFILE=str(home), RIVAL_HOME=str(home / ".rival"),
                   RIVAL_NO_UPDATE_CHECK="1", RIVAL_NO_TELEMETRY="1", DO_NOT_TRACK="1")
        result = subprocess.run([str(path), "version"], cwd=home, env=env,
                                capture_output=True, text=True, timeout=60)
        require(result.returncode == 0,
                f"rival version exited {result.returncode}: {result.stderr.strip()}")
        require(not result.stderr.strip(), f"rival version wrote stderr: {result.stderr.strip()}")
        return result.stdout


def check_version_output(stdout, version):
    """The CLI prints the banner, then `  <version>`."""
    lines = stdout.splitlines()
    require(bool(lines) and lines[-1] == f"  {version}",
            f"rival version printed {lines[-1:]!r}, want '  {version}'")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("dist", type=Path)
    parser.add_argument("--only", choices=[f"{s}_{a}" for s, a in TARGETS])
    parser.add_argument("--formula", action="store_true", help="also check homebrew/rival.rb")
    parser.add_argument("--run", action="store_true", help="run the archive built for this host")
    parser.add_argument("--version", help="expected version (default: dist/metadata.json)")
    args = parser.parse_args(argv)

    try:
        dist = args.dist.resolve()
        sums = parse_checksums((dist / "checksums.txt").read_text())
        targets = [tuple(args.only.split("_"))] if args.only else TARGETS
        host = host_target()
        if args.run and args.only:
            require(host == targets[0], f"host is {host[0]}_{host[1]}, archive is {args.only}")
        ran = False
        for system, arch in targets:
            binary, exe = check_archive(dist, sums, system, arch)
            print(f"ok   {archive_name(system, arch)}")
            if args.run and (system, arch) == host:
                version = args.version or json.loads((dist / "metadata.json").read_text())["version"]
                check_version_output(run_version(binary, exe), version)
                print(f"ran  {archive_name(system, arch)}: rival version {version}")
                ran = True
        if args.run:
            require(ran, f"no archive for this host ({host[0]}_{host[1]})")
        if args.formula:
            check_formula((dist / "homebrew" / "rival.rb").read_text(), sums)
            print("ok   homebrew/rival.rb")
    except (CheckError, OSError, KeyError, ValueError, tarfile.TarError, zipfile.BadZipFile,
            subprocess.TimeoutExpired) as err:
        print(f"FAIL {err}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
