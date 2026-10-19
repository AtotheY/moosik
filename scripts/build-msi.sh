#!/usr/bin/env bash
# Build dist/refryt-<version>-windows-x86_64.msi from the Windows zip that build-windows.sh makes:
# the same files the setup .exe installs, into Program Files\refryt, with a Start menu entry and
# an Add/Remove Programs entry. Built on Linux with msitools' `wixl`, so it has no setup screens:
# an MSI installs as it is asked to, which is what deployment tools (Intune, Group Policy, winget)
# want from one.
#
# The MSI and the setup .exe install to the same folder; use one or the other, not both.
#
# `ARCH=aarch64` builds refryt-<version>-windows-arm64.msi from the ARM64 zip, the one installer
# Windows on ARM gets. wixl has no ARM64 target, so the package is built as x64 and its summary
# information's platform is then set to Arm64 with msibuild: that field is what Windows Installer
# reads to decide which machines take the package, and nothing else in it differs (the 64-bit
# Program Files folder and component flags are the same on both).
#
#   ./scripts/build-windows.sh && ./scripts/build-msi.sh
#   ARCH=aarch64 ./scripts/build-windows.sh && ARCH=aarch64 ./scripts/build-msi.sh
#
# Needs `wixl` and `msibuild` (Arch: `pacman -S msitools`).
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

for tool in wixl msibuild; do
    command -v "$tool" > /dev/null || {
        echo "error: $tool not found; install msitools (Arch: pacman -S msitools)" >&2
        exit 1
    }
done

version="$(cargo metadata --no-deps --format-version 1 \
    | python3 -c 'import json, sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "refryt"))')"
case "${ARCH:-x86_64}" in
    x86_64) arch_name=x86_64 platform=x64 ;;
    aarch64) arch_name=arm64 platform=Arm64 ;;
    *)
        echo "error: ARCH must be x86_64 or aarch64, not '$ARCH'" >&2
        exit 1 ;;
esac
zip="dist/refryt-$version-windows-$arch_name.zip"
out="dist/refryt-$version-windows-$arch_name.msi"
[ -f "$zip" ] || {
    echo "error: $zip is missing; run ${ARCH:+ARCH=$ARCH }./scripts/build-windows.sh first" >&2
    exit 1
}

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
unzip -q "$zip" -d "$work"
stage="$work/refryt"
for f in refryt.exe libmpv-2.dll WebView2Loader.dll yt-dlp.exe NotoColorEmoji.ttf; do
    [ -f "$stage/$f" ] || {
        echo "error: $f is not in $zip" >&2
        exit 1
    }
done

