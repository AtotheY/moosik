#!/usr/bin/env bash
# Build dist/Moosik.app, a zip of it, and a drag-to-Applications disk image
# (dist/moosik-<version>-macos-<arch>.zip and .dmg).
#
# The bundle holds the release binary, the Moosik icon, libmpv and its whole dylib
# tail copied into Contents/Frameworks, and a standalone yt-dlp next to the executable (where
# `backend::yt_dlp_program` looks first). Nothing outside macOS itself is needed at run time.
#
# Needs Homebrew's mpv and dylibbundler (`brew install mpv dylibbundler`), plus Xcode command line tools, codesign and hdiutil.
# Local builds reuse the identity configured by setup-macos-signing.sh. Without one,
# packaging falls back to ad-hoc signing; a copy downloaded elsewhere needs the quarantine
# flag cleared (`xattr -cr Moosik.app`) or a real Developer ID signature.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

command -v dylibbundler >/dev/null || { echo "missing dylibbundler: brew install dylibbundler" >&2; exit 1; }

version="$(cargo metadata --no-deps --format-version 1 \
    | python3 -c 'import json, sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "moosik"))')"
arch="$(uname -m)"
app_name="${MOOSIK_APP_NAME:-Moosik}"
prefix=moosik
app="dist/$app_name.app"
zip="dist/$prefix-$version-macos-$arch.zip"

echo "building release binary"
cargo build --locked --release -p moosik --bin moosik

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$app/Contents/Frameworks"
cp target/release/moosik "$app/Contents/MacOS/moosik"
cp LICENSE "$app/Contents/Resources/LICENSE"

echo "building the icon"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cp assets/icons/app/moosik.icns "$app/Contents/Resources/moosik.icns"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>$app_name</string>
    <key>CFBundleDisplayName</key><string>$app_name</string>
    <key>CFBundleIdentifier</key><string>io.github.gridoe.refryt</string>
    <key>CFBundleExecutable</key><string>moosik</string>
    <key>CFBundleIconFile</key><string>moosik</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleVersion</key><string>$version</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.music</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

echo "bundling libmpv"
dylibbundler -od -b -x "$app/Contents/MacOS/moosik" \
    -d "$app/Contents/Frameworks" -p "@executable_path/../Frameworks" \
    -s /opt/homebrew/lib -s /usr/local/lib >/dev/null 2>&1

# dylibbundler adds its rpath once per library it rewrites, and dyld refuses a binary with the
# same LC_RPATH twice. Keep exactly one.
rpath="@executable_path/../Frameworks"
for bin in "$app/Contents/MacOS/moosik" "$app"/Contents/Frameworks/*.dylib; do
    count="$(otool -l "$bin" | grep -c "path $rpath/\? " || true)"
    while [ "$count" -gt 1 ]; do
        install_name_tool -delete_rpath "$rpath/" "$bin" 2>/dev/null \
            || install_name_tool -delete_rpath "$rpath" "$bin"
        count=$((count - 1))
    done
done

echo "fetching yt-dlp"
curl -fsSL -o "$app/Contents/MacOS/yt-dlp" \
    https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_macos
chmod +x "$app/Contents/MacOS/yt-dlp"

if [ "$app_name" = Moosik ]; then
    "$root/scripts/prepare-macos-media-helper.sh" "$app"
    plutil -replace CFBundleURLTypes -json "$(cat "$root/packaging/macos/url-types.json")" "$app/Contents/Info.plist"
    "$root/scripts/prepare-macos-updater.sh" "$app"
fi

echo "signing"
# codesign fails now and then on a CI runner with nothing to say, and passes on the next try.
signed=0
for attempt in 1 2 3; do
    if "$root/scripts/sign-macos.sh" --allow-adhoc "$app"; then
        signed=1
        break
    fi
    echo "codesign failed (attempt $attempt of 3)" >&2
    sleep 3
done
[ "$signed" = 1 ] || { echo "error: codesign failed" >&2; exit 1; }

rm -f "$zip"
ditto -c -k --keepParent "$app" "$zip"

# A drag-to-Applications disk image: the app plus a symlink to /Applications.
dmg="dist/$prefix-$version-macos-$arch.dmg"
stage="$work/dmg"
mkdir -p "$stage"
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"
rm -f "$dmg"
hdiutil create -quiet -volname "$app_name" -srcfolder "$stage" -ov -format UDZO "$dmg"
echo "built $app, $zip and $dmg"
