#!/usr/bin/env bash
# Cutting a release: bump the version, check what was built, publish it.
#
#   scripts/release.sh prep <version> [--dry-run] [--skip-gate] [--perf]
#       Sets the version (Cargo.toml, Cargo.lock) and the Flatpak metainfo release entry, starts the
#       release notes (release-notes/<version>.md, from release-notes/TEMPLATE.md), runs the gate
#       (fmt, clippy, tests; with --perf also scripts/perf.sh), and commits "Release <version>".
#       Write the notes, then `git commit -a --amend --no-edit` before pushing the tag.
#       Refuses a dirty tree, a tree carrying the private local patch, a version that
#       is not newer than the current one, or one whose tag already exists.
#
#   scripts/release.sh verify <version> [--repo-only] [--strict]
#       Checks that the repo and everything in dist/ agree on <version>: the version in Cargo.toml,
#       the metainfo, a consistent Cargo.lock, a clean tree, a tag (if there is one) on HEAD, the
#       release notes, and that each artifact exists, is newer than HEAD's commit (so it was built
#       from it) and has the right contents. The macOS image is built on a Mac, so its absence is
#       a warning; --strict makes it a failure (CI builds it). --repo-only stops after the repo
#       checks, before any artifact is looked for.
#
#   scripts/release.sh publish <version> [--notes FILE] [--without-macos] [--yes]
#       Verifies, then tags v<version>, pushes main and the tag, and creates the GitHub release
#       with the artifacts attached, then its checksums. The notes default to
#       release-notes/<version>.md. Prints what it would do unless --yes is given. Pushing a v*
#       tag also starts .github/workflows/release.yml, which builds and publishes on its own: this
#       command is the manual fallback.
#
#   scripts/release.sh checksums <version> [--yes]
#       Downloads the published release's files and writes refryt-<version>-SHA256SUMS.txt for
#       them; with --yes it is attached to the release, replacing an older one. Run it again after
#       adding the macOS image, so the sums cover it too. Works on the Mac as well.
#
# Nothing here builds: the release workflow does (.github/workflows/release.yml), or run
# `just appimage`, `./scripts/build-windows.sh`, `./scripts/build-msi.sh`, the same two with
# `ARCH=aarch64`, `just flatpak` (and `./scripts/build-macos.sh` on the Mac) after `prep`.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

die() { echo "error: $*" >&2; exit 1; }
warn() { echo "warning: $*" >&2; }

current_version() {
    cargo metadata --no-deps --offline --format-version 1 \
        | python3 -c 'import json, sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "refryt"))'
}

# Whether Cargo.lock agrees with the manifests. Resolved for this machine's platform only: the
# full resolve wants the crates for every platform downloaded, which an offline run does not have.
lock_consistent() {
    cargo metadata --locked --offline --format-version 1 \
        --filter-platform "$(rustc -vV | sed -n 's/^host: //p')" > /dev/null 2>&1
}

