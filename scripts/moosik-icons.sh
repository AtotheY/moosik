#!/bin/bash
# Package the approved artwork into native icon sizes; no design changes.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
master="$root/assets/branding/moosik/eclipse/ceramic-final.png"
iconset="$root/.ark-build/moosik.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
    sips -z "$size" "$size" "$master" --out "$iconset/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    sips -z "$double" "$double" "$master" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
cp "$iconset/icon_256x256.png" "$root/assets/icons/app/moosik-256.png"
iconutil -c icns "$iconset" -o "$root/assets/icons/app/moosik.icns"
