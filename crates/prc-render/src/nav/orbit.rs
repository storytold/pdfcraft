//! Orbiting: a turn about an up axis, a zoom and a pan, each relative to the view that fits the
//! model or, once [`Orbit::anchor_at`] picks a model point, relative to that point.
//!
//! The fitted view frames the sphere round the model's centre that holds every vertex
//! ([`reach`]), so turning the model or reshaping the picture never changes its size.

use std::f64::consts::{FRAC_PI_2, TAU};

use super::axes::Upright;
use crate::WorldMesh;
use crate::render::{Bounds, Camera, Projection, RenderError, vertices};
use crate::vec3::{Vec3, add, cross, dot, length, normalize, scale, sub};

/// How far the view may look up or down, radians: kept off ±90° so it never looks along the up axis.
pub const PITCH_LIMIT: f64 = FRAC_PI_2 - 0.01;
/// The zoom range, as a multiple of the fitted view.
pub const ZOOM_RANGE: (f64, f64) = (0.05, 50.0);
/// How far the pan may move the target, in model radii.
const PAN_LIMIT: f64 = 1e3;
/// The perspective field of view, degrees, vertically.
const FOV_Y: f64 = 30.0;
/// The clear space round the fitted sphere, as a multiple of its radius.
const MARGIN: f64 = 1.05;

/// A named view, turned about an [`Upright`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedView {
    /// From the front, right and above, as an isometric drawing looks.
    Isometric,
    /// Along the Front direction.
    Front,
    /// From the right, a quarter turn from Front.
    Right,
    /// Looking down the up axis, just short of it.
    Top,
    /// Against the Front direction.
    Back,
}

impl NamedView {
    /// Every named view, in [`NamedView::token`] order.
    pub const ALL: [NamedView; 5] = [NamedView::Isometric, NamedView::Front, NamedView::Right, NamedView::Top, NamedView::Back];

    /// Its token: `iso`, `front`, `right`, `top` or `back`.
    #[must_use]
    pub fn token(self) -> &'static str {
        match self {
            NamedView::Isometric => "iso",
            NamedView::Front => "front",
            NamedView::Right => "right",
            NamedView::Top => "top",
            NamedView::Back => "back",
        }
    }

    /// The view whose [`NamedView::token`] is `token`.
    #[must_use]
    pub fn from_token(token: &str) -> Option<NamedView> {
        NamedView::ALL.into_iter().find(|v| v.token() == token)
    }

    /// (yaw, pitch) in degrees: yaw 0 looks along Front, positive pitch looks down.
    fn angles(self) -> (f64, f64) {
        match self {
            NamedView::Isometric => (45.0, 35.264),
            NamedView::Front => (0.0, 0.0),
            NamedView::Right => (90.0, 0.0),
            NamedView::Top => (0.0, 89.0),
            NamedView::Back => (180.0, 0.0),
        }
    }
}

/// The model point an orbit turns about: its depth along the look, and its offset right and up
/// from the look's axis in model units. An orthographic view shows the height a perspective one
/// would at that depth, so switching projection keeps the scale.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Anchor {
    point: Vec3,
    distance: f64,
    offset: [f64; 2],
}

impl Anchor {
    /// The point on the look's axis level with the anchor, for picture axes `right` and `up`.
    fn base(&self, right: Vec3, up: Vec3) -> Vec3 {
        let [x, y] = self.offset;
        sub(sub(self.point, scale(right, x)), scale(up, y))
    }
}

/// Yaw 0, pitch 0 looks along `front`, and `up` appears upward; both unit and perpendicular.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Axes {
    front: Vec3,
    up: Vec3,
}

impl Axes {
    /// The unit direction `yaw` round and `pitch` down from `front`.
    fn look(&self, yaw: f64, pitch: f64) -> Vec3 {
        let (sy, cy) = yaw.sin_cos();
        let (sp, cp) = pitch.sin_cos();
        let right = cross(self.front, self.up);
        add(add(scale(right, -sy * cp), scale(self.front, cy * cp)), scale(self.up, -sp))
    }

