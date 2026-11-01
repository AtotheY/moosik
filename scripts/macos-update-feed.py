#!/usr/bin/env python3
"""Create a Sparkle feed for an already signed Moosik archive."""
import argparse
import base64
from datetime import datetime, timezone
from email.utils import format_datetime
from pathlib import Path
import re
import xml.etree.ElementTree as ET

SPARKLE = 'http://www.andymatuschak.org/xml-namespaces/sparkle'
RELEASES = 'https://github.com/AtotheY/moosik/releases/'
ET.register_namespace('sparkle', SPARKLE)

def make_feed(version, archive, signature, notes='', minimum_system_version='11.0'):
    if not re.fullmatch(r'\d+\.\d+(?:\.\d+)?', minimum_system_version):
        raise ValueError('Invalid minimum macOS version')
    if not re.fullmatch(r'\d+\.\d+\.\d+', version):
        raise ValueError('Public versions must be major.minor.patch')
    if archive.name != f'moosik-{version}-macos-arm64.zip':
        raise ValueError('Archive name/version/architecture mismatch')
    if len(base64.b64decode(signature, validate=True)) != 64:
        raise ValueError('Invalid EdDSA archive signature')
    rss=ET.Element('rss', version='2.0'); channel=ET.SubElement(rss,'channel')
    ET.SubElement(channel,'title').text='Moosik updates'
    ET.SubElement(channel,'link').text=RELEASES
    item=ET.SubElement(channel,'item')
    ET.SubElement(item,'title').text=f'Moosik {version}'
    ET.SubElement(item,'pubDate').text=format_datetime(datetime.now(timezone.utc))
    for tag,text in [('version',version),('shortVersionString',version),('minimumSystemVersion',minimum_system_version),('hardwareRequirements','arm64')]:
        ET.SubElement(item,f'{{{SPARKLE}}}{tag}').text=text
    if notes:
        # Sparkle 2.9+ renders Markdown natively; omitting the format defaults to HTML
        # and collapses our release notes into a single paragraph.
        ET.SubElement(item,'description', {f'{{{SPARKLE}}}format':'markdown'}).text=notes
    ET.SubElement(item,'enclosure', {
        'url':f'{RELEASES}download/moosik-v{version}/{archive.name}',
        f'{{{SPARKLE}}}edSignature':signature,
        'length':str(archive.stat().st_size), 'type':'application/octet-stream'})
    ET.indent(rss)
    return ET.tostring(rss,encoding='utf-8',xml_declaration=True)+b'\n'

def main():
    p=argparse.ArgumentParser();p.add_argument('version');p.add_argument('archive',type=Path)
    p.add_argument('--signature-file',required=True,type=Path);p.add_argument('--notes',type=Path)
    p.add_argument('--output',type=Path,default=Path('dist/appcast.xml'))
    p.add_argument('--minimum-system-version', default='11.0');args=p.parse_args()
    args.output.write_bytes(make_feed(args.version,args.archive,args.signature_file.read_text().strip(),
        args.notes.read_text() if args.notes else '', args.minimum_system_version))
if __name__=='__main__':main()
