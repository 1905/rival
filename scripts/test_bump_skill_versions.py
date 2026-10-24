"""bump-skill-versions.sh reads the skill names from skills.rs NAMES in
both rustfmt forms (one line, one name per line) and bumps each SKILL.md.
Runs the script on a copy in a temp tree; the repo is not changed.
"""

import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "bump-skill-versions.sh"
NAMES_FILE = Path("crates/rival-core/src/skills.rs")
SKILLS_DIR = Path("crates/rival-core/skills")

ONE_LINE = 'pub const NAMES: [&str; 1] = ["rival"];\n'
MULTI_LINE = 'pub const NAMES: [&str; 2] = [\n    "rival-a",\n    "rival-b",\n];\n'
TAIL = '\npub const DEPRECATED: [&str; 1] = ["rival-old"];\n'


def skill_md(name, version):
    return f"---\nname: {name}\nversion: {version}\n---\n\nbody\n"


class BumpSkillVersionsTest(unittest.TestCase):
    def run_bump(self, names_rs, names, version="9.9.9"):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "scripts").mkdir()
            shutil.copy(SCRIPT, root / "scripts" / SCRIPT.name)
            (root / NAMES_FILE).parent.mkdir(parents=True)
            (root / NAMES_FILE).write_text(names_rs)
            for name in names + ["rival-old"]:
                (root / SKILLS_DIR / name).mkdir(parents=True)
                (root / SKILLS_DIR / name / "SKILL.md").write_text(skill_md(name, "1.0.0"))
            proc = subprocess.run(
                ["bash", str(root / "scripts" / SCRIPT.name), version],
                capture_output=True,
                text=True,
            )
            texts = {
                name: (root / SKILLS_DIR / name / "SKILL.md").read_text()
                for name in names + ["rival-old"]
            }
            return proc, texts

    def test_one_line_names(self):
        proc, texts = self.run_bump(ONE_LINE + TAIL, ["rival"])
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(texts["rival"], skill_md("rival", "9.9.9"))
        self.assertEqual(texts["rival-old"], skill_md("rival-old", "1.0.0"))

    def test_multi_line_names(self):
        proc, texts = self.run_bump(MULTI_LINE + TAIL, ["rival-a", "rival-b"])
        self.assertEqual(proc.returncode, 0, proc.stderr)
        for name in ["rival-a", "rival-b"]:
            self.assertEqual(texts[name], skill_md(name, "9.9.9"))
        self.assertEqual(texts["rival-old"], skill_md("rival-old", "1.0.0"))

    def test_the_repo_names_parse(self):
        real = (ROOT / NAMES_FILE).read_text()
        proc, texts = self.run_bump(real, ["rival", "china"])
        self.assertEqual(proc.returncode, 0, proc.stderr)
        for name in ["rival", "china"]:
            self.assertEqual(texts[name], skill_md(name, "9.9.9"))

    def test_no_names_fails(self):
        proc, _ = self.run_bump("pub const OTHER: [&str; 0] = [];\n", [])
        self.assertEqual(proc.returncode, 1)
        self.assertIn("could not read skill names", proc.stderr)


if __name__ == "__main__":
    unittest.main()
