//! Walking through a model: a first-person camera at a body's eye height.
//!
//! Solid (ghost off), the walker stops at what it meets, slides along it and falls onto the floor
//! under it; with nothing under it, it keeps its height. It rises only up a walkable ramp, at any
//! pace or frame time, or over a step: with stair climbing on, anything lower than a third of its
//! eye height; off, only a floor seam under 5% of it. It never rises into what is over its head. A
//! ghost flies through everything and rises and sinks as asked.
//! Moving is horizontal when solid or level, so it never drifts up or down; a ghost that is not
//! level flies where it looks.
//!
//! The body and the camera turn apart, as a drone and its gimbal do: the heading is the body's,
//! which moving follows, and the camera turns from it. Pitch is the camera's alone.
//!
//! Every distance is a share of the model's radius, so a building and a bracket walk alike, except
//! an eye height set in model units ([`Walk::set_height`]).

use super::axes::Upright;
use super::orbit::{Axes, PITCH_LIMIT};
use crate::render::{Camera, Projection, RenderError};
use crate::vec3::{Vec3, add, cross, dot, length, normalize, scale, sub};
use crate::{Hit, Ray};

/// The walking field of view, degrees, vertically.
const FOV: f64 = 50.0;
/// The widest aspect the vertical field holds for: a wider picture keeps a 16:9 picture's sideways
/// field and narrows the vertical one, so widening it never stretches the edges.
const WIDEST: f64 = 16.0 / 9.0;
/// The widest vertical field zooming out reaches, degrees.
const WIDEST_FOV: f64 = 100.0;
/// The narrowest zoom-in, as a multiple of [`FOV`]'s view.
const ZOOM_IN: f64 = 6.0;
/// Eye height, as a share of the model's radius.
const BODY_SHARE: f64 = 0.08;
/// Walking speed at speed 1, in model radii per second.
const PACE: f64 = 0.25;
/// Running speed, as a multiple of walking.
const RUN: f64 = 3.0;
/// The highest obstacle climbed with stair climbing on, as a share of eye height.
const CLIMB: f64 = 1.0 / 3.0;
/// The highest step climbed with it off: a seam or a threshold, never an object.
const BUMP: f64 = 0.05;
/// A hit whose normal is this close to the up axis is a floor or a ramp, stood on, not walked into.
const WALKABLE: f64 = 0.7;
/// How close a wall stops the eye, as a share of eye height.
const REACH: f64 = 0.25;
/// How far below a ceiling a rising floor stops the eye, as a share of eye height. Small, so a
/// threshold under a door header stays walkable; more than nothing, so an eye that lands exactly at
/// a plate's height counts as blocked instead of passing it.
const HEADROOM: f64 = 0.01;
/// Gravity, in eye heights per second squared: about Earth's for a 1.6 m eye.
const GRAVITY: f64 = 6.0;
/// The longest step [`Walk::step`] takes, seconds; a longer frame is clamped, so a stalled frame
/// never carries the walker through a wall.
const MAX_STEP: f64 = 0.1;
/// The fastest speed a [`Rules::speed`] asks for, as a multiple of walking.
const MAX_SPEED: f64 = 100.0;
/// The rounding allowance on heights, model units.
const EPSILON: f64 = 1e-9;

/// What is asked for in one step: `forward`, `right` and `rise` each from -1 to 1 (a stick's tilt,
/// or a held key's 1). Values outside are clamped and non-finite ones count as 0.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Intent {
    /// Ahead, along the heading.
    pub forward: f64,
    /// To the right of the heading.
    pub right: f64,
    /// Up the up axis; a solid walker ignores it.
    pub rise: f64,
    /// Three times faster.
    pub run: bool,
}

impl Intent {
    fn clamped(self) -> Intent {
        let c = |v: f64| if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 };
        Intent { forward: c(self.forward), right: c(self.right), rise: c(self.rise), run: self.run }
    }
}

