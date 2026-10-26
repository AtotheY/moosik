#!/bin/bash
# Build our app while retaining the verified upstream playback libraries and artwork.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
project="$(dirname "$root")"
cd "$root"
template="${REFRYT_TEMPLATE_APP:-$project/Moosik.app}"
if [ ! -d "$template" ]; then template="$project/Refryt.app"; fi
test -f "$template/Contents/Frameworks/libmpv.2.dylib"
./scripts/ark-cargo.sh build --locked --release -p refryt --bin refryt
stage="$root/dist/Moosik.app"
mkdir -p "$root/dist"
if [ -e "$stage" ]; then
    mv "$stage" "$root/.ark-build/previous-package-$(date +%s).app"
fi
ditto "$template" "$stage"
cp target/release/refryt "$stage/Contents/MacOS/refryt"
version=$(./scripts/ark-cargo.sh metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"]=="refryt"))')
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $version" "$stage/Contents/Info.plist"
/usr/libexec/PlistBuddy -c 'Set :CFBundleVersion 0.1.1402' "$stage/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleDisplayName Moosik" "$stage/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleName Moosik" "$stage/Contents/Info.plist"
codesign --force --deep --sign - "$stage"
codesign --verify --deep --strict "$stage"
printf 'Built %s (%s)\n' "$stage" "$version"
