# Creating a release

Releases are built and published automatically by
[`.github/workflows/release.yml`](workflows/release.yml) whenever a tag
matching `v*.*.*` is pushed. No secrets need to be configured — the workflow
uses the default `GITHUB_TOKEN`.

## 1. Bump the version

All crates share one version through `[workspace.package]` in the root
[`Cargo.toml`](../Cargo.toml). Bump it there only:

```toml
[workspace.package]
version = "1.2.0"
```

Then refresh the lockfile and commit both files:

```bash
cargo check
git add Cargo.toml Cargo.lock
git commit -m "Bump version to 1.2.0"
```

Open a PR, get it merged into `main` before tagging.

## 2. Tag and push

```bash
git checkout main && git pull
git tag -a v1.2.0 -m "v1.2.0"
git push origin v1.2.0
```

Pushing the tag triggers the `Release` workflow (Actions tab). It runs six
jobs:

| Job | Produces |
|---|---|
| `check` | Runs `cargo clippy` + `cargo test`; every build job below waits on this |
| `native` (4-way matrix: Linux x64, Windows x64, macOS x64, macOS arm64) | `ldraw-tools-<version>-<target-triple>.(tar.gz\|zip)` — `baker`, `ldr2img`, `viewer_native` + README + LICENSE |
| `wasm-player` | `ldraw-player-<version>.zip` — the `.wasm`/`.js`/`.d.ts` bundle for `tools/player` |
| `wasm-viewer` | `ldraw-viewer-web-<version>.zip` — the static webpack build of `tools/viewer/web` |
| `docs` | `ldraw-rs-docs-<version>.zip` — rustdoc for `ldraw`, `ldraw-ir`, `ldraw-olr`, `ldraw-renderer` (not published to crates.io) |
| `release` | Downloads everything above, adds a combined `SHA256SUMS.txt`, publishes the GitHub Release with auto-generated notes |

Watch progress under the repo's **Actions** tab. The whole run takes a few
minutes; the `release` job only starts once all four build jobs succeed, and
those only start once `check` passes.

Note: builds only happen for tagged releases. [`.github/workflows/rust.yml`](workflows/rust.yml)
(clippy/test/build on every push) is manual-only (`workflow_dispatch`) so it
doesn't run on every commit; use it from the Actions tab if you want to
sanity-check a branch before tagging.

## 3. Verify

Once the workflow finishes, open the new release under **Releases** and
confirm all artifacts listed in the table above are attached. To verify a
download against the published checksums:

```bash
shasum -a 256 -c SHA256SUMS.txt --ignore-missing
```

## 4. Polish the release notes (optional)

The release is created with `generate_release_notes: true`, which fills in
a changelog from merged PRs. Edit the release on GitHub afterwards to add a
human-written summary above the auto-generated list if useful.

## Redoing a botched release

If a release needs to be redone (e.g. a build job failed after partially
publishing, or the wrong commit was tagged):

```bash
git tag -d v1.2.0
git push origin :refs/tags/v1.2.0
```

Then delete the release (and any draft) from the GitHub **Releases** page,
fix the issue, and repeat from step 2.

## Notes for consumers

- `baker`, `ldr2img` and `viewer_native` need a local LDraw parts library at
  runtime (`LDRAWDIR` environment variable or a CLI flag) — it is not bundled
  in the release archive.
- The `ldraw-viewer-web` bundle must be served over HTTP (e.g. `npx serve .`);
  opening `index.html` directly as a `file://` URL will not work due to
  WebAssembly/CORS restrictions.

## Local dry run

Every packaging step the workflow runs can be reproduced locally, which is
useful for testing changes to the release process itself:

```bash
# Native binaries (build first, then package):
cargo build --release -p baker -p ldr2img -p viewer_native
TARGET_TRIPLE=$(rustc -vV | sed -n 's/host: //p') scripts/package-native.sh

# WASM player (needs: rustup target add wasm32-unknown-unknown,
# cargo install wasm-bindgen-cli --version <matching Cargo.lock>):
tools/player/build.sh

# WASM viewer app (needs: wasm-pack, Node.js):
scripts/package-viewer-web.sh

# Docs bundle:
scripts/package-docs.sh
```

Packaged output lands in `scripts/dist/` (and `tools/player/dist/` for the
player), both gitignored.
