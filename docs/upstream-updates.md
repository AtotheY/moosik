# Keeping Moosik current with upstream

Moosik can keep upstream's fixes without giving up its own UI, stats, branding, or
macOS session handling. Treat upstream as an upstream source and integrate into a
review branch, then test the resulting Moosik app.

## Current comparison (November 1, 2026)

- `origin`: `git@github.com:AtotheY/moosik.git`
- `upstream`: `https://github.com/gridoe/refryt.git`
- Shared base: `12e899c4b941728680c42ba1a3841d92769cd5f2` (v0.1.14).
- Committed Moosik `main`: `21eef5a`, 9 fork commits beyond that base.
- upstream `main`: `e9685b0`, 18 commits beyond that base; latest release is v0.2.1.
- There are also uncommitted local changes. They must be reviewed and checkpointed
  before preparing an integration branch that includes all current Moosik work.

The existing remote fetch configuration was restricted to the v0.1.14 tag. The
current upstream branch has now been fetched explicitly into `upstream/main`.
Use `./scripts/upstream-check.sh --fetch` for future comparisons. This script
fetches and reports; it does not merge, reset, commit, or modify source files.

## Useful updates to review

- `53fd297`: sign-in fixes for larger YouTube replies.
- `e588700`: faster yt-dlp lookup and fallback for refused streams.
- `13bcd88`: earlier sign-in header parsing fix.
- `e0b6b63`: separates chosen songs and Autoplay, adds radio filters, and fixes
  adding songs during autoplay. Reconcile these rules with Moosik's editable queue.
- `b0eaefe`: playback controls and shuffle fixes.
- `015f886`: LRCLIB opt-out and privacy documentation.
- `6fa8432`, `af41a1d`: album/playlist extras and UI changes. Review individually
  against Moosik's design, interaction preferences, and shared song menus.

Sources: [upstream commits](https://github.com/gridoe/refryt/commits/main/),
[v0.2.0 notes](https://github.com/gridoe/refryt/blob/main/release-notes/0.2.0.md),
[v0.2.1 notes](https://github.com/gridoe/refryt/blob/main/release-notes/0.2.1.md).

## Recommended integration workflow

1. Review and commit the current Moosik work in focused commits. Inspect each diff
   before staging it; do not blindly add unrelated work, credentials, or generated files.
2. Run `./scripts/upstream-check.sh --fetch`.
3. Create a separate worktree and merge there:

   ```sh
   git worktree add ../upstream-integration -b integrate/upstream-0.2.1 main
   cd ../upstream-integration
   git merge --no-ff --no-commit upstream/main
   ```

4. Resolve overlaps deliberately. Preserve Moosik branding/assets, listening stats,
   Keychain service and signing identity, authenticated-request boundaries, shared
   song actions, and queue selection/reordering/Undo. Review upstream behavior in
   these areas even when Git merges the text automatically.
5. Run the full library tests, Clippy, the signed packaging script, and UI checks.
   Check login, playback, queue edits under shuffle and autoplay, relaunch ordering,
   song menus, stats, and normal/narrow layouts. Use dummy test data and signed
   bundles as described in `AGENTS.md`.
6. Commit the reviewed merge, inspect its diff, then merge that integration branch
   into Moosik `main`. Keep `origin/main` as the branch's upstream. Never replace
   Moosik with an upstream checkout or force-push a rewritten public history.

A dry run using `git merge-tree` on the **committed tips** reported textual conflicts
in `CONTRIBUTING.md`, `Cargo.lock`, `Cargo.toml`, `README.md`, and
`crates/moosik/src/widgets.rs`. This is not a full preview of the uncommitted work;
checkpointing it can reveal further conflicts, especially around the queue.
No upstream merge has been applied by this workflow's setup.

## Small fixes between full integrations

For an urgent isolated fix, create a review branch from checkpointed Moosik and use
`git cherry-pick -x <upstream-commit>`. The `-x` records its origin. Review dependent
commits and run the same relevant checks. Prefer periodic normal merges for broad
updates: their shared history helps Git identify updates already integrated.

Do not use a blanket "ours" conflict strategy. That can silently discard the very
fixes being imported. Keep the implementation that satisfies Moosik's behavior and
bring in the upstream fix around it.
