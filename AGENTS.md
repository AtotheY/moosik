# Moosik UI work

## Default testing workflow

For routine UI work on a Mac, build and drive the full app on the private Linux desktop. From the worktree containing the change, run `./scripts/ui-session start --page library`, or choose another starting page such as `home`, `search` or `settings`. The session includes uncommitted edits. If the runner lives in a different checkout, invoke it there with `--checkout /path/to/feature-worktree`.

Use the returned session ID to navigate through the changed flow and verify the outcome. Inspect screenshots at normal and narrow widths with relevant empty, sparse and populated data. For playback changes, check the reported track and transport state with `media`. Stop the session and power off the idle VM when finished. See the [offscreen testing guide](docs/offscreen-testing.md).

Use focused Rust tests for logic and `./scripts/ui-check queue` for queue widget regressions. For Apple-specific behavior such as Keychain or media keys, use a dedicated macOS session. Ask before using the listener’s active desktop if it’s the only way to verify the change.

## Local macOS builds

Use `scripts/ark-package.sh` to package local builds and
`scripts/sign-macos.sh <app-path>` when re-signing a local app. These reuse
the certificate configured by `scripts/setup-macos-signing.sh`, retaining a
consistent designated requirement. Self-signed certificates still have a per-build
Keychain partition: do not promise prompt-free updates based only on matching
signatures. Test access across different executable hashes with dummy credentials.
Do not replace local signatures with
`codesign --sign -` or change the bundle identifier. See
`docs/macos-local-signing.md` for setup and distribution differences.
For checks that exercise a native macOS window or OS service, use the signed Moosik bundle in a dedicated test session. Raw `cargo run`,
`target/*/moosik`, and diagnostic binaries have separate signing identities
and can trigger extra Keychain approvals if they load the user's session.
Use dummy profiles/credentials for diagnostic tests; do not repeatedly run
unsigned diagnostics against the saved login.

## UI changes

Before building UI, inspect visual references for Home, Library, and a detail page. Use offscreen captures from the current checkout where available and existing app screenshots for broader design context. Read `crates/moosik/src/theme.rs`, shared `widgets.rs`, and nearby `ui` implementations. Consult relevant visual references first.

Use the existing Inter weights, semantic palette, page/section type sizes, 20pt page inset, 56pt rows, 40pt row artwork, pill filters, Lucide icons, themed menus, and artwork shapes. Prefer shared widgets over stock egui controls. Keep detail in drilldowns, tooltips, or dialogs; do not crowd the primary view with explanatory copy.

Check truncation, alignment, spacing, focus/hover states, and reachability of actions. Do not declare completion based only on tests/build success.