/// How walking treats the model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rules {
    /// Walls stop the walker and floors hold it (ghost mode off).
    pub solid: bool,
    /// Move horizontally whatever the camera's pitch.
    pub level: bool,
    /// A multiple of the walking pace, at most 100; a non-finite or negative one stands
    /// still.
    pub speed: f64,
    /// Climb obstacles lower than a third of the eye height.
    pub stairs: bool,
}

impl Default for Rules {
    /// Solid, level, speed 1, no stair climbing.
    fn default() -> Self {
        Rules { solid: true, level: true, speed: 1.0, stairs: false }
    }
}

impl Rules {
    /// The highest step climbed, as a share of eye height.
    fn step(self) -> f64 {
        if self.stairs { CLIMB } else { BUMP }
    }
}

/// The walker.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Walk {
    eye: Vec3,
    /// The body's heading, radians round the up axis.
    yaw: f64,
    /// The camera's turn from the heading, radians.
    look_yaw: f64,
    /// The camera's radians below the horizon.
    pitch: f64,
    /// A multiple of the [`FOV`] view.
    zoom: f64,
    axes: Axes,
    /// The eye height set in model units; `None` for the radius's share.
    height: Option<f64>,
    /// The model's radius, which every distance is a share of.
    scale: f64,
    /// Falling speed, model units per second; 0 on a floor.
    fall: f64,
}

impl Walk {
    /// A walker standing at `eye`, looking along `look`, turning about `upright`'s up axis; `radius`
    /// is the model's. `None` when `eye` or `radius` is not finite, the radius is not positive, or
    /// `look` is zero or not finite; a look along the up axis stops just short of it.
    #[must_use]
    pub fn standing(eye: [f64; 3], look: [f64; 3], upright: Upright, radius: f64) -> Option<Walk> {
        let axes = Axes::upright(upright);
        let (yaw, pitch) = axes.angles(look)?;
        let usable = eye.iter().all(|c| c.is_finite()) && radius.is_finite() && radius > 0.0;
        usable.then_some(Walk { eye, yaw, look_yaw: 0.0, pitch, zoom: 1.0, axes, height: None, scale: radius, fall: 0.0 })
    }

    /// A walker where `camera` is, looking where it looks; as [`Walk::standing`].
    #[must_use]
    pub fn from_camera(camera: &Camera, upright: Upright, radius: f64) -> Option<Walk> {
        Walk::standing(camera.eye, sub(camera.target, camera.eye), upright, radius)
    }

    /// Where the eye is.
    #[must_use]
    pub fn eye(&self) -> [f64; 3] {
        self.eye
    }

    /// The eye's height above a floor, model units.
    #[must_use]
    pub fn body(&self) -> f64 {
        self.height.unwrap_or(self.scale * BODY_SHARE)
    }

    /// Set the eye height in model units; `None`, or a non-finite or non-positive height, goes back
    /// to the share of the model's radius.
    pub fn set_height(&mut self, height: Option<f64>) {
        self.height = height.filter(|h| h.is_finite() && *h > 0.0);
    }

    /// Whether it is still falling, so a viewer must keep stepping.
    #[must_use]
    pub fn falling(&self) -> bool {
        self.fall != 0.0
    }

    /// The camera's pitch below the horizon, radians.
    #[must_use]
    pub fn pitch(&self) -> f64 {
        self.pitch
    }

    /// The camera's zoom, a multiple of the default view.
    #[must_use]
    pub fn zoom(&self) -> f64 {
        self.zoom
    }

    /// The unit direction the camera looks along.
    #[must_use]
    pub fn direction(&self) -> [f64; 3] {
        self.axes.look(self.yaw + self.look_yaw, self.pitch)
    }

