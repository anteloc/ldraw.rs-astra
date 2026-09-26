//! The JavaScript API: [`Player`].
//!
//! Borrowing rule (the demo viewer broke on this): nothing keeps the player's
//! state borrowed across an `.await`. Loading a model downloads and prepares
//! everything on the side and only then swaps it in, in one synchronous step,
//! so frames, input and JS calls can go on while a model loads.

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    rc::{Rc, Weak},
    sync::{Arc, RwLock},
};

use js_sys::{Array, Float32Array, Function, Object, Promise, Reflect};
use ldraw::{
    PartAlias,
    color::ColorCatalog,
    error::ResolutionError,
    library::{CacheCollectionStrategy, LibraryLoader, PartCache, resolve_dependencies_multipart},
    parser::parse_multipart_document,
};
use ldraw_ir::{
    geometry::BoundingBox3,
    model::{Model, ObjectId},
    part::bake_part_from_multipart_document,
};
use ldraw_renderer::{
    Entity,
    display_list::DisplayList,
    part::{Part, PartQuerier},
    pipeline::RenderingPipelineManager,
    projection::{Projection, ProjectionModifier},
    util::{calculate_model_bounding_box, request_device},
};
use reqwest::Url;
use tokio::io::BufReader;
use wasm_bindgen::{JsCast, prelude::*};
use wasm_bindgen_futures::{JsFuture, future_to_promise};
use web_sys::{
    AddEventListenerOptions, Event, EventTarget, HtmlCanvasElement, KeyboardEvent, PointerEvent,
    WheelEvent,
};

use crate::{
    camera::{Camera, CameraMode},
    gpu::Texture,
    loader::WebLoader,
    timeline::{Pacing, Playhead, Timeline},
};

/// Wheel zoom: factor = exp(delta * this), delta in pixels.
const WHEEL_ZOOM: f32 = 0.0015;
/// Trackpad pinch (a wheel event with ctrlKey) sends small deltas.
const PINCH_ZOOM: f32 = 0.01;

fn js_error(message: impl AsRef<str>) -> JsValue {
    js_sys::Error::new(message.as_ref()).into()
}

fn window() -> web_sys::Window {
    web_sys::window().expect("no window")
}

fn page_url() -> Result<Url, JsValue> {
    let href = window().location().href()?;
    Url::parse(&href).map_err(|e| js_error(format!("Bad page URL {href}: {e}")))
}

/// `url` resolved against the page, as a folder (trailing slash).
fn folder_url(page: &Url, url: &str) -> Result<Url, JsValue> {
    let url = if url.ends_with('/') {
        url.to_owned()
    } else {
        format!("{url}/")
    };
    page.join(&url)
        .map_err(|e| js_error(format!("Bad URL {url}: {e}")))
}

/// "#rrggbb" as 0..1 sRGB components.
fn parse_hex_color(value: &str) -> Option<[f64; 3]> {
    let hex = value.trim().strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let channel = |i: usize| {
        u8::from_str_radix(&hex[i..i + 2], 16)
            .ok()
            .map(|v| v as f64 / 255.0)
    };
    Some([channel(0)?, channel(2)?, channel(4)?])
}

fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// wgpu's WebGL backend wants the display a surface belongs to; on the web
/// there's just the one. (The demo viewer got it from its winit window.)
#[derive(Debug)]
struct WebDisplay;

impl raw_window_handle::HasDisplayHandle for WebDisplay {
    fn display_handle(
        &self,
    ) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        Ok(raw_window_handle::DisplayHandle::web())
    }
}

/// Let the browser breathe (paint, handle input) during long synchronous work.
async fn next_tick() {
    let promise = Promise::new(&mut |resolve, _| {
        let _ = window().set_timeout_with_callback(&resolve);
    });
    let _ = JsFuture::from(promise).await;
}

struct Options {
    library: String,
    resolver: Option<String>,
    background: [f64; 3],
    antialias: bool,
    autoplay: bool,
    auto_rotate: bool,
    max_pixel_ratio: f64,
    webgl_only: bool,
}

impl Options {
    fn read(value: &JsValue) -> Self {
        let get = |key: &str| {
            Reflect::get(value, &JsValue::from_str(key))
                .ok()
                .filter(|v| !v.is_undefined() && !v.is_null())
        };
        let string = |key: &str| get(key).and_then(|v| v.as_string());
        let flag = |key: &str, default: bool| get(key).and_then(|v| v.as_bool()).unwrap_or(default);
        Self {
            library: string("libraryUrl").unwrap_or_else(|| "ldraw/".into()),
            resolver: string("resolverUrl"),
            background: string("background")
                .and_then(|v| parse_hex_color(&v))
                .unwrap_or([1.0, 1.0, 1.0]),
            antialias: flag("antialias", true),
            autoplay: flag("autoplay", true),
            auto_rotate: flag("autoRotate", true),
            max_pixel_ratio: get("maxPixelRatio").and_then(|v| v.as_f64()).unwrap_or(2.0),
            webgl_only: string("backend").as_deref() == Some("webgl"),
        }
    }
}

