# Local macOS signing

Run `./scripts/setup-macos-signing.sh` once, then build with
`./scripts/ark-package.sh` as usual. macOS may request authentication during
setup and **Always Allow** approval when an existing credential first sees
the new signature. The app also migrates older credential names as described below.

Setup creates a ten-year, self-signed **Moosik Local Development** certificate
in the current user's login Keychain. Its private key is non-exportable, with
automatic access restricted to `/usr/bin/codesign`. Any required certificate
trust is limited to code signing in the current user's trust settings.
Temporary private-key files are removed when setup exits. The ignored file
`.ark-build/signing-identity` stores only the certificate's public fingerprint.
Running setup again reuses the same certificate.

Both macOS packaging scripts reuse that fingerprint, so rebuilt apps have a
consistent signing requirement and Keychain can retain the app's approval.
If a configured identity is missing or unusable, signing fails rather than
silently changing identity. Preserve the app's bundle identifier as well.

`MOOSIK_SIGNING_IDENTITY` can explicitly select another identity. The general
`build-macos.sh` script retains its ad hoc fallback on machines without local
configuration; `ark-package.sh` requires setup. An explicit value of `-`
selects ad hoc signing and brings back the approval problem across rebuilds.

This certificate is for local development. Public GitHub updates can use Sparkle Ed25519 signatures without an Apple
membership. Developer ID signing and notarization are optional for a smoother
first install; see [client updates](client-updates.md). Keep the existing session in
Keychain; this setup does not read it or change its access controls.

## Credential name and migration

The app stores its session under service `com.sistilli.moosik`, with the
visible Keychain label **Moosik**. On the first updated launch it can import
the old `com.sistilli.refryt` entry. The old entry is removed only after the
new one is written and read back successfully. That migration may require
approval of the old entry; subsequent launches use only the Moosik entry.
Sign-out removes both service names so an old credential cannot restore a
signed-out account. Refreshes that return the same cookie and source skip
the otherwise redundant Keychain write and verification.