    /// The perspective camera for a picture of `aspect`, width / height (a non-finite or
    /// non-positive one counts as 1).
    ///
    /// # Errors
    /// [`RenderError::Camera`] when the view does not form.
    pub fn camera(&self, aspect: f64) -> Result<Camera, RenderError> {
        let aspect = if aspect.is_finite() && aspect > 0.0 { aspect } else { 1.0 };
        let target = add(self.eye, scale(self.direction(), self.body() * 4.0));
        let fov_y = fov_y(FOV / self.zoom, aspect);
        let camera = Camera { eye: self.eye, target, up: self.axes.up, projection: Projection::Perspective { fov_y } };
        if !target.iter().all(|c| c.is_finite()) {
            return Err(RenderError::Camera("the walking view is out of range"));
        }
        camera.basis()?;
        Ok(camera)
    }

    /// Turn the body by `yaw` and the camera's pitch by `pitch` radians; pitch stops short of
    /// straight up or down, and non-finite turns are ignored.
    pub fn turn(&mut self, yaw: f64, pitch: f64) {
        if yaw.is_finite() && pitch.is_finite() {
            self.yaw = wrap(self.yaw + yaw);
            self.tilt(pitch);
        }
    }

    /// Turn the camera alone by `yaw` and `pitch` radians, keeping the heading; non-finite turns are
    /// ignored.
    pub fn look(&mut self, yaw: f64, pitch: f64) {
        if yaw.is_finite() && pitch.is_finite() {
            self.look_yaw = wrap(self.look_yaw + yaw);
            self.tilt(pitch);
        }
    }

