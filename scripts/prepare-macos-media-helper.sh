#!/bin/bash
# Bundle the opt-in media fallback before signing the containing app.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
app="$1"
mkdir -p "$app/Contents/Library/LaunchAgents" "$root/.ark-build/swift-module-cache"
xcrun swiftc -O -target "$(uname -m)-apple-macos13.0" \
    -module-cache-path "$root/.ark-build/swift-module-cache" \
    "$root/packaging/macos/MediaFallback.swift" \
    -o "$app/Contents/MacOS/moosik-media-helper"
cp "$root/packaging/macos/com.moosik.media-fallback.plist" "$app/Contents/Library/LaunchAgents/"
monitor="$app/Contents/Helpers/MoosikMediaHelper.app/Contents"
mkdir -p "$monitor/MacOS"
cp "$root/packaging/macos/MediaHelper-Info.plist" "$monitor/Info.plist"
cp "$app/Contents/MacOS/moosik-media-helper" "$monitor/MacOS/moosik-media-monitor"
