import base64
import importlib.util
from pathlib import Path
import tempfile
import unittest
import xml.etree.ElementTree as ET

spec=importlib.util.spec_from_file_location('feed',Path(__file__).parents[1]/'macos-update-feed.py')
feed=importlib.util.module_from_spec(spec);spec.loader.exec_module(feed)

class ReleaseContract(unittest.TestCase):
    def test_feed_binds_version_architecture_signature_and_project(self):
        with tempfile.TemporaryDirectory() as tmp:
            archive=Path(tmp)/'moosik-1.2.3-macos-arm64.zip';archive.write_bytes(b'fixture')
            signature=base64.b64encode(bytes(64)).decode()
            root=ET.fromstring(feed.make_feed('1.2.3',archive,signature,'Fixed <queue> & playback', '14.0'))
            item=root.find('channel/item');enclosure=item.find('enclosure')
            self.assertEqual(enclosure.get('url'),'https://github.com/AtotheY/moosik/releases/download/moosik-v1.2.3/'+archive.name)
            self.assertEqual(enclosure.get('{'+feed.SPARKLE+'}edSignature'),signature)
            self.assertEqual(enclosure.get('length'),'7')
            self.assertEqual(item.find('{'+feed.SPARKLE+'}hardwareRequirements').text,'arm64')
            self.assertEqual(item.find('{'+feed.SPARKLE+'}minimumSystemVersion').text,'14.0')
            self.assertEqual(item.find('description').text,'Fixed <queue> & playback')
            self.assertEqual(item.find('description').get('{'+feed.SPARKLE+'}format'),'markdown')
    def test_wrong_version_or_architecture_cannot_publish(self):
        with tempfile.TemporaryDirectory() as tmp:
            archive=Path(tmp)/'moosik-1.2.3-macos-x86_64.zip';archive.touch()
            with self.assertRaises(ValueError):feed.make_feed('1.2.3',archive,base64.b64encode(bytes(64)).decode())
    def test_missing_signature_cannot_publish(self):
        with tempfile.TemporaryDirectory() as tmp:
            archive=Path(tmp)/'moosik-1.2.3-macos-arm64.zip';archive.touch()
            with self.assertRaises(ValueError):feed.make_feed('1.2.3',archive,'')
    def test_development_versions_are_not_public_releases(self):
        with self.assertRaises(ValueError):feed.make_feed('1.2.3+ark.1',Path('unused'),'')

if __name__=='__main__':unittest.main()
