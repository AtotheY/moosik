# In-app Moosik updates

Moosik uses GitHub Releases to distribute client updates. An Apple Developer
membership or App Store listing is **not required**. On macOS, Sparkle 2 handles
checking, verifying the archive, installation, permissions, and relaunch.

A lit download button appears immediately left of the profile picture when an update is
available. Moosik checks at launch and every 15 minutes while running. Dismissing an
update offer leaves the button visible. Older macOS installs with Sparkle scheduling
disabled use an information-only check to discover eligible updates.
Clicking Update checks the release source again. If a newer release
appeared after the badge, Moosik uses the newer version. A failed lookup leaves the app running
and the user can retry. On Windows setup installs, one click downloads the
installer, verifies its checksum and restarts Moosik. Portable builds offer the release page.
On macOS, Sparkle can prepare the signed update in the background and the button restarts
into it if it still matches the published appcast. A superseded installer is discarded before
Sparkle fetches the current release. Sparkle handles authorization and any updates that need its native dialog.
If Sparkle is still busy, Moosik keeps the click queued briefly and tells you if you need
to retry. Clicking Update again brings an existing Sparkle offer or installer window forward.
If an older Mac version cannot update itself, quit Moosik, download the DMG from the
[latest release](https://github.com/AtotheY/moosik/releases/latest), and use Finder to
replace Moosik in Applications. Reopen that copy.
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

Merging to `main` starts **Release merged main**. It tags the current merged
commit with the next public patch version and starts **Moosik release** on that
tag. Several close merges can ship together. An explicitly higher workspace
version selects a major or minor release. Each build checkout stamps the tag
version into the manifest and lockfile. The tagged source stays on reviewed
main history and branch protection applies as usual.

Write `release-notes/<version>.md` for curated notes. Otherwise the tag carries
GitHub-generated release notes. To reproduce a tagged build, check out the tag
and run `python3 scripts/public-version.py stamp <tag>` before building.

Mac, Windows and Ubuntu each publish their files to the same tagged release as
soon as that platform passes its build and checks. The Windows checks exercise
the installer and shortcut upgrade. Ubuntu installs the .deb and renders a
window. macOS verifies the signed bundle and Sparkle archive. Each platform adds
its own checksum manifest and updates the version-wide manifest. A failed job
stays visible in Actions. Use **Re-run failed jobs** to add the missing platform
to the same release. Previously published packages keep their original bytes.

GitHub's Latest release follows the newest verified Mac version so installed
Sparkle clients always have a signed feed. Windows and Linux clients built with
platform-aware updates look through the releases for a verified package for
their OS. Older Windows and Linux clients still follow GitHub Latest. All
published packages also appear on their version's release page.

Dispatch **Moosik release** on a branch for rehearsal artifacts or on a public
tag to rebuild that version. The macOS feed declares Apple silicon and the
minimum macOS version required by the bundle. For interactive Explorer and
artwork checks, dispatch the Windows workflow with `interactive_pin` enabled.

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
