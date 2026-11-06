**Linux** — `moosik-@VERSION@-x86_64.AppImage`: download, `chmod +x`, run. Or use
`scripts/appimage-install.sh install` from source for a desktop launcher entry.

**Linux, Flatpak** — `moosik-@VERSION@-x86_64.flatpak`. It isn't on Flathub, so install the file:
`flatpak install --user moosik-@VERSION@-x86_64.flatpak`, then `flatpak run io.github.AtotheY.moosik`.
It needs the GNOME 51 runtime, which Flatpak offers to fetch the first time. mpv and yt-dlp are
inside. The sandbox can't see your browsers' profile folders, so "Import from a browser" is hidden
and you sign in through the app's own window; "Launch on startup" is hidden too, since a sandboxed
app can't write a login item.

**Windows** — three options for x64, and two for ARM, cross-compiled from Linux with limited testing on real Windows
hardware:
- `moosik-@VERSION@-windows-x86_64-setup.exe` — a real installer (Program Files, Start Menu,
  uninstaller). Unsigned, so SmartScreen will warn on first run — expected, not a bug.
- `moosik-@VERSION@-windows-x86_64.msi` — the same files as an MSI, for deployment tools or a
  silent install (`msiexec /i moosik-@VERSION@-windows-x86_64.msi /qn`). It installs to the same
  folder as the setup, so use one or the other.
- `moosik-@VERSION@-windows-x86_64.zip` — portable, no install: unzip and run `moosik.exe`.
  Extract the whole folder before running; the exe needs the DLLs beside it.
- `moosik-@VERSION@-windows-arm64.msi` / `.zip` — a native build for Windows on ARM, not yet tested
  on ARM hardware. The x64 files also run there under emulation.

**macOS** — `moosik-@VERSION@-macos-arm64.dmg`, for Apple Silicon only. Open it and drag
Moosik to Applications. It is signed ad hoc and not notarized, so
macOS will refuse to open it on the first launch. Run `xattr -cr /Applications/Moosik.app` once,
or allow it in System Settings → Privacy & Security. libmpv and yt-dlp are bundled; nothing needs
installing.

Checksums for every file are in `moosik-@VERSION@-SHA256SUMS.txt`.

What changed since @PREVIOUS@:
<!-- TODO: write the changes as bullets, then delete this comment and the commit list below.
Commits since @PREVIOUS@:
@COMMITS@
-->
