//! 3-D extrusion (`a:sp3d extrusionH`, `a:extrusionClr`) seen through a turned camera, drawn in
//! 2-D: quads join the shape's outline on its front face to the same outline on its back face (the
//! camera's view of the depth axis), and the face is drawn on top as usual.
//!
//! Each side (one quad per flattened outline edge) is lit for its own normal by the shape's light
//! rig and material, and the sides are drawn farthest first. Without a turned camera an extrusion
//! points at the viewer and nothing of it shows.

use deckcraft_color::{Rgba, SchemeSlot};
use deckcraft_model::ColorRef;
use deckcraft_model::resolve::Ctx;
use kurbo::{Affine, BezPath, Vec2};
use vello_cpu::RenderContext;

use crate::camera::{self, Camera};
use crate::light::Lighting;

/// Outlines with more edges than this (after flattening) are not extruded.
const MAX_EDGES: usize = 4096;

/// An extrusion: its depth in points and its own colour, if set.
#[derive(Clone, Debug, PartialEq)]
pub struct Extrusion {
    pub depth: f64,
    pub color: Option<ColorRef>,
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=\"");
    let s = tag.find(&key)? + key.len();
    let rest = tag.get(s..)?;
    rest.get(..rest.find('"')?)
}

/// The extrusion in 3-D XML kept from the file (`a:sp3d extrusionH="…"` with an `a:scene3d`).
pub fn from_raw(raw: &str) -> Option<Extrusion> {
    if !raw.contains("scene3d") {
        return None;
    }
    let i = raw.find("sp3d ")?;
    let tag = raw.get(i..)?;
    let tag = tag.get(..tag.find('>')?)?;
    let depth = attr(tag, "extrusionH")?.parse::<f64>().ok()? / 12700.0;
    if !(depth.is_finite() && depth > 0.01) {
        return None;
    }
    let color = color_in(raw, "extrusionClr");
    Some(Extrusion { depth: depth.min(4000.0), color })
}

/// The colour in the first `<a:{name}>` element of `raw` (an `srgbClr` or a `schemeClr`).
fn color_in(raw: &str, name: &str) -> Option<ColorRef> {
    let j = raw.find(&format!("{name}>"))?;
    let body = raw.get(j..)?;
    let end = body.find(&format!("/a:{name}")).or_else(|| body.find(&format!("/{name}"))).unwrap_or(body.len());
    let body = body.get(..end)?;
    if let Some(k) = body.find("srgbClr ") {
        let v = attr(body.get(k..)?, "val")?;
        let n = u32::from_str_radix(v, 16).ok()?;
        Some(ColorRef::rgb(Rgba::rgb((n >> 16) as u8, (n >> 8) as u8, n as u8)))
    } else if let Some(k) = body.find("schemeClr ") {
        SchemeSlot::from_xml(attr(body.get(k..)?, "val")?).map(ColorRef::scheme)
    } else {
        None
    }
}

/// A 3-D contour: a band of `width` points around the shape's outline (`a:sp3d contourW`,
/// `a:contourClr`), drawn behind the face so only its outer half shows past the outline.
#[derive(Clone, Debug, PartialEq)]
pub struct Contour {
    pub width: f64,
    pub color: Option<ColorRef>,
}

/// The contour in 3-D XML kept from the file, if it has one.
pub fn contour_from_raw(raw: &str) -> Option<Contour> {
    let i = raw.find("sp3d ")?;
    let tag = raw.get(i..)?;
    let tag = tag.get(..tag.find('>')?)?;
    let width = attr(tag, "contourW")?.parse::<f64>().ok()? / 12700.0;
    if !(width.is_finite() && width > 0.01) {
        return None;
    }
    Some(Contour { width: width.min(2000.0), color: color_in(raw, "contourClr") })
}

/// Draw `c` around `outline` (shape space, mapped by `m`), lit as a surface facing the viewer.
pub fn draw_contour(ctx: &mut RenderContext, outline: &BezPath, m: Affine, width: f64, color: Rgba, lit: &Lighting) {
    ctx.set_transform(m);
    ctx.set_paint(crate::color(lit.shade(color, [0.0, 0.0, 1.0]), 1.0));
    ctx.set_stroke(kurbo::Stroke::new(2.0 * width).with_join(kurbo::Join::Round));
    ctx.stroke_path(outline);
}

/// The camera's view of the depth axis: where the back face sits relative to the front face, in
/// the turned shape's space (the space `camera::affine` maps into).
pub fn back_offset(c: &Camera, depth: f64, w: f64, h: f64) -> Vec2 {
    let size = w.max(h);
    camera::project3(c, 0.0, 0.0, -depth, size) - camera::project3(c, 0.0, 0.0, 0.0, size)
}