    fn tilt(&mut self, pitch: f64) {
        self.pitch = (self.pitch + pitch).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    /// Turn the camera back to the heading.
    pub fn centre_look(&mut self) {
        self.look_yaw = 0.0;
    }

    /// Level the camera.
    pub fn level(&mut self) {
        self.pitch = 0.0;
    }

    /// Multiply the zoom by `factor`, out to a 100 degree view and in to six times
    /// closer; a non-finite or non-positive factor is ignored.
    pub fn zoom_by(&mut self, factor: f64) {
        if factor.is_finite() && factor > 0.0 {
            self.zoom = (self.zoom * factor).clamp(FOV / WIDEST_FOV, ZOOM_IN);
        }
    }

    /// Head and look along `direction`; a zero or non-finite one changes nothing.
    pub fn face(&mut self, direction: [f64; 3]) {
        if let Some((yaw, pitch)) = self.axes.angles(direction) {
            (self.yaw, self.look_yaw, self.pitch) = (yaw, 0.0, pitch);
        }
    }

    /// Move for `dt` seconds (at most 0.1; a non-finite or negative `dt` is none) of
    /// `intent`. `world` is the model's nearest hit along a ray (a [`crate::PickIndex`] or
    /// [`crate::pick`]); it is asked only when solid, and never about a non-finite ray, which counts
    /// as hitting nothing. A solid walker ignores `intent.rise`, so it can never lift itself over
    /// what it would meet.
    pub fn step(&mut self, intent: Intent, dt: f64, rules: Rules, world: &mut impl FnMut(&Ray) -> Option<Hit>) {
        let dt = if dt.is_finite() { dt.clamp(0.0, MAX_STEP) } else { 0.0 };
        let speed = if rules.speed.is_finite() { rules.speed.clamp(0.0, MAX_SPEED) } else { 0.0 };
        let intent = intent.clamped();
        let pace = self.scale * PACE * speed * if intent.run { RUN } else { 1.0 };
        let world = &mut |ray: &Ray| {
            let finite = ray.origin.iter().chain(&ray.direction).all(|c| c.is_finite());
            if finite { world(ray) } else { None }
        };
        self.travel([intent.forward * pace * dt, intent.right * pace * dt], rules, world);
        if rules.solid {
            self.settle(dt, world);
        } else {
            self.move_to(along(self.eye, self.axes.up, intent.rise * pace * dt));
        }
    }

    /// Move `forward` and `right`, model units, sliding along any wall met.
    fn travel(&mut self, [forward, right]: [f64; 2], rules: Rules, world: &mut impl FnMut(&Ray) -> Option<Hit>) {
        let flat = self.axes.look(self.yaw, 0.0);
        let ahead = if rules.solid || rules.level { flat } else { self.axes.look(self.yaw, self.pitch) };
        let side = cross(flat, self.axes.up);
        let mut motion = add(scale(ahead, forward), scale(side, right));
        if rules.solid {
            motion = self.slide(motion, rules.step(), world);
            if self.lifts(motion, rules.step(), world) {
                motion = [0.0; 3];
            }
        }
        self.move_to(add(self.eye, motion));
    }

    /// Take `eye` as the new eye when it is finite.
    fn move_to(&mut self, eye: Vec3) {
        if eye.iter().all(|c| c.is_finite()) {
            self.eye = eye;
        }
    }

    /// The floor under `origin`.
    fn down(&self, origin: Vec3, world: &mut impl FnMut(&Ray) -> Option<Hit>) -> Option<Hit> {
        world(&Ray { origin, direction: scale(self.axes.up, -1.0), min_distance: 0.0 })
    }

    /// The rise per unit of horizontal travel of a walkable floor; 0 for any other surface.
    fn slope(&self, floor: &Hit) -> f64 {
        let flat = dot(floor.normal, self.axes.up).abs();
        if flat < WALKABLE { 0.0 } else { (1.0 - flat * flat).max(0.0).sqrt() / flat }
    }

    /// Whether `rise` straight above `origin` leaves the eye [`HEADROOM`] of its height clear of anything
    /// over it. A surface `origin` is on does not count, so a walker placed on the floor stands up.
    fn head_room(&self, origin: Vec3, rise: f64, world: &mut impl FnMut(&Ray) -> Option<Hit>) -> bool {
        world(&Ray { origin, direction: self.axes.up, min_distance: EPSILON }).is_none_or(|hit| hit.distance > rise + self.body() * HEADROOM)
    }

    /// Whether the straight way from the eye to `to` is clear; a way too long to measure counts as
    /// clear, since the eye won't take it. Only [`Self::head_room`] keeps a margin: a plate exactly at
    /// `to` is straight above the floor point too.
    fn way_clear(&self, to: Vec3, world: &mut impl FnMut(&Ray) -> Option<Hit>) -> bool {
        let way = sub(to, self.eye);
        let Some(direction) = normalize(way) else {
            return true;
        };
        world(&Ray { origin: self.eye, direction, min_distance: EPSILON }).is_none_or(|hit| hit.distance > length(way))
    }

    /// Whether `motion` would stand the eye higher than standing here does, and either by more than
    /// `step` of its height and than the walkable floors under its two ends rise over that distance,
    /// or into what is over its head. The wall rays see only two heights, so a slab between them, a
    /// rounded front or a wall behind a ramp's lip passes them; the floor under the far end does not.
    fn lifts(&self, motion: Vec3, step: f64, world: &mut impl FnMut(&Ray) -> Option<Hit>) -> bool {
        let to = add(self.eye, motion);
        let here = self.down(self.eye, world);
        let Some(there) = self.down(to, world) else {
            return false;
        };
        if here.as_ref().is_some_and(|floor| there.distance >= floor.distance - EPSILON) {
            return false;
        }
        let slope = here.as_ref().map_or(0.0, |floor| self.slope(floor)).max(self.slope(&there));
        let allowed = (step * self.body()).max(length(motion) * slope);
        let rise = self.body() - there.distance;
        rise > allowed + EPSILON || (rise > 0.0 && !(self.head_room(to, rise, world) && self.way_clear(along(to, self.axes.up, rise), world)))
    }

    /// `motion` with the part into the nearest wall in reach dropped, or none when the rest meets
    /// a wall too.
    fn slide(&self, motion: Vec3, step: f64, world: &mut impl FnMut(&Ray) -> Option<Hit>) -> Vec3 {
        let Some(wall) = self.blocked(motion, step, world) else {
            return motion;
        };
        let along_wall = sub(motion, scale(wall, dot(motion, wall)));
        if self.blocked(along_wall, step, world).is_some() { [0.0; 3] } else { along_wall }
    }

    /// The horizontal unit normal, facing the walker, of the nearest wall `motion` comes within
    /// reach of, at eye height or at `step` of it above the floor; `None` when the way is clear. A
    /// floor or ramp met is no wall: [`Walk::settle`] stands on it.
    fn blocked(&self, motion: Vec3, step: f64, world: &mut impl FnMut(&Ray) -> Option<Hit>) -> Option<Vec3> {
        let direction = normalize(motion)?;
        let limit = length(motion) + self.body() * REACH;
        let up = self.axes.up;
        let knee = along(self.eye, up, -self.body() * (1.0 - step));
        let hit = [self.eye, knee]
            .into_iter()
            .filter_map(|origin| world(&Ray { origin, direction, min_distance: 0.0 }))
            .filter(|hit| hit.distance < limit && dot(hit.normal, up).abs() < WALKABLE)
            .min_by(|a, b| a.distance.total_cmp(&b.distance))?;
        let flat = sub(hit.normal, scale(up, dot(hit.normal, up)));
        let n = normalize(flat).unwrap_or(scale(direction, -1.0));
        Some(if dot(n, direction) > 0.0 { scale(n, -1.0) } else { n })
    }

    /// Stand on the floor under the eye, falling onto it from above and stepping up onto it from
    /// below, unless that would put the eye into what is over its head. With no floor under it the
    /// walker keeps its height.
    fn settle(&mut self, dt: f64, world: &mut impl FnMut(&Ray) -> Option<Hit>) {
        let Some(floor) = self.down(self.eye, world) else {
            self.fall = 0.0;
            return;
        };
        let above = floor.distance - self.body();
        if above <= 0.0 {
            if self.head_room(self.eye, -above, world) {
                self.move_to(along(self.eye, self.axes.up, -above));
            }
            self.fall = 0.0;
            return;
        }
        self.fall += GRAVITY * self.body() * dt;
        let drop = (self.fall * dt).min(above);
        self.move_to(along(self.eye, self.axes.up, -drop));
        if drop >= above {
            self.fall = 0.0;
        }
    }

    /// Go to `hit`, the surface under a picked point seen from `from`: standing on it when it faces
    /// up, else an eye height back from it towards `from`.
    pub fn go_to(&mut self, hit: &Hit, from: [f64; 3]) {
        let up = self.axes.up;
        let back = normalize(sub(from, hit.point)).unwrap_or(up);
        let facing = if dot(hit.normal, back) < 0.0 { scale(hit.normal, -1.0) } else { hit.normal };
        let toward = if dot(facing, up) > WALKABLE { up } else { back };
        self.move_to(along(hit.point, toward, self.body()));
        self.fall = 0.0;
    }
}

/// `angle` in [-π, π].
fn wrap(angle: f64) -> f64 {
    use std::f64::consts::{PI, TAU};
    (angle + PI).rem_euclid(TAU) - PI
}

fn along(point: Vec3, direction: Vec3, distance: f64) -> Vec3 {
    add(point, scale(direction, distance))
}

/// `natural`'s vertical field, degrees, narrowed past [`WIDEST`] so the sideways field stops growing
/// with the picture's width.
fn fov_y(natural: f64, aspect: f64) -> f64 {
    if aspect <= WIDEST {
        return natural;
    }
    let half = (natural.to_radians() / 2.0).tan() * WIDEST / aspect;
    2.0 * half.atan().to_degrees()
}

#[cfg(test)]
#[path = "walk_tests.rs"]
mod tests;
