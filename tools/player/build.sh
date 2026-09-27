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
# Optional: wasm-opt (Binaryen) on PATH shrinks the .wasm further (5.2 -> 3.4 MB).
# Use a recent one (releases use version 133): Ubuntu 24.04's 108 exports the
# wrong table, and the player fails to start in every browser.
# Optional: node on PATH checks that the result starts.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
version="${VERSION:-$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/Cargo.toml" | head -n1)}"
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
    # Only the features rustc used (read from the module), never --all-features:
    # that lets wasm-opt emit proposals browsers don't have yet.
    wasm-opt -Os "$dist/$name/ldraw_player_bg.wasm" -o "$dist/$name/ldraw_player_bg.wasm"
fi

# The module has to start: instantiate it the way a page does (init), in node.
# (The glue is imported as a data: URL, so node doesn't go looking for a package.json.)
if command -v node > /dev/null; then
    node --input-type=module -e '
        import { readFileSync } from "node:fs";
        const dir = process.argv[1];
        const glue = readFileSync(dir + "/ldraw_player.js").toString("base64");
        const { default: init } = await import("data:text/javascript;base64," + glue);
        try {
            await init({ module_or_path: readFileSync(dir + "/ldraw_player_bg.wasm") });
        } catch (e) {
            console.error("ldraw_player_bg.wasm does not start: " + e.message);
            process.exit(1);
        }
    ' "$dist/$name"
else
    echo "node not found: skipped checking that the module starts" >&2
fi
cp "$here/README.md" "$root/LICENSE.md" "$dist/$name/"

(cd "$dist" && zip -qrX "$name.zip" "$name" && shasum -a 256 "$name.zip" > "$name.zip.sha256")
ls -l "$dist/$name"
cat "$dist/$name.zip.sha256"
