"""Inspect and operate only the private X display belonging to this session."""

import csv
import io
import json
import os
import subprocess
import sys
import time
from pathlib import Path

session = Path(__file__).resolve().parent
command, *args = sys.argv[1:]
xd = ["xdotool"]
deadline = time.monotonic() + (40 if command == "ready" else 3)
while time.monotonic() < deadline:
    if (session / "app.pid").exists():
        os.environ.update(json.loads((session / "display.json").read_text()))
        match = subprocess.run(
            xd + ["search", "--onlyvisible", "--pid", (session / "app.pid").read_text()],
            check=False,
            capture_output=True,
            text=True,
        )
        if match.returncode == 0 and match.stdout.strip():
            window = match.stdout.splitlines()[-1]
            break
    time.sleep(0.15)
else:
    sys.exit("Moosik window unavailable. Use the log command to inspect the session.")


def run(*argv):
    subprocess.run(argv, check=True)


if command == "ready":
    run(*xd, "windowactivate", "--sync", window)
elif command == "media":
    from gi.repository import Gio, GLib

    bus = Gio.bus_get_sync(Gio.BusType.SESSION)
    state = bus.call_sync(
        "org.mpris.MediaPlayer2.Moosik",
        "/org/mpris/MediaPlayer2",
        "org.freedesktop.DBus.Properties",
        "GetAll",
        GLib.Variant("(s)", ("org.mpris.MediaPlayer2.Player",)),
        None,
        Gio.DBusCallFlags.NONE,
        5000,
        None,
    ).unpack()[0]
    print(json.dumps(state, indent=2))
elif command in ("read", "click-text"):
    # OCR includes custom-painted labels. Coordinates refer to the original window pixels.
    png = subprocess.check_output(["import", "-silent", "-window", window, "png:-"])
    scaled = subprocess.check_output(
        ["convert", "png:-", "-alpha", "remove", "-resize", "200%", "png:-"], input=png
    )
    tsv = subprocess.check_output(
        ["tesseract", "stdin", "stdout", "--psm", "11", "tsv"], input=scaled, stderr=subprocess.DEVNULL
    ).decode()
    lines = {}
    for row in csv.DictReader(io.StringIO(tsv), delimiter="\t", quoting=csv.QUOTE_NONE):
        if row["level"] != "5" or not row["text"].strip():
            continue
        key = tuple(row[k] for k in ("block_num", "par_num", "line_num"))
        box = [int(row[k]) for k in ("left", "top", "width", "height")]
        lines.setdefault(key, []).append((row["text"], box))
    spans = []
    for words in lines.values():
        left, top = words[0][1][:2]
        right = max(b[0] + b[2] for _, b in words)
        bottom = max(b[1] + b[3] for _, b in words)
        spans.append(
            {"text": " ".join(w for w, _ in words), "x": (left + right) // 4, "y": (top + bottom) // 4}
        )
    if command == "read":
        print(json.dumps(spans, ensure_ascii=False))
    else:
        matches = [line for line in spans if line["text"].casefold() == args[0].casefold()]
        if len(matches) != 1:
            sys.exit(
                f"Expected one visible line matching {args[0]!r}, found {len(matches)}. Use read or shot for coordinates."
            )
        run(
            *xd,
            "mousemove",
            "--window",
            window,
            str(matches[0]["x"]),
            str(matches[0]["y"]),
            "sleep",
            "0.1",
            "mousedown",
            "1",
            "sleep",
            "0.08",
            "mouseup",
            "1",
        )
elif command == "shot":
    run("import", "-silent", "-window", window, "png:-")
elif command in ("click", "right-click", "move", "scroll", "drag"):
    x, y, *extra = args
    run(*xd, "mousemove", "--window", window, x, y)
    if command in ("click", "right-click"):
        button = "1" if command == "click" else "3"
        run(*xd, "sleep", "0.1", "mousedown", button, "sleep", "0.08", "mouseup", button)
    elif command == "scroll":
        direction, count = extra
        run(*xd, "click", "--repeat", count, "4" if direction == "up" else "5")
    elif command == "drag":
        run(
            *xd,
            "mousedown",
            "1",
            "sleep",
            "0.12",
            "mousemove",
            "--window",
            window,
            *extra,
            "sleep",
            "0.15",
            "mouseup",
            "1",
        )
elif command == "key":
    run(*xd, "key", "--clearmodifiers", *args)
elif command == "type":
    run(*xd, "type", "--clearmodifiers", "--", args[0])
elif command == "resize":
    run(*xd, "windowsize", window, *args)
    time.sleep(0.15)
    run(*xd, "getwindowgeometry", "--shell", window)
elif command == "size":
    run(*xd, "getwindowgeometry", "--shell", window)
else:
    sys.exit(f"Unknown action: {command}")
