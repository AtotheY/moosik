#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
project="$(dirname "$root")"
export CARGO_HOME="$project/toolchain/cargo"
export RUSTUP_HOME="$project/toolchain/rustup"
export PATH="$CARGO_HOME/bin:$PATH"
frameworks="$project/Refryt.app/Contents/Frameworks"
mkdir -p "$root/.ark-build/lib"
ln -sf "$frameworks/libmpv.2.dylib" "$root/.ark-build/lib/libmpv.dylib"
export RUSTFLAGS="-L native=$root/.ark-build/lib"
export DYLD_LIBRARY_PATH="$frameworks"
cd "$root"
exec cargo "$@"