/// GPU parts, kept across loads. Library parts are reused as they are; parts
/// defined inside a model (`local`) are rebuilt for every model.
#[derive(Default)]
struct PartsPool {
    parts: HashMap<PartAlias, Part>,
    local: HashSet<PartAlias>,
}

impl PartQuerier<PartAlias> for PartsPool {
    fn get(&self, alias: &PartAlias) -> Option<&Part> {
        self.parts.get(alias)
    }
}

/// What a model load needs, without borrowing the player's state.
struct Shared {
    device: wgpu::Device,
    colors: ColorCatalog,
    cache: Arc<RwLock<PartCache>>,
    parts: RefCell<PartsPool>,
    library: Url,
    resolver: Option<Url>,
    page: Url,
    generation: Cell<u32>,
}

#[derive(Default)]
struct Pointers {
    active: Vec<(i32, f32, f32)>,
}

impl Pointers {
    fn position(&self, id: i32) -> Option<(f32, f32)> {
        self.active.iter().find(|p| p.0 == id).map(|p| (p.1, p.2))
    }

    fn set(&mut self, id: i32, x: f32, y: f32) {
        match self.active.iter_mut().find(|p| p.0 == id) {
            Some(p) => (p.1, p.2) = (x, y),
            None => self.active.push((id, x, y)),
        }
    }

    fn remove(&mut self, id: i32) {
        self.active.retain(|p| p.0 != id);
    }

    /// Midpoint and spread of the first two pointers.
    fn pinch(&self) -> Option<((f32, f32), f32)> {
        let [a, b, ..] = self.active.as_slice() else {
            return None;
        };
        let mid = ((a.1 + b.1) * 0.5, (a.2 + b.2) * 0.5);
        Some((mid, ((a.1 - b.1).powi(2) + (a.2 - b.2).powi(2)).sqrt()))
    }
}

/// Walk mode keys held down (`KeyboardEvent.code`): W A S D, E/Q up/down, Shift runs.
#[derive(Default)]
struct Keys(HashSet<String>);

impl Keys {
    const WALK: [&str; 8] = [
        "KeyW",
        "KeyA",
        "KeyS",
        "KeyD",
        "KeyE",
        "KeyQ",
        "ShiftLeft",
        "ShiftRight",
    ];

    fn held(&self, code: &str) -> f32 {
        if self.0.contains(code) { 1.0 } else { 0.0 }
    }

    /// (forward, right, up), each -1, 0 or 1.
    fn direction(&self) -> (f32, f32, f32) {
        (
            self.held("KeyW") - self.held("KeyS"),
            self.held("KeyD") - self.held("KeyA"),
            self.held("KeyE") - self.held("KeyQ"),
        )
    }

    fn running(&self) -> bool {
        self.0.contains("ShiftLeft") || self.0.contains("ShiftRight")
    }
}

struct Inner {
    shared: Rc<Shared>,
    canvas: HtmlCanvasElement,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    view_format: wgpu::TextureFormat,
    max_texture_size: u32,
    sample_count: u32,
    framebuffer: Option<Texture>,
    depth: Texture,
    pipelines: RenderingPipelineManager,
    projection: Entity<Projection>,
    display_list: Entity<DisplayList<ObjectId, PartAlias>>,
    background: [f64; 3],
    backend: &'static str,
    max_pixel_ratio: f64,

    timeline: Timeline,
    playhead: Playhead,
    part_count: usize,
    time: f32,
    playing: bool,
    speed: f32,
    autoplay: bool,
    auto_rotate: bool,
    last_frame: Option<f64>,

    camera: Camera,
    pointers: Pointers,
    keys: Keys,

    camera_moved: bool, // projection needs updating
    dirty: bool,        // needs a new frame
    changed: bool,      // playback state changed: tell JS
    on_change: Option<Function>,
}

impl Inner {
    fn css_size(&self) -> (f32, f32) {
        (
            self.canvas.client_width() as f32,
            self.canvas.client_height() as f32,
        )
    }

