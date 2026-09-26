#!/usr/bin/env bash
# Builds the WebAssembly player and packages it as a release zip:
#
#   tools/player/dist/ldraw-player-<version>.zip (+ .sha256)
#     ldraw-player-<version>/
#       ldraw_player.js  ldraw_player_bg.wasm  ldraw_player.d.ts  README.md  LICENSE.md
#
# Needs, once:
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-bindgen-cli --version <the wasm-bindgen version in Cargo.lock>
# Optional: wasm-opt (binaryen) on PATH shrinks the .wasm further.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
version="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$here/Cargo.toml" | head -n1)"
name="ldraw-player-$version"
dist="$here/dist"
cd "$root"

# The CLI has to match the wasm-bindgen crate exactly.
want="$(awk '$0 == "name = \"wasm-bindgen\"" { getline; gsub(/version = |"/, ""); print; exit }' Cargo.lock)"
have="$(wasm-bindgen --version 2>/dev/null | awk '{ print $2 }' || true)"
if [ "$want" != "$have" ]; then
    echo "wasm-bindgen CLI is '${have:-missing}', Cargo.lock has $want:" >&2
    echo "  cargo install wasm-bindgen-cli --version $want" >&2
    exit 1
fi

cargo build -p ldraw-player --target wasm32-unknown-unknown --profile wasm-release

rm -rf "${dist:?}/$name" "$dist/$name.zip" "$dist/$name.zip.sha256"
mkdir -p "$dist/$name"
wasm-bindgen --target web --out-name ldraw_player --out-dir "$dist/$name" \
    target/wasm32-unknown-unknown/wasm-release/ldraw_player.wasm
if command -v wasm-opt > /dev/null; then
    wasm-opt -Os --all-features "$dist/$name/ldraw_player_bg.wasm" -o "$dist/$name/ldraw_player_bg.wasm"
fi
cp "$here/README.md" "$root/LICENSE.md" "$dist/$name/"

(cd "$dist" && zip -qrX "$name.zip" "$name" && shasum -a 256 "$name.zip" > "$name.zip.sha256")
ls -l "$dist/$name"
cat "$dist/$name.zip.sha256"
