#!/usr/bin/env bash
# Packages the native binaries (baker, ldr2img, viewer_native) built by
# `cargo build --release` into a release archive:
#
#   scripts/dist/ldraw-tools-<version>-<target-triple>.(tar.gz|zip) (+ .sha256)
#     ldraw-tools-<version>-<target-triple>/
#       baker(.exe)  ldr2img(.exe)  viewer_native(.exe)  README.md  LICENSE.md
#
# Usage: TARGET_TRIPLE=x86_64-unknown-linux-gnu scripts/package-native.sh
# Expects release binaries already built at target/release/.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"
version="${VERSION:-$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/Cargo.toml" | head -n1)}"
target_triple="${TARGET_TRIPLE:?Set TARGET_TRIPLE, e.g. x86_64-unknown-linux-gnu}"
name="ldraw-tools-$version-$target_triple"
dist="$root/scripts/dist"
bin_dir="${BIN_DIR:-$root/target/release}"

exe=""
if [[ "$target_triple" == *windows* ]]; then
    exe=".exe"
fi

rm -rf "${dist:?}/$name" "$dist/$name.zip" "$dist/$name.tar.gz" "$dist/$name.zip.sha256" "$dist/$name.tar.gz.sha256"
mkdir -p "$dist/$name"
for bin in baker ldr2img viewer_native; do
    cp "$bin_dir/$bin$exe" "$dist/$name/"
done
cp "$root/README.md" "$root/LICENSE.md" "$dist/$name/"

cd "$dist"
sha256() { command -v shasum >/dev/null && shasum -a 256 "$1" || sha256sum "$1"; }
if [[ "$target_triple" == *windows* ]]; then
    # Git Bash on the windows-latest runner has no `zip`; PowerShell's
    # Compress-Archive is always available there.
    powershell -NoProfile -Command "Compress-Archive -Path '$name' -DestinationPath '$name.zip' -Force"
    sha256 "$name.zip" > "$name.zip.sha256"
else
    tar -czf "$name.tar.gz" "$name"
    sha256 "$name.tar.gz" > "$name.tar.gz.sha256"
fi
# Only the archive (not the staging directory) is a release asset.
rm -rf "${name:?}"
ls -l "$dist"/"$name".*
