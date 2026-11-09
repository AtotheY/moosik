"""Publish one verified platform while holding the workflow's release-edit lock."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tempfile

PREFIX = "moosik-v"
FILES = {
    "macos": ("moosik-{}-macos-arm64.dmg", "moosik-{}-macos-arm64.zip", "appcast.xml"),
    "windows": ("moosik-{}-windows-x86_64-setup.exe", "moosik-{}-windows-x86_64.zip"),
    "linux": ("moosik-{}-linux-amd64.deb",),
}


def version_of(tag):
    if not re.fullmatch(r"moosik-v\d+\.\d+\.\d+", tag):
        raise ValueError(f"Unexpected release tag {tag!r}")
    return tuple(map(int, tag[len(PREFIX):].split(".")))


def gh(*args):
    return subprocess.check_output(["gh", "release", *args], text=True)


def view(*tag):
    try:
        return json.loads(subprocess.check_output(
            ["gh", "release", "view", *tag, "--json", "tagName,isDraft,assets"],
            text=True, stderr=subprocess.PIPE,
        ))
    except subprocess.CalledProcessError as error:
        if error.stderr.strip() in ("release not found", "no releases found"):
            return None
        raise RuntimeError(error.stderr.strip()) from error


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def asset_hashes(release):
    return {a["name"]: a["digest"].removeprefix("sha256:")
            for a in release["assets"] if a["state"].lower() == "uploaded"}


def upload_changed(tag, path, remote):
    checksum = digest(path)
    if remote.get(path.name) != checksum:
        gh("upload", tag, str(path), "--clobber")
        remote[path.name] = checksum


def manifest_text(files):
    return "".join(f"{checksum}  {name}\n" for name, checksum in sorted(files.items()))


def publish(tag, platform, dist, temp):
    number = version_of(tag)
    version = tag[len(PREFIX):]
    combined = f"moosik-{version}-SHA256SUMS.txt"
    expected = [pattern.format(version) for pattern in FILES[platform]]
    verified = {name: digest(dist / name) for name in expected}
    source = dist / (combined if platform == "macos" else f"moosik-{platform}-SHA256SUMS.txt")
    # Windows sha256sum writes a binary-mode asterisk before the filename.
    lines = [line.replace(" *", "  ", 1) for line in source.read_text().splitlines()]
    if sorted(lines) != sorted(manifest_text(verified).splitlines()):
        raise ValueError(f"Unexpected files or checksums in {source}")

    release = view(tag)
    if release is None:
        gh("create", tag, "--verify-tag", "--draft", "--title", f"Moosik {version}",
           "--notes-file", f"release-notes/{version}.md")
        release = view(tag)
    remote = asset_hashes(release)
    marker = temp / f"moosik-{version}-{platform}-SHA256SUMS.txt"
    # A completed platform is immutable even when a fresh build of the tag has
    # different signing timestamps. An interrupted upload has no completion marker.
    if marker.name not in remote:
        for name in expected:
            upload_changed(tag, dist / name, remote)
        marker.write_text(manifest_text(verified))
    else:
        recorded = {name: remote[name] for name in expected}
        marker.write_text(manifest_text(recorded))
        if digest(marker) != remote[marker.name]:
            raise ValueError(f"Published {platform} files differ from their recorded checksums")

    # Preserve the version-wide manifest used by already-installed Windows clients.
    files = {}
    for os_name, patterns in FILES.items():
        if os_name == platform or f"moosik-{version}-{os_name}-SHA256SUMS.txt" in remote:
            for pattern in patterns:
                name = pattern.format(version)
                files[name] = remote[name]
    sums = temp / combined
    sums.write_text(manifest_text(files))
    upload_changed(tag, sums, remote)
    upload_changed(tag, marker, remote)

    # /releases/latest/download/appcast.xml is baked into existing Mac bundles.
    # Publish other platforms immediately without moving that URL to a missing feed.
    latest = view()
    promote = platform == "macos" and (latest is None or number > version_of(latest["tagName"]))
    if release["isDraft"] or promote:
        gh("edit", tag, "--draft=false", "--latest=" + str(promote).lower())


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("tag")
    parser.add_argument("platform", choices=FILES)
    parser.add_argument("dist", type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory() as tmp:
        publish(args.tag, args.platform, args.dist, Path(tmp))
