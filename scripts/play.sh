#!/usr/bin/env bash
# Resolve a YouTube video id and play it with mpv.
#
# A stand-in for the real player until M2 links libmpv. Useful for checking by ear that a
# track resolves and decodes end to end.
#
#   scripts/play.sh dQw4w9WgXcQ
#   scripts/play.sh dQw4w9WgXcQ --start=60 --length=15
set -euo pipefail

id="${1:?usage: scripts/play.sh <video_id> [extra mpv args...]}"
shift

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
spike="$root/target/release/resolve-spike"
[[ -x "$spike" ]] || cargo build --release -p moosik-resolve --bin resolve-spike

url="$(SPIKE_PRINT_URL=1 "$spike" "$id" 2>/dev/null | sed -n 's/^  url  *//p')"
[[ -n "$url" ]] || { echo "could not resolve $id" >&2; exit 1; }

# --no-video: audio only. mpv does ranged chunked reads by default, which YouTube requires.
exec mpv --no-video --term-osd-bar --msg-level=all=error,cplayer=info "$@" "$url"