valid_version() { [[ "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; }

newer_than() { # newer_than <a> <b>: a > b
    [ "$1" != "$2" ] && [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -1)" = "$1" ]
}

# Edits to tracked files make a build that is not the commit it claims to be.
need_clean_tree() {
    if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
        git status --short --untracked-files=no >&2
        die "tracked files have uncommitted changes; commit or stash them first"
    fi
}

# The private build's feature must never reach a release: the patch lives outside the repo and
# is applied only to a local tree, so a tree that has it is not a release tree.
private_patch_applied() {
    grep -rq "unlocked-downloads" --exclude=release.sh crates/refryt/Cargo.toml crates/refryt/src scripts 2>/dev/null
}

need_no_private_patch() {
    if private_patch_applied; then
        die "the private local patch is applied; remove it before a release"
    fi
}

need_branch_main() {
    local b
    b="$(git rev-parse --abbrev-ref HEAD)"
    [ "$b" = "main" ] || die "releases are cut from main, not '$b'"
}

# The CPU each .exe and .dll in a Windows zip is built for, as "<file> <machine>" lines, read from
# the PE header: 8664 is x64, aa64 is ARM64. A zip with one wrong file in it (an x64 libmpv in the
# ARM build, say) installs fine and then fails to load on the machine it was meant for.
pe_machines() { # pe_machines <zip>
    python3 -I - "$1" <<'PY'
import struct, sys, zipfile
with zipfile.ZipFile(sys.argv[1]) as z:
    for name in z.namelist():
        if name.lower().endswith((".exe", ".dll")):
            data = z.read(name)
            pe = struct.unpack_from("<I", data, 0x3C)[0]
            print(name.rsplit("/", 1)[-1], format(struct.unpack_from("<H", data, pe + 4)[0], "x"))
PY
}

artifacts() { # artifacts <version>: "<kind> <path>" lines
    local v="$1"
    echo "appimage dist/refryt-$v-x86_64.AppImage"
    echo "windows-zip dist/refryt-$v-windows-x86_64.zip"
    echo "windows-setup dist/refryt-$v-windows-x86_64-setup.exe"
    echo "windows-msi dist/refryt-$v-windows-x86_64.msi"
    echo "windows-arm64-zip dist/refryt-$v-windows-arm64.zip"
    echo "windows-arm64-msi dist/refryt-$v-windows-arm64.msi"
    echo "flatpak dist/refryt-$v-x86_64.flatpak"
    echo "macos dist/refryt-$v-macos-arm64.dmg"
}

cmd_prep() {
    local v="${1:-}"; shift || true
    local dry=0 gate=1 perf=0
    for a in "$@"; do
        case "$a" in
            --dry-run) dry=1 ;;
            --skip-gate) gate=0 ;;
            --perf) perf=1 ;;
            *) die "unknown option $a" ;;
        esac
    done
    valid_version "$v" || die "usage: release.sh prep <major.minor.patch>"
    local cur
    cur="$(current_version)"
    newer_than "$v" "$cur" || die "$v is not newer than the current version $cur"
    need_clean_tree
    need_no_private_patch
    need_branch_main
    if git rev-parse -q --verify "refs/tags/v$v" > /dev/null; then
        die "tag v$v already exists"
    fi
    local today
    today="$(date +%F)"
    echo "release $cur -> $v ($today)"
    if [ "$dry" = 1 ]; then
        echo "dry run: would set the version in Cargo.toml and Cargo.lock, add the metainfo entry,"
        echo "         run the gate$([ "$perf" = 1 ] && echo ' and perf.sh'), and commit \"Release $v\""
        return 0
    fi

    python3 - "$cur" "$v" "$today" <<'EOF'
import re, sys
cur, new, today = sys.argv[1:4]
# Cargo.toml: the workspace package's version line, and only that.
p = "Cargo.toml"
s = open(p).read()
head, sep, tail = s.partition("[workspace.package]")
assert sep, "no [workspace.package] in Cargo.toml"
tail, n = re.subn(r'^version = "%s"$' % re.escape(cur), 'version = "%s"' % new, tail, count=1, flags=re.M)
assert n == 1, "the workspace version line was not found"
open(p, "w").write(head + sep + tail)
# Flatpak metainfo: the newest release goes first.
p = "packaging/flatpak/io.github.gridoe.refryt.metainfo.xml"
s = open(p).read()
entry = '<release version="%s" date="%s"/>' % (new, today)
if 'version="%s"' % new not in s:
    s = s.replace("<releases>\n", "<releases>\n    %s\n" % entry, 1)
    assert entry in s, "no <releases> block in the metainfo"
    open(p, "w").write(s)
EOF
    # The notes start from the template, with the commits since the last release listed to write
    # from. Its marker keeps `verify` from letting an unwritten file through.
    if [ -f release-notes/TEMPLATE.md ]; then
        python3 - "$cur" "$v" <<'PY'
