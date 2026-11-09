#!/bin/bash
# Runs inside the dedicated Linux guest. Read a source snapshot on stdin.
set -euo pipefail
session=$1
work="$HOME/moosik-build/current"
mkdir -m 700 -p "$work" "$session/src"
tar -xf - -C "$session/src"
exec 9>"$work/build.lock"
flock 9
mkdir -p "$work/src"
rsync -rcpl --delete "$session/src/" "$work/src/"
rm -rf "$session/src"
if ! test -x "$HOME/.cargo/bin/cargo"; then
    curl --proto '=https' --tlsv1.2 --fail -sS https://sh.rustup.rs -o "$session/rustup"
    sh "$session/rustup" -y --profile minimal --no-modify-path
fi
cd "$work/src"
# Content checksums and one locked source directory let worktrees share compiled dependencies.
export CARGO_TARGET_DIR="$work/target"
export CARGO_PROFILE_DEV_DEBUG=0
"$HOME/.cargo/bin/cargo" build --locked -p moosik --bin moosik --config 'profile.dev.package."*".opt-level=0'
# A running session keeps its own executable when a later build replaces Cargo's output.
cp --reflink=auto "$CARGO_TARGET_DIR/debug/moosik" "$session/moosik"
