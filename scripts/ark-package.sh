#!/bin/bash
# Build our app while retaining the verified upstream playback libraries and artwork.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
project="$(dirname "$root")"
cd "$root"
template="${MOOSIK_TEMPLATE_APP:-$project/Moosik.app}"
test -f "$template/Contents/Frameworks/libmpv.2.dylib"
./scripts/ark-cargo.sh build --locked --release -p moosik --bin moosik
stage="$root/dist/Moosik.app"
mkdir -p "$root/dist"
if [ -e "$stage" ]; then
    mv "$stage" "$root/.ark-build/previous-package-$(date +%s).app"
fi
ditto "$template" "$stage"
old_exe=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$stage/Contents/Info.plist")
if [ "$old_exe" = refryt ]; then rm -f "$stage/Contents/MacOS/$old_exe"; fi
/usr/libexec/PlistBuddy -c 'Set :CFBundleExecutable moosik' "$stage/Contents/Info.plist"
cp target/release/moosik "$stage/Contents/MacOS/moosik"
version=$(./scripts/ark-cargo.sh metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"]=="moosik"))')
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $version" "$stage/Contents/Info.plist"
/usr/libexec/PlistBuddy -c 'Set :CFBundleVersion 0.1.1407' "$stage/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleDisplayName Moosik" "$stage/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleName Moosik" "$stage/Contents/Info.plist"
cp "$root/assets/icons/app/moosik.icns" "$stage/Contents/Resources/moosik.icns"
/usr/libexec/PlistBuddy -c "Set :CFBundleIconFile moosik" "$stage/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Delete :CFBundleIconName" "$stage/Contents/Info.plist" 2>/dev/null || true
# The upstream compiled icon catalog would override our classic ICNS on newer macOS.
if [ -f "$stage/Contents/Resources/Assets.car" ]; then
    mv "$stage/Contents/Resources/Assets.car" "$root/.ark-build/upstream-icon-catalog-$(date +%s).car"
fi
"$root/scripts/prepare-macos-media-helper.sh" "$stage"
plutil -replace CFBundleURLTypes -json "$(cat "$root/packaging/macos/url-types.json")" "$stage/Contents/Info.plist"
"$root/scripts/prepare-macos-updater.sh" "$stage"
"$root/scripts/sign-macos.sh" "$stage"
printf 'Built %s (%s)\n' "$stage" "$version"
