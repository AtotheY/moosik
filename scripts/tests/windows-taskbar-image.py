"""Compare the selected art to the visible Explorer taskbar on an interactive Windows runner."""
import ctypes
from ctypes import wintypes
from pathlib import Path
import sys

from PIL import Image, ImageGrab

user32 = ctypes.windll.user32
user32.SetProcessDPIAware()
user32.FindWindowW.restype = wintypes.HWND
bar = user32.FindWindowW("Shell_TrayWnd", None)
rect = wintypes.RECT()
if not bar or not user32.GetWindowRect(wintypes.HWND(bar), ctypes.byref(rect)):
    print("No Explorer taskbar in this runner session; only shell metadata was verified.")
    sys.exit(0)

shot = ImageGrab.grab((rect.left, rect.top, rect.right, rect.bottom), all_screens=True).convert("RGB")
shot.save(Path(sys.argv[2]))
# Match opaque interior samples, ignoring the button background, active underline, and padding.
icon = Image.open(sys.argv[1]).convert("RGBA")
samples = [(x, y) for y in (.25, .375, .5, .625, .75) for x in (.25, .375, .5, .625, .75)]
def distance(a, b):
    return sum(abs(x-y) for x, y in zip(a, b)) / 3

best = float("inf")
pixels = shot.load()
for size in (16, 20, 24, 28, 30, 32, 36, 40, 48, 56, 64):
    if size > min(shot.size):
        continue
    art = icon.resize((size, size), Image.Resampling.LANCZOS)
    points = [(int(x*size), int(y*size)) for x, y in samples]
    refs = [(x, y, art.getpixel((x, y))[:3]) for x, y in points]
    center = art.getpixel((size//2, size//2))[:3]
    for top in range(shot.height-size+1):
        for left in range(shot.width-size+1):
            if distance(center, pixels[left+size//2, top+size//2]) > 23:
                continue
            error = sum(distance(rgb, pixels[left+x, top+y]) for x, y, rgb in refs)/len(refs)
            best = min(error, best)
if best > 16:
    sys.exit(f"Selected artwork was not visible on the Explorer taskbar (best RGB error {best:.1f})")
print(f"Verified selected artwork on Explorer's taskbar (RGB error {best:.1f}).")
