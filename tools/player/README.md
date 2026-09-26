# ldraw-player

Plays back how an LDraw model is built, in the browser: parts drop into place
step by step while the camera slowly turns. Playback can be paused, sought and
stepped through like chapters of a video, and the camera can be moved at any
time, also while paused, in two modes:

* **Inspect**: rotate, pan and zoom. Zooming doesn't stop at the model: once
  close, it keeps flying forward, so you can look around inside buildings.
* **Walk**: first person, like a game. The mouse looks around, W A S D move.

Rendering is ldraw.rs's own (`renderer` crate, wgpu): WebGPU where the browser
has it, WebGL2 otherwise.

## Use

```js
import init, { Player } from "./ldraw_player.js";

await init();                       // loads ldraw_player_bg.wasm next to the .js
const player = await Player.create(canvas, {
  libraryUrl: "/ldraw/",            // LDraw folder: LDConfig.ldr, parts/, p/
  resolverUrl: "/ldraw-id/",        // optional, see below
});
player.onChange(() => updateControls(player));
const { parts, steps, duration, missing } = await player.load("/models/car.mpd");
```

The page styles the canvas (its CSS size is what gets rendered, at up to 2x the
device pixel ratio); the player handles its pointer and wheel input.

### Options for `Player.create`

| Option | Default | |
|---|---|---|
| `libraryUrl` | `"ldraw/"` | LDraw library folder (relative to the page) |
| `resolverUrl` | none | URL that serves any library file by name |
| `background` | `"#ffffff"` | clear colour |
| `autoplay` | `true` | start building when a model is loaded; if `false`, show it finished |
| `autoRotate` | `true` | turn the camera while playing |
| `antialias` | `true` | 4x MSAA |
| `maxPixelRatio` | `2` | cap on the drawing buffer's resolution |
| `backend` | automatic | `"webgl"` skips WebGPU |

Library files are fetched from `libraryUrl` + `parts/<name>`, then `p/<name>`.
With `resolverUrl`, each is one request to `resolverUrl` + `<name>` instead; the
server finds the file (parts/, p/, models/) and says where in an
`X-LDraw-Folder: parts|p|models` response header. A reference that isn't in the
library is looked up next to the model, for models split over several files.

### Player

| | |
|---|---|
| `load(url, onProgress?)` | load and (auto)play a model; resolves to `{parts, steps, duration, missing}`, or `null` if another load started meanwhile |
| `loadText(text, baseUrl?, onProgress?)` | same, for a model given as text |
| `onProgress(stage, done, total)` | `"parts"`: files fetched (total unknown: 0); `"prepare"`: parts turned into meshes |
| `play()`, `pause()`, `playing` | from the start again once finished |
| `time`, `seek(t)`, `duration` | seconds at 1x |
| `speed` | playback rate (0.05–16) |
| `stepTimes()` | `Float32Array` of when each step starts: the chapter marks |
| `stepAt(t)`, `stepCount` | the step (0-based) being built at `t` |
| `partCount` | parts placed by the build |
| `autoRotate` | turn the camera while playing (Inspect only) |
| `cameraMode` | `"inspect"` or `"walk"` |
| `resetView()` | back to the default view |
| `setBackground("#rrggbb")` | |
| `onChange(callback)` | called after frames in which the playback state changed |
| `backend` | `"webgpu"` or `"gl"` |
| `destroy()` | stop rendering, remove listeners |

Input, Inspect: drag to rotate, Shift+drag (or middle-drag) to pan, wheel or
pinch to zoom towards the pointer, two fingers to pan and zoom.

Input, Walk: click the canvas and the mouse looks around (pointer lock; Esc
frees it; dragging works too). W A S D move level, whichever way you look; E and
Q go up and down; Shift runs (3x); the wheel flies along the pointer. The keys
are read from the whole page. Switching back to Inspect orbits a point just
ahead of where you walked to, or the same point as before if you didn't move.
Nothing stops the camera at walls, in either mode.

### Pacing

Parts of a step start 0.2 s apart (the whole step within 10 s) and take 0.5 s to
drop; each step is followed by a 0.3 s pause. Builds longer than a minute are
compressed to a minute. Drops stay at least 0.12 s long, unless a build has so
many steps that they would take up more than half of that minute. `speed`
changes it from there. Submodels are built in place, with their own steps,
where the parent model uses them.

## Build

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129   # = wasm-bindgen in Cargo.lock
tools/player/build.sh    # -> tools/player/dist/ldraw-player-<version>.zip (+ .sha256)
```

`cargo test -p ldraw-player` runs the timeline and camera tests natively.