    /// Keeps the drawing buffer at the canvas' displayed size.
    fn fit_canvas(&mut self) {
        let ratio = window()
            .device_pixel_ratio()
            .clamp(1.0, self.max_pixel_ratio.max(1.0));
        let (css_width, css_height) = self.css_size();
        let width = ((css_width as f64 * ratio).round() as u32).min(self.max_texture_size);
        let height = ((css_height as f64 * ratio).round() as u32).min(self.max_texture_size);
        if width == 0 || height == 0 || (width == self.config.width && height == self.config.height)
        {
            return;
        }
        self.canvas.set_width(width);
        self.canvas.set_height(height);
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.framebuffer = (self.sample_count > 1)
            .then(|| Texture::framebuffer(&self.device, &self.config, self.sample_count));
        self.depth = Texture::depth(&self.device, &self.config, self.sample_count);
        self.camera_moved = true;
        self.dirty = true;
    }

    fn aspect(&self) -> f32 {
        self.config.width as f32 / self.config.height.max(1) as f32
    }

    /// One animation frame. Returns the change callback to call (after the
    /// state is no longer borrowed, since it may call back into the player).
    fn frame(&mut self, now: f64) -> Option<Function> {
        let seconds = self
            .last_frame
            .map_or(0.0, |last| ((now - last) / 1000.0).clamp(0.0, 0.1) as f32);
        self.last_frame = Some(now);
        self.fit_canvas();

        if self.playing {
            let duration = self.timeline.duration();
            self.time = (self.time + seconds * self.speed).min(duration);
            let inspecting = self.camera.mode() == CameraMode::Inspect;
            if self.auto_rotate && inspecting && self.pointers.active.is_empty() {
                self.camera.auto_rotate(seconds);
                self.camera_moved = true;
            }
            if self.time >= duration {
                self.playing = false;
            }
            self.changed = true;
        }
        if self.camera.mode() == CameraMode::Walk {
            let (forward, right, up) = self.keys.direction();
            if forward != 0.0 || right != 0.0 || up != 0.0 {
                let run = if self.keys.running() { 3.0 } else { 1.0 };
                let distance = self.camera.walk_speed() * run * seconds;
                self.camera.walk(forward, right, up, distance);
                self.camera_moved = true;
            }
        }
        if self
            .playhead
            .apply(&self.timeline, self.time, &mut self.display_list)
        {
            self.dirty = true;
        }
        if self.camera_moved {
            let aspect = self.aspect();
            self.projection
                .mutate_all(self.camera.update_projections(aspect.into()).into_iter());
            self.camera_moved = false;
            self.dirty = true;
        }
        if self.dirty {
            self.dirty = false;
            self.render();
        }
        if std::mem::take(&mut self.changed) {
            self.on_change.clone()
        } else {
            None
        }
    }

