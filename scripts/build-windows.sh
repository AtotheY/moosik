#!/usr/bin/env bash
# Build dist/refryt-<version>-windows-x86_64.zip: a cross-compiled release .exe plus the one
# non-system DLL it needs (libmpv-2.dll) and yt-dlp.exe, zipped up as a portable, unzip-and-run
# folder — no installer, matching the AppImage's own "no system changes" philosophy.
#
# `ARCH=aarch64` builds the Windows on ARM zip instead, refryt-<version>-windows-arm64.zip, and no
# setup .exe: ARM's installer is the MSI that build-msi.sh makes from this zip. It is native ARM64
# code end to end: mpv-player-windows and yt-dlp both publish
# ARM64 builds, and WebView2's loader ships one in the same crate as the x64 copy.
#
# Needs, on this machine (once):
#   - mingw-w64-gcc (and the crt/headers/winpthreads/binutils that come with it); for ARM64,
#     llvm-mingw instead, since GCC has no aarch64 Windows target
#   - a Rust toolchain with the x86_64-pc-windows-gnu target added (aarch64-pc-windows-gnullvm
#     for ARM64) — the system-package `rust`
#     on Arch/CachyOS does not carry cross targets, so this expects one managed by `rustup`,
#     installed separately from (and not shadowing) that system toolchain: see the project's own
#     notes on why `rustup`'s shims must not land in ~/.cargo/bin ahead of the system `cargo`.
#   - curl and 7z (for the mpv dev package, cached under .windows-tools like the AppImage
#     script's own linuxdeploy/appimagetool caching)
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

version="$(cargo metadata --no-deps --format-version 1 \
    | python3 -c 'import json, sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "refryt"))')"
tools=".windows-tools"
# Everything that differs between the two architectures. The ARM64 target is the `gnullvm` one:
# it links with llvm-mingw's clang and lld, and the mpv ARM64 package is built with the same
# toolchain, so its import library matches.
case "${ARCH:-x86_64}" in
    x86_64)
        target=x86_64-pc-windows-gnu cc=x86_64-w64-mingw32-gcc toolchain=mingw-w64-gcc
        mpv_arch=x86_64 mpv_dev="$tools/mpv-dev" loader_dir=x64 ytdlp=yt-dlp.exe
        arch_name=x86_64 ;;
    aarch64)
        target=aarch64-pc-windows-gnullvm cc=aarch64-w64-mingw32-clang toolchain=llvm-mingw
        mpv_arch=aarch64 mpv_dev="$tools/mpv-dev-aarch64" loader_dir=arm64 ytdlp=yt-dlp_arm64.exe
        arch_name=arm64
        # The C parts of the dependency tree (ring, aws-lc-sys) are built by `cc`, which would
        # otherwise look for a compiler named after the target triple and not find one.
        export CC_aarch64_pc_windows_gnullvm=aarch64-w64-mingw32-clang
        export AR_aarch64_pc_windows_gnullvm=aarch64-w64-mingw32-ar ;;
    *)
        echo "error: ARCH must be x86_64 or aarch64, not '$ARCH'" >&2
        exit 1 ;;
esac
base="moosik-$version-windows-$arch_name"
setup="$base-setup"
out="dist/$base.zip"

# The rustup-managed toolchain, if one is set up separately from the system `cargo` — see the
# header comment above for why this cannot just be plain `cargo` on this machine. `RUSTC` is
# set explicitly alongside it: cargo looks up a bare `rustc` on `PATH` regardless of which
# `cargo` binary ran it, so without this it silently reaches the system `rustc` (no Windows
# target installed) even while running the rustup toolchain's own `cargo`.
rustup_bin=""
cargo_bin="cargo"
if [ -x "$HOME/.cargo/bin/rustup" ]; then
    rustup_bin="$HOME/.cargo/bin/rustup"
    cargo_bin="$("$rustup_bin" which cargo)"
    export RUSTC="$("$rustup_bin" which rustc)"
fi

# Arch's llvm-mingw package installs to /opt/llvm-mingw and puts it on PATH only from the next login.
[ -d /opt/llvm-mingw/bin ] && PATH="$PATH:/opt/llvm-mingw/bin"
if ! command -v "$cc" > /dev/null; then
    echo "error: $cc not found — install $toolchain first" >&2
    exit 1
fi
if [ -n "$rustup_bin" ] && ! "$rustup_bin" target list --installed | grep -q "^$target$"; then
    echo "error: the $target Rust target is not installed" >&2
    echo "       run: rustup target add $target" >&2
    exit 1
fi

