#!/usr/bin/env bash
# Package an already signed (and optionally stapled) app without opening Finder.
set -euo pipefail
app="${1:?expected an app bundle path}"
dmg="${2:?expected an output DMG path}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
name="$(basename "$app" .app)"
exec uvx dmgbuild@1.6.7 -s "$root/packaging/macos/dmg-settings.py" -D app="$app" "$name" "$dmg"
