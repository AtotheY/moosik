#!/usr/bin/env python3
"""Report the macOS version needed by every Mach-O in a bundled application."""
from pathlib import Path
import re
import subprocess
import sys

def minimum_version(app):
    versions = [(11, 0)]
    magic = {bytes.fromhex(value) for value in
             ('feedface', 'cefaedfe', 'feedfacf', 'cffaedfe', 'cafebabe', 'bebafeca', 'cafebabf', 'bfbafeca')}
    for path in Path(app).rglob('*'):
        if not path.is_file() or path.is_symlink():
            continue
        with path.open('rb') as stream:
            if stream.read(4) not in magic:
                continue
        commands = subprocess.check_output(['otool', '-l', str(path)], text=True)
        for command in commands.split('Load command'):
            field = 'minos' if 'cmd LC_BUILD_VERSION\n' in command else 'version'
            if field == 'version' and 'cmd LC_VERSION_MIN_MACOSX\n' not in command:
                continue
            match = re.search(r'\b' + field + r'\s+(\d+(?:\.\d+)+)', command)
            if match:
                versions.append(tuple(map(int, match[1].split('.'))))
    return '.'.join(map(str, max(versions)))

if __name__ == '__main__':
    print(minimum_version(sys.argv[1]))
