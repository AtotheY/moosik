#!/usr/bin/env python3
"""macOS integration check with silent disposable apps and a separate LaunchAgent.

Run from a logged-in macOS session with the project's signing certificate available:
    python3 scripts/tests/macos-media-fallback-integration.py
No real music app, account, key events, or production service registration is touched.
"""
from pathlib import Path
import os
import plistlib
import re
import signal
import shutil
import subprocess
import sys
import time
repo = Path(__file__).resolve().parents[2]
os.chdir(repo)
if sys.platform != 'darwin':
    raise SystemExit('This integration check requires macOS.')
root = repo / '.ark-build/media-helper-test'
root.mkdir(parents=True, exist_ok=True)

def run(*args):
    return subprocess.run(list(map(str, args)), capture_output=True, text=True, check=True).stdout.strip()
for name, identifier in [('Player', 'com.moosik.media-fallback.player-test'), ('Target', 'com.moosik.media-fallback.fixture'), ('Unrelated', 'com.moosik.media-fallback.unrelated')]:
    app = root / (name + '.app')
    (app / 'Contents/MacOS').mkdir(parents=True, exist_ok=True)
    (app / 'Contents/Info.plist').write_bytes(plistlib.dumps(dict(CFBundleIdentifier=identifier, CFBundleExecutable='moosik', CFBundleName=name, CFBundlePackageType='APPL', CFBundleVersion='1', LSUIElement=True)))
agent = root / 'Player.app/Contents/Library/LaunchAgents'
agent.mkdir(parents=True, exist_ok=True)
p = plistlib.loads(Path('packaging/macos/com.moosik.media-fallback.plist').read_bytes())
p['Label'] = 'com.moosik.media-fallback.test'
(agent / 'com.moosik.media-fallback.test.plist').write_bytes(plistlib.dumps(p))
(root / 'player.swift').write_text('import Foundation\nlet path = Bundle.main.bundleURL.deletingLastPathComponent().appendingPathComponent("resumes.log")\nlet data = Data((CommandLine.arguments.dropFirst().joined(separator:" ")+"\\n").utf8)\nif !FileManager.default.fileExists(atPath:path.path) { FileManager.default.createFile(atPath:path.path,contents:nil) }\nlet file = try FileHandle(forWritingTo:path)\ntry file.seekToEnd()\ntry file.write(contentsOf:data)\ntry file.close()\n')
(root / 'target.swift').write_text('import AppKit\nlet app = NSApplication.shared\napp.setActivationPolicy(.regular)\napp.run()\n')
cache = repo / '.ark-build/swift-module-cache'
cache.mkdir(parents=True, exist_ok=True)
for name, source in [('Player', 'player.swift'), ('Target', 'target.swift'), ('Unrelated', 'target.swift')]:
    run('xcrun', 'swiftc', '-module-cache-path', cache, root / source, '-o', root / (name + '.app/Contents/MacOS/moosik'))
run('xcrun', 'swiftc', '-D', 'MEDIA_FALLBACK_TEST', '-module-cache-path', cache, repo / 'packaging/macos/MediaFallback.swift', '-o', root / 'Player.app/Contents/MacOS/moosik-media-helper')
monitor = root / 'Player.app/Contents/Helpers/MoosikMediaHelper.app/Contents'
(monitor / 'MacOS').mkdir(parents=True, exist_ok=True)
info = plistlib.loads((repo / 'packaging/macos/MediaHelper-Info.plist').read_bytes())
info['CFBundleIdentifier'] = 'com.moosik.media-fallback.test.helper'
(monitor / 'Info.plist').write_bytes(plistlib.dumps(info))
shutil.copyfile(root / 'Player.app/Contents/MacOS/moosik-media-helper', monitor / 'MacOS/moosik-media-monitor')
(monitor / 'MacOS/moosik-media-monitor').chmod(0o755)
for name in ['Player', 'Target', 'Unrelated']:
    run(repo / 'scripts/sign-macos.sh', root / (name + '.app'))
    run('/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister', '-f', root / (name + '.app'))
helper = root / 'Player.app/Contents/MacOS/moosik-media-helper'
log = root / 'resumes.log'

def pids(name):
    p = subprocess.run(['pgrep', '-f', '^' + re.escape(str(root / (name + '.app/Contents/MacOS/moosik'))) + '$'], capture_output=True, text=True)
    return [int(x) for x in p.stdout.split()]

def stop(name):
    for pid in pids(name):
        os.kill(pid, signal.SIGTERM)

def wait(fn):
    for _ in range(100):
        if fn():
            return
        time.sleep(0.1)
    raise AssertionError('Timed out')

def launch(name):
    run('/usr/bin/open', '-g', '-n', root / (name + '.app'))

def count():
    return len(log.read_text().splitlines()) if log.exists() else 0
try:
    run(helper, '--disable')
    if log.exists():
        log.unlink()
    launch('Target')
    wait(lambda: bool(pids('Target')))
    time.sleep(2)
    assert run(helper, '--enable') == 'on'
    run('launchctl', 'print', f'gui/{os.getuid()}/com.moosik.media-fallback.test')
    time.sleep(1)
    assert pids('Target') and count() == 0, 'Enabling must leave an existing player alone'
    stop('Target')
    wait(lambda: not pids('Target'))
    launch('Unrelated')
    wait(lambda: bool(pids('Unrelated')))
    launch('Target')
    wait(lambda: count() == 1 and (not pids('Target')))
    time.sleep(0.5)
    assert count() == 1, 'Duplicate launch notifications must resume only once'
    assert log.read_text().strip() == '--resume'
    assert pids('Unrelated'), 'Other apps must remain running'
    # A plain app launch must still start the player while the monitor is running.
    run('/usr/bin/open', '-g', root / 'Player.app')
    wait(lambda: count() == 2)
    assert log.read_text().splitlines()[-1] == '', 'A normal launch must not be swallowed by the monitor'
    launch('Target')
    wait(lambda: count() == 3 and (not pids('Target')))
    assert run(helper, '--disable') == 'off'
    launch('Target')
    wait(lambda: bool(pids('Target')))
    time.sleep(0.5)
    assert count() == 3 and pids('Target'), 'Disabled helper must leave launches alone'
    print('PASS: existing player preserved; unrelated app preserved; two target launches each resume once; disable stops interception')
finally:
    run(helper, '--disable')
    stop('Target')
    stop('Unrelated')
