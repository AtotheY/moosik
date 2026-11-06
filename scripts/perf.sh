#!/usr/bin/env bash
# Measure cold start, idle memory, and — the part that matters — a *loaded* app.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

# Gates from the plan. These numbers are the product, so they are asserted, not just printed.
MAX_START_MS=300
# Not RSS: see below for why RSS is meaningless for a GPU-accelerated app.
#
# Private dirty is the number that tracks *our* code, and it is the one to watch. PSS adds our
# proportional share of every shared library, which moves with whatever else on the machine
# happens to map the same pages — it drifted 69→85 MB across milestones that allocated nothing
# extra, and on 2026-09-08 an experiment that *removed* work (no artwork in the rail) measured
# 88 MB against 84–86 MB with the work left in. It is kept because a real leak would still show
# up in it, but its gate is set where noise cannot trip it rather than tight enough to be
# meaningful on its own.
#
# 2026-09-10 — every memory gate was raised by ~15 MB in one deliberate step, and this is the
# record of why. The Nvidia module and kernel were upgraded on 2026-09-09 and only took effect
# with the reboot on 2026-09-10 11:11; the same binary that measured 62 MB idle on the old
# module measures 77 MB on the new one. The delta is the driver, not us: of those 77 MB, 53 MB
# is anonymous mappings held by libnvidia/LLVM and the heap is 5.6 MB (smaps, by mapping). The
# AppImage's older code, run against system libs on this same boot, measures 90 MB — higher
# than this tree — so nothing here regressed. The numbers below are today's measured best plus
# the old gates' headroom, and they exist to catch an M20-era leak, not to re-litigate the
# driver.
#
# 2026-09-10 (M21) — the two idle gates sat inside the driver's own run-to-run spread and were
# tripping on noise alone: four launches of unchanged binaries measured 77, 82 and 90 MB idle
# private against a <90 gate, and 112–119 MB PSS against a <118 gate. Raised to 100/125, which
# still catches a real leak (tens of MB) while leaving the driver's ±10 MB out of it.
MAX_PRIVATE_MB=100
MAX_PSS_MB=125
# The app as it actually starts for a signed-in user: the rail fetches your playlists
# immediately (it used to wait for the session round trip, which only ever meant an empty
# sidebar for a second). Parsing that browse response costs ~15 MB that the allocator does not
# hand back — measured 2026-09-09 by starting with the fetch switched off, which lands on the
# floor below: 43 MB against 57. That is real, it is ours, and it is the number the user's
# machine actually holds, so it gets a gate of its own rather than being smuggled into the
# floor's.
MAX_RAIL_PRIVATE_MB=122
# After browsing. The idle gates above would pass no matter how badly the art cache behaved,
# which is exactly how the M3 regressions got in unnoticed.
MAX_LOADED_PRIVATE_MB=125
MAX_LOADED_PSS_MB=158
MAX_TEXTURE_MB=32
MAX_FRAME_MS=8

cargo build --release -p moosik --bin moosik
bin=./target/release/moosik

# Time to first painted frame. The app reports it itself: everything before the first frame is
# invisible to the user and everything after it is not startup.
echo "=== cold start ==="
best=99999
for _ in 1 2 3; do
  ms=$("$bin" --startup-timing 2>/dev/null | grep -m1 first_frame_ms | cut -d= -f2)
  ms=${ms%.*}
  [ -n "$ms" ] && [ "$ms" -lt "$best" ] && best=$ms
done
printf "  first frame     : %4s ms   (gate < %s ms)\n" "$best" "$MAX_START_MS"

# Sample a running pid's memory. RSS is misleading for any GPU app: the GL driver maps tens of MB
# of shared libraries into every process that opens a context. glxgears — which draws three gears
# — costs ~64MB RSS on this box. So we gate on what is actually ours:
#   Private_Dirty = pages only we hold and nobody can reclaim
#   PSS           = our proportional share of everything, shared libs included
sample() {
  awk '/^Rss:/{r=int($2/1024)} /^Pss:/{p=int($2/1024)} /^Private_Dirty:/{d+=int($2/1024)}
       END{print r, p, d}' "/proc/$1/smaps_rollup" 2>/dev/null || echo "0 0 0"
}

# PSS is our *proportional* share of shared libraries, so it moves with how many other GL
# processes happen to be alive — a single sample has been seen to vary by 7MB run to run for an
# unchanged binary. Take the best of three, as the cold-start measurement already does.
# `--page search` is the app with nothing fetched: the floor of the UI plus the audio stack. The
# default launch now lands on Home and loads a feed, which is a different (also gated) number.
# Signed out on purpose — a scratch config directory, so no stored cookie is found. This phase
# is meant to be the floor of the UI plus the audio stack with nothing fetched, and once the
# rail loads at startup a signed-in run is not that: it measured the floor plus one browse and
# tripped its own gate. The signed-in figure is gated below instead, and this one keeps
# measuring what it says it does.
echo "=== memory (idle, nothing fetched) ==="
scratch_cfg=$(mktemp -d)
trap 'rm -rf "$scratch_cfg"' EXIT
rss=0 pss=0 priv=0
for _ in 1 2 3; do
  XDG_CONFIG_HOME="$scratch_cfg" "$bin" --page search >/dev/null 2>&1 &
  pid=$!
  for _ in $(seq 1 100); do grep -q . /proc/$pid/smaps_rollup 2>/dev/null && break; sleep 0.05; done
  sleep 2
  read -r r p d <<<"$(sample $pid)"
  kill $pid 2>/dev/null || true
  wait $pid 2>/dev/null || true
  if [ "$pss" -eq 0 ] || { [ "$p" -gt 0 ] && [ "$p" -lt "$pss" ]; }; then
    rss=$r pss=$p priv=$d
  fi