    /// The yaw and pitch [`Axes::look`] turns into `direction`, the pitch held inside
    /// [`PITCH_LIMIT`]; `None` for a zero or non-finite direction.
    fn angles(&self, direction: Vec3) -> Option<(f64, f64)> {
        let d = normalize(direction)?;
        let pitch = (-dot(d, self.up)).clamp(-1.0, 1.0).asin();
        let yaw = (-dot(d, cross(self.front, self.up))).atan2(dot(d, self.front));
        Some((yaw, pitch.clamp(-PITCH_LIMIT, PITCH_LIMIT)))
    }

    /// The picture's right and up when looking along `dir`; `None` when `dir` lies along up.
    fn picture(&self, dir: Vec3) -> Option<(Vec3, Vec3)> {
        let right = normalize(cross(dir, self.up))?;
        Some((right, cross(right, dir)))
    }
}

/// Where the camera is, relative to the fitted view or to the point turned about.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Orbit {
    yaw: f64,
    pitch: f64,
    zoom: f64,
    pan: [f64; 2],
    perspective: bool,
    axes: Axes,
    anchor: Option<Anchor>,
}

impl Orbit {
    /// `view`, turned about `upright`.
    #[must_use]
    pub fn named(view: NamedView, perspective: bool, upright: Upright) -> Orbit {
        let (yaw, pitch) = view.angles();
        let axes = Axes { front: upright.front().vector(), up: upright.up().vector() };
        Orbit { yaw: yaw.to_radians(), pitch: pitch.to_radians(), zoom: 1.0, pan: [0.0; 2], perspective, axes, anchor: None }
    }

    /// Looking along `direction`, turning about `up` with the part along `direction` removed; `None`
    /// when either is zero or not finite, or they are parallel.
    #[must_use]
    pub fn looking(direction: [f64; 3], up: [f64; 3], perspective: bool) -> Option<Orbit> {
        let front = normalize(direction)?;
        let up = normalize(up)?;
        let up = normalize(sub(up, scale(front, dot(up, front))))?;
        Some(Orbit { yaw: 0.0, pitch: 0.0, zoom: 1.0, pan: [0.0; 2], perspective, axes: Axes { front, up }, anchor: None })
    }

    /// Radians round the up axis, in `[0, 2π)` once turned.
    #[must_use]
    pub fn yaw(&self) -> f64 {
        self.yaw
    }

    /// Radians above the horizon, looking down when positive; within [`PITCH_LIMIT`].
    #[must_use]
    pub fn pitch(&self) -> f64 {
        self.pitch
    }

    /// The zoom, as a multiple of the fitted view; within [`ZOOM_RANGE`].
    #[must_use]
    pub fn zoom(&self) -> f64 {
        self.zoom
    }

    /// The target's shift along the picture's right and up, in model radii.
    #[must_use]
    pub fn pan(&self) -> [f64; 2] {
        self.pan
    }

    /// Whether the projection is perspective rather than orthographic.
    #[must_use]
    pub fn perspective(&self) -> bool {
        self.perspective
    }

    /// The point turned about, once [`Orbit::anchor_at`] has picked one.
    #[must_use]
    pub fn anchor(&self) -> Option<[f64; 3]> {
        self.anchor.map(|a| a.point)
    }

    /// Switch projection; an anchored view keeps its scale.
    pub fn set_perspective(&mut self, perspective: bool) {
        self.perspective = perspective;
    }

    /// The unit direction the camera looks along.
    #[must_use]
    pub fn direction(&self) -> [f64; 3] {
        self.axes.look(self.yaw, self.pitch)
    }

    /// Turn by `yaw` round the up axis and `pitch` down, radians; non-finite turns are ignored.
    pub fn turn(&mut self, yaw: f64, pitch: f64) {
        if yaw.is_finite() && pitch.is_finite() {
            self.yaw = (self.yaw + yaw).rem_euclid(TAU);
            self.pitch = (self.pitch + pitch).clamp(-PITCH_LIMIT, PITCH_LIMIT);
        }
    }

    /// Move the target by `[right, up]` model radii; non-finite moves are ignored.
    pub fn pan_by(&mut self, [right, up]: [f64; 2]) {
        if right.is_finite() && up.is_finite() {
            self.set_pan([self.pan[0] + right, self.pan[1] + up]);
        }
    }

