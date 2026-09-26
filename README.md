# LDraw.rs

[LDraw] is an open standard for virtual LEGO CAD.

LDraw.rs is a library for manipulating and rendering LDraw model files. Built with [Rust] language, it can be compiled to [WebAssembly] and run it in your web browser directly.

LDraw.rs is a part of a project which aims to create a web-based LEGO CAD service.

## Crates and tools

Libraries (not published to crates.io — see [Building from source](#building-from-source) or [Installing a release](#installing-a-release) below):

* [`ldraw`](ldraw) — basic I/O and structuring of LDraw files, including a resolver for loading the parts library from the filesystem or over HTTP.
* [`ir`](ir) (`ldraw-ir`, Internal Representation) — higher level concepts beyond what LDraw provides, and processing of part data into a form friendly to modern graphics pipelines.
* [`renderer`](renderer) (`ldraw-renderer`) — rendering of models with [wgpu] (Vulkan/Metal/DX12/WebGPU/WebGL2).
* [`olr`](olr) (`ldraw-olr`, Offline Renderer) — renders a model to an image off-screen, on top of `renderer`.
* [`tools/viewer/common`](tools/viewer/common) (`viewer-common`) — shared, windowing-agnostic viewer logic used by both viewer binaries below.

Binaries and WebAssembly bundles (see [Installing a release](#installing-a-release) for prebuilt downloads):

* [`tools/baker`](tools/baker) (`baker`) — command-line tool that preprocesses/bakes LDraw parts into `ir`'s format.
* [`tools/ldr2img`](tools/ldr2img) (`ldr2img`) — command-line tool that renders a model to a PNG using `olr`.
* [`tools/viewer/native`](tools/viewer/native) (`viewer_native`) — native desktop model viewer (Windows/macOS/Linux), built on `viewer-common` + [winit].
* [`tools/viewer/web`](tools/viewer/web) (`viewer_web`) — the same viewer compiled to WebAssembly and run in the browser, bundled with webpack + wasm-pack.
* [`tools/player`](tools/player) (`ldraw-player`) — WebAssembly library that plays back how a model is built, step by step, in the browser; see [tools/player/README.md](tools/player/README.md) for its JS/TS API.

## Building from source

Prerequisites:

* A recent stable [Rust] toolchain (the workspace uses the 2024 edition).
* For anything that uses `viewer_native` or `renderer`/`olr`/`ldr2img` on Linux: X11/Wayland and Vulkan development headers, e.g. on Debian/Ubuntu:
  `sudo apt-get install libx11-dev libxkbcommon-dev libwayland-dev libudev-dev mesa-vulkan-drivers`.
* For anything targeting WebAssembly: the `wasm32-unknown-unknown` target (`rustup target add wasm32-unknown-unknown`).

Native binaries:

```bash
cargo build --release -p baker -p ldr2img -p viewer_native
```

The `ldraw-player` WebAssembly bundle (also needs `wasm-bindgen-cli`, pinned to the version in `Cargo.lock`, and optionally `wasm-opt` from [Binaryen] to shrink the output):

```bash
cargo install wasm-bindgen-cli --version "$(awk '$0 == "name = \"wasm-bindgen\"" { getline; gsub(/version = |"/, ""); print; exit }' Cargo.lock)"
tools/player/build.sh
```

The `viewer_web` WebAssembly app (also needs Node.js and [wasm-pack]):

```bash
scripts/package-viewer-web.sh
```

API docs for the library crates:

```bash
cargo doc --no-deps -p ldraw -p ldraw-ir -p ldraw-olr -p ldraw-renderer --open
```

## Installing a release

Every tagged release ([Releases](../../releases)) is built for all supported platforms by [`.github/workflows/release.yml`](.github/workflows/release.yml) and published with a `SHA256SUMS.txt`. Download the archive for your platform/use case:

| Artifact | Contents |
|---|---|
| `ldraw-tools-<version>-x86_64-unknown-linux-gnu.tar.gz` | `baker`, `ldr2img`, `viewer_native` for Linux x64 |
| `ldraw-tools-<version>-x86_64-pc-windows-msvc.zip` | same, for Windows x64 |
| `ldraw-tools-<version>-aarch64-apple-darwin.tar.gz` | same, for macOS (Apple Silicon) |
| `ldraw-player-<version>.zip` | the `ldraw-player` WebAssembly bundle (`.wasm`/`.js`/`.d.ts`) for embedding in a web page |
| `ldraw-viewer-web-<version>.zip` | a ready-to-serve static build of the WebAssembly model viewer |
| `ldraw-rs-docs-<version>.zip` | prebuilt rustdoc for `ldraw`, `ldraw-ir`, `ldraw-olr`, `ldraw-renderer` |

After extracting a native archive, `baker`/`ldr2img`/`viewer_native` need a local LDraw parts library, given either with `LDRAWDIR` or a CLI flag — it isn't bundled. `ldraw-viewer-web` must be served over HTTP (e.g. `npx serve .`); opening `index.html` as a `file://` URL won't work.

Maintainers: see [`.github/RELEASE.md`](.github/RELEASE.md) for how to cut a new release.

## Examples

You can see a simple model viewer in action on your web browser:

* [Car model](https://segfault87.github.io/ldraw-rs-preview/#models/car.ldr) (from official LDraw samples)
* [Pyramid model](https://segfault87.github.io/ldraw-rs-preview/#models/pyramid.ldr) (from official LDraw samples)
* [6973 Deep Freeze Defender](https://segfault87.github.io/ldraw-rs-preview/#models/6973.ldr)

## License

This project is licensed under of MIT license ([LICENSE.md](LICENSE.md) or http://opensource.org/licenses/MIT).

## Trademarks

LDraw is a trademark of the Estate of James Jessiman. LEGO is a registered trademark of the LEGO Group.

  [Binaryen]: https://github.com/WebAssembly/binaryen
  [LDraw]: http://www.ldraw.org
  [Rust]: https://www.rust-lang.org
  [WebAssembly]: https://webassembly.org
  [wgpu]: https://wgpu.rs
  [winit]: https://github.com/rust-windowing/winit
  [wasm-pack]: https://rustwasm.github.io/wasm-pack/