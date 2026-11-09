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

Run routine checks in the background. For app changes, use `scripts/ui-session start --checkout <working-tree> --page <page>` to build and drive the full application inside its private desktop. Inspect Home, Library and a detail page there for design context. Exercise the changed flow, then inspect screenshots at normal and narrow widths with relevant empty, sparse and populated data. See [offscreen testing](docs/offscreen-testing.md) for fixtures, keyboard and pointer input, OCR and playback state. Stop sessions and power off the idle VM when done.

Use focused Rust tests for logic and `./scripts/ui-check queue` for the fast queue widget check. Keep accounts and playback isolated when testing. Use a dedicated Mac session for Apple-specific behavior and coordinate with the user when a check actually requires their active desktop.

Check truncation, alignment, spacing, focus/hover states, and reachability of actions. Do not declare completion based only on tests/build success.
