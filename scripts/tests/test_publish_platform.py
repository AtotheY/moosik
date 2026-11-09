import hashlib
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location("publisher", Path(__file__).resolve().parents[1] / "publish-platform.py")
publisher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(publisher)


class PlatformReleaseTests(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        self.latest = "moosik-v0.1.50"
        self.assets = {self.latest: {}}
        self.drafts = set()
        self.fail_upload = None
        self.enterContext(patch.object(publisher, "view", self.view))
        self.enterContext(patch.object(publisher, "gh", self.gh))

    def view(self, tag=None):
        tag = tag or self.latest
        if tag in self.assets:
            return {"tagName": tag, "isDraft": tag in self.drafts, "assets": [
                {"name": name, "state": "uploaded", "digest": "sha256:" + hashlib.sha256(data).hexdigest()}
                for name, data in self.assets[tag].items()
            ]}

    def gh(self, command, tag, *args):
        if command == "create":
            self.assertNotIn(tag, self.assets)
            self.drafts.add(tag)
            self.assets[tag] = {}
        elif command == "upload":
            path = Path(args[0])
            if path.name == self.fail_upload:
                raise RuntimeError("interrupted upload")
            self.assets[tag][path.name] = path.read_bytes()
        elif command == "edit":
            self.drafts.discard(tag)
            if "--latest=true" in args:
                self.latest = tag
        else:
            self.fail(command)

    def publish(self, platform, version="0.1.52", build="one", corrupt=False):
        with tempfile.TemporaryDirectory(dir=self.root) as work:
            dist = Path(work)
            hashes = {}
            for template in publisher.FILES[platform]:
                name = template.format(version)
                (dist / name).write_text(name + build)
                hashes[name] = publisher.digest(dist / name)
            sums = publisher.manifest_text(hashes)
            if platform == "windows":
                sums = sums.replace("  ", " *")
            source = f"moosik-{version if platform == 'macos' else platform}-SHA256SUMS.txt"
            (dist / source).write_text("corrupt" if corrupt else sums)
            publisher.publish("moosik-v" + version, platform, dist, self.root)

    def test_independent_publication_and_late_reruns(self):
        tag = "moosik-v0.1.52"
        self.publish("windows")
        self.assertNotIn(tag, self.drafts)
        self.assertNotEqual(tag, self.latest)
        self.publish("macos")
        self.assertEqual(self.latest, tag)
        before = dict(self.assets[tag])
        self.publish("windows", build="two")
        self.assertEqual(self.assets[tag], before)
        self.publish("linux")
        self.publish("macos", version="0.1.51")
        self.assertEqual(self.latest, tag)
        for name, data in before.items():
            if name != "moosik-0.1.52-SHA256SUMS.txt":
                self.assertEqual(self.assets[tag][name], data)
        files = {name: hashlib.sha256(data).hexdigest() for name, data in self.assets[tag].items()
                 if not name.endswith("SHA256SUMS.txt")}
        self.assertEqual(self.assets[tag]["moosik-0.1.52-SHA256SUMS.txt"].decode(), publisher.manifest_text(files))

    def test_bad_checksums_and_interrupted_upload(self):
        tag = "moosik-v0.1.52"
        with self.assertRaises(ValueError):
            self.publish("linux", corrupt=True)
        self.assertNotIn(tag, self.assets)
        self.publish("linux")
        self.fail_upload = "appcast.xml"
        with self.assertRaises(RuntimeError):
            self.publish("macos")
        self.assertNotIn("moosik-0.1.52-macos-SHA256SUMS.txt", self.assets[tag])
        self.assertNotEqual(self.latest, tag)
        self.publish("windows")
        self.assertNotIn("macos-arm64", self.assets[tag]["moosik-0.1.52-SHA256SUMS.txt"].decode())
        self.fail_upload = None
        self.publish("macos", build="retry")
        self.assertEqual(self.latest, tag)
        self.assertTrue(self.assets[tag]["moosik-0.1.52-macos-arm64.zip"].endswith(b"retry"))