import subprocess, sys
cur, new = sys.argv[1:3]
text = open("release-notes/TEMPLATE.md").read().replace("@VERSION@", new).replace("@PREVIOUS@", cur)
log = subprocess.run(
    ["git", "log", "--format=%h %s", "v%s..HEAD" % cur], capture_output=True, text=True
).stdout.strip()
open("release-notes/%s.md" % new, "w").write(text.replace("@COMMITS@", log or "(none)"))
PY
    fi
    # The lock file records the workspace crates' versions; refresh just those.
    cargo update --workspace --offline > /dev/null
    [ "$(current_version)" = "$v" ] || die "the version did not change to $v"
    lock_consistent || die "Cargo.lock is not consistent with the new version"

    if [ "$gate" = 1 ]; then
        echo "== gate =="
        cargo fmt --all --check
        cargo clippy --workspace --all-targets -- -D warnings
        cargo clippy -p refryt-resolve --features native-resolver -- -D warnings
        cargo test --workspace
        [ "$perf" = 1 ] && ./scripts/perf.sh
    else
        warn "the gate was skipped"
    fi

    git add Cargo.toml Cargo.lock packaging/flatpak/io.github.gridoe.refryt.metainfo.xml
    [ ! -f "release-notes/$v.md" ] || git add "release-notes/$v.md"
    git commit -q -m "Release $v"
    echo "committed: $(git log -1 --oneline)"
    echo
    if [ -f "release-notes/$v.md" ]; then
        echo "write release-notes/$v.md, then: git commit -a --amend --no-edit"
        echo
    fi
    echo "next: push main and the tag v$v (the release workflow builds and publishes), or build the"
    echo "      artifacts by hand and run scripts/release.sh verify $v:"
    echo "      just appimage && ./scripts/build-windows.sh && ./scripts/build-msi.sh"
    echo "      ARCH=aarch64 ./scripts/build-windows.sh && ARCH=aarch64 ./scripts/build-msi.sh"
    echo "      just flatpak   (and the macOS image on a Mac)"
}

