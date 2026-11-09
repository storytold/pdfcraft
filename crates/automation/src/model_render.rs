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
        let (width_arg, height_arg) = (side(a, "width")?, side(a, "height")?);
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
        let framing = a.get("direction").is_some() || a.get("perspective").is_some();
        let chosen = match a.opt_str("view")? {
            // A saved view has its own camera and projection: framing arguments would be ignored.
            Some("saved") if framing => {
                return Err(ToolError::InvalidArgs("direction and perspective frame view \"fit\"; a saved view keeps its own camera".into()));
            }
            Some("saved") => Some(annot.and_then(|m| m.view).ok_or_else(|| failed("this 3D model has no saved view; use view \"fit\""))?),
            Some("fit") => None,
            Some(_) => return Err(Args::wrong("view", "\"saved\" or \"fit\"")),
            None if !framing => annot.and_then(|m| m.view),
            None => None,
        };
        let saved = chosen.zip(annot);
        // A saved view is drawn in its annotation's shape unless a size is asked for.
        let shape = saved.map(|(_, m)| [f64::from(m.rect[2] - m.rect[0]), f64::from(m.rect[3] - m.rect[1])]);
        let (width, height) = image_size(width_arg, height_arg, shape)?;
        let mut options = RenderOptions { width, height, ..RenderOptions::default() };
        let camera = match saved {
            Some((view, _)) => {
                let centre = Bounds::of(&meshes).map_or([0.0; 3], |b| b.centre());
                // The lens is converted for the image's own sides, so the view's scaled side
                // shows what it says even when the image is not the annotation's shape.
                let size = [f64::from(width), f64::from(height)];
                if let Some([r, g, b]) = view.background {
                    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                    options.background = [byte(r), byte(g), byte(b), 255];
                }
                Camera::from_view(view.camera_to_world, view.center_of_orbit, lens(view.projection), size, centre).map_err(failed)?
            }
            None => Camera::fit_meshes(&meshes, direction, up_for(direction), perspective, f64::from(width) / f64::from(height)).map_err(failed)?,
        };
        let image = render(&meshes, &camera, &options).map_err(failed)?;
        let data = encode_png(image.width, image.height, &premultiplied(image.rgba))?;
        Ok(vec![Content::Png { data, width: image.width, height: image.height }])
    }
}

fn side(a: &Args, key: &str) -> Result<Option<u32>> {
    match a.opt_int(key)? {
        None => Ok(None),
        Some(n) => u32::try_from(n)
            .ok()
            .filter(|n| (1..=MAX_SIDE).contains(n))
            .map(Some)
            .ok_or_else(|| Args::wrong(key, &format!("between 1 and {MAX_SIDE}"))),
    }
}

/// The image's size: the sides asked for; a side not asked for follows `shape` (width, height)
/// from the other, or is [`DEFAULT_SIDE`] on the longer side when neither is given. A side that
/// would follow past [`MAX_SIDE`] is refused, as one asked for is, rather than cut and the
/// picture squashed.
fn image_size(width: Option<u32>, height: Option<u32>, shape: Option<[f64; 2]>) -> Result<(u32, u32)> {
    let aspect = shape.map(|[w, h]| w.abs() / h.abs()).filter(|r| r.is_finite() && *r > 0.0);
    let follow = |v: f64, given: &str, key: &str| {
        let side = v.round().max(1.0);
        if side <= f64::from(MAX_SIDE) {
            Ok(side as u32)
        } else {
            Err(ToolError::InvalidArgs(format!(
                "the model's shape would make the {key} {side}, past {MAX_SIDE}: give a smaller {given}, or both width and height"
            )))
        }
    };
    Ok(match (width, height, aspect) {
        (Some(w), Some(h), _) => (w, h),
        (Some(w), None, Some(r)) => (w, follow(f64::from(w) / r, "width", "height")?),
        (None, Some(h), Some(r)) => (follow(f64::from(h) * r, "height", "width")?, h),
        // The default side is the longer one, so the other never passes it.
        (None, None, Some(r)) if r >= 1.0 => (DEFAULT_SIDE, (f64::from(DEFAULT_SIDE) / r).round().max(1.0) as u32),
        (None, None, Some(r)) => ((f64::from(DEFAULT_SIDE) * r).round().max(1.0) as u32, DEFAULT_SIDE),
        (w, h, None) => (w.unwrap_or(DEFAULT_SIDE), h.unwrap_or(DEFAULT_SIDE)),
    })
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
    fn a_saved_view_image_takes_its_annotation_shape_unless_sized() {
        let wide = Some([700.0, 350.0]);
        let size = |w, h, shape| image_size(w, h, shape).ok();
        assert_eq!(size(None, None, wide), Some((512, 256)));
        assert_eq!(size(None, None, Some([100.0, 400.0])), Some((128, 512)));
        assert_eq!(size(Some(300), None, wide), Some((300, 150)));
        assert_eq!(size(None, Some(100), wide), Some((200, 100)));
        assert_eq!(size(Some(64), Some(64), wide), Some((64, 64)));
        assert_eq!(size(None, Some(100), None), Some((512, 100)));
        assert_eq!(size(None, None, Some([0.0, 0.0])), Some((512, 512)));
        // A very thin model stays at least a pixel across.
        assert_eq!(size(None, None, Some([10_000.0, 1.0])), Some((512, 1)));
    }

    #[test]
    fn a_side_that_would_follow_past_the_largest_is_refused_not_squashed() {
        let tall = Some([100.0, 400.0]);
        assert_eq!(image_size(Some(1024), None, tall).ok(), Some((1024, 4096)));
        assert!(matches!(image_size(Some(1025), None, tall), Err(ToolError::InvalidArgs(m)) if m.contains("height 4100")));
        assert!(matches!(image_size(None, Some(2049), Some([700.0, 350.0])), Err(ToolError::InvalidArgs(m)) if m.contains("width 4098")));
    }

    #[test]
    fn a_view_along_z_keeps_y_up() {
        assert_eq!(up_for([0.0, 0.0, -1.0]), [0.0, 1.0, 0.0]);
        assert_eq!(up_for(DEFAULT_DIRECTION), [0.0, 0.0, 1.0]);
    }
}
