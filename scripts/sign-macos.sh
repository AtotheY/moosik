#!/bin/bash
# Keep local builds recognizable to Keychain across changes to the executable.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
allow_adhoc=0
if [ "${1:-}" = --allow-adhoc ]; then allow_adhoc=1; shift; fi
[ "$#" = 1 ] || { echo "usage: $0 [--allow-adhoc] app-path" >&2; exit 1; }
app="$1"
identity="${MOOSIK_SIGNING_IDENTITY:-}"
if [ -z "$identity" ] && [ -f "$root/.ark-build/signing-identity" ]; then
    identity="$(cat "$root/.ark-build/signing-identity")"
fi
if [ -z "$identity" ]; then
    if [ "$allow_adhoc" = 1 ]; then
        echo "No local signing identity configured; using ad hoc signing. Keychain approvals will not survive rebuilds." >&2
        identity=-
    else
        echo "Run scripts/setup-macos-signing.sh once before packaging Moosik." >&2
        exit 1
    fi
fi

echo "Signing $app with $identity"
if [ "${MOOSIK_NOTARIZE:-0}" = 1 ]; then
    [ "$identity" != - ] || { echo 'Notarized releases require Developer ID signing.' >&2; exit 1; }
    # Sign nested code inside out, preserving Sparkle helpers' own entitlements.
    while IFS= read -r file; do
        if file -b "$file" | grep -q 'Mach-O'; then
            codesign --force --options runtime --timestamp --preserve-metadata=entitlements --sign "$identity" "$file"
        fi
    done < <(find "$app/Contents" -type f)
    while IFS= read -r bundle; do
        codesign --force --options runtime --timestamp --preserve-metadata=entitlements --sign "$identity" "$bundle"
    done < <(find "$app/Contents" -depth -type d \( -name '*.xpc' -o -name '*.app' -o -name '*.framework' \))
    codesign --force --options runtime --timestamp --sign "$identity" "$app"
else
    codesign --force --deep --sign "$identity" "$app"
fi
codesign --verify --deep --strict "$app"
