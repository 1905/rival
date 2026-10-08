"""Tests for check_release_archives.py on synthetic archives (no real build)."""

import contextlib
import hashlib
import io
import os
import struct
import sys
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_release_archives as cra  # noqa: E402

MACHO = {"amd64": 0x1000007, "arm64": 0x100000C}
ELF = {"amd64": 62, "arm64": 183}
PE = {"amd64": 0x8664, "arm64": 0xAA64}


def fake_binary(system, arch):
    """The smallest header binary_target accepts."""
    if system == "darwin":
        return b"\xcf\xfa\xed\xfe" + struct.pack("<I", MACHO[arch]) + bytes(56)
    if system == "linux":
        return b"\x7fELF\x02\x01" + bytes(12) + struct.pack("<H", ELF[arch]) + bytes(44)
    data = bytearray(0x80)
    data[:2] = b"MZ"
    struct.pack_into("<I", data, 0x3C, 0x40)
    data[0x40:0x44] = b"PE\0\0"
    struct.pack_into("<H", data, 0x44, PE[arch])
    struct.pack_into("<H", data, 0x40 + 24, 0x20B)
    return bytes(data)


def bundled():
    return {name: (cra.ROOT / name).read_bytes() for name in cra.BUNDLED}


def write_archive(path, members):
    """members: [(name, bytes, mode)]."""
    if path.name.endswith(".zip"):
        with zipfile.ZipFile(path, "w") as archive:
            for name, data, mode in members:
                info = zipfile.ZipInfo(name)
                info.external_attr = mode << 16
                archive.writestr(info, data)
    else:
        with tarfile.open(path, "w:gz") as archive:
            for name, data, mode in members:
                info = tarfile.TarInfo(name)
                info.size = len(data)
                info.mode = mode
                archive.addfile(info, io.BytesIO(data))


def default_members(system, arch):
    exe = "rival.exe" if system == "windows" else "rival"
    members = [(exe, fake_binary(system, arch), 0o755)]
    members += [(name, data, 0o644) for name, data in bundled().items()]
    return members


class Dist:
    """A temp dist directory with six archives and their checksums."""

    def __init__(self, tmp, overrides=None):
        self.path = Path(tmp)
        for system, arch in cra.TARGETS:
            name = cra.archive_name(system, arch)
            members = (overrides or {}).get(name) or default_members(system, arch)
            write_archive(self.path / name, members)
        self.write_checksums()

    def write_checksums(self):
        lines = [f"{hashlib.sha256((self.path / cra.archive_name(s, a)).read_bytes()).hexdigest()}"
                 f"  {cra.archive_name(s, a)}" for s, a in cra.TARGETS]
        (self.path / "checksums.txt").write_text("\n".join(lines) + "\n")

    def sums(self):
        return cra.parse_checksums((self.path / "checksums.txt").read_text())

    def formula(self, skip=None):
        sums = self.sums()
        parts = ["class Rival < Formula\n"]
        for system, arch in cra.TARGETS:
            name = cra.archive_name(system, arch)
            if system == "windows" or name == skip:
                continue
            parts.append(f'  url "{cra.FORMULA_URL}v4.1.1/{name}"\n  sha256 "{sums[name]}"\n')
        parts.append('  def install\n    bin.install "rival"\n  end\nend\n')
        return "".join(parts)


class BinaryTargetTest(unittest.TestCase):
    def test_all_six_headers(self):
        for system, arch in cra.TARGETS:
            with self.subTest(target=f"{system}_{arch}"):
                self.assertEqual(cra.binary_target(fake_binary(system, arch)), (system, arch))

    def test_unknown_header(self):
        with self.assertRaisesRegex(cra.CheckError, "unknown executable header"):
            cra.binary_target(b"#!/bin/sh\n")

    def test_pe32_is_rejected(self):
        data = bytearray(fake_binary("windows", "amd64"))
        struct.pack_into("<H", data, 0x40 + 24, 0x10B)
        with self.assertRaisesRegex(cra.CheckError, "not PE32"):
            cra.binary_target(bytes(data))


