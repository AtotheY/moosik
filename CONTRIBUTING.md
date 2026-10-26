# Contributing to Moosik

Bug reports, ideas and small fixes are welcome. Larger changes start as an issue, so the approach can be agreed before the work is done.

## Reporting a bug

Open a [bug report](https://github.com/AtotheY/moosik/issues/new?template=bug.yml). It asks for:

- The version and the package, for example "0.1.10, Windows ARM64 MSI" or "0.1.10, Flatpak". The version is shown in Settings.
- What happened, what was expected, and the steps to get there.
- The end of `refryt.log`, which is in the config folder:

| Platform | Config folder |
|---|---|
| Linux | `~/.config/refryt` |
| Flatpak | `~/.var/app/io.github.gridoe.refryt/config/refryt` |
| macOS | `~/Library/Application Support/refryt` |
| Windows | `%APPDATA%\refryt\config` |

Review logs before sharing. They can show song names, file paths, and your username; remove anything personal and never assume a diagnostic is safe to publish. Never post `browser.json` or `oauth.json` from the same folder: they hold the signed-in session and work like a password.

After a crash, review any report the app offers and submit a sanitized excerpt to the Moosik tracker above. Some inherited diagnostic links may still point upstream; fork-specific reports belong here. Never attach your listening database or raw exports unless you deliberately want to publish that history.

When a page or playback suddenly stops working for everyone, YouTube has usually changed something on its side. Check the open issues first.

## Ideas

Open a [feature request](https://github.com/AtotheY/moosik/issues/new?template=feature.yml) describing what it is for. For anything bigger than a fix, wait for a reply before writing code.

## Pull requests

- Keep each pull request to one change.
- `just check` has to pass. It runs `cargo fmt --check`, clippy with `-D warnings` for both resolver feature sets, and the tests. GitHub CI runs on pushes to `main` and pull requests; platform build success is not a substitute for testing the installed app.
- Follow the style of the code around the change, including how much it is commented.
- For a change to the interface, inspect nearby components and `theme.rs` / `widgets.rs` first. Reuse existing tokens and controls. Include before/after screenshots with synthetic or consented data, and check normal/narrow sizes plus empty and populated states. See `AGENTS.md`.

[docs/BUILDING.md](docs/BUILDING.md) covers the setup for each platform. A new theme is a single CSS file; [docs/THEMES.md](docs/THEMES.md) describes the format.

## Licence

Moosik is an independent fork of [Refryt](https://github.com/gridoe/refryt), by gridoe and contributors. Keep upstream attribution and existing copyright notices. Moosik is licensed under the [GPL-3.0-or-later](LICENSE), and contributions are accepted under the same licence.
