# Media controls and fallback launches

Moosik registers its playback controls with the operating system. Play/Pause, Next,
and Previous follow the player selected by macOS; Moosik does not globally rebind
the media keys.

## macOS fallback setting

In **Settings → System → Media keys**, turn on **Use Moosik instead of Apple
Music**. This is off by default. A bundled background helper watches for Apple
Music/iTunes launches and resumes Moosik instead. It remains available after
quitting Moosik and after login. If macOS asks for approval, use the setting's
**Open System Settings** button and allow Moosik's background activity.

This does not intercept Play/Pause, Next or Previous, so another active player
continues receiving its normal media controls. macOS does not tell the helper
whether an Apple Music launch came from a media key or a manual click: **manual
Apple Music launches are blocked too**. Turn the setting off to use Apple Music.
An already-running Apple Music session is left alone when enabling the setting.

Turning the toggle off unregisters and stops the helper. Disabling it in macOS
System Settings is respected; starting Moosik will not automatically re-enable it.
The OS registration is per Mac user, rather than synced with a music account.
Remove any independently installed fallback utility (such as noTunes) from Login
Items and quit it before enabling this setting, to avoid duplicate launch handlers.

The helper is installed inside the signed app bundle; it requires no downloads,
Accessibility permission or keyboard monitoring. It uses Apple's
[SMAppService](https://developer.apple.com/documentation/servicemanagement/smappservice)
with `Contents/Library/LaunchAgents/com.moosik.media-fallback.plist` and a relative
`BundleProgram`, so no user-specific installation path is embedded in the release.

## Explicit resume launch

A normal launch restores the saved queue paused. On macOS and Linux, a launcher
that explicitly intends to start listening can pass `--resume`. With a saved
queue, this loads its current song and saved playback position. If Moosik is
already running, the request resumes its existing player without raising the
window. Repeated requests do not pause music or restart a loaded song. An empty
queue stays empty.

For a macOS app installed in Applications:

```sh
open -g -n /Applications/Moosik.app --args --resume
```

The `-n` option lets the launch arguments reach a new process; Moosik's
single-instance guard forwards the request to an existing player and exits before
loading the saved account. The `-g` option avoids activating the app over the
current foreground application. Adjust the path for a different installation.

This command is intended for an explicit resume action or a fallback launcher
that runs only when macOS would otherwise open Apple Music. It should not replace
the system's global Play/Pause key, because that would prevent other active
players from receiving their normal controls.

External utilities such as [noTunes](https://github.com/tombonez/noTunes) can also
use this command. Opening the Moosik bundle alone leaves its restored queue paused.
Moosik does not install or configure third-party helpers.

## Native integration check

On a logged-in Mac with the project's signing certificate available, run
`python3 scripts/tests/macos-media-fallback-integration.py`. It compiles the same
helper with a test-only target identifier and a separate LaunchAgent. Disposable
apps verify that an already-running app survives enabling, unrelated launches
stay untouched, each target launch resumes once with `--resume`, and disabling
stops interception. It never launches Apple Music or loads a music account.
