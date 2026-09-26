//! ldraw-player: plays back how an LDraw model is built, in the browser.
//!
//! Parts drop into place step by step while the camera slowly turns. Playback
//! can be paused, sought and stepped through like chapters of a video, and the
//! camera can be moved at any time: Inspect (rotate, pan, zoom, all the way
//! inside) or Walk (first person: mouse to look, W A S D to move).
//!
//! The JavaScript API is [`web::Player`] (wasm32 only); see README.md.

pub mod camera;
pub mod timeline;

#[cfg(target_arch = "wasm32")]
mod gpu;
#[cfg(target_arch = "wasm32")]
mod loader;
#[cfg(target_arch = "wasm32")]
pub mod web;
