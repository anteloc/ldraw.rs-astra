#!/usr/bin/env bash
# Builds rustdoc for the library crates (not published to crates.io) and
# packages it as a release zip:
#
#   scripts/dist/ldraw-rs-docs-<version>.zip (+ .sha256)
#     doc/ldraw/index.html  doc/ldraw_ir/index.html  ...
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"
version="${VERSION:-$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/Cargo.toml" | head -n1)}"
name="ldraw-rs-docs-$version"
dist="$root/scripts/dist"

cd "$root"
cargo doc --no-deps -p ldraw -p ldraw-ir -p ldraw-olr -p ldraw-renderer

rm -rf "${dist:?}/$name" "$dist/$name.zip" "$dist/$name.zip.sha256"
mkdir -p "$dist/$name"
cp -R "$root/target/doc/." "$dist/$name/"

(cd "$dist" && zip -qrX "$name.zip" "$name")
sha256() { command -v shasum >/dev/null && shasum -a 256 "$1" || sha256sum "$1"; }
(cd "$dist" && sha256 "$name.zip" > "$name.zip.sha256")
# Only the archive (not the staging directory) is a release asset.
rm -rf "${dist:?}/${name:?}"
ls -l "$dist"/"$name".*
