# Media controls and fallback launches

Moosik registers its playback controls with the operating system. Play/Pause, Next,
and Previous follow the player selected by macOS; Moosik does not globally rebind
the media keys.

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

Utilities such as [noTunes](https://github.com/tombonez/noTunes) can replace Apple
Music launch attempts. Their replacement must invoke the command above to resume
playback; opening the Moosik bundle alone leaves its restored queue paused. Such
helpers are optional and are not installed or configured by Moosik updates.
