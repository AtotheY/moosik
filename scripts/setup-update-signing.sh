#!/bin/bash
# One-time release key setup. Only the public key is written to the repository.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
[ -d "$root/.ark-build/sparkle-2.10.1/bin" ] || {
    echo 'Package Moosik once to fetch the pinned Sparkle tools.' >&2; exit 1;
}
tool="$root/.ark-build/sparkle-2.10.1/bin/generate_keys"
"$tool" --account com.sistilli.moosik.updates
mkdir -p "$root/packaging/macos"
"$tool" --account com.sistilli.moosik.updates -p > "$root/packaging/macos/update-public-key.txt"
echo 'Saved the public update key. Keep the private key in Keychain; see docs/client-updates.md.'
