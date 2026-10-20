#!/usr/bin/env bash
# Build dist/Refryt.app, a zip of it, and a drag-to-Applications disk image
# (dist/refryt-<version>-macos-<arch>.zip and .dmg).
#
# The bundle holds the release binary, an adaptive icon (see below), libmpv and its whole dylib
# tail copied into Contents/Frameworks, and a standalone yt-dlp next to the executable (where
# `backend::yt_dlp_program` looks first). Nothing outside macOS itself is needed at run time.
#
# Needs Homebrew's mpv and dylibbundler (`brew install mpv dylibbundler`), plus Xcode 26 for
# actool (the icon) and the command line tools for iconutil, sips, codesign and hdiutil. The app is ad-hoc signed, which is
# enough to run on the machine that built it; a copy downloaded elsewhere needs the quarantine
# flag cleared (`xattr -cr Refryt.app`) or a real Developer ID signature.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

command -v dylibbundler >/dev/null || { echo "missing dylibbundler: brew install dylibbundler" >&2; exit 1; }

version="$(cargo metadata --no-deps --format-version 1 \
    | python3 -c 'import json, sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "refryt"))')"
arch="$(uname -m)"
app="dist/Refryt.app"
zip="dist/refryt-$version-macos-$arch.zip"

echo "building release binary"
cargo build --release -p refryt --bin refryt

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$app/Contents/Frameworks"
cp target/release/refryt "$app/Contents/MacOS/refryt"

echo "building the icon"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
# macOS 26 restyles an icon for Dark, Clear and Tinted (monochrome) mode, but only one built as an
# Icon Composer document and compiled into Assets.car; a plain .icns just gets the generic
# treatment. actool also writes a .icns fallback for older systems.
icon_keys="<key>CFBundleIconFile</key><string>refryt</string>"
mkdir -p "$work/icon"
if xcrun actool assets/icons/app/Refryt.icon --compile "$work/icon" \
    --output-format human-readable-text --errors --warnings \
    --output-partial-info-plist "$work/icon.plist" --app-icon Refryt \
    --include-all-app-icons --enable-on-demand-resources NO --development-region en \
    --target-device mac --minimum-deployment-target 26.0 --platform macosx >/dev/null 2>&1 \
    && [ -f "$work/icon/Assets.car" ]; then
    cp "$work/icon/Assets.car" "$app/Contents/Resources/Assets.car"
    cp "$work/icon/Refryt.icns" "$app/Contents/Resources/refryt.icns"
    icon_keys="$icon_keys<key>CFBundleIconName</key><string>Refryt</string>"
else
    echo "actool could not compile the .icon (needs Xcode 26); falling back to a plain .icns"
    iconset="$work/refryt.iconset"
    mkdir -p "$iconset"
    master="assets/icons/app/refryt-master.png"
    for size in 16 32 128 256 512; do
        sips -z "$size" "$size" "$master" --out "$iconset/icon_${size}x${size}.png" >/dev/null
        sips -z "$((size * 2))" "$((size * 2))" "$master" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
    done
    iconutil -c icns "$iconset" -o "$app/Contents/Resources/refryt.icns"
fi

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Refryt</string>
    <key>CFBundleDisplayName</key><string>Refryt</string>
    <key>CFBundleIdentifier</key><string>io.github.gridoe.refryt</string>
    <key>CFBundleExecutable</key><string>refryt</string>
    $icon_keys
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
dylibbundler -od -b -x "$app/Contents/MacOS/refryt" \
    -d "$app/Contents/Frameworks" -p "@executable_path/../Frameworks" \
    -s /opt/homebrew/lib -s /usr/local/lib >/dev/null 2>&1

# dylibbundler adds its rpath once per library it rewrites, and dyld refuses a binary with the
# same LC_RPATH twice. Keep exactly one.
rpath="@executable_path/../Frameworks"
for bin in "$app/Contents/MacOS/refryt" "$app"/Contents/Frameworks/*.dylib; do
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

echo "signing (ad hoc)"
# codesign fails now and then on a CI runner with nothing to say, and passes on the next try.
signed=0
for attempt in 1 2 3; do
    if codesign --force --deep --sign - "$app"; then
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
dmg="dist/refryt-$version-macos-$arch.dmg"
stage="$work/dmg"
mkdir -p "$stage"
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"
rm -f "$dmg"
hdiutil create -quiet -volname "Refryt" -srcfolder "$stage" -ov -format UDZO "$dmg"
echo "built $app, $zip and $dmg"
