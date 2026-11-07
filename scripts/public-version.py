"""Choose the next public tag and stamp that version into a build checkout."""
import argparse
from pathlib import Path
import re
import subprocess
import tomllib


def semver(text):
    if not re.fullmatch(r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)", text):
        raise ValueError(f"Invalid public version {text!r}")
    return tuple(map(int, text.split(".")))


def tag_version(tag):
    if not tag.startswith("moosik-v"):
        raise ValueError(f"Unexpected public tag {tag!r}")
    version = tag.removeprefix("moosik-v")
    semver(version)
    return version


def stamp(version, tag):
    manifest = Path("Cargo.toml")
    old = tomllib.loads(manifest.read_text())["workspace"]["package"]["version"]
    if semver(version) < semver(old):
        raise ValueError(f"Tag {version} is older than workspace {old}")
    if version != old:
        text, count = re.subn(
            rf'(?ms)(^\[workspace\.package\]\s*\n(?:(?!^\[).)*?^version = "){re.escape(old)}(")$',
            rf'\g<1>{version}\2', manifest.read_text(), count=1)
        if count != 1:
            raise ValueError("Workspace version not found")
        manifest.write_text(text)
        lock = Path("Cargo.lock")
        blocks = lock.read_text().split("[[package]]")
        for index, block in enumerate(blocks[1:], 1):
            package = tomllib.loads(block)
            if "source" not in package and package["version"] == old:
                blocks[index] = block.replace(f'version = "{old}"', f'version = "{version}"', 1)
        lock.write_text("[[package]]".join(blocks))
    notes = Path("release-notes") / f"{version}.md"
    if not notes.is_file():
        if subprocess.check_output(["git", "cat-file", "-t", tag], text=True).strip() != "tag":
            raise ValueError(f"Missing notes for {tag}")
        body = subprocess.check_output(["git", "for-each-ref", "--format=%(contents)", f"refs/tags/{tag}"], text=True).strip()
        if not body:
            raise ValueError(f"Empty notes for {tag}")
        notes.write_text(body + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["next", "stamp"])
    parser.add_argument("tag")
    args = parser.parse_args()
    version = tag_version(args.tag)
    if args.action == "stamp":
        stamp(version, args.tag)
    else:
        major, minor, patch = semver(version)
        workspace = tomllib.loads(Path("Cargo.toml").read_text())["workspace"]["package"]["version"]
        print(workspace if semver(workspace) > semver(version) else f"{major}.{minor}.{patch+1}")


if __name__ == "__main__":
    main()
