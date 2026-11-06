#!/usr/bin/env bash
# Native Ubuntu 24.04 amd64 package with a private yt-dlp beside the player.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
[[ "$(dpkg --print-architecture)" = amd64 && "$(uname -m)" = x86_64 ]] || {
    echo 'Build this package on x86-64 Ubuntu.' >&2
    exit 1
}
version="$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml","rb"))["workspace"]["package"]["version"])')"
cargo build --locked --release -p moosik --bin moosik

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
pkg="$work/pkg"
install -Dm755 "${CARGO_TARGET_DIR:-target}/release/moosik" "$pkg/usr/lib/moosik/moosik"
mkdir -p "$pkg/usr/bin" "$pkg/DEBIAN" "$work/debian" dist
ln -s ../lib/moosik/moosik "$pkg/usr/bin/moosik"
install -Dm644 packaging/linux/moosik.desktop "$pkg/usr/share/applications/moosik.desktop"
install -Dm644 assets/icons/app/moosik-256.png "$pkg/usr/share/icons/hicolor/256x256/apps/moosik.png"
install -Dm644 LICENSE "$pkg/usr/share/doc/moosik/copyright"

# Select yt-dlp and its published checksum from the same release.
release="$(curl -fsSL --retry 3 -o /dev/null -w '%{url_effective}' https://github.com/yt-dlp/yt-dlp/releases/latest)"
tag="${release##*/}"
base="https://github.com/yt-dlp/yt-dlp/releases/download/$tag"
(
    cd "$work"
    curl -fsSLO --retry 3 "$base/yt-dlp_linux"
    curl -fsSLO --retry 3 "$base/SHA2-256SUMS"
    awk '$2 == "yt-dlp_linux" {print}' SHA2-256SUMS | sha256sum --check --strict -
    install -m755 yt-dlp_linux "$pkg/usr/lib/moosik/yt-dlp"
)

# Derive package names and version floors from the linked ELF on the build distro.
touch "$work/debian/control"
depends="$(cd "$work" && dpkg-shlibdeps -O -e"$pkg/usr/lib/moosik/moosik")"
[[ "$depends" == shlibs:Depends=?* ]]
cat > "$pkg/DEBIAN/control" <<CONTROL
Package: moosik
Version: $version
Architecture: amd64
Section: sound
Priority: optional
Maintainer: Moosik contributors <noreply@github.com>
Depends: ${depends#shlibs:Depends=}, libegl1, libgl1, libxkbcommon-x11-0, ca-certificates
Description: Desktop player for YouTube Music and local music
CONTROL
dpkg-deb --root-owner-group -Zzstd --build "$pkg" "dist/moosik-$version-linux-amd64.deb"
