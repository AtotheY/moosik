# In-app Moosik updates

Moosik uses GitHub Releases to distribute client updates. An Apple Developer
membership or App Store listing is **not required**. On macOS, Sparkle 2 handles
checking, verifying the archive, installation, permissions, and relaunch.

A lit download button appears immediately left of the profile picture when an update is
available. With automatic checks enabled (the default), Moosik checks at launch and every
15 minutes while running. On Windows setup installs, one click downloads the
installer, verifies its checksum and restarts Moosik. Portable builds offer the release page.
On macOS, Sparkle can prepare the signed update in the background and the button restarts
into it. Sparkle handles authorization and any updates that need its native dialog.
Manual checks are available in **Settings → About → Updates**.

Before the first release is published, a manual check shows “Updates aren’t
published yet.” Missing feeds are distinct from “up to date”; offline, rate-limit,
and server failures remain errors. The macOS check probes feed availability in
the background before opening Sparkle, which still validates the feed and archive.

The current queue, playback position, shuffle ordering, settings, and pending liked
state are checkpointed before Sparkle continues its relaunch. Listening records
and the saved account stay in their existing stores; the updater replaces the app
bundle. Failed downloads/signature verification leave the installed app in place.

## Free GitHub distribution

The client checks **AtotheY/moosik**, and
uses this HTTPS feed:

`https://github.com/AtotheY/moosik/releases/latest/download/appcast.xml`

Update archives are signed with our own Ed25519 key. This is free and independent
of Apple. `SUVerifyUpdateBeforeExtraction` requires Sparkle to validate the archive
before unpacking it. The private key stays out of the repository; only
[the public key](../packaging/macos/update-public-key.txt) ships in the client.
The framework download itself is pinned by version and SHA-256. Its license is
included in the app bundle.

Without Apple notarization, macOS can warn users during their first manual
installation. Public builds require a pinned app-signing certificate, separately
from the Ed25519 archive key, so the workflow cannot silently change app identity.

**A self-signed certificate does not eliminate Keychain prompts across updates.**
macOS additionally partitions Keychain access by Apple-verified Team ID, or by the
individual executable's code hash for other signed apps. Our two-build test with a
self-signed certificate retained identical designated requirements, but the second
build was denied access to a dummy credential with interaction disabled. Merely
checking matching certificates is not sufficient evidence of retained access.

For the standard updater and Keychain store to preserve permission across releases,
use an Apple-issued Developer ID Application certificate. Notarization also improves
first-install trust. Distribution can continue entirely through GitHub.
The updater does not disable Gatekeeper or change system trust settings.

Existing clients that predate this updater need to install the first updater-enabled
Moosik release once. Those old binaries have no way to discover the new updater.
After that, subsequent releases are installed in the app.

## One-time maintainer setup

1. Run `./scripts/setup-update-signing.sh`. It reuses/creates the dedicated
   `com.sistilli.moosik.updates` Sparkle key in the Mac login Keychain and writes
   only the public key to `packaging/macos/update-public-key.txt`. Keep this key
   backed up securely: losing it can prevent existing clients from accepting updates.
2. Configure the repository secret **SPARKLE_PRIVATE_KEY** with a secure export of
   that same key. Sparkle's `generate_keys --account com.sistilli.moosik.updates -x
   <protected-file>` exports it. Send the file to `gh secret set SPARKLE_PRIVATE_KEY
   --repo AtotheY/moosik < <protected-file>` without printing it, then remove the
   temporary export. Set a restrictive umask before exporting. Do not paste the
   key into an issue, chat, or source file.
3. Keep the public key file committed, or set the repository variable
   **MOOSIK_UPDATE_PUBLIC_KEY** to that same public key. The workflow checks that
   the private key used to sign the archive matches the key embedded in the app.

4. Configure a persistent code-signing certificate through **MACOS_CERTIFICATE_P12**
   (base64 PKCS#12) and **MACOS_CERTIFICATE_PASSWORD**. Keep the same certificate
   across releases. Commit only its public certificate and SHA-1 certificate
   fingerprint in `packaging/macos/release-signing-certificate.pem` and
   `packaging/macos/release-signing-identity.txt`. The fingerprint identifies the
   certificate; it is not the update archive's integrity hash. Private keys and
   passwords must never be committed or printed. A self-signed certificate with
   the code-signing extended key usage works without an Apple account.

The workflow fails if the certificate is missing or differs from the pinned
identity. It verifies the built app against that identity and rejects a designated
requirement tied to a single build hash. Ed25519 archive verification is still
mandatory. Local builds continue to reuse their configured local certificate.

Version 0.1.15 used ad hoc bundle signing. Moving to another signing identity may
require **Always Allow** approval in Keychain. Self-signed releases can require this
again after each code update; do not promise a one-time approval for that route.
**Allow** grants only the current request. Never solve this by granting every app
access to the credential, removing Keychain partition controls, or moving the
session to plaintext.

## Publishing an update

Review and commit the intended Moosik source changes first. Bump the workspace
version to a new public `major.minor.patch`, refresh the lockfile, and write
`release-notes/<version>.md`. The fork's `+ark.N` development suffix is accepted
when comparing installed clients, but public releases use numeric versions.

The **Moosik release** workflow builds macOS, Windows x64 and Ubuntu 24.04 amd64
packages in parallel. Dispatch it on `main` to rehearse all three builds and warm
the release caches. A branch dispatch only uploads workflow artifacts. A reviewed `moosik-v<version>` tag on
`main` runs the same checks and publishes the complete GitHub release. For example,
`moosik-v0.1.15` must match workspace version `0.1.15`.

The release includes the initial-install DMG, the signed update ZIP, checksums, and
`appcast.xml`. It stays a draft until all files are uploaded, then becomes Latest.
The feed rejects mismatched versions/architectures or absent signatures. The current
packaging job ships Apple silicon; its feed declares `arm64` and the minimum macOS
version required by the actual bundled executables and playback libraries.
Publication also requires Windows installer launch and shortcut migration from 0.1.20,
and installation and rendering from the Linux .deb on clean Ubuntu. The versioned
checksum manifest covers the Windows setup and ZIP and the Ubuntu package. To test actual Explorer clicks and artwork on an interactive runner, dispatch the Windows workflow with `interactive_pin` enabled.

The alternative packaging workflow runs on manual dispatch.

## Optional Apple notarization later

Set repository variable **MOOSIK_NOTARIZE=1**, use a Developer ID Application
certificate for **MACOS_CERTIFICATE_P12**, and configure **APPLE_ID**,
**APPLE_APP_PASSWORD**, and variable **APPLE_TEAM_ID**. The workflow then signs
nested code with hardened runtime, notarizes and staples the app and disk image,
and assesses the app before publishing. This is optional and does not require
moving distribution to the App Store.

## Verification

The `updater-check` developer binary never loads the user's account or listening
profile. Run it only in a separately signed fixture app with a test feed/key and
`MoosikUpdaterTestState` pointing to dummy JSON. The actual Sparkle callbacks,
indicator, and checkpoint-before-relaunch bridge are shared with the client.
Use an older and newer fixture bundle to verify install/relaunch, and corrupt a
signed archive to verify rejection. Do not use the real app/profile as the target
of an updater integration test.

References: [Apple Keychain partition identity implementation](https://github.com/apple-oss-distributions/Security/blob/main/securityd/src/clientid.cpp),
[Sparkle setup](https://sparkle-project.org/documentation/),
[gentle reminders](https://sparkle-project.org/documentation/gentle-reminders/),
[publishing updates](https://sparkle-project.org/documentation/publishing/).