    fn render(&mut self) {
        self.projection.update(&self.device, &self.queue);
        self.display_list.update(&self.device, &self.queue);

        let output = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(o)
            | wgpu::CurrentSurfaceTexture::Suboptimal(o) => o,
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
                self.dirty = true;
                return;
            }
            _ => {
                self.dirty = true;
                return;
            }
        };
        let view = output.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.view_format),
            ..Default::default()
        });
        let (target, resolve_target) = match &self.framebuffer {
            Some(framebuffer) => (&framebuffer.view, Some(&view)),
            None => (&view, None),
        };
        let [r, g, b] = if self.view_format.is_srgb() {
            self.background.map(srgb_to_linear)
        } else {
            self.background
        };

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Player"),
            });
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Main render pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target,
                        resolve_target,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a: 1.0 }),
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.depth.view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    occlusion_query_set: None,
                    timestamp_writes: None,
                    multiview_mask: None,
                })
                .forget_lifetime();
            let parts = self.shared.parts.borrow();
            self.pipelines.render(
                &mut pass,
                self.projection.get(),
                &*parts,
                &self.display_list,
            );
        }
        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();
    }

    /// Shows a freshly loaded model.
    fn install(&mut self, timeline: Timeline, bounds: &BoundingBox3) {
        self.part_count = timeline.items().len();
        self.timeline = timeline;
        self.display_list = DisplayList::new().into();
        self.playhead.reset();
        // Autoplay builds it from scratch; otherwise show the finished model.
        self.time = if self.autoplay {
            0.0
        } else {
            self.timeline.duration()
        };
        self.playing = self.autoplay && self.timeline.duration() > 0.0;
        let aspect = self.aspect();
        self.camera.fit(bounds, aspect);
        self.camera_moved = true;
        self.dirty = true;
        self.changed = true;
    }

    fn on_pointer(&mut self, event: &PointerEvent) {
        let (id, x, y) = (
            event.pointer_id(),
            event.offset_x() as f32,
            event.offset_y() as f32,
        );
        match event.type_().as_str() {
            "pointerdown" => {
                // left or middle button, pen or touch; right-click is left alone
                let mouse = event.pointer_type() == "mouse";
                if mouse && event.button() > 1 {
                    return;
                }
                if self.camera.mode() == CameraMode::Walk && mouse {
                    // Mouse-look like a game: the pointer is captured until Esc.
                    // (If the browser refuses, dragging still looks around.)
                    if !self.pointer_locked() {
                        self.canvas.request_pointer_lock();
                    }
                } else {
                    let _ = self.canvas.set_pointer_capture(id);
                }
                self.pointers.set(id, x, y);
                event.prevent_default();
            }
            "pointermove" if self.camera.mode() == CameraMode::Walk => {
                if self.pointer_locked() {
                    let (dx, dy) = (event.movement_x() as f32, event.movement_y() as f32);
                    self.camera.look(dx, dy);
                } else if let Some((last_x, last_y)) = self.pointers.position(id) {
                    self.camera.look(x - last_x, y - last_y);
                    self.pointers.set(id, x, y);
                } else {
                    return;
                }
                self.camera_moved = true;
            }
            "pointermove" => {
                let Some((last_x, last_y)) = self.pointers.position(id) else {
                    return;
                };
                let (_, css_height) = self.css_size();
                match self.pointers.active.len() {
                    1 => {
                        let (dx, dy) = (x - last_x, y - last_y);
                        if event.shift_key() || event.buttons() & 4 != 0 {
                            self.camera.pan(dx, dy, css_height);
                        } else {
                            self.camera.rotate(dx, dy);
                        }
                    }
                    _ => {
                        let before = self.pointers.pinch();
                        self.pointers.set(id, x, y);
                        if let (Some((m0, d0)), Some((m1, d1))) = (before, self.pointers.pinch()) {
                            let (css_width, css_height) = self.css_size();
                            self.camera.pan(m1.0 - m0.0, m1.1 - m0.1, css_height);
                            if d1 > 1.0 {
                                self.camera.zoom(d0 / d1, m1.0, m1.1, css_width, css_height);
                            }
                        }
                    }
                }
                self.pointers.set(id, x, y);
                self.camera_moved = true;
            }
            _ => {
                // pointerup, pointercancel
                self.pointers.remove(id);
                let _ = self.canvas.release_pointer_capture(id);
            }
        }
    }

    fn pointer_locked(&self) -> bool {
        let canvas = JsValue::from(self.canvas.clone());
        window()
            .document()
            .and_then(|d| d.pointer_lock_element())
            .is_some_and(|element| JsValue::from(element) == canvas)
    }

    fn on_key(&mut self, event: &Event) {
        if event.type_() == "blur" {
            self.keys.0.clear(); // keys released while the page wasn't looking
            return;
        }
        let Some(event) = event.dyn_ref::<KeyboardEvent>() else {
            return;
        };
        let code = event.code();
        if event.type_() == "keyup" {
            self.keys.0.remove(&code);
        } else if Keys::WALK.contains(&code.as_str())
            && !(event.ctrl_key() || event.meta_key() || event.alt_key())
        {
            self.keys.0.insert(code);
        }
    }

    fn set_camera_mode(&mut self, mode: CameraMode) {
        if mode == CameraMode::Inspect {
            self.keys.0.clear();
            if self.pointer_locked()
                && let Some(document) = window().document()
            {
                document.exit_pointer_lock();
            }
        }
        self.camera.set_mode(mode);
        self.camera_moved = true;
    }

    fn on_wheel(&mut self, event: &WheelEvent) {
        event.prevent_default();
        let (css_width, css_height) = self.css_size();
        let mut delta = event.delta_y() as f32;
        match event.delta_mode() {
            1 => delta *= 16.0,       // lines
            2 => delta *= css_height, // pages
            _ => {}
        }
        let per_pixel = if event.ctrl_key() {
            PINCH_ZOOM
        } else {
            WHEEL_ZOOM
        };
        let factor = (delta.clamp(-400.0, 400.0) * per_pixel).exp();
        self.camera.zoom(
            factor,
            event.offset_x() as f32,
            event.offset_y() as f32,
            css_width,
            css_height,
        );
        self.camera_moved = true;
    }
}

type Listener = Closure<dyn FnMut(Event)>;
type FrameCallback = Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>>;

