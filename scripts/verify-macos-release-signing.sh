#!/bin/bash
# Verify the pinned certificate. This does not establish Keychain partition continuity.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
app="${1:?usage: verify-macos-release-signing.sh app-path}"
fingerprint="$(tr -d '[:space:]' < "$root/packaging/macos/release-signing-identity.txt")"
[[ "$fingerprint" =~ ^[[:xdigit:]]{40}$ ]] || { echo 'Invalid release certificate fingerprint' >&2; exit 1; }
requirement="identifier \"io.github.gridoe.refryt\" and certificate leaf = H\"$fingerprint\""
codesign --verify --deep --strict -R "=$requirement" "$app"
designated="$(codesign -dr- "$app" 2>&1)"
if [[ "$designated" == *cdhash* ]]; then
    echo 'Release identity is tied to a build hash instead of the persistent certificate.' >&2
    exit 1
fi
echo 'Verified persistent Moosik release identity.'
