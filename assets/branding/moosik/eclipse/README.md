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
| 11 | Pearl |
| 12 | Vinyl |
| 13 | Amethyst |
| 14 | Matcha |
| 15 | Solar |
| 16 | Denim |
| 17 | Marble |
| 18 | Lagoon |
| 19 | Cherry |
| 20 | Carbon |

Open `index.html` to compare the finishes. The comparison boards are design options; the bundle default comes from `ceramic-final.png` and the picker assets are in `variants/`. Generated with built-in image_gen; production prompts are in [variants/prompts.json](variants/prompts.json).

`scripts/moosik-icons.sh` converts the approved PNG into native macOS icon sizes and the ICNS file without changing the design. `scripts/ark-package.sh` embeds the icon in the bundle. The runtime UI/tray uses the embedded `variants/*.png` files.

Settings → Appearance → App icon offers ten standard finishes. Finishes 11–20 unlock in order at 5-hour intervals, from Pearl at 5 hours to Carbon at 50. Moosik uses the highest credited listening time in any local Monday–Sunday week, including weeks recorded before the milestones were added. Selection changes the running macOS Dock icon, app UI, and tray immediately and is restored at launch. Finder and the not-running launcher keep the Ceramic bundle default. Existing app identity, Keychain item, and user-data paths are unchanged.
