# Moosik Eclipse icons

Default: **01 Ceramic Terracotta**. Approved by the user.

| Number | Finish |
| --- | --- |
| 01 | Ceramic Terracotta (default) |
| 02 | Celadon ceramic |
| 03 | Midnight anodized metal |
| 04 | Obsidian |
| 05 | Amber resin |
| 06 | Frosted glass |
| 07 | Travertine |
| 08 | Brushed copper |
| 09 | Ink graphic |
| 10 | Bloom ceramic |

Open `index.html` to compare all ten treatments. The comparison boards are design options; the bundle default comes from `ceramic-final.png` and the ten picker assets are in `variants/`. Generated with built-in image_gen; full prompts are in `prompts.json`.

`scripts/moosik-icons.sh` converts the approved PNG into native macOS icon sizes and the ICNS file without changing the design. `scripts/ark-package.sh` embeds the icon in the bundle. The runtime UI/tray uses the embedded `variants/*.png` files.

The ten choices are available in Settings → Appearance → App icon. Selection changes the running macOS Dock icon, app UI, and tray immediately and is restored at launch. Finder and the not-running launcher keep the Ceramic bundle default. Existing app identity, Keychain item, and user-data paths are unchanged.
