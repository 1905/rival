"""Release contracts that `goreleaser check` does not cover: the publication
boundary in release.yml, the six targets and archive names, the formula's
tap, and target-scoped build environment. Static checks; nothing is built.

Requires PyYAML (`import yaml`); a missing module fails the run.
"""

import unittest
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent
TAG_GUARD = "github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v')"
SIX = {f"{system}_{arch}" for system in ("darwin", "linux", "windows") for arch in ("amd64", "arm64")}
GOARCH = {"x86_64": "amd64", "aarch64": "arm64"}
GOOS = {"apple-darwin": "darwin", "unknown-linux-gnu": "linux", "pc-windows-msvc": "windows"}


def load(path):
    return yaml.safe_load((ROOT / path).read_text())


def os_arch(triple):
    arch, rest = triple.split("-", 1)
    return f"{GOOS[rest]}_{GOARCH[arch]}"


class GoreleaserTest(unittest.TestCase):
    def setUp(self):
        self.cfg = load(".goreleaser.yaml")
        self.builds = {b["id"]: b for b in self.cfg["builds"]}

    def test_builds_cover_six_targets_once(self):
        targets = [os_arch(t) for b in self.builds.values() for t in b["targets"]]
        self.assertEqual(sorted(targets), sorted(SIX))
        for build in self.builds.values():
            self.assertEqual(build["builder"], "rust")
            self.assertIn("-p=rival", build["flags"])  # workspace: GoReleaser needs the package

    def test_windows_runs_cargo_xwin_build(self):
        # GoReleaser runs `tool command --target=<triple> flags...`; `build`
        # in flags would land after --target.
        win = self.builds["rival-windows"]
        self.assertEqual({os_arch(t).split("_")[0] for t in win["targets"]}, {"windows"})
        self.assertEqual((win["tool"], win["command"]), ("cargo-xwin", "build"))
        self.assertNotIn("build", win["flags"])

    def test_target_scoped_environment(self):
        unix, win = self.builds["rival-unix"]["env"], self.builds["rival-windows"]["env"]
        for env in (unix, win):
            self.assertIn("RIVAL_VERSION={{ .Version }}", env)
        sdk = [e for e in unix + win if "SDKROOT" in e]
        self.assertEqual(len(sdk), 1)
        self.assertTrue(sdk[0].startswith('{{ if eq .Os "darwin" }}') and sdk[0].endswith("{{ end }}"))
        self.assertIn(sdk[0], unix)
        for name in ("RUSTFLAGS", "PATH", "RIVAL_LLVM_BIN"):
            self.assertFalse(any(name in e for e in unix), name)
        self.assertIn("RUSTFLAGS=-C target-feature=+crt-static", win)

    def test_windows_selects_clang_cross_compiler(self):
        # cargo-xwin's default clang-cl mode passes /imsvc to clang when ring
        # builds its C code; the clang mode uses its own sysroot instead.
        unix, win = self.builds["rival-unix"]["env"], self.builds["rival-windows"]["env"]
        self.assertEqual([e for e in win if e.startswith("XWIN_")], ["XWIN_CROSS_COMPILER=clang"])
        self.assertFalse(any("XWIN_" in e for e in unix))
        self.assertFalse(any("XWIN_" in e for e in self.cfg.get("env", [])))

    def test_archive_names_and_files(self):
        (archive,) = self.cfg["archives"]
        names = {archive["name_template"].replace("{{ .ProjectName }}", "rival")
                 .replace("{{ .Os }}_{{ .Arch }}", target) for target in SIX}
        self.assertEqual(names, {f"rival_{t}" for t in SIX})
        self.assertEqual(archive["format_overrides"], [{"goos": "windows", "formats": ["zip"]}])
        for name in ("LICENSE", "README.md", "licenses/Go-LICENSE"):
            self.assertIn(name, archive["files"])
            self.assertTrue((ROOT / name).is_file(), name)

    def test_formula_keeps_the_tap(self):
        (brew,) = self.cfg["brews"]
        self.assertEqual(brew["name"], "rival")
        self.assertNotIn("directory", brew)  # rival.rb at the tap root
        self.assertEqual(brew["repository"], {"owner": "1905", "name": "homebrew-tap", "branch": "master",
                                              "token": "{{ .Env.HOMEBREW_TAP_TOKEN }}"})


class WorkflowTest(unittest.TestCase):
    def setUp(self):
        self.wf = load(".github/workflows/release.yml")
        self.jobs = self.wf["jobs"]

    def job_text(self, name):
        return yaml.safe_dump(self.jobs[name])

    def test_dispatch_has_no_inputs(self):
        on = self.wf.get("on", self.wf.get(True))  # YAML 1.1 reads `on` as true
        self.assertEqual(on, {"push": {"tags": ["v*"]}, "workflow_dispatch": None})
        self.assertEqual(self.wf["permissions"], {"contents": "read"})

    def test_secrets_and_write_access_only_on_the_tag_path(self):
        for name, job in self.jobs.items():
            if "secrets." in self.job_text(name) or job.get("permissions", {}).get("contents") == "write":
                self.assertEqual(job.get("if"), TAG_GUARD, name)
        self.assertEqual(self.jobs["release"]["if"], TAG_GUARD)
        self.assertEqual(self.jobs["app"]["if"], TAG_GUARD)

    def test_snapshot_jobs_publish_nothing(self):
        for name, job in self.jobs.items():
            if job.get("if") == TAG_GUARD:
                continue
            self.assertEqual(job["if"], "github.event_name == 'workflow_dispatch'", name)
            self.assertEqual(job["permissions"], {"contents": "read"}, name)
            text = self.job_text(name)
            for word in ("secrets.", "gh release", "git push", "git tag", "goreleaser publish"):
                self.assertNotIn(word, text, name)
            for step in job["steps"]:
                if "goreleaser" in step.get("run", ""):
                    self.assertIn("--snapshot", step["run"], name)

    def test_native_checks_cover_six_targets(self):
        job = self.jobs["snapshot-native"]
        targets = {entry["target"] for entry in job["strategy"]["matrix"]["include"]}
        self.assertEqual(targets, SIX)
        self.assertEqual(job["needs"], "snapshot")


if __name__ == "__main__":
    unittest.main()
