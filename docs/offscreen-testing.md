# Test the full desktop app from a Mac

`scripts/ui-session` builds the working checkout and opens Moosik on a private Linux display inside a Lima VM. The Mac's windows, mouse, keyboard, audio and signed-in Moosik profile stay available to the listener. Each session gets its own profile, X server, sound sink and media bus. The default demo profile has three playable silent tracks, a local playlist and recent searches. Choose `--fixture empty` for a fresh profile.

Install Lima 2.x on a Mac with hardware virtualization. The runner also recognizes the project-local Lima installation used by the macOS development workspace. The first run downloads Ubuntu, installs build and desktop packages, and compiles Moosik. Subsequent sessions reuse the VM and Cargo build cache. The VM has no host filesystem mounts. The runner sends build inputs from the chosen checkout, including uncommitted source files.

Run from the checkout you want to test. A session continues between commands.

```bash
session=$(./scripts/ui-session start --page library)
./scripts/ui-session read "$session"
./scripts/ui-session click-text "$session" Local
./scripts/ui-session shot "$session" target/ui-session/library.png
./scripts/ui-session resize "$session" 730 575
./scripts/ui-session shot "$session" target/ui-session/library-narrow.png
./scripts/ui-session stop "$session"
./scripts/ui-session poweroff
```

- Start at any supported `--page`, including `home`, `search`, `library`, `settings`, `history`, `stats` and `playlist:local:studio`. Navigate normally during the session. Use `start --checkout /path/to/feature` to build a different worktree. This also works for worktrees that predate the runner.
- `read` returns recognized on-screen text and window-relative coordinates as JSON. `click-text` clicks one exact visible line, ignoring case. OCR can miss labels or combine adjacent controls. Use `shot` and `click SESSION X Y` for icons, duplicate labels and other ambiguous targets.
- Use `key`, `type`, `move`, `right-click`, `drag` and `scroll` for interactions. For example, `key SESSION ctrl+a`, `type SESSION Northbound`, then `key SESSION Return` submits a search in a focused field. Each command supports `--help`.
- `media` returns Moosik's actual MPRIS state, including track metadata, position and playback status. `log` shows app diagnostics. Online services depend on guest connectivity and account state.
- `list` shows session IDs. `stop` discards that session and its profile. `poweroff` shuts down the idle VM and retains the build cache. Screenshots saved on the Mac remain available.

Use the signed macOS bundle on a separate Mac session for Apple-specific behavior, including Keychain, the menu bar, Sparkle and physical media keys.
