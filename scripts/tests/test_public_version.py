from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import tomllib
import unittest


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts/public-version.py"


class PublicVersionTest(unittest.TestCase):
    def test_select_and_stamp_release(self):
        with tempfile.TemporaryDirectory() as folder:
            repo = Path(folder)

            def run(*args):
                return subprocess.check_output(args, cwd=repo, text=True).strip()

            for name in ["Cargo.lock", "Cargo.toml"]:
                shutil.copy(ROOT / name, repo / name)
            (repo / "release-notes").mkdir()
            old = tomllib.loads((repo / "Cargo.toml").read_text())["workspace"]["package"]["version"]
            major, minor, patch = map(int, old.split("."))
            new = f"{major}.{minor}.{patch + 1}"
            tag = "moosik-v" + new
            self.assertEqual(run(sys.executable, SCRIPT, "next", "moosik-v" + old), new)
            self.assertEqual(run(sys.executable, SCRIPT, "next", "moosik-v0.0.0"), old)
            run("git", "init", "--quiet")
            run("git", "config", "user.name", "Release test")
            run("git", "config", "user.email", "release@example.invalid")
            run("git", "commit", "--allow-empty", "-qm", "Fixture")
            run("git", "tag", "-a", tag, "-m", "Changes in this release")
            lock_before = tomllib.loads((repo / "Cargo.lock").read_text())["package"]
            for _ in range(2):
                run(sys.executable, SCRIPT, "stamp", tag)
            manifest = tomllib.loads((repo / "Cargo.toml").read_text())
            self.assertEqual(manifest["workspace"]["package"]["version"], new)
            lock_after = tomllib.loads((repo / "Cargo.lock").read_text())["package"]
            changed = [p["name"] for before, p in zip(lock_before, lock_after, strict=True) if before != p]
            self.assertCountEqual(changed, ["moosik", "moosik-resolve"])
            self.assertTrue(all(p["version"] == new for p in lock_after if p["name"] in changed))
            self.assertEqual((repo / f"release-notes/{new}.md").read_text(), "Changes in this release\n")
            result = subprocess.run([sys.executable, SCRIPT, "stamp", "moosik-v" + old],
                                    cwd=repo, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
