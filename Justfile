# moosik — a fast, native YouTube Music client
set shell := ["bash", "-cu"]

# Build + run the app (dev profile, fast rebuilds).
default args="":
    cargo run -p moosik --bin moosik -- {{args}}

# Straight into a search.
search q:
    cargo run -p moosik --bin moosik -- --search "{{q}}"

# Open on a named page: home, search, library, settings.
page name:
    cargo run -p moosik --bin moosik -- --page {{name}}

# Open at a desktop coordinate without taking focus — for testing on a second screen.
# Wayland compositors may ignore the position; the focus part still holds.
at x y args="":
    cargo run -p moosik --bin moosik -- --position {{x}},{{y}} {{args}}

# Search and start playing the first hit.
autoplay q:
    cargo run -p moosik --bin moosik -- --search "{{q}}" --autoplay

# Run with a normal OS titlebar (fallback for odd WMs).
decorated args="":
    cargo run -p moosik --bin moosik -- --decorated {{args}}

# Optimised run, for judging real performance.
release args="":
    cargo run --release -p moosik --bin moosik -- {{args}}

# Run with the live counter overlay (F2 toggles it at runtime).
stats args="":
    cargo run -p moosik --bin moosik -- --stats {{args}}

# Check a widget or drive the full app on the private desktop.
[positional-arguments]
ui-check *args="queue":
    ./scripts/ui-check "$@"

# Unit tests.
test:
    cargo test --workspace --lib --bins

# fmt + clippy + tests — what CI runs. Covers both resolver feature sets, so the optional
# native path cannot rot unnoticed while it is switched off.
check:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo clippy -p moosik-resolve --features native-resolver --all-targets -- -D warnings
    cargo test --workspace --lib --bins
    cargo test -p moosik-resolve --features native-resolver

# The M1 gate: resolve, verify Opus, verify throughput.
resolve *ids:
    cargo run --release -q -p moosik-resolve --bin resolve-spike -- {{ids}}

# The same gate with the in-process resolver compiled in — the way to check whether upstream has
# fixed the `n` parameter and the fast path can come back. Slow to build: it pulls in Boa.
resolve-native *ids:
    cargo run --release -q -p moosik-resolve --features native-resolver --bin resolve-spike -- {{ids}}

# Check unauthenticated search still works.
api q="radiohead nude":
    cargo run --release -q -p moosik --bin api-check -- "{{q}}"

# Run the api canary daily via a systemd user timer, so YouTube breakage pages us first.
canary-install:
    ./scripts/canary-install.sh install

# Remove the daily api canary again.
canary-uninstall:
    ./scripts/canary-install.sh uninstall

# Build dist/Moosik.app and a zip of it: release binary, icns icon, libmpv and its dylibs, yt-dlp.
macos:
    ./scripts/build-macos.sh

# Build dist/moosik-<version>-x86_64.AppImage: release binary, the logo as the icon, and every
# library short of glibc (libmpv and its tail) bundled inside.
appimage:
    ./scripts/build-appimage.sh

# Install the built AppImage as a desktop application: ~/.local/bin, launcher entry, icons.
appimage-install:
    ./scripts/appimage-install.sh install

# Build the Flatpak bundle into dist/.
flatpak:
    ./scripts/build-flatpak.sh

# Release: bump the version and run the gate, check what was built, publish. See docs/BUILDING.md.
release-prep version:
    ./scripts/release.sh prep {{version}}

release-verify version:
    ./scripts/release.sh verify {{version}}

# By hand: tags, pushes and creates the GitHub release; a dry run unless --yes is added by hand.
# The notes default to release-notes/<version>.md. Pushing the tag lets the release workflow do it.
release-publish version notes="":
    ./scripts/release.sh publish {{version}} {{ if notes == "" { "" } else { "--notes " + notes } }}

# Resolve + play through mpv, no GUI.
play id:
    ./scripts/play.sh {{id}}

# Measure cold start and idle memory against the gates.
perf:
    ./scripts/perf.sh

# Stripped release binary size, with a section breakdown. Tracked so a dependency or
# feature added later cannot quietly undo M21's cut (23.0 MB -> 19.6 MB).
size:
    cargo build --release -p moosik --bin moosik
    @ls -la target/release/moosik | awk '{printf "  binary : %.2f MiB\n", $5/1048576}'
    @size target/release/moosik | awk 'NR==2 {printf "  text   : %.2f MiB\n  data   : %.2f MiB\n", $1/1048576, $2/1048576}'
    @printf "  crates : "; cargo tree -e normal -p moosik --prefix none 2>/dev/null | sed 's/ (\*)//' | awk '{print $1" "$2}' | sort -u | wc -l

# Install as a desktop application: binary, hicolor icons, desktop entry.
install:
    #!/usr/bin/env bash
    set -eu
    cargo install --path crates/moosik --bin moosik
    for s in 16 32 48 64 128 256 512; do
      install -Dm644 "assets/icons/app/moosik-$s.png" \
        "$HOME/.local/share/icons/hicolor/${s}x${s}/apps/moosik.png"
    done
    install -Dm644 assets/moosik.desktop "$HOME/.local/share/applications/moosik.desktop"
    update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true

# Remove the installed desktop application.
uninstall:
    #!/usr/bin/env bash
    set -eu
    cargo uninstall moosik
    rm -f "$HOME/.local/share/applications/moosik.desktop"
    for s in 16 32 48 64 128 256 512; do
      rm -f "$HOME/.local/share/icons/hicolor/${s}x${s}/apps/moosik.png"
    done
    update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true
