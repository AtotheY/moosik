# Moosik UI work

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

Run routine checks in the background. Use focused Rust tests for behavior. For visual changes, run the affected real widgets offscreen with fixture data and inspect the PNGs at normal and narrow widths, including empty, sparse, and populated states. Use the checkout containing the change.

Run `./scripts/ui-check --list` to see available scenarios. The first scenario, `./scripts/ui-check queue` (also `just ui-check queue`), exercises queue removal and prints the screenshot directory. For an uncovered component, build on the queue scenario with the production widget and in-memory fixtures. Keep playback, account services, and saved profiles out of these checks. Use an isolated desktop or CI for whole-window and native integration checks. Coordinate with the user if a check actually requires their active desktop.

Check truncation, alignment, spacing, focus/hover states, and reachability of actions. Do not declare completion based only on tests/build success.
