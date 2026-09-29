"""python3 -m unittest discover -s app/scripts -p 'test_*.py'"""

import datetime as dt
import json
import plistlib
import tempfile
import unittest
from pathlib import Path

import appbundle
import dev_bundle
import render_cask

SHA = "ab" * 32


class InfoPlistTest(unittest.TestCase):
    def test_release_keys(self):
        info = appbundle.info_plist("3.35.0", appbundle.RELEASE_ID)
        self.assertEqual(info["CFBundleIdentifier"], "dev.1905.rival")
        self.assertEqual(info["CFBundleName"], "Rival")
        self.assertEqual(info["CFBundleExecutable"], "Rival")
        self.assertEqual(info["CFBundleIconFile"], "AppIcon")
        self.assertEqual(info["CFBundleShortVersionString"], "3.35.0")
        self.assertEqual(info["CFBundleVersion"], "3.35.0")
        self.assertEqual(info["LSMinimumSystemVersion"], "14.0")
        self.assertIs(info["LSUIElement"], False)
        self.assertIs(info["NSHighResolutionCapable"], True)

    def test_dev_keeps_its_id(self):
        info = appbundle.info_plist("0.0.0-dev", appbundle.DEV_ID, display_name="Rival (dev)", build="0")
        self.assertEqual(info["CFBundleIdentifier"], "dev.1905.rival.dev")
        self.assertEqual(info["CFBundleDisplayName"], "Rival (dev)")
        self.assertEqual(info["CFBundleVersion"], "0")

    def test_assemble_writes_bundle(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            binary = root / "RivalApp"
            binary.write_bytes(b"bin")
            icon = root / "icon.icns"
            icon.write_bytes(b"icns")
            app = appbundle.assemble(root / "Rival.app", binary, appbundle.info_plist("1.2.3", appbundle.RELEASE_ID), icon)
            self.assertEqual((app / "Contents/MacOS/Rival").read_bytes(), b"bin")
            self.assertEqual((app / "Contents/Resources/AppIcon.icns").read_bytes(), b"icns")
            with open(app / "Contents/Info.plist", "rb") as f:
                info = plistlib.load(f)
            self.assertEqual(info["CFBundleShortVersionString"], "1.2.3")
            self.assertEqual(info["CFBundleIdentifier"], "dev.1905.rival")


class RenderCaskTest(unittest.TestCase):
    BASE = "https://github.com/1905/rival/releases/download"

    def test_release_url(self):
        out = render_cask.render("v3.35.0", SHA, self.BASE + "/")
        self.assertIn('cask "rival-app" do', out)
        self.assertIn('version "3.35.0"', out)
        self.assertIn(f'sha256 "{SHA}"', out)
        self.assertIn('url "https://github.com/1905/rival/releases/download/v#{version}/Rival-app.zip"', out)
        self.assertIn('depends_on macos: ">= :sonoma"', out)
        self.assertIn('app "Rival.app"', out)
        self.assertIn('"-dr", "com.apple.quarantine", "#{appdir}/Rival.app"', out)
        self.assertNotIn("{{", out)

    def test_release_job_writes_cask_under_casks_dir(self):
        # Homebrew loads a tap's casks only from Casks/; a root-level .rb is
        # never seen by `brew install --cask 1905/tap/rival-app`.
        wf = (Path(__file__).resolve().parents[2] / ".github/workflows/release.yml").read_text()
        self.assertIn("mkdir -p tap/Casks", wf)
        self.assertIn("> tap/Casks/rival-app.rb", wf)
        self.assertIn("git add Casks/rival-app.rb", wf)
        self.assertNotIn("> tap/rival-app.rb", wf)
        self.assertNotIn("git add rival-app.rb", wf)

    def test_local_zip(self):
        out = render_cask.render("0.0.0-dev", SHA, self.BASE, local_zip="/tmp/x/Rival-app.zip")
        self.assertIn('url "file:///', out)
        self.assertIn('Rival-app.zip"', out)
        self.assertNotIn("releases/download", out)

    def test_rejects_bad_input(self):
        with self.assertRaises(ValueError):
            render_cask.render("1.0", "nothex", self.BASE)
        with self.assertRaises(ValueError):
            render_cask.render('1.0"; system "x', SHA, self.BASE)


class ManyFixtureTest(unittest.TestCase):
    NOW = dt.datetime(2026, 9, 26, 12, 0, tzinfo=dt.timezone.utc)

    def parse(self, s):
        return dt.datetime.fromisoformat(s.replace("Z", "+00:00"))

    def test_deterministic_and_finished(self):
        a = dev_bundle.many_sessions(130, self.NOW)
        self.assertEqual(a, dev_bundle.many_sessions(130, self.NOW))
        self.assertEqual(len(a), 130)
        self.assertEqual(len({s["id"] for s in a}), 130)
        self.assertEqual({s["status"] for s in a}, {"completed", "failed"})
        self.assertTrue(all("error" in s for s in a if s["status"] == "failed"))

    def test_spread_over_40_days_newest_first(self):
        starts = [self.parse(s["start_time"]) for s in dev_bundle.many_sessions(120, self.NOW)]
        self.assertEqual(starts, sorted(starts, reverse=True))
        self.assertLess(starts[0], self.NOW)
        self.assertGreater(starts[-1], self.NOW - dt.timedelta(days=41))
        self.assertLess(starts[-1], self.NOW - dt.timedelta(days=38))

    def test_ids_do_not_clash_with_checked_in_fixtures(self):
        fixture_ids = {json.loads(p.read_text())["id"] for p in dev_bundle.FIXTURES.glob("*.json")}
        self.assertFalse(fixture_ids & {s["id"] for s in dev_bundle.many_sessions(200, self.NOW)})

    def test_make_fixture_writes_many(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "home"
            dev_bundle.make_fixture(root, many=12)
            sessions = root / "sessions"
            base = len(list(dev_bundle.FIXTURES.glob("*.json")))
            self.assertEqual(len(list(sessions.glob("*.json"))), base + 12)
            one = json.loads((sessions / "f0000000-0000-4000-8000-000000000000.json").read_text())
            self.assertTrue(Path(one["log_file"]).is_file())


class TrashTest(unittest.TestCase):
    def test_trash_moves_with_timestamp_and_ns_suffix(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            dest = root / "trash"
            names = []
            for _ in range(2):
                src = root / "Rival.app"
                src.mkdir()
                moved = appbundle.trash(src, dest=dest)
                self.assertFalse(src.exists())
                self.assertTrue(moved.is_dir())
                self.assertEqual(moved.parent, dest)
                names.append(moved.name)
            for name in names:
                app, stamp, ns = name.rsplit(".", 2)
                self.assertEqual(app, "Rival.app")
                self.assertRegex(stamp, r"^\d{8}-\d{6}$")
                self.assertRegex(ns, r"^\d+$")
            self.assertNotEqual(names[0], names[1], "two moves in one second stay apart")


class LaunchLineTest(unittest.TestCase):
    def test_select_and_home(self):
        line = dev_bundle.launch_line(Path("/x/Rival.app"), ["RIVAL_HOME=/f", "RIVAL_SELECT=solo:abc"])
        self.assertEqual(line, "launch: open -n --env RIVAL_HOME=/f --env RIVAL_SELECT=solo:abc /x/Rival.app")


if __name__ == "__main__":
    unittest.main()