/// Plays back how an LDraw model is built, on a canvas.
///
/// ```js
/// import init, { Player } from "./ldraw_player.js";
/// await init();
/// const player = await Player.create(canvas, { libraryUrl: "/ldraw/", resolverUrl: "/ldraw-id/" });
/// await player.load("/models/car.mpd");
/// ```
#[wasm_bindgen]
pub struct Player {
    inner: Rc<RefCell<Inner>>,
    frame: FrameCallback,
    frame_request: Rc<Cell<Option<i32>>>,
    listeners: Vec<(EventTarget, &'static str, Listener)>,
}

#[wasm_bindgen]
impl Player {
    /// Sets up rendering on `canvas` and loads the colour definitions.
    ///
    /// Options (all optional): `libraryUrl` (the LDraw folder with LDConfig.ldr,
    /// parts/ and p/; default "ldraw/"), `resolverUrl` (a URL that serves any
    /// part by name, see README), `background` ("#rrggbb"), `antialias`,
    /// `autoplay` and `autoRotate` (all true), `maxPixelRatio` (2),
    /// `backend` ("webgl" to skip WebGPU).
    pub async fn create(canvas: HtmlCanvasElement, options: JsValue) -> Result<Player, JsValue> {
        console_error_panic_hook::set_once();
        let options = Options::read(&options);
        let page = page_url()?;
        let library = folder_url(&page, &options.library)?;
        let resolver = options
            .resolver
            .as_deref()
            .map(|r| folder_url(&page, r))
            .transpose()?;

        let colors = WebLoader::new(library.clone(), None, None)
            .load_colors()
            .await
            .map_err(|e| js_error(format!("Could not load LDConfig.ldr from {library}: {e}")))?;

        let backends = if options.webgl_only {
            wgpu::Backends::GL
        } else {
            wgpu::Backends::BROWSER_WEBGPU | wgpu::Backends::GL
        };
        let instance = wgpu::util::new_instance_with_webgpu_detection(wgpu::InstanceDescriptor {
            backends,
            backend_options: Default::default(),
            flags: Default::default(),
            memory_budget_thresholds: Default::default(),
            display: Some(Box::new(WebDisplay)),
        })
        .await;
        let surface = instance
            .create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone()))
            .map_err(|e| js_error(format!("Could not use the canvas: {e}")))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: Some(&surface),
            })
            .await
            .map_err(|e| js_error(format!("No WebGPU or WebGL2 adapter: {e}")))?;
        let backend = adapter.get_info().backend.to_str();
        let (device, queue, max_texture_size) = request_device(&adapter, Some("Player"))
            .await
            .map_err(|e| js_error(format!("Could not open the GPU device: {e}")))?;

        // The shaders work on LDraw's colour values as they are (sRGB), so
        // render to a non-sRGB target: colours come out as defined, the same
        // on WebGPU and WebGL (the demo viewer only gets this on WebGPU).
        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .or_else(|| caps.formats.first().copied())
            .ok_or_else(|| js_error("The canvas supports no texture format"))?;
        let view_format = format;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: canvas.width().max(1),
            height: canvas.height().max(1),
            present_mode: caps.present_modes[0],
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![view_format],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);
        let sample_count = if options.antialias { 4 } else { 1 };
        let pipelines = RenderingPipelineManager::new(&device, &queue, view_format, sample_count);

        if let Some(style) = canvas.dyn_ref::<web_sys::HtmlElement>().map(|e| e.style()) {
            let _ = style.set_property("touch-action", "none"); // we handle touch gestures
        }

        let shared = Rc::new(Shared {
            device: device.clone(),
            colors,
            cache: Arc::new(RwLock::new(PartCache::default())),
            parts: RefCell::new(PartsPool::default()),
            library,
            resolver,
            page,
            generation: Cell::new(0),
        });
        let inner = Rc::new(RefCell::new(Inner {
            shared,
            canvas: canvas.clone(),
            surface,
            framebuffer: (sample_count > 1)
                .then(|| Texture::framebuffer(&device, &config, sample_count)),
            depth: Texture::depth(&device, &config, sample_count),
            device,
            queue,
            config,
            view_format,
            max_texture_size,
            sample_count,
            pipelines,
            projection: Projection::new().into(),
            display_list: DisplayList::new().into(),
            background: options.background,
            backend,
            max_pixel_ratio: options.max_pixel_ratio,
            timeline: Timeline::default(),
            playhead: Playhead::default(),
            part_count: 0,
            time: 0.0,
            playing: false,
            speed: 1.0,
            autoplay: options.autoplay,
            auto_rotate: options.auto_rotate,
            last_frame: None,
            camera: Camera::default(),
            pointers: Pointers::default(),
            keys: Keys::default(),
            camera_moved: true,
            dirty: true,
            changed: false,
            on_change: None,
        }));

        let mut player = Player {
            inner,
            frame: Rc::new(RefCell::new(None)),
            frame_request: Rc::new(Cell::new(None)),
            listeners: Vec::new(),
        };
        player.listen(&canvas);
        player.start_frames();
        Ok(player)
    }

    fn listen(&mut self, canvas: &HtmlCanvasElement) {
        for name in ["pointerdown", "pointermove", "pointerup", "pointercancel"] {
            let weak = Rc::downgrade(&self.inner);
            let listener = Listener::new(move |event: Event| {
                if let (Some(inner), Ok(event)) = (weak.upgrade(), event.dyn_into::<PointerEvent>())
                {
                    inner.borrow_mut().on_pointer(&event);
                }
            });
            let _ =
                canvas.add_event_listener_with_callback(name, listener.as_ref().unchecked_ref());
            self.listeners.push((canvas.clone().into(), name, listener));
        }
        let weak = Rc::downgrade(&self.inner);
        let wheel = Listener::new(move |event: Event| {
            if let (Some(inner), Ok(event)) = (weak.upgrade(), event.dyn_into::<WheelEvent>()) {
                inner.borrow_mut().on_wheel(&event);
            }
        });
        let options = AddEventListenerOptions::new();
        options.set_passive(false); // so it can preventDefault the page scroll
        let _ = canvas.add_event_listener_with_callback_and_add_event_listener_options(
            "wheel",
            wheel.as_ref().unchecked_ref(),
            &options,
        );
        self.listeners.push((canvas.clone().into(), "wheel", wheel));

        // Walk mode keys: anywhere on the page.
        let window = window();
        for name in ["keydown", "keyup", "blur"] {
            let weak = Rc::downgrade(&self.inner);
            let listener = Listener::new(move |event: Event| {
                if let Some(inner) = weak.upgrade() {
                    inner.borrow_mut().on_key(&event);
                }
            });
            let _ =
                window.add_event_listener_with_callback(name, listener.as_ref().unchecked_ref());
            self.listeners.push((window.clone().into(), name, listener));
        }
    }

    fn start_frames(&self) {
        let weak: Weak<RefCell<Inner>> = Rc::downgrade(&self.inner);
        let frame = Rc::clone(&self.frame);
        let request = Rc::clone(&self.frame_request);
        *self.frame.borrow_mut() = Some(Closure::new(move |now: f64| {
            request.set(None);
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let on_change = inner.borrow_mut().frame(now);
            if let Some(callback) = on_change {
                let _ = callback.call0(&JsValue::NULL);
            }
            if let Some(closure) = frame.borrow().as_ref() {
                request.set(
                    window()
                        .request_animation_frame(closure.as_ref().unchecked_ref())
                        .ok(),
                );
            }
        }));
        if let Some(closure) = self.frame.borrow().as_ref() {
            self.frame_request.set(
                window()
                    .request_animation_frame(closure.as_ref().unchecked_ref())
                    .ok(),
            );
        }
    }

    /// Loads and plays the model at `url` (.ldr/.mpd/.dat).
    ///
    /// Resolves to `{parts, steps, duration, missing}` (`missing`: files that
    /// couldn't be found; those parts are left out), or to `null` if another
    /// load started in the meantime. `onProgress(stage, done, total)` is called
    /// with stage "parts" (files fetched; total 0: not known yet) and "prepare".
    pub fn load(&self, url: String, on_progress: Option<Function>) -> Promise {
        let inner = Rc::clone(&self.inner);
        future_to_promise(async move {
            let (shared, generation) = begin_load(&inner);
            let url = shared
                .page
                .join(&url)
                .map_err(|e| js_error(format!("Bad model URL {url}: {e}")))?;
            let response = reqwest::Client::new()
                .get(url.clone())
                .send()
                .await
                .map_err(|e| js_error(format!("Could not fetch {url}: {e}")))?;
            if !response.status().is_success() {
                return Err(js_error(format!(
                    "Could not fetch {url}: HTTP {}",
                    response.status()
                )));
            }
            let text = response
                .text()
                .await
                .map_err(|e| js_error(format!("Could not read {url}: {e}")))?;
            load_document(inner, shared, generation, text, Some(url), on_progress).await
        })
    }

    /// Like `load`, for a model given as text. `baseUrl`: where to look for
    /// files it references that aren't in the library.
    #[wasm_bindgen(js_name = loadText)]
    pub fn load_text(
        &self,
        text: String,
        base_url: Option<String>,
        on_progress: Option<Function>,
    ) -> Promise {
        let inner = Rc::clone(&self.inner);
        future_to_promise(async move {
            let (shared, generation) = begin_load(&inner);
            let base = base_url.and_then(|b| shared.page.join(&b).ok());
            load_document(inner, shared, generation, text, base, on_progress).await
        })
    }

    /// Plays from where it is; from the start once the build has finished.
    pub fn play(&self) {
        let mut inner = self.inner.borrow_mut();
        if inner.timeline.duration() <= 0.0 {
            return;
        }
        if inner.time >= inner.timeline.duration() {
            inner.time = 0.0;
        }
        inner.playing = true;
        inner.changed = true;
    }

    pub fn pause(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.playing = false;
        inner.changed = true;
    }

    #[wasm_bindgen(getter)]
    pub fn playing(&self) -> bool {
        self.inner.borrow().playing
    }

    /// Playback position, seconds.
    #[wasm_bindgen(getter)]
    pub fn time(&self) -> f32 {
        self.inner.borrow().time
    }

    /// Jumps to `time` seconds (keeps playing if it was).
    pub fn seek(&self, time: f32) {
        let mut inner = self.inner.borrow_mut();
        inner.time = time.clamp(0.0, inner.timeline.duration());
        inner.changed = true;
    }

    /// Length of the build at 1x speed, seconds.
    #[wasm_bindgen(getter)]
    pub fn duration(&self) -> f32 {
        self.inner.borrow().timeline.duration()
    }

    #[wasm_bindgen(getter)]
    pub fn speed(&self) -> f32 {
        self.inner.borrow().speed
    }

    #[wasm_bindgen(setter)]
    pub fn set_speed(&self, speed: f32) {
        let mut inner = self.inner.borrow_mut();
        inner.speed = if speed.is_finite() {
            speed.clamp(0.05, 16.0)
        } else {
            1.0
        };
        inner.changed = true;
    }

    /// Whether the camera turns slowly while playing.
    #[wasm_bindgen(getter, js_name = autoRotate)]
    pub fn auto_rotate(&self) -> bool {
        self.inner.borrow().auto_rotate
    }

    #[wasm_bindgen(setter, js_name = autoRotate)]
    pub fn set_auto_rotate(&self, on: bool) {
        self.inner.borrow_mut().auto_rotate = on;
    }

    /// "inspect" (orbit, pan, zoom into the model) or "walk" (first person:
    /// mouse looks, W A S D move, E/Q up/down, Shift runs, scroll flies).
    #[wasm_bindgen(getter, js_name = cameraMode)]
    pub fn camera_mode(&self) -> String {
        match self.inner.borrow().camera.mode() {
            CameraMode::Inspect => "inspect".into(),
            CameraMode::Walk => "walk".into(),
        }
    }

    #[wasm_bindgen(setter, js_name = cameraMode)]
    pub fn set_camera_mode(&self, mode: &str) {
        let mode = match mode {
            "inspect" => CameraMode::Inspect,
            "walk" => CameraMode::Walk,
            _ => return,
        };
        self.inner.borrow_mut().set_camera_mode(mode);
    }

    /// When each step starts, seconds: the "chapters".
    #[wasm_bindgen(js_name = stepTimes)]
    pub fn step_times(&self) -> Float32Array {
        Float32Array::from(self.inner.borrow().timeline.steps())
    }

    /// The step (0-based) being built at `time`.
    #[wasm_bindgen(js_name = stepAt)]
    pub fn step_at(&self, time: f32) -> u32 {
        self.inner.borrow().timeline.step_at(time) as u32
    }

    #[wasm_bindgen(getter, js_name = stepCount)]
    pub fn step_count(&self) -> u32 {
        self.inner.borrow().timeline.steps().len() as u32
    }

    /// Parts placed by the build (each use of a submodel counts its parts again).
    #[wasm_bindgen(getter, js_name = partCount)]
    pub fn part_count(&self) -> u32 {
        self.inner.borrow().part_count as u32
    }

    /// "webgpu" or "gl".
    #[wasm_bindgen(getter)]
    pub fn backend(&self) -> String {
        self.inner.borrow().backend.to_owned()
    }

    /// Back to the default view of the whole model.
    #[wasm_bindgen(js_name = resetView)]
    pub fn reset_view(&self) {
        let mut inner = self.inner.borrow_mut();
        let aspect = inner.aspect();
        inner.camera.home(aspect);
        inner.camera_moved = true;
    }

    /// "#rrggbb".
    #[wasm_bindgen(js_name = setBackground)]
    pub fn set_background(&self, color: &str) {
        if let Some(rgb) = parse_hex_color(color) {
            let mut inner = self.inner.borrow_mut();
            inner.background = rgb;
            inner.dirty = true;
        }
    }

    /// `callback()` runs after frames in which the playback state changed
    /// (time, playing, speed, a new model).
    #[wasm_bindgen(js_name = onChange)]
    pub fn on_change(&self, callback: Option<Function>) {
        self.inner.borrow_mut().on_change = callback;
    }

    /// Stops rendering and removes the event listeners.
    pub fn destroy(&mut self) {
        if let Some(request) = self.frame_request.take() {
            let _ = window().cancel_animation_frame(request);
        }
        self.frame.borrow_mut().take();
        for (target, name, listener) in self.listeners.drain(..) {
            let _ =
                target.remove_event_listener_with_callback(name, listener.as_ref().unchecked_ref());
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.destroy();
    }
}

