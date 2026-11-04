//! `model_render`: a page's 3D model (ISO 32000-2 §13.6) drawn to a PNG, without a viewer.

use pdfcraft_engine::model3d::Model3dKind;
use pdfcraft_prc::PrcFile;
use pdfcraft_prc_render::{Bounds, Camera, RenderOptions, ViewLens, ViewSide, assemble, render};
use pdfcraft_render::{Dimension3d, Projection3d};

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
        let direction = direction(a)?;
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
        let annot = doc.info.annots_3d.iter().find(|m| m.page == page && m.index == index);
        let chosen = match a.opt_str("view")? {
            Some("saved") => Some(annot.and_then(|m| m.view).ok_or_else(|| failed("this 3D model has no saved view; use view \"fit\""))?),
            Some("fit") => None,
            Some(_) => return Err(Args::wrong("view", "\"saved\" or \"fit\"")),
            None if a.get("direction").is_none() && a.get("perspective").is_none() => annot.and_then(|m| m.view),
            None => None,
        };
        let mut options = RenderOptions { width, height, ..RenderOptions::default() };
        let camera = match (chosen, annot) {
            (Some(view), Some(annot)) => {
                let centre = Bounds::of(&meshes).map_or([0.0; 3], |b| b.centre());
                let size = [f64::from(annot.rect[2] - annot.rect[0]), f64::from(annot.rect[3] - annot.rect[1])];
                if let Some([r, g, b]) = view.background {
                    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                    options.background = [byte(r), byte(g), byte(b), 255];
                }
                Camera::from_view(view.camera_to_world, view.center_of_orbit, lens(view.projection), size, centre).map_err(failed)?
            }
            _ => Camera::fit_meshes(&meshes, direction, up_for(direction), perspective, f64::from(width) / f64::from(height)).map_err(failed)?,
        };
        let image = render(&meshes, &camera, &options).map_err(failed)?;
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

/// A PDF 3D view's projection as the renderer's lens.
fn lens(p: Projection3d) -> ViewLens {
    let side = |d: Dimension3d| match d {
        Dimension3d::Width => ViewSide::Width,
        Dimension3d::Height => ViewSide::Height,
        Dimension3d::Min => ViewSide::Min,
        Dimension3d::Max => ViewSide::Max,
    };
    match p {
        Projection3d::Perspective { fov, scaled } => ViewLens::Perspective { fov, side: side(scaled) },
        Projection3d::Orthographic { scale, scaled } => ViewLens::Orthographic { scale, side: side(scaled) },
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