cmd_verify() {
    local v="${1:-}"; shift || true
    local repo_only=0 strict=0
    for a in "$@"; do
        case "$a" in
            --repo-only) repo_only=1 ;;
            --strict) strict=1 ;;
            *) die "unknown option $a" ;;
        esac
    done
    valid_version "$v" || die "usage: release.sh verify <major.minor.patch> [--repo-only] [--strict]"
    local errors=0 warnings=0
    bad() { echo "  FAIL  $*"; errors=$((errors + 1)); }
    ok() { echo "  ok    $*"; }
    meh() { echo "  warn  $*"; warnings=$((warnings + 1)); }

    echo "== repository =="
    [ "$(current_version)" = "$v" ] && ok "Cargo.toml is $v" || bad "Cargo.toml is $(current_version), not $v"
    grep -q "<release version=\"$v\"" packaging/flatpak/io.github.gridoe.refryt.metainfo.xml \
        && ok "the Flatpak metainfo lists $v" || bad "the Flatpak metainfo has no release entry for $v"
    lock_consistent && ok "Cargo.lock matches" || bad "Cargo.lock is out of step with Cargo.toml"
    if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
        bad "tracked files have uncommitted changes"
    else
        ok "the tree is clean"
    fi
    if private_patch_applied; then
        bad "the private local patch is applied"
    else
        ok "no private patch"
    fi
    local head_time
    head_time="$(git log -1 --format=%ct)"
    if git rev-parse -q --verify "refs/tags/v$v" > /dev/null; then
        [ "$(git rev-parse "v$v^{commit}")" = "$(git rev-parse HEAD)" ] \
            && ok "tag v$v is HEAD" || bad "tag v$v is not HEAD"
    else
        ok "tag v$v not made yet"
    fi

    # The notes are read by the release workflow and published as they are, so a missing file or
    # the template's marker still in it stops the release here, not after twenty minutes of builds.
    # Releases before the notes folder existed have none; TEMPLATE.md says it is in use.
    if [ -f release-notes/TEMPLATE.md ]; then
        if [ ! -s "release-notes/$v.md" ]; then
            bad "release-notes/$v.md is missing"
        elif grep -q "TODO: write" "release-notes/$v.md"; then
            bad "release-notes/$v.md still has its TODO marker"
        else
            ok "release-notes/$v.md is written"
        fi
    fi
    if [ "$repo_only" = 1 ]; then
        echo
        echo "$errors failure(s), $warnings warning(s)"
        [ "$errors" -eq 0 ]
        return
    fi

    echo "== artifacts =="
    local kind path
    while read -r kind path; do
        if [ ! -f "$path" ]; then
            if [ "$kind" = macos ] && [ "$strict" = 0 ]; then
                meh "$path is missing: build it on the Mac (scripts/build-macos.sh) and copy it into dist/"
            else
                bad "$path is missing"
            fi
            continue
        fi
        local size mtime
        size="$(du -h "$path" | cut -f1)"
        mtime="$(stat -c %Y "$path")"
        if [ "$mtime" -lt "$head_time" ]; then
            bad "$path ($size) is older than HEAD's commit: rebuild it"
            continue
        fi
        case "$kind" in
            windows-zip | windows-arm64-zip)
                local list want machines wrong
                list="$(unzip -l "$path" 2>/dev/null)" || { bad "$path is not a readable zip"; continue; }
                if ! grep -q "refryt.exe" <<<"$list" || ! grep -q "libmpv-2.dll" <<<"$list"; then
                    bad "$path ($size) is missing refryt.exe or libmpv-2.dll"
                    continue
                fi
                [ "$kind" = windows-zip ] && want=8664 || want=aa64
                machines="$(pe_machines "$path")" || { bad "$path: could not read its executables"; continue; }
                wrong="$(awk -v want="$want" '$2 != want {print $1}' <<<"$machines" | paste -sd ' ')"
                [ -z "$wrong" ] \
                    && ok "$path ($size): refryt.exe and libmpv-2.dll inside, all $([ "$want" = 8664 ] && echo x64 || echo ARM64)" \
                    || bad "$path ($size): built for the wrong CPU: $wrong" ;;
            windows-msi | windows-arm64-msi)
                # The version inside has to be this one: an MSI built from an older zip upgrades
                # nothing and installs the wrong release. The platform is what Windows checks
                # before installing: the ARM64 one is stamped after wixl builds it as x64.
                if command -v msiinfo > /dev/null; then
                    local msiver platform want
                    msiver="$(msiinfo export "$path" Property 2>/dev/null \
                        | awk -F'\t' '$1 == "ProductVersion" {print $2}' | tr -d '\r')"
                    platform="$(msiinfo suminfo "$path" 2>/dev/null | sed -n 's/^Template: \([^;]*\);.*/\1/p')"
                    [ "$kind" = windows-msi ] && want=x64 || want=Arm64
                    if [ "$msiver" != "$v" ]; then
                        bad "$path ($size) says ProductVersion '$msiver', not $v"
                    elif [ "$platform" != "$want" ]; then
                        bad "$path ($size) is for platform '$platform', not $want"
                    else
                        ok "$path ($size): ProductVersion $msiver, $platform"
                    fi
                else
                    meh "$path ($size): msiinfo not installed, version inside not checked"
                fi ;;
            *) ok "$path ($size)" ;;
        esac
    done < <(artifacts "$v")

    echo
    echo "$errors failure(s), $warnings warning(s)"
    [ "$errors" -eq 0 ]
}

