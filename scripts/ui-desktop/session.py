"""Run inside an isolated Linux guest, once per full application session."""

import json
import os
import subprocess
import sys
import wave
from pathlib import Path

session = Path(__file__).resolve().parent
profile = session / "profile"
profile.mkdir(mode=0o700, exist_ok=True)
os.environ.update(
    HOME=str(profile),
    XDG_CONFIG_HOME=str(profile / ".config"),
    XDG_DATA_HOME=str(profile / ".local/share"),
    XDG_CACHE_HOME=str(profile / ".cache"),
    XDG_STATE_HOME=str(profile / ".local/state"),
    LIBGL_ALWAYS_SOFTWARE="1",
    MOOSIK_DISABLE_ACTIVE_COUNT="1",
    WINIT_UNIX_BACKEND="x11",
    GDK_BACKEND="x11",
    PULSE_SERVER=f"unix:{session}/pulse",
    PULSE_RUNTIME_PATH=str(session / "pulse-runtime"),
)
config = profile / ".config/moosik"
config.mkdir(parents=True, exist_ok=True)
if sys.argv[1] == "demo":
    music = profile / "Music"
    music.mkdir()
    paths = []
    for title in ("Northbound", "Interlude", "Night Train"):
        path = music / (title + ".wav")
        with wave.open(str(path), "wb") as sample:
            sample.setparams((1, 2, 8000, 0, "NONE", ""))
            sample.writeframes(bytes(8000 * 2 * 30))
        paths.append(str(path))
    (config / "settings.json").write_text(json.dumps({"local_folders": [str(music)]}))
    (config / "local_playlists.json").write_text(
        json.dumps([{"id": "local:studio", "title": "Studio Sessions", "track_paths": paths}])
    )
    (config / "search_history.json").write_text(json.dumps({"entries": ["Studio Sessions", "Night Train"]}))

# Both the WM and the app use this session's X server, profile and private D-Bus.
subprocess.Popen(
    [
        "pulseaudio",
        "-n",
        "--exit-idle-time=-1",
        "--use-pid-file=no",
        "--load=module-null-sink sink_name=moosik",
        f"--load=module-native-protocol-unix socket={session}/pulse auth-anonymous=1",
    ]
)
subprocess.Popen(["openbox"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
(session / "display.json").write_text(
    json.dumps({k: os.environ[k] for k in ("DISPLAY", "XAUTHORITY", "DBUS_SESSION_BUS_ADDRESS")})
)
(session / "app.pid").write_text(str(os.getpid()))
os.execv(session / "moosik", ["moosik", "--no-autoplay", *sys.argv[2:]])