done

printf "  private dirty   : %4s MB   (gate < %s MB)  <- ours alone\n" "$priv" "$MAX_PRIVATE_MB"
printf "  proportional set: %4s MB   (gate < %s MB)  <- our share of shared libs too\n" "$pss" "$MAX_PSS_MB"
printf "  resident        : %4s MB   (no gate: GL driver dominates)\n" "$rss"

if command -v glxgears >/dev/null; then
  glxgears >/dev/null 2>&1 & g=$!
  sleep 2
  base=$(awk '/VmRSS/{print int($2/1024)}' /proc/$g/status 2>/dev/null || echo 0)
  kill $g 2>/dev/null || true; wait $g 2>/dev/null || true
  printf "  (glxgears on this machine: %s MB resident, for scale)\n" "$base"
fi

# The same page, with whatever session is on this machine: what the app holds after the rail
# has loaded. Signed out it lands on the floor above, which passes; signed in it is the number
# a user actually pays at rest.
echo "=== memory (idle, signed in as configured) ==="
"$bin" --page search >/dev/null 2>&1 &
pid=$!
for _ in $(seq 1 100); do grep -q . /proc/$pid/smaps_rollup 2>/dev/null && break; sleep 0.05; done
sleep 8
read -r _rrss _rpss rail_priv <<<"$(sample $pid)"
kill $pid 2>/dev/null || true
wait $pid 2>/dev/null || true
printf "  private dirty   : %4s MB   (gate < %s MB)  <- rail loaded\n" "$rail_priv" "$MAX_RAIL_PRIVATE_MB"

# The measurement the idle numbers cannot make. Load a real page, let the covers land, then read
# both the app's own counters and its memory. `rows_drawn` vs `rows_total` is the virtualisation
# canary: if they are equal on a long list, a list stopped being virtualised.
#
# The default launch lands on Home, which is the heaviest page for cover art — five shelves of
# cards — so it is the one worth gating.
echo "=== loaded (home feed + covers) ==="
stats_file=$(mktemp)
"$bin" --stats-after 8 >"$stats_file" 2>/dev/null &
pid=$!
for _ in $(seq 1 100); do grep -q . /proc/$pid/smaps_rollup 2>/dev/null && break; sleep 0.05; done
sleep 7
read -r lrss lpss lpriv <<<"$(sample $pid)"
wait $pid 2>/dev/null || true

line=$(grep -m1 '^stats ' "$stats_file" || true)
rm -f "$stats_file"
get() { echo "$line" | tr ' ' '\n' | grep -m1 "^$1=" | cut -d= -f2; }
frame_ms=$(get frame_ms); textures=$(get textures); tbytes=$(get texture_bytes)
rows_drawn=$(get rows_drawn); rows_total=$(get rows_total)
: "${frame_ms:=0}" "${textures:=0}" "${tbytes:=0}" "${rows_drawn:=0}" "${rows_total:=0}"
tmb=$(( tbytes / 1048576 ))

printf "  frame cost      : %6s ms   (gate < %s ms)\n" "$frame_ms" "$MAX_FRAME_MS"
printf "  cover textures  : %4s     %s MB  (gate < %s MB)\n" "$textures" "$tmb" "$MAX_TEXTURE_MB"
printf "  rows drawn      : %4s of %s   <- equal on a long list means no virtualisation\n" \
  "$rows_drawn" "$rows_total"
printf "  private dirty   : %4s MB   (gate < %s MB)  <- ours alone\n" "$lpriv" "$MAX_LOADED_PRIVATE_MB"
printf "  proportional set: %4s MB   (gate < %s MB)\n" "$lpss" "$MAX_LOADED_PSS_MB"
printf "  resident        : %4s MB\n" "$lrss"

fail=0
note() { echo "  FAIL: $1"; fail=1; }
[ "$best" -lt "$MAX_START_MS" ] || note "slow first frame"
{ [ "$priv" -gt 0 ] && [ "$priv" -lt "$MAX_PRIVATE_MB" ]; } || note "idle private memory"
{ [ "$pss" -gt 0 ] && [ "$pss" -lt "$MAX_PSS_MB" ]; } || note "idle proportional memory"
{ [ "$rail_priv" -gt 0 ] && [ "$rail_priv" -lt "$MAX_RAIL_PRIVATE_MB" ]; } \
  || note "private memory with the rail loaded"
[ -n "$line" ] || note "the app reported no stats line"
{ [ "$lpriv" -gt 0 ] && [ "$lpriv" -lt "$MAX_LOADED_PRIVATE_MB" ]; } || note "loaded private memory"
{ [ "$lpss" -gt 0 ] && [ "$lpss" -lt "$MAX_LOADED_PSS_MB" ]; } || note "loaded proportional memory"
[ "$tmb" -lt "$MAX_TEXTURE_MB" ] || note "texture cache over budget"
awk -v v="$frame_ms" -v m="$MAX_FRAME_MS" 'BEGIN{exit !(v>0 && v<m)}' || note "frame cost $frame_ms ms"
[ "$fail" -eq 0 ] && echo "  PASS"
exit $fail
