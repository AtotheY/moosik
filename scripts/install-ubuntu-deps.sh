#!/usr/bin/env bash
set -euo pipefail
# Use Ubuntu archives for the build runners and clean smoke container. Bound network
# work separately so a stalled mirror can be retried without interrupting dpkg.
sources="$RUNNER_TEMP/moosik.sources"
prefix=()
if (( EUID != 0 )); then prefix=(sudo); fi
# The minimal Ubuntu container starts without a CA bundle. APT verifies
# repository metadata and packages with its preinstalled Ubuntu signing key.
scheme=https
if [[ ! -s /etc/ssl/certs/ca-certificates.crt ]]; then scheme=http; fi
apt=("${prefix[@]}" apt-get -o "Dir::Etc::sourcelist=$sources" -o Dir::Etc::sourceparts=-
     -o Acquire::Retries=1 -o Acquire::http::Timeout=15 -o APT::Update::Error-Mode=any)
for mirror in azure.archive.ubuntu.com archive.ubuntu.com; do
  printf 'Types: deb\nURIs: %s://%s/ubuntu\nSuites: noble noble-updates noble-security\nComponents: main universe\nSigned-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg\n' "$scheme" "$mirror" > "$sources"
  if timeout 120 "${apt[@]}" update && timeout 300 "${apt[@]}" install -y --download-only --no-install-recommends "$@"; then
    break
  fi
  echo "::warning::Could not fetch Ubuntu dependencies from $mirror"
done
"${apt[@]}" install -y --no-download --no-install-recommends "$@"
