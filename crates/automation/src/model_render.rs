//! `model_render`: a page's 3D model (ISO 32000-2 §13.6) drawn to a PNG, without a viewer.

use craft_prc::PrcFile;
use pdfcraft_engine::model3d::Model3dKind;
use pdfcraft_prc_render::nav::{Axis, NamedView, Orbit, Upright, reach};
use pdfcraft_prc_render::{Bounds, Camera, RenderOptions, WorldMesh, assemble, render};
use serde_json::Value;

use super::Automation;
use super::dispatch::Content;
use super::error::{Result, ToolError, failed};
use super::json::Args;
use super::pages::encode_png;

/// Default and largest side of the image, in pixels.
pub(crate) const DEFAULT_SIDE: u32 = 512;
pub(crate) const MAX_SIDE: u32 = 4096;

/// The default view: from the front, right and above, as an isometric view looks.
const DEFAULT_DIRECTION: [f64; 3] = [-1.0, 1.0, -1.0];

impl Automation {
    pub(crate) fn model_render(&mut self, a: &Args) -> Result<Vec<Content>> {
        let page = self.page(a)?;
        let width = side(a, "width")?;
        let height = side(a, "height")?;
        let perspective = a.opt_bool("perspective")?.unwrap_or(true);
        let doc = self.doc(a)?;
        let found = doc.model_3d_annotations(page).map_err(failed)?;
        let index = match a.opt_int("model")? {
            None => *found.first().ok_or_else(|| failed(format!("page {} has no 3D model", page.saturating_add(1))))?,
            Some(n) => {
                let pos = usize::try_from(n).ok().and_then(|n| n.checked_sub(1)).ok_or_else(|| Args::wrong("model", "a positive integer"))?;
                *found
                    .get(pos)
                    .ok_or_else(|| ToolError::InvalidArgs(format!("page {} has {} 3D models, not {n}", page.saturating_add(1), found.len())))?
            }
        };
        let model = doc.model_3d(page, index).map_err(failed)?;
        if model.kind == Model3dKind::U3d {
            return Err(failed("this 3D model is in U3D, which can't be drawn yet; only PRC models can"));
        }
        let prc = PrcFile::parse(&model.data).map_err(|e| failed(format!("the 3D model could not be read: {e}")))?;
        let meshes = assemble(&prc).map_err(failed)?;
        let aspect = f64::from(width) / f64::from(height);
        let camera = match a.get("orbit") {
            Some(_) if a.get("direction").is_some() => return Err(Args::wrong("orbit", "left out when direction is given")),
            Some(moves) => orbit_camera(moves, &meshes, perspective, aspect)?,
            None => {
                let direction = direction(a)?;
                Camera::fit_meshes(&meshes, direction, up_for(direction), perspective, aspect).map_err(failed)?
            }
        };
        let image = render(&meshes, &camera, &RenderOptions { width, height, ..RenderOptions::default() }).map_err(failed)?;
        let data = encode_png(image.width, image.height, &premultiplied(image.rgba))?;
        Ok(vec![Content::Png { data, width: image.width, height: image.height }])
    }
}

fn side(a: &Args, key: &str) -> Result<u32> {
    match a.opt_int(key)? {
        None => Ok(DEFAULT_SIDE),
        Some(n) => u32::try_from(n).ok().filter(|n| (1..=MAX_SIDE).contains(n)).ok_or_else(|| Args::wrong(key, &format!("between 1 and {MAX_SIDE}"))),
    }
}

/// `direction`: three finite numbers, not all zero, from the eye towards the model.
fn direction(a: &Args) -> Result<[f64; 3]> {
    let Some(v) = a.get("direction") else { return Ok(DEFAULT_DIRECTION) };
    let wrong = || Args::wrong("direction", "three numbers [x, y, z], not all zero");
    let items = v.as_array().filter(|x| x.len() == 3).ok_or_else(wrong)?;
    let mut out = [0.0; 3];
    for (o, x) in out.iter_mut().zip(items) {
        *o = x.as_f64().filter(|f| f.is_finite()).ok_or_else(wrong)?;
    }
    if out.iter().all(|c| *c == 0.0) {
        return Err(wrong());
    }
    Ok(out)
}

/// The keys `orbit` takes.
const ORBIT_KEYS: [&str; 7] = ["view", "up", "front", "turn", "zoom", "zoom_at", "pan"];

