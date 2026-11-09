<p align="center">
  <img src="assets/icons/app/moosik-256.png" width="100" alt="Moosik app icon">
</p>

# Moosik

Moosik is an open-source YouTube Music desktop player. Listen along on Discord, let friends add songs to your queue, and find tracks in your listening history.

[Download for Apple Silicon Mac](https://moosik.dev/?platform=mac#download) · [Windows x64 and Ubuntu 24.04 x86-64 packages](https://github.com/AtotheY/moosik/releases/latest)

[Install](#install) · [Listening stats](docs/listening-stats.md) · [Queue](docs/queue.md) · [Contribute](CONTRIBUTING.md) · [Report a bug](https://github.com/AtotheY/moosik/issues/new?template=bug.yml)

## Screenshots

These macOS screenshots show signed-out browsing and synthetic listening stats. [Screenshot details](assets/screenshots/moosik/README.md)

<table>
  <tr>
    <td width="50%"><a href="assets/screenshots/moosik/album-spotify.jpg"><img src="assets/screenshots/moosik/album-spotify.jpg" alt="Moosik album view in the Spotify theme"></a><br>Album view</td>
    <td width="50%"><a href="assets/screenshots/moosik/listening-catppuccin-frappe.jpg"><img src="assets/screenshots/moosik/listening-catppuccin-frappe.jpg" alt="Moosik listening overview with synthetic listening stats"></a><br>Listening stats · Catppuccin Frappé</td>
  </tr>
  <tr>
    <td width="50%"><a href="assets/screenshots/moosik/explore-catppuccin-latte.jpg"><img src="assets/screenshots/moosik/explore-catppuccin-latte.jpg" alt="Moosik Explore page in the Catppuccin Latte theme"></a><br>Explore · Catppuccin Latte</td>
    <td width="50%"><a href="assets/screenshots/moosik/icons-rose-pine.jpg"><img src="assets/screenshots/moosik/icons-rose-pine.jpg" alt="Moosik appearance settings with the Ceramic Eclipse icon selected in the Rosé Pine theme"></a><br>Appearance · Rosé Pine</td>
  </tr>
</table>

## Player

- **Listen along on Discord.** Open Discord desktop and enable **Discord Rich Presence** and **Host Listen along** in **Settings → Discord**. Play a YouTube Music track. Friends can join from your profile, follow your playback in Moosik, and add songs to the shared queue. [Setup guide](https://moosik.dev/blog/youtube-music-discord/)
- **Queue and playlists.** Move several upcoming tracks together, undo edits, or save the queue as a private playlist. Organize playlists in nested folders and shuffle an entire folder. [Queue controls](docs/queue.md)
- Follow synced lyrics, use keyboard shortcuts, and customize the player’s theme, sidebars, and fullscreen view.

## Listening history

Search local listening history by title, artist, or date. See listening time, skips and completed plays, add mood tags and ratings, or share a recap for a date range you choose. Export song stats as CSV, history as JSON, or back up the SQLite database.

[Listening history guide](https://moosik.dev/guides/listening-history/) · [Stat definitions and limitations](docs/listening-stats.md)

## Install

Download a package from the [latest release](https://github.com/AtotheY/moosik/releases/latest) or [moosik.dev](https://moosik.dev/#download).

| Platform | Package and status |
| --- | --- |
| macOS, Apple Silicon | DMG installer and signed in-app updates. See the release for macOS requirements. |
| Windows, x64 | Experimental setup EXE and portable ZIP. Optional [Spotify playback](https://moosik.dev/blog/alternative-spotify-client-windows/) requires WebView2. |
| Ubuntu 24.04, x86-64 | Experimental [Debian package](docs/BUILDING.md#ubuntu-2404-x86-64) with a clean-install launch check. |

On macOS, open the DMG and drag Moosik to Applications. First launch may require approval in **System Settings → Privacy & Security**. See [in-app updates](docs/client-updates.md) for signing and update details.

Interactive sign-in and playback on the experimental packages still need testing on real hardware.

### Build from source

Install Rust 1.90 or newer and the platform dependencies in the [build guide](docs/BUILDING.md). Playback uses `libmpv`; `yt-dlp` provides fallback resolution and other media features.

```sh
git clone https://github.com/AtotheY/moosik.git
cd moosik
cargo run --locked --release -p moosik --bin moosik
```

## Account and playlists

Browse YouTube Music signed out, or choose **Sign in → Use my [browser] login** for your library and playlists. **Open sign-in window** also works with protected browser profiles. YouTube Music downloads require a verified YouTube Premium or YouTube Music Premium membership.

YouTube playlist edits follow the playlist’s visibility settings. **Save privately** keeps songs and local files in a playlist on this computer. Keep session files and cookies private when sharing diagnostics.

## Contribute

To test a feature on a Mac, run `./scripts/ui-session start --page library` from the checkout you’re editing. It opens the full app on a private Linux desktop with sample music, so you can navigate, test playback and take screenshots while continuing to use your Mac. See the [offscreen testing guide](docs/offscreen-testing.md) for setup, controls and cleanup.

[Report a bug](https://github.com/AtotheY/moosik/issues/new?template=bug.yml), [suggest a feature](https://github.com/AtotheY/moosik/issues/new?template=feature.yml), or help test a platform. Read [CONTRIBUTING.md](CONTRIBUTING.md) before starting a larger change. Planned work is in the [roadmap](docs/ROADMAP.md).

## Credits and license

- [gridoe](https://github.com/gridoe) and the [original contributors](https://github.com/AtotheY/moosik/tree/12e899c4b941728680c42ba1a3841d92769cd5f2): playback, API integration, and the original desktop interface.
- [Sonora](https://github.com/sonorahq/sonora) and [`ytmusic-rs`](https://github.com/sonorahq/ytmusic-rs): direct stream URL resolution.
- [yt-dlp](https://github.com/yt-dlp/yt-dlp): fallback resolution.
- [youtui](https://github.com/nick42d/youtui): `ytmapi-rs` and the `rusty_ytdl` fork used by the optional native resolver.
- [ytmdesktop](https://github.com/ytmdesktop/ytmdesktop): queue continuation.
- Spotify's desktop app: shortcuts, selection behavior, and album, playlist, and artist page layouts.
- [LRCLIB](https://lrclib.net): synced lyrics.
- Theme palettes: [Catppuccin](https://github.com/catppuccin/palette),
  [Nord](https://www.nordtheme.com/docs/colors-and-palettes),
  [Gruvbox](https://github.com/morhetz/gruvbox),
  [Tokyo Night](https://github.com/folke/tokyonight.nvim),
  [Dracula](https://spec.draculatheme.com), [Rosé Pine](https://github.com/rose-pine/palette),
  [Everforest](https://github.com/sainnhe/everforest),
  [Solarized](https://ethanschoonover.com/solarized) and Spotify's own colours. Each file in
  `crates/moosik/themes` names its source.

Maintained by [AtotheY](https://github.com/AtotheY). Licensed under [GPL-3.0-or-later](LICENSE), including this fork's changes. Existing copyright notices and dependency licenses apply.

Moosik is an unofficial client, independent of Google and Spotify. YouTube and YouTube Music are trademarks of Google LLC. Changes to either service can affect browsing and playback. Use is subject to the applicable service terms.
