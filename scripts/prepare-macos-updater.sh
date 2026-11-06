#!/bin/bash
# Bundle a pinned Sparkle distribution and configure only Moosik's signed update feed.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
app="${1:?usage: prepare-macos-updater.sh app-path}"
version=2.10.1
checksum=1a856d2a9edb8e628de88121d7be04cf0e2167f8641650b78fca00b14244cf27
cache="$root/.ark-build/sparkle-$version"
archive="$cache.tar.xz"
mkdir -p "$root/.ark-build"
if [ ! -f "$archive" ]; then
    curl --fail --location --retry 3 "https://github.com/sparkle-project/Sparkle/releases/download/$version/Sparkle-$version.tar.xz" -o "$archive.part"
    mv "$archive.part" "$archive"
fi
printf '%s  %s\n' "$checksum" "$archive" | shasum -a 256 -c - >/dev/null
if [ ! -d "$cache/Sparkle.framework" ]; then
    mkdir -p "$cache"
    tar -xJf "$archive" -C "$cache"
fi
mkdir -p "$app/Contents/Frameworks" "$app/Contents/Resources/Licenses"
ditto "$cache/Sparkle.framework" "$app/Contents/Frameworks/Sparkle.framework"
cp "$cache/LICENSE" "$app/Contents/Resources/Licenses/Sparkle.txt"
minimum=$(python3 "$root/scripts/macos-minimum-version.py" "$app")
/usr/libexec/PlistBuddy -c "Set :LSMinimumSystemVersion $minimum" "$app/Contents/Info.plist"
export MOOSIK_UPDATE_PUBLIC_KEY="${MOOSIK_UPDATE_PUBLIC_KEY:-}"
export MOOSIK_UPDATE_FEED_URL="${MOOSIK_UPDATE_FEED_URL:-https://github.com/AtotheY/moosik/releases/latest/download/appcast.xml}"
export MOOSIK_UPDATER_TEST="${MOOSIK_UPDATER_TEST:-0}"
export MOOSIK_DISTRIBUTION="${MOOSIK_DISTRIBUTION:-0}"
python3 - "$app/Contents/Info.plist" "$root/packaging/macos/update-public-key.txt" <<'PY'
import base64, os, pathlib, plistlib, sys
path=pathlib.Path(sys.argv[1]); key_path=pathlib.Path(sys.argv[2])
key=os.environ['MOOSIK_UPDATE_PUBLIC_KEY'].strip() or (key_path.read_text().strip() if key_path.exists() else '')
feed=os.environ['MOOSIK_UPDATE_FEED_URL']
public=os.environ['MOOSIK_DISTRIBUTION']=='1'
if public and feed!='https://github.com/AtotheY/moosik/releases/latest/download/appcast.xml':
    raise SystemExit('Public releases must use the Moosik release feed')
if not feed.startswith('https://') and not (not public and os.environ['MOOSIK_UPDATER_TEST']=='1' and feed.startswith('http://127.0.0.1:')):
    raise SystemExit('The update feed must use HTTPS')
if key and len(base64.b64decode(key,validate=True))!=32:
    raise SystemExit('Sparkle public key must contain 32 bytes')
if public and not key:
    raise SystemExit('Missing Sparkle public key: run scripts/setup-update-signing.sh first')
with path.open('rb') as f: info=plistlib.load(f)
for k in ['SUFeedURL','SUPublicEDKey','SUEnableAutomaticChecks','SUAutomaticallyUpdate','SUScheduledCheckInterval','SUVerifyUpdateBeforeExtraction','SUEnableSystemProfiling']:
    info.pop(k,None)
if key:
    info.update(SUFeedURL=feed,SUPublicEDKey=key,SUEnableAutomaticChecks=True,
        SUAutomaticallyUpdate=True,SUScheduledCheckInterval=900,SUVerifyUpdateBeforeExtraction=True,SUEnableSystemProfiling=False)
else:
    print('Sparkle bundled; automatic installation awaits the release signing public key.')
# Every public release bumps the SemVer; Sparkle compares this numeric bundle version.
info['CFBundleVersion']=info['CFBundleShortVersionString'].split('+')[0]
with path.open('wb') as f: plistlib.dump(info,f)
PY