class ChecksumsTest(unittest.TestCase):
    def test_parse(self):
        digest = "a" * 64
        self.assertEqual(cra.parse_checksums(f"{digest}  x.zip\n{'b' * 64} *y.tar.gz\n"),
                         {"x.zip": digest, "y.tar.gz": "b" * 64})

    def test_bad_lines(self):
        for name, text in [("duplicate", f"{'a' * 64}  x\n{'b' * 64}  x\n"),
                           ("short digest", "abc  x\n"),
                           ("one field", "a" * 64 + "\n")]:
            with self.subTest(name), self.assertRaises(cra.CheckError):
                cra.parse_checksums(text)


class CheckArchiveTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)

    def check(self, system, arch, overrides=None):
        dist = Dist(self.tmp.name, overrides)
        return cra.check_archive(dist.path, dist.sums(), system, arch)

    def test_all_six_pass(self):
        dist = Dist(self.tmp.name)
        for system, arch in cra.TARGETS:
            with self.subTest(target=f"{system}_{arch}"):
                binary, exe = cra.check_archive(dist.path, dist.sums(), system, arch)
                self.assertEqual(binary, fake_binary(system, arch))
                self.assertEqual(exe, "rival.exe" if system == "windows" else "rival")

    def test_checksum_mismatch(self):
        dist = Dist(self.tmp.name)
        write_archive(dist.path / "rival_linux_amd64.tar.gz", default_members("linux", "amd64")[::-1])
        with self.assertRaisesRegex(cra.CheckError, "sha256"):
            cra.check_archive(dist.path, dist.sums(), "linux", "amd64")

    def test_wrong_cpu(self):
        members = default_members("linux", "arm64")
        with self.assertRaisesRegex(cra.CheckError, "binary is linux/arm64"):
            self.check("linux", "amd64", {"rival_linux_amd64.tar.gz": members})

    def test_wrong_os(self):
        members = [("rival.exe", fake_binary("linux", "amd64"), 0o755)] + default_members("windows", "amd64")[1:]
        with self.assertRaisesRegex(cra.CheckError, "binary is linux/amd64"):
            self.check("windows", "amd64", {"rival_windows_amd64.zip": members})

    def test_missing_bundled_file(self):
        members = [m for m in default_members("darwin", "arm64") if m[0] != "README.md"]
        with self.assertRaisesRegex(cra.CheckError, "README.md"):
            self.check("darwin", "arm64", {"rival_darwin_arm64.tar.gz": members})

    def test_changed_bundled_file(self):
        members = [(n, b"other" if n == "LICENSE" else d, m) for n, d, m in default_members("darwin", "arm64")]
        with self.assertRaisesRegex(cra.CheckError, "LICENSE differs"):
            self.check("darwin", "arm64", {"rival_darwin_arm64.tar.gz": members})

    def test_crlf_bundled_file_matches(self):
        members = [(n, d.replace(b"\n", b"\r\n") if n == "README.md" else d, m)
                   for n, d, m in default_members("windows", "arm64")]
        self.check("windows", "arm64", {"rival_windows_arm64.zip": members})

    def test_wrapped_directory_is_found(self):
        members = [(f"rival_linux_arm64/{n}", d, m) for n, d, m in default_members("linux", "arm64")]
        self.check("linux", "arm64", {"rival_linux_arm64.tar.gz": members})

    def test_binary_not_executable(self):
        members = [(n, d, 0o644) for n, d, _ in default_members("linux", "amd64")]
        with self.assertRaisesRegex(cra.CheckError, "not executable"):
            self.check("linux", "amd64", {"rival_linux_amd64.tar.gz": members})

    def test_unsafe_member(self):
        members = default_members("linux", "amd64") + [("../evil", b"x", 0o644)]
        with self.assertRaisesRegex(cra.CheckError, "unsafe archive member"):
            self.check("linux", "amd64", {"rival_linux_amd64.tar.gz": members})

    def test_missing_archive(self):
        dist = Dist(self.tmp.name)
        os.replace(dist.path / "rival_windows_arm64.zip", dist.path / "other.zip")
        with self.assertRaisesRegex(cra.CheckError, "expected one file"):
            cra.check_archive(dist.path, dist.sums(), "windows", "arm64")


class FormulaTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dist = Dist(self.tmp.name)

    def test_four_unix_archives(self):
        cra.check_formula(self.dist.formula(), self.dist.sums())

    def test_missing_archive(self):
        with self.assertRaisesRegex(cra.CheckError, "rival_linux_arm64.tar.gz"):
            cra.check_formula(self.dist.formula(skip="rival_linux_arm64.tar.gz"), self.dist.sums())

    def test_wrong_hash(self):
        sums = self.dist.sums()
        text = self.dist.formula().replace(sums["rival_darwin_amd64.tar.gz"], "0" * 64)
        with self.assertRaisesRegex(cra.CheckError, "no sha256 for rival_darwin_amd64"):
            cra.check_formula(text, sums)

    def test_windows_listed(self):
        text = self.dist.formula() + f'url "{cra.FORMULA_URL}v1/rival_windows_amd64.zip"\n'
        with self.assertRaisesRegex(cra.CheckError, "lists rival_windows_amd64.zip"):
            cra.check_formula(text, self.dist.sums())


class VersionTest(unittest.TestCase):
    def test_banner_then_version(self):
        cra.check_version_output("\n  _ banner\n /_/\n  4.2.0\n", "4.2.0")

    def test_wrong_version(self):
        for stdout in ["  dev\n", "rival 4.2.0\n", ""]:
            with self.subTest(stdout=stdout), self.assertRaises(cra.CheckError):
                cra.check_version_output(stdout, "4.2.0")

    @unittest.skipIf(os.name == "nt", "POSIX shell stub")
    def test_run_version_isolates_home(self):
        stub = (b"#!/bin/sh\n"
                b'[ "$HOME" = "$USERPROFILE" ] && [ "$RIVAL_HOME" = "$HOME/.rival" ] || exit 7\n'
                b'[ "$RIVAL_NO_UPDATE_CHECK$RIVAL_NO_TELEMETRY$DO_NOT_TRACK" = 111 ] || exit 8\n'
                b'[ "$1" = version ] || exit 9\n'
                b'printf "banner\\n  1.2.3\\n"\n')
        self.assertEqual(cra.run_version(stub, "rival"), "banner\n  1.2.3\n")

    @unittest.skipIf(os.name == "nt", "POSIX shell stub")
    def test_run_version_failure(self):
        with self.assertRaisesRegex(cra.CheckError, "exited 3"):
            cra.run_version(b"#!/bin/sh\necho boom >&2\nexit 3\n", "rival")


class MainTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dist = Dist(self.tmp.name)
        (self.dist.path / "homebrew").mkdir()
        (self.dist.path / "homebrew" / "rival.rb").write_text(self.dist.formula())

    def main(self, *args):
        err = io.StringIO()
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(err):
            code = cra.main([str(self.dist.path), *args])
        return code, err.getvalue()

    def test_all_archives_and_formula(self):
        self.assertEqual(self.main("--formula"), (0, ""))

    def test_run_on_a_foreign_host_fails(self):
        host = cra.host_target()
        foreign = next(f"{s}_{a}" for s, a in cra.TARGETS if (s, a) != host)
        code, err = self.main("--only", foreign, "--run")
        self.assertEqual(code, 1)
        self.assertIn(f"archive is {foreign}", err)

    def test_broken_formula_fails(self):
        (self.dist.path / "homebrew" / "rival.rb").write_text("class Other\n")
        code, err = self.main("--formula")
        self.assertEqual(code, 1)
        self.assertIn("not the Rival formula", err)


if __name__ == "__main__":
    unittest.main()