cmd_publish() {
    local v="${1:-}"; shift || true
    local notes="" yes=0 nomac=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --notes) notes="${2:-}"; shift ;;
            --yes) yes=1 ;;
            --without-macos) nomac=1 ;;
            *) die "unknown option $1" ;;
        esac
        shift
    done
    valid_version "$v" || die "usage: release.sh publish <version> [--notes FILE] [--yes]"
    [ -n "$notes" ] || notes="release-notes/$v.md"
    [ -f "$notes" ] || die "no notes file: write $notes or pass --notes"
    need_branch_main
    cmd_verify "$v" || die "verify failed; fix that first"
    local assets=() kind path
    while read -r kind path; do
        if [ -f "$path" ]; then
            assets+=("$path")
        elif [ "$kind" = macos ] && [ "$nomac" = 0 ]; then
            die "the macOS image is missing; build it on the Mac, or pass --without-macos"
        fi
    done < <(artifacts "$v")
    command -v gh > /dev/null || die "gh is needed to create the release"

    local tag_cmd=(git tag -a "v$v" -m "refryt $v")
    local push_cmd=(git push origin main "v$v")
    local release_cmd=(gh release create "v$v" "${assets[@]}" --title "refryt $v" --notes-file "$notes")
    if git rev-parse -q --verify "refs/tags/v$v" > /dev/null; then
        tag_cmd=(true)
    fi
    echo
    echo "== publish =="
    printf '  %q ' "${tag_cmd[@]}"; echo
    printf '  %q ' "${push_cmd[@]}"; echo
    printf '  %q ' "${release_cmd[@]}"; echo
    if [ "$yes" != 1 ]; then
        echo
        echo "dry run: nothing was tagged, pushed or published. Run again with --yes to do it."
        return 0
    fi
    "${tag_cmd[@]}"
    "${push_cmd[@]}"
    "${release_cmd[@]}"
    cmd_checksums "$v" --yes
}

# A SHA-256 of each published file, so a download can be checked with `sha256sum -c`. Read from
# the release itself rather than dist/: the macOS image is added from another machine, and the
# sums must cover what people actually download.
cmd_checksums() {
    local v="${1:-}"; shift || true
    local yes=0
    for a in "$@"; do
        case "$a" in
            --yes) yes=1 ;;
            *) die "unknown option $a" ;;
        esac
    done
    valid_version "$v" || die "usage: release.sh checksums <version> [--yes]"
    command -v gh > /dev/null || die "gh is needed to read the release"
    local sums="refryt-$v-SHA256SUMS.txt" tmp
    tmp="$(mktemp -d)"
    trap "rm -rf '$tmp'; trap - RETURN" RETURN
    # The files in one folder, the sums in another: written beside them, the sums file would be
    # created before the list is read and end up hashing itself.
    mkdir "$tmp/files" "$tmp/out"
    gh release download "v$v" --dir "$tmp/files" --pattern 'refryt-*' > /dev/null
    rm -f "$tmp/files/$sums"
    [ -n "$(ls -A "$tmp/files")" ] || die "release v$v has no files"
    if command -v sha256sum > /dev/null; then
        (cd "$tmp/files" && sha256sum refryt-*) > "$tmp/out/$sums"
    else
        (cd "$tmp/files" && shasum -a 256 refryt-*) > "$tmp/out/$sums"
    fi
    cat "$tmp/out/$sums"
    if [ "$yes" != 1 ]; then
        echo
        echo "dry run: not attached. Run again with --yes to attach $sums to v$v."
        return 0
    fi
    gh release upload "v$v" "$tmp/out/$sums" --clobber
    echo "attached $sums to v$v"
}

case "${1:-}" in
    prep) shift; cmd_prep "$@" ;;
    verify) shift; cmd_verify "$@" ;;
    publish) shift; cmd_publish "$@" ;;
    checksums) shift; cmd_checksums "$@" ;;
    *) sed -n '2,34p' "$0" | sed 's/^# \{0,1\}//'; exit 1 ;;
esac
