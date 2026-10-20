# Building refryt

## Prerequisites

- Rust 1.90+ (see `rust-version` in `Cargo.toml`)
- [`mpv`](https://mpv.io/) development libraries (`libmpv`) — refryt plays audio through it
- [`yt-dlp`](https://github.com/yt-dlp/yt-dlp) — the fallback for songs YouTube's `player`
  endpoint won't hand over, plus the video view and downloads. Not needed to build, and songs
  play without it.
- [`just`](https://github.com/casey/just) (optional) — the `Justfile` has a command for
  everything below; without it, the plain `cargo`/`./scripts/...` equivalent is noted alongside.

## Everyday development

```sh
just              # cargo run, dev profile
just search "some song"
just release      # optimised run, for judging real performance
just check        # fmt --check + clippy (both feature sets) + tests — what CI runs
```

Without `just`, the equivalents are `cargo run -p refryt --bin refryt`, and the four commands
listed in `just check`'s own recipe in the `Justfile`.

## Linux: the AppImage

```sh
just appimage          # or: ./scripts/build-appimage.sh
just appimage-install  # installs it to ~/.local/bin with a desktop launcher entry
```

This bundles the release binary, every shared library short of glibc (`libmpv` included), a
freshly-fetched `yt-dlp`, and the app icon into a single `dist/refryt-<version>-x86_64.AppImage`.
Needs `python3` with Pillow and `curl`; no root required anywhere.

## Linux: the Flatpak

```sh
just flatpak           # or: ./scripts/build-flatpak.sh
flatpak install --user dist/refryt-<version>-x86_64.flatpak
```

This builds `dist/refryt-<version>-x86_64.flatpak` from `packaging/flatpak/`. It needs
`flatpak`, `flatpak-builder`, the GNOME 49 Platform and Sdk, and the `rust-stable` Sdk extension
(`org.freedesktop.Sdk.Extension.rust-stable//25.08`), all installed for your user. The first
build takes a while: it compiles libass, libplacebo, a libmpv-only mpv and the tray's libraries,
because the GNOME runtime has none of them. The refryt module also needs the network, for the
crates.

The Flatpak differs from the AppImage in three ways. The sandbox cannot read a browser's profile
folder, so Settings leaves out "Import from a browser" and the sign-in window is the only way in.
Its autostart entry would land in the sandbox's own config folder, where nothing reads it, so
"Launch on startup" is left out too. And `yt-dlp` is bundled at build time, the same as the
AppImage's, so it is as old as the build.

## macOS: the app bundle

```sh
brew install rustup just mpv dylibbundler
just macos             # or: ./scripts/build-macos.sh
```

`cargo run` works too once Homebrew's libmpv is on the linker path, which `.cargo/config.toml`
does for Apple Silicon (`/opt/homebrew/lib`). The script builds the release binary, makes an
icon, copies libmpv and its whole dylib tail into `Contents/Frameworks` (`dylibbundler`), puts a
standalone `yt-dlp` next to the executable, signs the bundle ad hoc, and writes a zip and a
DMG. Output is `dist/Refryt.app`, `dist/refryt-<version>-macos-arm64.zip` and
`dist/refryt-<version>-macos-arm64.dmg`. Nothing outside macOS itself is needed to run it.

Things that are specific to macOS and easy to trip over:

- **The icon.** `assets/icons/app/Refryt.icon` is an Icon Composer document, compiled by `actool`
  (Xcode 26) into `Assets.car`. macOS only restyles an icon for Dark, Clear and Tinted
  (monochrome) mode when it is built this way; a plain `.icns` gets a generic treatment. `actool`
  also writes an `.icns` for older systems, and the script falls back to a `sips` and `iconutil`
  one if `actool` is not available.
- **The window.** On macOS the window is a native one with its title bar hidden and the content
  running under it (`main.rs`), not a borderless rectangle. That lets the system round the
  corners, clip the shadow and the blur to them, and keep them correct when the window is zoomed.
  The app paints a square background there. In true fullscreen macOS squares the corners itself.
- **The renderer.** macOS draws through wgpu (Metal), as Windows does. The OpenGL path ran on
  Apple's OpenGL-on-Metal layer and held about 126 MB of window buffers, against about 52 MB
  now, and OpenGL is deprecated on macOS.
- **Blur.** `blur.rs` calls `CGSSetWindowBackgroundBlurRadius`, a private but long-stable
  CoreGraphics function that Terminal, iTerm2 and WezTerm also use. It blurs whatever is behind
  any non-opaque window. A native `NSVisualEffectView` behind the app's own view was tried first
  and never drew.
- **Sign-in.** The in-app window is a WKWebView with a Safari user agent and a non-persistent
  store, so Google's session isn't kept between sign-ins. See the `webview_login` module docs.
- **The tray.** A hidden window gets no frames on macOS, so `tray.rs` raises the window through
  AppKit instead of waiting for the UI loop.
- **Gatekeeper.** The app is ad-hoc signed and not notarized. On another Mac, clear the
  quarantine flag with `xattr -cr Refryt.app`, or sign and notarize with a Developer ID.

## Windows: cross-compiled from Linux

`./scripts/build-windows.sh` produces both a portable, no-install zip and a real installer, the
same "unzip and run, or install like anything else" choice most desktop apps offer. One-time
setup on the build machine:

```sh
sudo pacman -S mingw-w64-gcc mingw-w64-crt mingw-w64-headers mingw-w64-winpthreads mingw-w64-binutils
rustup target add x86_64-pc-windows-gnu
```

(If your system `cargo` doesn't come from `rustup` — e.g. Arch's `rust` package, which has no
cross-compile targets — install `rustup` separately rather than replacing it; the two can
coexist as long as `rustup`'s shims don't end up ahead of the system toolchain on `PATH`. The
build script finds `rustup`'s `cargo`/`rustc` explicitly either way, so it does not depend on
which one your shell resolves to by default.)

`ARCH=aarch64 ./scripts/build-windows.sh` builds the Windows on ARM zip instead,
`refryt-<version>-windows-arm64.zip`, and `ARCH=aarch64 ./scripts/build-msi.sh` turns it into
`refryt-<version>-windows-arm64.msi`, ARM's only installer. GCC has no aarch64 Windows target, so
the build links with llvm-mingw's clang and uses the `aarch64-pc-windows-gnullvm` Rust target:

```sh
sudo pacman -S llvm-mingw
rustup target add aarch64-pc-windows-gnullvm
```

The mpv ARM64 dev package, `yt-dlp_arm64.exe` and the ARM64 `WebView2Loader.dll` are picked up the
same way as their x64 versions. `wixl` has no ARM64 target, so the MSI is built as x64 and its
platform is then set to Arm64 with `msibuild`; an x64 machine refuses it, as it should.

The installer needs [Inno Setup](https://jrsoftware.org/isinfo.php) too — there's no Linux
build of it, so it runs for real under Wine:

```sh
sudo pacman -S wine
wine <path-to-innosetup-6.x.exe> /VERYSILENT /SUPPRESSMSGBOXES /NORESTART
```

(installs to `~/.wine/drive_c/Program Files (x86)/Inno Setup 6/ISCC.exe`, which is where the
build script looks for it). Skipping this step is fine — the script still builds the portable
zip on its own and just notes that it's skipping the installer.

```sh
./scripts/build-windows.sh
```

The script fetches the current [mpv Windows dev
package](https://sourceforge.net/projects/mpv-player-windows/files/libmpv/) (cached under
`.windows-tools/`, since — unlike yt-dlp's GitHub releases — there's no stable "latest" URL for
it) to link against, cross-builds with `mingw-w64-gcc`, fetches a fresh `yt-dlp.exe`, zips
`refryt.exe` + `libmpv-2.dll` + `WebView2Loader.dll` + `NotoColorEmoji.ttf` + `yt-dlp.exe` into
`dist/refryt-<version>-windows-x86_64.zip`, and — when Wine + Inno Setup are set up — compiles
`scripts/windows-installer.iss` (via `ISCC.exe` under Wine) into
`dist/refryt-<version>-windows-x86_64-setup.exe`: Program Files, a Start Menu entry, an optional
desktop shortcut, and a real uninstaller.

Every other Windows dependency (`tray-icon`, `rfd`, and the WebView2 *runtime* the sign-in window
runs on) resolves to DLLs that ship with Windows itself — `libmpv-2.dll` and `WebView2Loader.dll`
are the only two that have to be bundled by hand. The loader is only a loader: the GNU target
links it dynamically, so it must sit next to `refryt.exe`, and a native Windows dev build needs
the same copy beside `target/debug/refryt.exe` (it is in the `webview2-com-sys` crate's source
tree under `~/.cargo/registry/src/...`; `scripts/build-windows.sh` finds it there for the packaged
zip). `NotoColorEmoji.ttf` is not a library at all: it is the one colour emoji font with the PNG
bitmap strikes the emoji painter reads (`emoji.rs`) — Windows' own Segoe UI Emoji is COLR — so it
is copied from `assets/fonts` into the package (a native dev run finds the same file through the
repository path compiled into `emoji.rs`).

The `.exe` carries its own icon (`assets/icons/app/refryt.ico`, generated from the same master
PNG the Linux icon set comes from) via `build.rs` + `embed-resource` — Windows-only, and gated
on the actual cross-compile *target*, not the host running the build, which is what lets it
stay a no-op on every other platform without a `cfg` that would just check the wrong thing.

The installer is unsigned — nobody's bought a code-signing certificate for this project — so
Windows' SmartScreen will warn on first run. That's expected, not a bug in the build.

## Releasing

The version lives in one place, `[workspace.package]` in the root `Cargo.toml`. Every build
script reads it from there, so the AppImage, the Windows zips, installers and MSI, the macOS bundle and
the Flatpak all name themselves after it. The one other place is the Flatpak's metainfo, which
lists each release by hand. `scripts/release.sh` does the bumping and checks the rest:

Pushing a `v*` tag builds and publishes the release on GitHub
(`.github/workflows/release.yml`), so a release is:

```sh
just release-prep 0.1.13     # version, Cargo.lock, metainfo, the gate, release-notes/0.1.13.md,
                             # and a "Release 0.1.13" commit
$EDITOR release-notes/0.1.13.md                    # write the changes
git commit -a --amend --no-edit
git push origin main
git tag -a v0.1.13 -m "refryt 0.1.13" && git push origin v0.1.13
```

`prep` refuses a dirty tree, a version that isn't newer, a tag that already exists, and a tree
with the private local patch applied (that patch is a build for one machine and never part of a
release). The notes start from `release-notes/TEMPLATE.md`: the install section is filled in for
the version, and a marker stays in the changes section until it is written, since `verify` fails
while it is there.

The workflow checks the tag first (it is on `main` and matches `Cargo.toml`, and the notes are
written), then builds on three runners: the AppImage, the Windows files and the Flatpak on
Ubuntu, and the macOS image on macOS. It then runs `scripts/release.sh verify --strict` over all
of them, and only then creates the release, as a draft with every file and
`refryt-<version>-SHA256SUMS.txt`, and publishes it. A run started by hand from the Actions tab
does the same without the publishing, which is how a change to the scripts or the workflow is
tried. The build scripts are the ones used locally; the workflow only installs their tools.

### By hand

The same steps run on a developer machine, which is the way to release when Actions is
unavailable, or for a build that is only for one machine:

```sh
just release-prep 0.1.13
just appimage                # build what ships, on the machine that can:
./scripts/build-windows.sh   #   Linux and Windows build here,
ARCH=aarch64 ./scripts/build-windows.sh && ARCH=aarch64 ./scripts/build-msi.sh   # Windows on ARM,
./scripts/build-msi.sh       #   the Windows MSI from that build (needs msitools' wixl),
just flatpak                 #   the Flatpak here too,
./scripts/build-macos.sh     #   and the macOS image on a Mac, copied into dist/
just release-verify 0.1.13   # checks the repo and dist/ agree
just release-publish 0.1.13  # shows what it would do
./scripts/release.sh publish 0.1.13 --yes      # does it
```

`verify` checks that each file in `dist/` exists, was built after `HEAD`'s commit (an older one is
stale), and has the right things in it. A missing macOS image is a warning here, since by hand it
can only come from a Mac; `--strict` makes it a failure, as in the workflow. `publish` runs
`verify` again, then tags `v<version>`, pushes `main` and the tag, and creates the release with
every artifact attached, then the sums file. A macOS image uploaded later from the Mac needs the
sums redone: `./scripts/release.sh checksums <version> --yes` reads the published files, from any
machine, and replaces the old sums file. Pushing the tag also starts the workflow, so by hand
means publishing before the tag is pushed from anywhere else, or cancelling that run.

The gate in `prep` (fmt, clippy `-D warnings`, the tests, and with `--perf` the memory and
frame-time gates) is still the only check between a change and a release: no workflow runs on
ordinary pushes.

## Packaged size

`just size` builds the release binary and prints its stripped size plus a section breakdown —
tracked so a dependency added later doesn't quietly undo the effort that went into keeping it
small.
