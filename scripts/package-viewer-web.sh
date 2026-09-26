#!/usr/bin/env bash
# Builds the browser viewer (tools/viewer/web, webpack + wasm-pack) and packages
# its static bundle as a release zip:
#
#   scripts/dist/ldraw-viewer-web-<version>.zip (+ .sha256)
#     ldraw-viewer-web-<version>/
#       index.html  <bundled js/wasm>  NOTES.md
#
# Needs, once: rustup target add wasm32-unknown-unknown, wasm-pack, Node.js/npm.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"
web="$root/tools/viewer/web"
version="${VERSION:-$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/Cargo.toml" | head -n1)}"
name="ldraw-viewer-web-$version"
dist="$root/scripts/dist"

(cd "$web" && npm ci && npm run build)

rm -rf "${dist:?}/$name" "$dist/$name.zip" "$dist/$name.zip.sha256"
mkdir -p "$dist/$name"
cp -R "$web/dist/." "$dist/$name/"
cat > "$dist/$name/NOTES.md" <<'EOF'
# ldraw-viewer-web

Static WebAssembly build of the LDraw model viewer. Serve this folder over
HTTP (e.g. `npx serve .`) — opening index.html directly as a `file://` URL
will not work because of WebAssembly/CORS restrictions.

Needs an LDraw parts library reachable by the page at runtime (not bundled).
EOF

(cd "$dist" && zip -qrX "$name.zip" "$name")
sha256() { command -v shasum >/dev/null && shasum -a 256 "$1" || sha256sum "$1"; }
(cd "$dist" && sha256 "$name.zip" > "$name.zip.sha256")
ls -l "$dist/$name"