# The GUIDs are fixed for good. The upgrade code is what makes a newer MSI replace an older one
# rather than install beside it; each component's is what Windows tracks the file by across
# versions. Never reuse one for something else, and never change them. The ARM64 MSI shares
# them: it installs the same files to the same place, so on an ARM machine it replaces an x64
# install instead of sitting beside it.
cat > "$work/refryt.wxs" <<EOF
<?xml version="1.0" encoding="utf-8"?>
<Wix xmlns="http://schemas.microsoft.com/wix/2006/wi">
  <Product Id="*" Name="refryt" Language="1033" Version="$version" Manufacturer="refryt"
           UpgradeCode="3BC13FF4-48B0-4B15-82F1-B2AF7FD35E35">
    <Package InstallerVersion="500" Compressed="yes" InstallScope="perMachine"
             Description="refryt $version" Comments="A native YouTube Music client" />
    <Media Id="1" Cabinet="refryt.cab" EmbedCab="yes" />
    <MajorUpgrade DowngradeErrorMessage="A newer version of refryt is already installed." />

    <Icon Id="refryt.ico" SourceFile="$root/assets/icons/app/refryt.ico" />
    <Property Id="ARPPRODUCTICON" Value="refryt.ico" />
    <Property Id="ARPURLINFOABOUT" Value="https://github.com/gridoe/refryt" />

    <Directory Id="TARGETDIR" Name="SourceDir">
      <Directory Id="ProgramFiles64Folder">
        <Directory Id="INSTALLDIR" Name="refryt">
          <Component Id="App" Guid="CBFAAC8B-7037-4B9E-8A93-44F64E6CF35A" Win64="yes">
            <File Id="refryt.exe" Name="refryt.exe" Source="$stage/refryt.exe" KeyPath="yes" />
          </Component>
          <Component Id="Mpv" Guid="2FA232FE-34FA-4F1C-9336-0FBE7027ED28" Win64="yes">
            <File Id="libmpv.dll" Name="libmpv-2.dll" Source="$stage/libmpv-2.dll" KeyPath="yes" />
          </Component>
          <Component Id="WebView2" Guid="1134A7CE-1ABC-42AF-8447-CBE6BAA2F38B" Win64="yes">
            <File Id="WebView2Loader.dll" Name="WebView2Loader.dll" Source="$stage/WebView2Loader.dll" KeyPath="yes" />
          </Component>
          <Component Id="YtDlp" Guid="52BDD765-F8FB-4F46-8B93-56F714A36261" Win64="yes">
            <File Id="yt_dlp.exe" Name="yt-dlp.exe" Source="$stage/yt-dlp.exe" KeyPath="yes" />
          </Component>
          <!-- The colour emoji font: Windows' own is COLR, which the emoji painter cannot read. -->
          <Component Id="EmojiFont" Guid="E9AE562B-21C5-4344-AA5C-2D5BD6415CDB" Win64="yes">
            <File Id="NotoColorEmoji.ttf" Name="NotoColorEmoji.ttf" Source="$stage/NotoColorEmoji.ttf" KeyPath="yes" />
          </Component>
        </Directory>
      </Directory>
      <Directory Id="ProgramMenuFolder">
        <Component Id="StartMenu" Guid="16878CE6-DEB5-4087-B106-2BE8DEC7F23F" Win64="yes">
          <Shortcut Id="StartMenuShortcut" Name="refryt" Target="[INSTALLDIR]refryt.exe"
                    WorkingDirectory="INSTALLDIR" />
          <RegistryValue Root="HKLM" Key="Software\refryt" Name="StartMenuShortcut" Type="integer"
                         Value="1" KeyPath="yes" />
        </Component>
      </Directory>
    </Directory>

    <Feature Id="Complete" Level="1">
      <ComponentRef Id="App" />
      <ComponentRef Id="Mpv" />
      <ComponentRef Id="WebView2" />
      <ComponentRef Id="YtDlp" />
      <ComponentRef Id="EmojiFont" />
      <ComponentRef Id="StartMenu" />
    </Feature>
  </Product>
</Wix>
EOF

mkdir -p dist
wixl -a x64 -o "$out" "$work/refryt.wxs"

# wixl leaves the File table's Version column empty, and Windows Installer then treats every file
# as unversioned. On an upgrade it compares that against the file already installed, which does
# carry a version resource (libmpv, WebView2Loader, yt-dlp), decides the installed one is "higher"
# and skips the component; RemoveExistingProducts then deletes the old copy, and the app is left
# without its DLLs. So copy each PE file's fixed version (VS_FIXEDFILEINFO) into the table.
python3 - "$stage" <<'PY' | while read -r name ver; do
import pathlib, struct, sys
for f in sorted(pathlib.Path(sys.argv[1]).iterdir()):
    data = f.read_bytes()
    # The VS_FIXEDFILEINFO signature followed by its structure version, 1.0.
    i = data.find(b"\xbd\x04\xef\xfe\x00\x00\x01\x00")
    if i >= 0:
        ms, ls = struct.unpack_from("<II", data, i + 8)
        print(f.name, f"{ms >> 16}.{ms & 0xffff}.{ls >> 16}.{ls & 0xffff}")
PY
    msibuild "$out" -q "UPDATE \`File\` SET \`Version\` = '$ver' WHERE \`FileName\` = '$name'"
done
if [ "$platform" != x64 ]; then
    # Rewrites the whole summary stream: title, subject (the name argument), author, platform
    # and a fresh package code. The schema version (500) and the rest carry over.
    msibuild "$out" -s "refryt $version" refryt "$platform;1033" "{$(python3 -c 'import uuid; print(str(uuid.uuid4()).upper())')}"
fi
echo "built $out ($(du -h "$out" | cut -f1))"
