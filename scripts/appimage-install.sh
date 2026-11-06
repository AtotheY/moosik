#!/usr/bin/env bash
# Install (or remove) the AppImage as a desktop application: the image into
# ~/.local/bin, the logo into the user icon theme, and a desktop entry pointing at it,
# so it shows up in the application launcher like anything else installed.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

bin_dir="$HOME/.local/bin"
icon_dir="$HOME/.local/share/icons/hicolor"
desktop_dir="$HOME/.local/share/applications"

usage() {
    echo "usage: appimage-install.sh [install|uninstall] [dist/moosik-<version>-x86_64.AppImage]" >&2
    exit 1
}

[ "${1:-install}" = "install" ] && action="install" || action="$1"
shift || true

if [ "$action" = install ]; then
    image="${1:-}"
    [ -n "$image" ] || image="$(ls -t "$(dirname "$0")/../dist"/moosik-*-x86_64.AppImage 2>/dev/null | head -1)"
    [ -n "$image" ] && [ -f "$image" ] || {
        echo "no AppImage found — run just appimage first" >&2
        exit 1
    }
    image="$(readlink -f "$image")"

    mkdir -p "$bin_dir" "$icon_dir" "$desktop_dir"
    install -m 755 "$image" "$bin_dir/moosik.AppImage"

    # The preprocessed master, not the raw `logov1.png`: the raw source has an opaque black
    # margin outside the rounded square, which the launcher then shows at every corner.
    python3 - "$root/assets/icons/app/moosik-master.png" "$icon_dir" << 'EOF'
import os, sys
from PIL import Image
logo, icon_dir = sys.argv[1], sys.argv[2]
src = Image.open(logo).convert("RGBA")
for s in (16, 32, 48, 64, 128, 256, 512):
    d = os.path.join(icon_dir, f"{s}x{s}/apps")
    os.makedirs(d, exist_ok=True)
    src.resize((s, s), Image.LANCZOS).save(os.path.join(d, "moosik.png"))
EOF

    cat > "$desktop_dir/moosik.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Moosik
GenericName=Music Player
Comment=A fast, native YouTube Music client
Exec="$bin_dir/moosik.AppImage" %u
MimeType=x-scheme-handler/moosik;
Icon=moosik
Terminal=false
Categories=AudioVideo;Audio;Player;
Keywords=music;youtube;player;
StartupNotify=true
EOF

    # /tmp extraction is per-run; the app stores settings and cookies in XDG dirs regardless.
    chmod +x "$bin_dir/moosik.AppImage"
    update-desktop-database "$desktop_dir" 2>/dev/null || true
    # Without this, KDE's icon lookup fails on first launch (before the theme
    # cache knows about moosik) and plasmashell caches the placeholder until restarted.
    command -v kbuildsycoca6 >/dev/null && kbuildsycoca6 >/dev/null 2>&1 || true
    echo "installed: $bin_dir/moosik.AppImage (launcher entry: $desktop_dir/moosik.desktop)"
else
    rm -f "$bin_dir/moosik.AppImage" "$desktop_dir/moosik.desktop"
    find "$icon_dir" -path "*/apps/moosik.png" -delete 2>/dev/null || true
    update-desktop-database "$desktop_dir" 2>/dev/null || true
    command -v kbuildsycoca6 >/dev/null && kbuildsycoca6 >/dev/null 2>&1 || true
    echo "removed"
fi