fn begin_load(inner: &Rc<RefCell<Inner>>) -> (Rc<Shared>, u32) {
    let shared = Rc::clone(&inner.borrow().shared);
    let generation = shared.generation.get().wrapping_add(1);
    shared.generation.set(generation);
    (shared, generation)
}

fn progress(callback: &Option<Function>, stage: &str, done: usize, total: usize) {
    if let Some(callback) = callback {
        let _ = callback.call3(
            &JsValue::NULL,
            &JsValue::from_str(stage),
            &JsValue::from(done as u32),
            &JsValue::from(total as u32),
        );
    }
}

async fn load_document(
    inner: Rc<RefCell<Inner>>,
    shared: Rc<Shared>,
    generation: u32,
    text: String,
    base: Option<Url>,
    on_progress: Option<Function>,
) -> Result<JsValue, JsValue> {
    let superseded = || shared.generation.get() != generation;
    let colors = &shared.colors;
    let document = parse_multipart_document(&mut BufReader::new(text.as_bytes()), colors)
        .await
        .map_err(|e| js_error(format!("Could not read the model: {e}")))?;

    // Every file the model needs, fetched level by level (in parallel within a level).
    let loader = WebLoader::new(shared.library.clone(), shared.resolver.clone(), base);
    let fetched = Cell::new(0usize);
    let missing = RefCell::new(Vec::<String>::new());
    let on_update = |alias: PartAlias, result: Result<(), ResolutionError>| {
        match result {
            Ok(()) => fetched.set(fetched.get() + 1),
            Err(_) => missing.borrow_mut().push(alias.original),
        }
        progress(&on_progress, "parts", fetched.get(), 0);
    };
    let resolution = resolve_dependencies_multipart(
        &document,
        Arc::clone(&shared.cache),
        colors,
        &loader,
        &on_update,
    )
    .await;
    let model: Model<PartAlias> = Model::from_ldraw_multipart_document(
        &document,
        colors,
        Some((&loader, Arc::clone(&shared.cache))),
    )
    .await;
    if superseded() {
        return Ok(JsValue::NULL);
    }

    // GPU meshes for the parts not built by an earlier load.
    let aliases: Vec<PartAlias> = document.list_dependencies().into_iter().collect();
    for (index, alias) in aliases.iter().enumerate() {
        let reusable = {
            let pool = shared.parts.borrow();
            pool.parts.contains_key(alias) && !pool.local.contains(alias)
        };
        if !reusable && let Some((part, local)) = resolution.query(alias, true) {
            let baked = bake_part_from_multipart_document(part, &resolution, local);
            let part = Part::new(&baked, &shared.device, colors);
            let mut pool = shared.parts.borrow_mut();
            if local {
                pool.local.insert(alias.clone());
            } else {
                pool.local.remove(alias);
            }
            pool.parts.insert(alias.clone(), part);
        }
        if index % 32 == 31 {
            progress(&on_progress, "prepare", index + 1, aliases.len());
            next_tick().await;
            if superseded() {
                return Ok(JsValue::NULL);
            }
        }
    }
    shared
        .cache
        .write()
        .unwrap()
        .collect(CacheCollectionStrategy::Parts);

    let parts = shared.parts.borrow();
    let timeline = Timeline::from_model(&model, colors, &Pacing::default(), |alias| {
        parts.get(alias).is_some()
    });
    let bounds = calculate_model_bounding_box(&model, None, &*parts);
    drop(parts);
    let result = Object::new();
    let set = |key: &str, value: JsValue| {
        let _ = Reflect::set(&result, &JsValue::from_str(key), &value);
    };
    set("parts", JsValue::from(timeline.items().len() as u32));
    set("steps", JsValue::from(timeline.steps().len() as u32));
    set("duration", JsValue::from(timeline.duration()));
    let missing: Array = missing
        .into_inner()
        .into_iter()
        .map(JsValue::from)
        .collect();
    set("missing", missing.into());

    inner.borrow_mut().install(timeline, &bounds);
    Ok(result.into())
}
