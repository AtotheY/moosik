#!/usr/bin/env python3
"""Write the Flathub copy of the Flatpak manifest for a released version.

    scripts/flathub-manifest.py 0.1.14 [--out dist/flathub]
    scripts/flathub-manifest.py 0.1.14 --test-ref HEAD   # a local test build, before the tag

The repo's own manifest (packaging/flatpak/io.github.gridoe.refryt.yml) builds from this checkout
with the network on, for the GitHub release. Flathub builds without a network and from a fixed
source, so the copy written here differs in three places:

- refryt is built from the git tag v<version> and its commit, not from the checkout;
- cargo gets every crate from cargo-sources.json, made from that tag's Cargo.lock by
  flatpak-cargo-generator (flatpak/flatpak-builder-tools), and builds with --offline;
- yt-dlp is upstream's Python zipapp (the runtime has Python), pinned by checksum, with
  x-checker-data so Flathub's bot proposes each new yt-dlp release.

Written to the output folder: the manifest, cargo-sources.json and flathub.json. They go in the
app's Flathub repo, which also carries flathub/shared-modules as a submodule at the revision
scripts/build-flatpak.sh pins.

Needs PyYAML, git, network access to GitHub, and the generator: FLATPAK_CARGO_GENERATOR names
flatpak-cargo-generator.py (default ~/.cache/flatpak-builder-tools/cargo/...), run with
FLATPAK_CARGO_PYTHON (default python3), which needs aiohttp and tomlkit.
"""

import argparse
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import urllib.request
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent
APP_ID = "io.github.gridoe.refryt"
MANIFEST = ROOT / "packaging" / "flatpak" / f"{APP_ID}.yml"
REPO_URL = "https://github.com/gridoe/refryt.git"
YT_DLP_LATEST = "https://api.github.com/repos/yt-dlp/yt-dlp/releases/latest"


def git(*args: str) -> str:
    return subprocess.run(["git", *args], cwd=ROOT, check=True, capture_output=True, text=True).stdout


def block_strings(dumper: yaml.SafeDumper, text: str) -> yaml.ScalarNode:
    """Multi-line strings (the icon loop) as YAML blocks, as in the repo's manifest."""
    style = "|" if "\n" in text else None
    return dumper.represent_scalar("tag:yaml.org,2002:str", text, style=style)


yaml.SafeDumper.add_representer(str, block_strings)


def module(manifest: dict, name: str) -> dict:
    found = [m for m in manifest["modules"] if isinstance(m, dict) and m.get("name") == name]
    if len(found) != 1:
        sys.exit(f"the manifest has {len(found)} modules named {name}")
    return found[0]


def yt_dlp_source() -> dict:
    """The newest yt-dlp zipapp as a pinned file source."""
    request = urllib.request.Request(YT_DLP_LATEST, headers={"User-Agent": "refryt-flathub-manifest"})
    with urllib.request.urlopen(request) as response:
        release = json.load(response)
    asset = next(a for a in release["assets"] if a["name"] == "yt-dlp")
    with urllib.request.urlopen(asset["browser_download_url"]) as response:
        sha256 = hashlib.sha256(response.read()).hexdigest()
    return {
        "type": "file",
        "url": asset["browser_download_url"],
        "sha256": sha256,
        "dest-filename": "yt-dlp",
        "x-checker-data": {
            "type": "json",
            "url": YT_DLP_LATEST,
            "version-query": ".tag_name",
            "url-query": '.assets[] | select(.name == "yt-dlp") | .browser_download_url',
        },
    }


def cargo_sources(tag: str, out: Path) -> None:
    generator = os.environ.get(
        "FLATPAK_CARGO_GENERATOR",
        str(Path.home() / ".cache/flatpak-builder-tools/cargo/flatpak-cargo-generator.py"),
    )
    python = os.environ.get("FLATPAK_CARGO_PYTHON", "python3")
    if not Path(generator).is_file():
        sys.exit(f"no flatpak-cargo-generator at {generator}; set FLATPAK_CARGO_GENERATOR")
    with tempfile.TemporaryDirectory() as tmp:
        lock = Path(tmp) / "Cargo.lock"
        lock.write_text(git("show", f"{tag}:Cargo.lock"))
        subprocess.run([python, "-I", generator, str(lock), "-o", str(out)], check=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("version")
    parser.add_argument("--out", type=Path, default=ROOT / "dist" / "flathub")
    parser.add_argument(
        "--test-ref",
        help="build this local commit instead of the tag, from this checkout: for a test build only",
    )
    args = parser.parse_args()
    tag = f"v{args.version}"
    ref = args.test_ref or tag
    try:
        commit = git("rev-parse", f"{ref}^{{commit}}").strip()
    except subprocess.CalledProcessError:
        sys.exit(f"no {ref}: Flathub builds a released version, so tag it first")

    # The manifest as it was at the tag, so the copy matches what was released.
    manifest = yaml.safe_load(git("show", f"{commit}:{MANIFEST.relative_to(ROOT)}"))

    refryt = module(manifest, "refryt")
    refryt.get("build-options", {}).pop("build-args", None)
    if not refryt.get("build-options"):
        refryt.pop("build-options", None)
    refryt["build-commands"] = [
        c.replace("cargo build ", "cargo --offline build ", 1) for c in refryt["build-commands"]
    ]
    source = {"type": "git", "url": REPO_URL, "tag": tag, "commit": commit}
    if args.test_ref:
        # A sandboxed build reads sources only from inside the manifest's folder, so the test
        # builds from a bare clone of this checkout put there.
        clone = args.out / "refryt.git"
        subprocess.run(["rm", "-rf", str(clone)], check=True)
        args.out.mkdir(parents=True, exist_ok=True)
        subprocess.run(["git", "clone", "-q", "--bare", str(ROOT), str(clone)], check=True)
        source = {"type": "git", "path": "refryt.git", "commit": commit}
    refryt["sources"] = [source, "cargo-sources.json"]

    yt_dlp = module(manifest, "yt-dlp")
    yt_dlp["sources"] = [yt_dlp_source()]

    args.out.mkdir(parents=True, exist_ok=True)
    header = (
        f"# Flathub's manifest for refryt {args.version}, written by scripts/flathub-manifest.py\n"
        f"# from packaging/flatpak/{APP_ID}.yml at {ref}. Edit that one, not this.\n"
    )
    text = yaml.safe_dump(manifest, sort_keys=False, allow_unicode=True, width=100)
    (args.out / f"{APP_ID}.yml").write_text(header + text)
    # Only x86_64 has been built and tried; aarch64 can be added once it has.
    (args.out / "flathub.json").write_text(json.dumps({"only-arches": ["x86_64"]}, indent=2) + "\n")
    cargo_sources(commit, args.out / "cargo-sources.json")
    print(f"wrote {args.out}/: {APP_ID}.yml, cargo-sources.json, flathub.json ({ref} = {commit[:7]})")


if __name__ == "__main__":
    main()
