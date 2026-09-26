//! The camera, in two modes:
//!
//! * Inspect: rotate around a point, pan, and zoom towards the cursor. Zooming
//!   doesn't stop at the model: once the camera is close to the point it
//!   orbits, zooming in further moves camera and orbit point forward together,
//!   so you can fly into a model and look around inside.
//! * Walk: first person. Looking around turns the camera where it stands;
//!   walking moves it level (W A S D), up or down.
//!
//! Nothing checks for walls in either.

use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};

use cgmath::{Deg, InnerSpace, PerspectiveFov, Rad};
use ldraw::{Matrix4, Point3, Vector3};
use ldraw_ir::geometry::BoundingBox3;
use ldraw_renderer::{
    AspectRatio,
    projection::{ProjectionModifier, ProjectionMutator},
};

/// Vertical field of view, degrees.
const FOV: f32 = 45.0;
const HOME_YAW: f32 = FRAC_PI_4;
const HOME_PITCH: f32 = 0.55;
const PITCH_LIMIT: f32 = FRAC_PI_2 - 0.01;
const ROTATE_PER_PIXEL: f32 = 0.006;
/// Walk: turning the head, radians per pixel of mouse movement.
const LOOK_PER_PIXEL: f32 = 0.0025;
/// Walk: speed in model radii per second (between 40 and 600 LDraw units).
const WALK_SPEED: f32 = 0.25;
/// Closest orbit, in model radii: zooming in further flies instead...
const MIN_DISTANCE: f32 = 0.08;
/// ...at this speed, in model radii per unit of zoom (per wheel notch: 1 - factor),
/// a little faster than the last orbit zoom steps so it doesn't crawl.
const FLY_SPEED: f32 = 0.2;
/// Automatic rotation while playing, radians per second (about 50 s per turn).
pub const AUTO_ROTATE_SPEED: f32 = 0.12;
/// -Y is up in LDraw.
const WORLD_UP: Vector3 = Vector3::new(0.0, -1.0, 0.0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraMode {
    Inspect,
    Walk,
}

#[derive(Clone, Debug)]
pub struct Camera {
    /// The point the camera orbits and looks at.
    pub target: Point3,
    /// Angle around the vertical axis, radians.
    pub yaw: f32,
    /// Angle above the horizon, radians.
    pub pitch: f32,
    /// From the camera to `target`, LDraw units.
    pub distance: f32,
    /// The model's bounding sphere: scales zoom limits, flying speed and clipping planes.
    center: Point3,
    radius: f32,
    mode: CameraMode,
    /// Where Walk mode started, to tell whether the camera moved in it.
    walk_start: Option<Pose>,
}

type Pose = (Point3, f32, f32, f32);

impl Default for Camera {
    fn default() -> Self {
        Self {
            target: Point3::new(0.0, 0.0, 0.0),
            yaw: HOME_YAW,
            pitch: HOME_PITCH,
            distance: 300.0,
            center: Point3::new(0.0, 0.0, 0.0),
            radius: 100.0,
            mode: CameraMode::Inspect,
            walk_start: None,
        }
    }
}

impl Camera {
    /// Frames a model with this bounding box.
    pub fn fit(&mut self, bounds: &BoundingBox3, aspect: f32) {
        if bounds.is_null() {
            self.center = Point3::new(0.0, 0.0, 0.0);
            self.radius = 100.0;
        } else {
            let mid = (bounds.min + bounds.max) * 0.5;
            self.center = Point3::new(mid.x, mid.y, mid.z);
            self.radius = ((bounds.max - bounds.min).magnitude() * 0.5).max(10.0);
        }
        self.home(aspect);
    }

    /// Back to the default view of the whole model.
    pub fn home(&mut self, aspect: f32) {
        let half_v = (FOV.to_radians() * 0.5).tan();
        let half = half_v.atan().min((half_v * aspect.max(0.1)).atan()); // the narrower of the two
        self.target = self.center;
        self.yaw = HOME_YAW;
        self.pitch = HOME_PITCH;
        self.distance = self.radius / half.sin() * 1.02;
        if self.mode == CameraMode::Walk {
            self.walk_start = Some(self.pose());
        }
    }

    pub fn mode(&self) -> CameraMode {
        self.mode
    }

    /// Switches between Inspect and Walk. Walk starts where the camera is. Back
    /// in Inspect, the camera orbits a point just ahead of where it walked to,
    /// or, if it didn't move at all, the same point as before.
    pub fn set_mode(&mut self, mode: CameraMode) {
        if mode == self.mode {
            return;
        }
        match mode {
            CameraMode::Walk => self.walk_start = Some(self.pose()),
            CameraMode::Inspect => {
                if self.walk_start.take() != Some(self.pose()) {
                    let eye = self.position();
                    self.distance = self.min_distance();
                    self.target = eye - self.offset() * self.distance;
                }
            }
        }
        self.mode = mode;
    }

    fn pose(&self) -> Pose {
        (self.target, self.yaw, self.pitch, self.distance)
    }

    /// Unit vector from `target` towards the camera.
    fn offset(&self) -> Vector3 {
        Vector3::new(
            self.yaw.sin() * self.pitch.cos(),
            -self.pitch.sin(),
            -self.yaw.cos() * self.pitch.cos(),
        )
    }

    pub fn position(&self) -> Point3 {
        self.target + self.offset() * self.distance
    }

    /// Forward, right and up, as the screen sees them.
    fn basis(&self) -> (Vector3, Vector3, Vector3) {
        let forward = -self.offset();
        let right = forward.cross(WORLD_UP).normalize();
        let up = right.cross(forward);
        (forward, right, up)
    }

    /// World direction through a point of the viewport (CSS pixels).
    fn ray(&self, x: f32, y: f32, width: f32, height: f32) -> Vector3 {
        let (forward, right, up) = self.basis();
        let (width, height) = (width.max(1.0), height.max(1.0));
        let half = (FOV.to_radians() * 0.5).tan();
        let nx = 2.0 * x / width - 1.0;
        let ny = 1.0 - 2.0 * y / height;
        (forward + right * (nx * half * width / height) + up * (ny * half)).normalize()
    }

    fn min_distance(&self) -> f32 {
        (self.radius * MIN_DISTANCE).clamp(5.0, 300.0)
    }

    fn max_distance(&self) -> f32 {
        (self.radius * 30.0).max(1000.0)
    }

    /// Drag by (dx, dy) CSS pixels.
    pub fn rotate(&mut self, dx: f32, dy: f32) {
        self.yaw -= dx * ROTATE_PER_PIXEL;
        self.pitch = (self.pitch + dy * ROTATE_PER_PIXEL).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    pub fn auto_rotate(&mut self, seconds: f32) {
        self.yaw += AUTO_ROTATE_SPEED * seconds;
    }

    /// Walk: turns the head by a mouse movement of (dx, dy) pixels; the camera
    /// stays where it is.
    pub fn look(&mut self, dx: f32, dy: f32) {
        let eye = self.position();
        self.yaw -= dx * LOOK_PER_PIXEL;
        self.pitch = (self.pitch + dy * LOOK_PER_PIXEL).clamp(-PITCH_LIMIT, PITCH_LIMIT);
        self.target = eye - self.offset() * self.distance;
    }

    /// Walk: moves `distance` towards a mix of `forward` (level, whichever way
    /// the camera looks), `right` and `up`, each -1, 0 or 1.
    pub fn walk(&mut self, forward: f32, right: f32, up: f32, distance: f32) {
        let (look, side, _) = self.basis();
        let level = Vector3::new(look.x, 0.0, look.z).normalize(); // pitch < 90°: never zero
        let direction = level * forward + side * right + WORLD_UP * up;
        if direction.magnitude2() > 0.0 {
            self.target += direction.normalize() * distance;
        }
    }

    /// Walk: how fast, in LDraw units per second.
    pub fn walk_speed(&self) -> f32 {
        (self.radius * WALK_SPEED).clamp(40.0, 600.0)
    }

    /// Drag by (dx, dy) CSS pixels: the model follows the pointer.
    pub fn pan(&mut self, dx: f32, dy: f32, viewport_height: f32) {
        let (_, right, up) = self.basis();
        let per_pixel =
            2.0 * self.distance * (FOV.to_radians() * 0.5).tan() / viewport_height.max(1.0);
        self.target += right * (-dx * per_pixel) + up * (dy * per_pixel);
    }

    /// Zooms by `factor` (< 1 is closer) towards the viewport point (x, y).
    ///
    /// Scales the whole view about the point under the cursor on the orbit
    /// plane, so that point stays put on screen. Once at the minimum distance,
    /// zooming in flies forward along the cursor ray instead; in Walk mode,
    /// zooming always flies (forwards or backwards).
    pub fn zoom(&mut self, factor: f32, x: f32, y: f32, width: f32, height: f32) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let ray = self.ray(x, y, width, height);
        let walking = self.mode == CameraMode::Walk;
        if walking || (factor < 1.0 && self.distance <= self.min_distance() * 1.001) {
            self.target += ray * (self.radius * FLY_SPEED * (1.0 - factor));
            return;
        }
        let factor = factor.clamp(
            self.min_distance() / self.distance,
            self.max_distance() / self.distance,
        );
        let (forward, _, _) = self.basis();
        let anchor = self.position() + ray * (self.distance / ray.dot(forward).max(1e-3));
        self.target = anchor + (self.target - anchor) * factor;
        self.distance *= factor;
    }

    fn clip_planes(&self) -> (f32, f32) {
        let close = match self.mode {
            CameraMode::Inspect => self.distance,
            CameraMode::Walk => self.distance.min(self.min_distance()), // walls can be right there
        };
        let near = (close * 0.05).clamp(0.5, 10.0);
        let to_center = (self.position() - self.center).magnitude();
        let far = (to_center + self.radius * 2.0).max(1000.0) + near;
        (near, far)
    }
}

impl ProjectionModifier for Camera {
    fn update_projections(&self, aspect_ratio: AspectRatio) -> Vec<ProjectionMutator> {
        let (near, far) = self.clip_planes();
        let projection = Matrix4::from(PerspectiveFov {
            fovy: Rad::from(Deg(FOV)),
            aspect: aspect_ratio.into(),
            near,
            far,
        });
        let view = Matrix4::look_at_rh(self.position(), self.target, WORLD_UP);
        vec![
            ProjectionMutator::SetProjectionMatrix {
                matrix: projection,
                is_orthographic: false,
            },
            ProjectionMutator::SetViewMatrix(view),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fitted() -> Camera {
        let mut camera = Camera::default();
        let bounds = BoundingBox3::new(
            &Vector3::new(-100.0, -200.0, -100.0),
            &Vector3::new(100.0, 0.0, 100.0),
        );
        camera.fit(&bounds, 1.5);
        camera
    }

    fn close(a: Point3, b: Point3) -> bool {
        (a - b).magnitude() < 1e-2
    }

    #[test]
    fn fit_frames_the_whole_model() {
        let camera = fitted();
        assert!(close(camera.target, Point3::new(0.0, -100.0, 0.0)));
        // the bounding sphere (radius 150) fits the vertical field of view
        let half = (FOV.to_radians() * 0.5).tan().atan();
        assert!(camera.distance * half.sin() >= 150.0);
        assert!(
            camera.position().y < camera.target.y,
            "home view is from above"
        );
    }

    #[test]
    fn zooming_at_the_centre_keeps_the_target() {
        let mut camera = fitted();
        let before = camera.target;
        camera.zoom(0.5, 400.0, 300.0, 800.0, 600.0);
        assert!(close(camera.target, before));
        assert!((camera.distance - fitted().distance * 0.5).abs() < 1e-2);
    }

    #[test]
    fn zooming_keeps_the_point_under_the_cursor() {
        let mut camera = fitted();
        let (forward, _, _) = camera.basis();
        let ray = camera.ray(700.0, 100.0, 800.0, 600.0);
        let anchor = camera.position() + ray * (camera.distance / ray.dot(forward));
        camera.zoom(0.7, 700.0, 100.0, 800.0, 600.0);
        let after = camera.ray(700.0, 100.0, 800.0, 600.0);
        let to_anchor = (anchor - camera.position()).normalize();
        assert!(after.dot(to_anchor) > 0.9999);
    }

    #[test]
    fn zooming_in_past_the_limit_flies_forward_through_the_model() {
        let mut camera = fitted();
        for _ in 0..200 {
            camera.zoom(0.8, 400.0, 300.0, 800.0, 600.0);
        }
        assert!((camera.distance - camera.min_distance()).abs() < 1e-3);
        // well past the centre of the model, still looking the same way
        let (forward, _, _) = camera.basis();
        let travelled = (camera.target - Point3::new(0.0, -100.0, 0.0)).dot(forward);
        assert!(travelled > 500.0, "{travelled}");
        let (near, far) = camera.clip_planes();
        assert!(near <= 1.0 && far > near);
    }

    #[test]
    fn panning_moves_sideways_only() {
        let mut camera = fitted();
        let (forward, _, _) = camera.basis();
        let before = camera.target;
        camera.pan(50.0, -20.0, 600.0);
        let moved = camera.target - before;
        assert!(moved.magnitude() > 1.0);
        assert!(moved.dot(forward).abs() < 1e-3);
    }

    #[test]
    fn pitch_stays_short_of_the_poles() {
        let mut camera = fitted();
        camera.rotate(0.0, 10_000.0);
        assert!(camera.pitch <= PITCH_LIMIT);
        camera.rotate(0.0, -20_000.0);
        assert!(camera.pitch >= -PITCH_LIMIT);
    }
    #[test]
    fn walk_looks_around_on_the_spot() {
        let mut camera = fitted();
        camera.set_mode(CameraMode::Walk);
        let eye = camera.position();
        camera.look(120.0, -40.0);
        assert!(close(camera.position(), eye));
        assert!(camera.yaw < fitted().yaw, "mouse right turns right");
        assert!(camera.pitch < fitted().pitch, "mouse up looks up");
    }

    #[test]
    fn walking_stays_level_and_goes_up_and_down() {
        let mut camera = fitted(); // looking down at the model
        camera.set_mode(CameraMode::Walk);
        let eye = camera.position();
        camera.walk(1.0, 0.0, 0.0, 100.0);
        let moved = camera.position() - eye;
        assert!(moved.y.abs() < 1e-3 && (moved.magnitude() - 100.0).abs() < 1e-2);
        let (forward, _, _) = camera.basis();
        assert!(moved.dot(forward) > 0.0);
        let eye = camera.position();
        camera.walk(0.0, 0.0, 1.0, 50.0);
        assert!(
            ((camera.position() - eye).y + 50.0).abs() < 1e-3,
            "up is -Y"
        );
        assert!(camera.walk_speed() >= 40.0);
    }

    #[test]
    fn leaving_walk_orbits_where_it_walked_to_or_where_it_was() {
        let mut camera = fitted();
        let target = camera.target;
        camera.set_mode(CameraMode::Walk);
        camera.set_mode(CameraMode::Inspect);
        assert!(
            close(camera.target, target),
            "didn't move: same orbit as before"
        );

        camera.set_mode(CameraMode::Walk);
        camera.walk(1.0, 0.0, 0.0, 300.0);
        let eye = camera.position();
        camera.set_mode(CameraMode::Inspect);
        assert!(close(camera.position(), eye));
        assert!((camera.distance - camera.min_distance()).abs() < 1e-3);
    }

    #[test]
    fn scrolling_in_walk_flies_both_ways() {
        let mut camera = fitted();
        camera.set_mode(CameraMode::Walk);
        let (eye, distance) = (camera.position(), camera.distance);
        camera.zoom(0.8, 400.0, 300.0, 800.0, 600.0);
        let (forward, _, _) = camera.basis();
        assert!((camera.position() - eye).dot(forward) > 0.0);
        assert_eq!(camera.distance, distance);
        camera.zoom(1.25, 400.0, 300.0, 800.0, 600.0);
        camera.zoom(1.25, 400.0, 300.0, 800.0, 600.0);
        assert!((camera.position() - eye).dot(forward) < 0.0);
    }
}