# The mpv Windows dev package: `libmpv-2.dll` to bundle, `libmpv.dll.a` (its MinGW import
# library) to link against. mpv-player-windows publishes these under dated filenames with no
# stable "latest" alias (unlike yt-dlp's GitHub releases below), so the newest is found from its
# own release feed instead of a hardcoded version.
if [ ! -f "$mpv_dev/libmpv-2.dll" ]; then
    mkdir -p "$mpv_dev"
    echo "finding the latest mpv Windows dev package ($mpv_arch)"
    file="$(curl -fsSL "https://sourceforge.net/projects/mpv-player-windows/rss?path=/libmpv" \
        | grep -o "mpv-dev-$mpv_arch-[0-9a-z-]*\.7z" | grep -v -- '-v3-' | head -1)"
    if [ -z "$file" ]; then
        echo "error: could not find a mpv-dev-$mpv_arch build in the release feed" >&2
        exit 1
    fi
    echo "fetching $file"
    archive="$tools/$file"
    curl -fsSL -o "$archive" \
        "https://sourceforge.net/projects/mpv-player-windows/files/libmpv/$file/download"
    7z x -y -o"$mpv_dev" "$archive" libmpv-2.dll libmpv.dll.a > /dev/null
    rm -f "$archive"
fi

echo "building release binary for $target"
"$cargo_bin" build --locked --release -p refryt --bin refryt --target "$target"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
pkg="$work/Moosik"
mkdir -p "$pkg"

cp "target/$target/release/refryt.exe" "$pkg/"
cp "$mpv_dev/libmpv-2.dll" "$pkg/"

# Colour emoji: Windows' own font, Segoe UI Emoji, is COLR — the emoji painter reads PNG strikes
# and nothing else (`emoji.rs`) — so Noto Color Emoji travels next to the exe. Committed under
# assets/fonts rather than fetched at build time: it is a compatibility asset, not a tool that
# has to chase upstream like yt-dlp does. It is the v2.042 build, deliberately: v2.043 and later
# are COLRv1-only, which is the same thing Segoe UI Emoji could not be read for.
cp assets/fonts/NotoColorEmoji.ttf "$pkg/"
cp LICENSE "$pkg/LICENSE.txt"
printf 'Moosik — https://github.com/AtotheY/moosik\nBased on Refryt — https://github.com/gridoe/refryt\nSource: https://github.com/AtotheY/moosik/tree/%s\nBundled playback: mpv (https://mpv.io), yt-dlp (https://github.com/yt-dlp/yt-dlp), Microsoft WebView2, Noto Color Emoji (SIL OFL).\n' "$(git rev-parse HEAD)" > "$pkg/NOTICE.txt"

# WebView2's loader: the GNU targets link it dynamically (only MSVC targets
# link the static library), so the sign-in window's one file has to travel next to the exe. It
# lives in the webview2-com-sys crate's own source tree, fetched with everything else.
loader="$(find "${CARGO_HOME:-$HOME/.cargo}/registry/src" -path "*webview2-com-sys-*/$loader_dir/WebView2Loader.dll" 2>/dev/null | head -1)"
if [ -z "$loader" ]; then
    echo "error: WebView2Loader.dll not found under ${CARGO_HOME:-$HOME/.cargo}/registry — run 'cargo fetch' first" >&2
    exit 1
fi
cp "$loader" "$pkg/"

# yt-dlp is the resolver that exists specifically to survive YouTube changing something — it is
# not optional, playback does not work at all without it — so it ships inside the zip rather than
# assuming the target machine already has it. Always freshly fetched, unlike the mpv package
# above: an old pinned copy would defeat the whole point of using it.
echo "fetching yt-dlp"
curl -fsSL -o "$pkg/yt-dlp.exe" \
    "https://github.com/yt-dlp/yt-dlp/releases/latest/download/$ytdlp"

mkdir -p dist
echo "packing $out"
rm -f "$out"
(cd "$work" && zip -qr "$root/$out" "Moosik")
echo "built $out ($(du -h "$out" | cut -f1))"

# The installer: Inno Setup has no Linux build, so this runs the real Windows compiler under
# Wine — see docs/BUILDING.md for the one-time setup. Skipped, not fatal, when that isn't set
# up: the portable zip above already built either way.
iscc="$HOME/.wine/drive_c/Program Files (x86)/Inno Setup 6/ISCC.exe"
if [ "$arch_name" != x86_64 ]; then
    echo "note: no setup .exe for $arch_name; its installer is the MSI (ARCH=$ARCH ./scripts/build-msi.sh)"
elif command -v wine > /dev/null && [ -f "$iscc" ]; then
    echo "building the installer"
    stage_win="$(WINEDEBUG=-all winepath -w "$pkg")"
    WINEDEBUG=-all wine "$iscc" /Q "/DAppVersion=$version" "/DStageDir=$stage_win" \
        "/DOutputName=$setup" scripts/windows-installer.iss
    installer="dist/$setup.exe"
    echo "built $installer ($(du -h "$installer" | cut -f1))"
else
    echo "note: Wine + Inno Setup not set up (see docs/BUILDING.md) — skipping the installer"
fi
