#!/usr/bin/env bash
set -euo pipefail
# Only the Ubuntu archives are needed for the cross compiler. Bound network
# work separately so a stalled mirror can be retried without interrupting dpkg.
sources="$RUNNER_TEMP/moosik.sources"
apt=(sudo apt-get -o "Dir::Etc::sourcelist=$sources" -o Dir::Etc::sourceparts=-
     -o Acquire::Retries=1 -o Acquire::http::Timeout=15 -o APT::Update::Error-Mode=any)
for mirror in azure.archive.ubuntu.com archive.ubuntu.com; do
  printf 'Types: deb\nURIs: https://%s/ubuntu\nSuites: noble noble-updates noble-security\nComponents: main universe\nSigned-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg\n' "$mirror" > "$sources"
  if timeout 120 "${apt[@]}" update && timeout 120 "${apt[@]}" install -y --download-only --no-install-recommends "$@"; then
    break
  fi
  echo "::warning::Could not fetch Windows build dependencies from $mirror"
done
"${apt[@]}" install -y --no-download --no-install-recommends "$@"