    /// Look along `direction`, fitted again; a direction along the up axis stops just short of it,
    /// and a zero or non-finite one changes nothing.
    pub fn look_along(&mut self, direction: [f64; 3]) {
        if let Some((yaw, pitch)) = self.axes.angles(direction) {
            (self.yaw, self.pitch) = (yaw, pitch);
            self.refit();
        }
    }

    /// Back to the view that fits the whole model, keeping the direction: no zoom, pan or anchor.
    pub fn refit(&mut self) {
        (self.zoom, self.pan, self.anchor) = (1.0, [0.0; 2], None);
    }

    /// Turn about `point` from now on without moving the picture; `camera` is this orbit's
    /// camera, and `point` stays where it shows on it. Whether the point was taken: not when it
    /// is not finite, or lies at or behind the eye of a perspective view.
    pub fn anchor_at(&mut self, camera: &Camera, point: [f64; 3]) -> bool {
        let dir = self.direction();
        let Some((right, image_up)) = self.axes.picture(dir) else { return false };
        let v = sub(point, camera.eye);
        let offset = [dot(v, right), dot(v, image_up)];
        let distance = match camera.projection {
            Projection::Orthographic { height } => height / 2.0 / half_fov().tan(),
            Projection::Perspective { .. } => dot(v, dir),
        };
        let taken = distance.is_finite() && distance > 0.0 && offset.iter().all(|c| c.is_finite());
        if taken {
            self.anchor = Some(Anchor { point, distance, offset });
            (self.zoom, self.pan) = (1.0, [0.0; 2]);
        }
        taken
    }

    /// Pan so `point` sits at the picture's centre, keeping the turn and the zoom.
    pub fn centre_on(&mut self, bounds: &Bounds, point: [f64; 3]) {
        let Some((right, image_up)) = self.axes.picture(self.direction()) else { return };
        let centre = self.anchor.map_or_else(|| bounds.centre(), |a| a.base(right, image_up));
        let off = sub(point, centre);
        let r = radius(bounds);
        let pan = [dot(off, right) / r, dot(off, image_up) / r];
        if pan.iter().all(|c| c.is_finite()) {
            self.set_pan(pan);
        }
    }

    /// Multiply the zoom by `factor` (held within [`ZOOM_RANGE`]), keeping the model point under
    /// the pointer where it is on the picture. `offset` is the pointer's position from the
    /// picture's centre, right and up, in picture heights; `bounds`, `reach` and `aspect` are as
    /// [`Orbit::camera`] takes them. Exact on the plane through the target facing the camera, the
    /// plane the pan moves in. A factor that is not positive and finite changes nothing.
    pub fn zoom_at(&mut self, bounds: &Bounds, reach: f64, aspect: f64, factor: f64, offset: [f64; 2]) {
        if !(factor.is_finite() && factor > 0.0 && offset.iter().all(|c| c.is_finite())) {
            return;
        }
        let before = self.zoom;
        let after = (before * factor).clamp(ZOOM_RANGE.0, ZOOM_RANGE.1);
        let fitted = Orbit { zoom: 1.0, pan: [0.0; 2], ..*self };
        // What one picture height spans at zoom 1, in model radii.
        let span = fitted.camera(bounds, reach, aspect).map_or(2.0, |c| visible_height(&c)) / radius(bounds);
        let shift = span * (1.0 / before - 1.0 / after);
        let [x, y] = offset;
        self.set_pan([self.pan[0] + x * shift, self.pan[1] + y * shift]);
        self.zoom = after;
    }

