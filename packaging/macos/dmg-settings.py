from pathlib import Path

app = Path(defines["app"])
files = [str(app)]
symlinks = {"Applications": "/Applications"}
format = "UDZO"

# Save a manual Finder layout so alphabetical sorting can't reverse the gesture.
default_view = "icon-view"
arrange_by = None
window_rect = ((120, 120), (400, 180))
icon_size = 56
text_size = 13
icon_locations = {app.name: (100, 72), "Applications": (300, 72)}