/// The side colour before lighting: the extrusion colour, or the face's.
pub fn side_color(rctx: &Ctx, e: &Extrusion, face: Option<Rgba>) -> Rgba {
    e.color.as_ref().map(|c| rctx.color(c, face)).or(face).unwrap_or(Rgba::rgb(0x80, 0x80, 0x80))
}

/// Draw the sides of `outline` (shape space, `w` × `h`): one quad per edge between the front face
/// and the back face, each lit by `lit` for its own normal as camera `cam` sees it, farthest first.
/// `pre` maps the turned shape's space to the output and `turn` is the camera's turn.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    ctx: &mut RenderContext,
    outline: &BezPath,
    w: f64,
    h: f64,
    cam: &Camera,
    depth: f64,
    pre: Affine,
    turn: Affine,
    side: Rgba,
    lit: &Lighting,
) {
    let offset = back_offset(cam, depth, w, h);
    let px = (pre * offset.to_point() - pre * kurbo::Point::ZERO).hypot();
    if !px.is_finite() || px < 0.25 {
        return;
    }
    let polys = crate::pathgrad::polygons(outline);
    let size = w.max(h);
    let mut quads: Vec<(f64, BezPath, Rgba)> = vec![];
    for poly in &polys {
        if poly.len() > MAX_EDGES {
            continue;
        }
        let area: f64 = poly.windows(2).map(|q| q[0].x * q[1].y - q[1].x * q[0].y).sum::<f64>() / 2.0;
        for e in poly.windows(2) {
            let (a, b) = (e[0], e[1]);
            let d = b - a;
            let len = d.hypot();
            if len < 1e-9 {
                continue;
            }
            // Outward normal in the shape's plane (as in the bevel), turned by the camera.
            let n = kurbo::Vec2::new(d.y, -d.x) / len;
            let n = if area > 0.0 { n } else { -n };
            let vn = camera::rotate(cam, [n.x, n.y, 0.0]);
            if vn[2] < -0.02 {
                continue;
            }
            let mid = a.midpoint(b);
            let z = camera::rotate(cam, [mid.x - w / 2.0, mid.y - h / 2.0, -depth / 2.0])[2];
            let (fa, fb) = (turn * a, turn * b);
            let mut q = BezPath::new();
            q.move_to(fa);
            q.line_to(fb);
            q.line_to(fb + offset);
            q.line_to(fa + offset);
            q.close_path();
            quads.push((z, q, lit.shade(side, vn)));
        }
    }
    let _ = size;
    quads.sort_by(|a, b| a.0.total_cmp(&b.0));
    ctx.set_transform(pre);
    for (_, q, c) in &quads {
        ctx.set_paint(crate::color(*c, 1.0));
        ctx.fill_path(q);
        // Close the hairline seams between neighbouring quads.
        ctx.set_stroke(kurbo::Stroke::new(0.6 / crate::scale_of(pre).max(1e-6)));
        ctx.stroke_path(q);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_depth_and_colour() {
        let raw = r#"<a:scene3d><a:camera prst="isometricOffAxis2Left"/><a:lightRig rig="threePt" dir="t"/></a:scene3d><a:sp3d extrusionH="381000"><a:extrusionClr><a:srgbClr val="C00000"/></a:extrusionClr></a:sp3d>"#;
        let e = from_raw(raw).unwrap();
        assert!((e.depth - 30.0).abs() < 1e-9);
        assert_eq!(e.color, Some(ColorRef::rgb(Rgba::rgb(0xC0, 0, 0))));
        assert!(from_raw(r#"<a:sp3d extrusionH="381000"/>"#).is_none(), "no scene: drawn flat");
        assert!(from_raw(r#"<a:scene3d/><a:sp3d/>"#).is_none());
    }

    #[test]
    fn the_back_face_goes_away_from_the_viewer() {
        // Turned left about the vertical axis, the back face shows to the right... of the turned
        // face for a positive longitude, and nothing for a front camera.
        let c = Camera { lat: 0.0, lon: 30.0, rev: 0.0, dist: 0.0 };
        let o = back_offset(&c, 20.0, 100.0, 50.0);
        assert!((o.x.abs() - 10.0).abs() < 1e-9 && o.y.abs() < 1e-9, "{o:?}");
        let front = Camera { lat: 0.0, lon: 0.0, rev: 0.0, dist: 0.0 };
        assert!(back_offset(&front, 20.0, 100.0, 50.0).hypot() < 1e-9);
    }
}