    /// The camera for a model in `bounds` whose every vertex lies within `reach` of its centre
    /// ([`reach`]; [`Bounds::radius`] stands in when it is not positive), for a picture of `aspect`
    /// (width / height; 1 stands in when it is not positive).
    ///
    /// # Errors
    /// [`RenderError::Camera`] when the view does not form: a position out of the finite range.
    pub fn camera(&self, bounds: &Bounds, reach: f64, aspect: f64) -> Result<Camera, RenderError> {
        let dir = self.direction();
        let (right, image_up) = self.axes.picture(dir).ok_or(RenderError::Camera("the view looks along its up axis"))?;
        let up = self.axes.up;
        let mut camera = match self.anchor {
            Some(a) => about(a, dir, (right, image_up), up, self.perspective),
            None => {
                let reach = if reach.is_finite() && reach > 0.0 { reach } else { radius(bounds) };
                fitted(bounds.centre(), reach, dir, up, self.perspective, aspect)
            }
        };
        let [x, y] = self.pan;
        let target = add(camera.target, scale(add(scale(right, x), scale(image_up, y)), radius(bounds)));
        camera.eye = add(target, scale(sub(camera.eye, camera.target), 1.0 / self.zoom));
        camera.target = target;
        if let Projection::Orthographic { height } = camera.projection {
            camera.projection = Projection::Orthographic { height: height / self.zoom };
        }
        let finite = camera.eye.iter().chain(&camera.target).all(|c| c.is_finite())
            && match camera.projection {
                Projection::Orthographic { height } => height.is_finite() && height > 0.0,
                Projection::Perspective { .. } => true,
            };
        if !finite {
            return Err(RenderError::Camera("the view is out of range"));
        }
        camera.basis()?;
        Ok(camera)
    }

    /// The pan, each part held within [`PAN_LIMIT`].
    fn set_pan(&mut self, pan: [f64; 2]) {
        self.pan = pan.map(|c| c.clamp(-PAN_LIMIT, PAN_LIMIT));
    }
}

/// How far the farthest vertex the meshes draw lies from `bounds`' centre: the radius of the
/// sphere [`Orbit::camera`] fits. Half the box's diagonal when there is none.
#[must_use]
pub fn reach(meshes: &[WorldMesh], bounds: &Bounds) -> f64 {
    let c = bounds.centre();
    let far = vertices(meshes).map(|p| length(sub(p, c))).filter(|d| d.is_finite()).fold(0.0_f64, f64::max);
    if far > 0.0 { far } else { radius(bounds) }
}

/// Half the vertical field of view, radians.
fn half_fov() -> f64 {
    (FOV_Y / 2.0).to_radians()
}

/// The camera looking along `dir` that holds the sphere of `reach` round `centre` in a picture of
/// `aspect`, whichever way it looks; only the picture's shorter side changes the model's size.
fn fitted(centre: Vec3, reach: f64, dir: Vec3, up: Vec3, perspective: bool, aspect: f64) -> Camera {
    let r = reach * MARGIN;
    let aspect = if aspect.is_finite() && aspect > 0.0 { aspect } else { 1.0 };
    let half = half_fov().min((half_fov().tan() * aspect).atan());
    let (distance, projection) = if perspective {
        (r / half.sin(), Projection::Perspective { fov_y: FOV_Y })
    } else {
        (2.0 * r, Projection::Orthographic { height: 2.0 * r / aspect.min(1.0) })
    };
    Camera { eye: sub(centre, scale(dir, distance)), target: centre, up, projection }
}

/// The camera looking along `dir` that shows `anchor`'s point at its depth and offset.
fn about(anchor: Anchor, dir: Vec3, (right, image_up): (Vec3, Vec3), up: Vec3, perspective: bool) -> Camera {
    let base = anchor.base(right, image_up);
    let projection = if perspective {
        Projection::Perspective { fov_y: FOV_Y }
    } else {
        Projection::Orthographic { height: 2.0 * anchor.distance * half_fov().tan() }
    };
    Camera { eye: sub(base, scale(dir, anchor.distance)), target: base, up, projection }
}

/// The model's radius, or 1 for a model with no extent.
fn radius(bounds: &Bounds) -> f64 {
    match bounds.radius() {
        r if r.is_finite() && r > 0.0 => r,
        _ => 1.0,
    }
}

/// How much of the target plane the picture shows vertically, in model units.
fn visible_height(camera: &Camera) -> f64 {
    match camera.projection {
        Projection::Orthographic { height } => height,
        Projection::Perspective { fov_y } => 2.0 * length(sub(camera.eye, camera.target)) * (fov_y.min(179.0).to_radians() / 2.0).tan(),
    }
}

#[cfg(test)]
#[path = "orbit_tests.rs"]
mod tests;
