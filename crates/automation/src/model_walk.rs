//! `model_render`'s and `model_pick`'s `walk`: the camera of a walker stepped through the model.

use pdfcraft_prc_render::nav::{Axis, Intent, Rules, Upright, Walk, reach};
use pdfcraft_prc_render::{Bounds, Camera, PickIndex, WorldMesh};
use serde_json::{Map, Value};

use super::error::{Result, failed};
use super::json::Args;

/// The keys `walk` takes.
const WALK_KEYS: [&str; 8] = ["from", "look", "up", "height", "ghost", "level", "stairs", "steps"];
/// The keys a step takes.
const STEP_KEYS: [&str; 7] = ["forward", "right", "rise", "run", "turn", "look", "seconds"];
/// The most steps `walk` takes, and the longest walk, seconds.
const MAX_STEPS: usize = 1000;
const MAX_SECONDS: f64 = 600.0;
/// The time step a step's seconds are walked in.
const TICK: f64 = 0.05;

fn wrong(what: &str) -> super::error::ToolError {
    Args::wrong("walk", what)
}

/// The camera `walk` ends at: a walker standing at `from`, looking along `look` (default towards
/// the model's centre), about the `up` axis, then stepped through `steps` against the model.
pub(crate) fn walk_camera(spec: &Value, meshes: &[WorldMesh], aspect: f64) -> Result<Camera> {
    let spec = spec.as_object().ok_or_else(|| wrong("an object"))?;
    known_keys(spec, &WALK_KEYS, "")?;
    let bounds = Bounds::of(meshes).ok_or_else(|| failed("the model has no vertex"))?;
    let from = spec.get("from").and_then(triple).ok_or_else(|| wrong("an object with from as [x, y, z]"))?;
    let look = match spec.get("look") {
        None => {
            let ([cx, cy, cz], [fx, fy, fz]) = (bounds.centre(), from);
            [cx - fx, cy - fy, cz - fz]
        }
        Some(v) => triple(v).ok_or_else(|| wrong("look as [x, y, z]"))?,
    };
    let up = match spec.get("up") {
        None => Axis::PosZ,
        Some(v) => v.as_str().and_then(Axis::from_token).ok_or_else(|| wrong("up as one of +x, +y, +z, -x, -y, -z"))?,
    };
    let mut walk = Walk::standing(from, look, Upright::default().with_up(up), reach(meshes, &bounds))
        .ok_or_else(|| wrong("a look that is not zero, from a finite model"))?;
    if let Some(v) = spec.get("height") {
        walk.set_height(Some(v.as_f64().filter(|h| h.is_finite() && *h > 0.0).ok_or_else(|| wrong("height as a positive number"))?));
    }
    let flag = |key: &str, default: bool| spec.get(key).map_or(Ok(default), |v| v.as_bool().ok_or_else(|| wrong(&format!("{key} as true or false"))));
    let rules = Rules { solid: !flag("ghost", false)?, level: flag("level", true)?, speed: 1.0, stairs: flag("stairs", false)? };
    let steps = match spec.get("steps") {
        None => &Vec::new(),
        Some(v) => v.as_array().filter(|s| s.len() <= MAX_STEPS).ok_or_else(|| wrong(&format!("steps as a list of at most {MAX_STEPS}")))?,
    };
    let index = PickIndex::new(meshes);
    let mut world = |ray: &_| index.pick(ray);
    let mut total = 0.0;
    for step in steps {
        let step = step.as_object().ok_or_else(|| wrong("each step an object"))?;
        known_keys(step, &STEP_KEYS, "step ")?;
        if let Some([yaw, pitch]) = angles(step, "turn")? {
            walk.turn(yaw.to_radians(), pitch.to_radians());
        }
        if let Some([yaw, pitch]) = angles(step, "look")? {
            walk.look(yaw.to_radians(), pitch.to_radians());
        }
        let seconds = number(step, "seconds", 0.0)?;
        total += seconds;
        if !(0.0..=MAX_SECONDS).contains(&seconds) || total > MAX_SECONDS {
            return Err(wrong(&format!("steps whose seconds are not negative and add up to at most {MAX_SECONDS}")));
        }
        let intent = Intent {
            forward: number(step, "forward", 0.0)?,
            right: number(step, "right", 0.0)?,
            rise: number(step, "rise", 0.0)?,
            run: step.get("run").map_or(Ok(false), |v| v.as_bool().ok_or_else(|| wrong("a step's run as true or false")))?,
        };
        let mut left = seconds;
        while left > 0.0 {
            walk.step(intent, left.min(TICK), rules, &mut world);
            left -= TICK;
        }
    }
    walk.camera(aspect).map_err(failed)
}

fn known_keys(object: &Map<String, Value>, keys: &[&str], what: &str) -> Result<()> {
    match object.keys().find(|k| !keys.contains(&k.as_str())) {
        Some(key) => Err(wrong(&format!("a {what}object without {key}; its keys are {}", keys.join(", ")))),
        None => Ok(()),
    }
}

/// A step's `key`: a number from -1 to 1 for the moves, or any finite non-negative one for seconds.
fn number(step: &Map<String, Value>, key: &str, default: f64) -> Result<f64> {
    let Some(v) = step.get(key) else { return Ok(default) };
    let ok = |f: &f64| f.is_finite() && (key == "seconds" || (-1.0..=1.0).contains(f));
    v.as_f64().filter(ok).ok_or_else(|| wrong(&format!("a step's {key} as a number{}", if key == "seconds" { "" } else { " from -1 to 1" })))
}

/// A step's `key` as [yaw, pitch] in degrees.
fn angles(step: &Map<String, Value>, key: &str) -> Result<Option<[f64; 2]>> {
    let Some(v) = step.get(key) else { return Ok(None) };
    let wrong_angles = || wrong(&format!("a step's {key} as [yaw, pitch] in degrees"));
    match v.as_array().map(Vec::as_slice) {
        Some([a, b]) => {
            Ok(Some([a.as_f64().filter(|f| f.is_finite()).ok_or_else(wrong_angles)?, b.as_f64().filter(|f| f.is_finite()).ok_or_else(wrong_angles)?]))
        }
        _ => Err(wrong_angles()),
    }
}

/// Three finite numbers.
fn triple(v: &Value) -> Option<[f64; 3]> {
    match v.as_array()?.as_slice() {
        [a, b, c] => Some([a.as_f64().filter(|f| f.is_finite())?, b.as_f64().filter(|f| f.is_finite())?, c.as_f64().filter(|f| f.is_finite())?]),
        _ => None,
    }
}