/// The camera `orbit` names: a named view about an up axis, then a turn, a zoom and a pan, as a
/// viewer's orbit control would apply them.
fn orbit_camera(moves: &Value, meshes: &[WorldMesh], perspective: bool, aspect: f64) -> Result<Camera> {
    let wrong = |what: &str| Args::wrong("orbit", what);
    let moves = moves.as_object().ok_or_else(|| wrong("an object"))?;
    if let Some(key) = moves.keys().find(|k| !ORBIT_KEYS.contains(&k.as_str())) {
        return Err(wrong(&format!("an object without {key}; its keys are {}", ORBIT_KEYS.join(", "))));
    }
    let token = |key: &str| moves.get(key).map(|v| v.as_str().ok_or_else(|| wrong(&format!("{key} as a string"))));
    let axis = |key: &str| token(key).map(|t| Axis::from_token(t?).ok_or_else(|| wrong(&format!("{key} as one of +x, +y, +z, -x, -y, -z"))));
    let mut upright = Upright::default();
    if let Some(up) = axis("up") {
        upright = upright.with_up(up?);
    }
    if let Some(front) = axis("front") {
        let front = front?;
        if !upright.up().perpendicular(front) {
            return Err(wrong("front perpendicular to up"));
        }
        upright = upright.with_front(front);
    }
    let view = match token("view") {
        None => NamedView::Isometric,
        Some(t) => NamedView::from_token(t?).ok_or_else(|| wrong("view as one of iso, front, right, top, back"))?,
    };
    let mut orbit = Orbit::named(view, perspective, upright);
    let bounds = Bounds::of(meshes).ok_or_else(|| failed("the model has no vertex"))?;
    let reach = reach(meshes, &bounds);
    if let Some(v) = moves.get("turn") {
        let [yaw, pitch] = pair(v).ok_or_else(|| wrong("turn as [yaw, pitch] in degrees"))?;
        orbit.turn(yaw.to_radians(), pitch.to_radians());
    }
    if let Some(v) = moves.get("zoom") {
        let factor = v.as_f64().filter(|f| f.is_finite() && *f > 0.0).ok_or_else(|| wrong("zoom as a positive number"))?;
        let at = match moves.get("zoom_at") {
            None => [0.0, 0.0],
            Some(v) => pair(v).ok_or_else(|| wrong("zoom_at as [right, up] in picture heights"))?,
        };
        orbit.zoom_at(&bounds, reach, aspect, factor, at);
    } else if moves.contains_key("zoom_at") {
        return Err(wrong("an object with zoom when it has zoom_at"));
    }
    if let Some(v) = moves.get("pan") {
        orbit.pan_by(pair(v).ok_or_else(|| wrong("pan as [right, up] in model radii"))?);
    }
    orbit.camera(&bounds, reach, aspect).map_err(failed)
}

/// Two finite numbers.
fn pair(v: &Value) -> Option<[f64; 2]> {
    match v.as_array()?.as_slice() {
        [a, b] => Some([a.as_f64().filter(|f| f.is_finite())?, b.as_f64().filter(|f| f.is_finite())?]),
        _ => None,
    }
}

/// Z is up, as CAD models are drawn, unless the view looks along it.
fn up_for([x, y, z]: [f64; 3]) -> [f64; 3] {
    if x.hypot(y) <= z.abs() * 1e-6 { [0.0, 1.0, 0.0] } else { [0.0, 0.0, 1.0] }
}

/// Straight RGBA to the premultiplied RGBA `encode_png` takes.
fn premultiplied(mut rgba: Vec<u8>) -> Vec<u8> {
    for [r, g, b, alpha] in rgba.as_chunks_mut::<4>().0 {
        let a = u16::from(*alpha);
        for c in [r, g, b] {
            *c = u8::try_from((u16::from(*c) * a + 127) / 255).unwrap_or(u8::MAX);
        }
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premultiplying_scales_colour_by_alpha() {
        assert_eq!(premultiplied(vec![200, 100, 0, 255, 200, 100, 0, 128, 9, 9, 9, 0]), vec![200, 100, 0, 255, 100, 50, 0, 128, 0, 0, 0, 0]);
    }

    #[test]
    fn a_view_along_z_keeps_y_up() {
        assert_eq!(up_for([0.0, 0.0, -1.0]), [0.0, 1.0, 0.0]);
        assert_eq!(up_for(DEFAULT_DIRECTION), [0.0, 0.0, 1.0]);
    }
}
