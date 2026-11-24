//! 3-D bevels (`a:sp3d/a:bevelT` seen through a `a:scene3d`), approximated in 2-D: the shape's
//! face and a band inside its outline as wide as the bevel are lit as surfaces, the band's normal
//! tilting out towards the outline by the bevel profile's slope.
//!
//! Fitted to PowerPoint's export of bevel presets on a solid-filled shape with the default
//! three-point light rig and matte material: each channel comes out as
//! `c · (0.453 + 0.731 · max(0, n · L)) + 14.7` for a light from the upper left (azimuth 240°,
//! 45° up), within about 15 grey levels RMS across the band (exact profiles, specular highlights
//! and the other rigs, materials and cameras are not modelled).

use std::sync::Arc;

use deckcraft_color::Rgba;
use kurbo::{Affine, BezPath, Point, Rect};
use vello_cpu::{Pixmap, RenderContext, peniko};

use crate::light::Lighting;

/// Longest side of the rasterised lighting, in pixels.
const MAX_SIDE: f64 = 512.0;

/// A top bevel: its width and height in points and its preset.
#[derive(Clone, Debug, PartialEq)]
pub struct Bevel {
    pub w: f64,
    pub h: f64,
    pub preset: String,
    /// The light rig and material that light it.
    pub lighting: Lighting,
}

/// The top bevel of 3-D XML kept from the file (`scene3d` + `sp3d`), when it has both a scene
/// (without one PowerPoint draws the shape flat) and a `bevelT`.
pub fn from_raw(raw: &str) -> Option<Bevel> {
    if !raw.contains("scene3d") || !raw.contains("sp3d") {
        return None;
    }
    // A 3-D shape without a bevel still has its face lit: a bevel of no width.
    let Some(i) = raw.find("bevelT") else {
        return Some(Bevel { w: 0.0, h: 0.0, preset: "circle".into(), lighting: Lighting::from_raw(raw) });
    };
    let tag = raw.get(i..)?;
    let tag = tag.get(..tag.find('>')?)?;
    let attr = |name: &str| -> Option<&str> {
        let key = format!(" {name}=\"");
        let s = tag.find(&key)? + key.len();
        let rest = tag.get(s..)?;
        rest.get(..rest.find('"')?)
    };
    let emu = |name: &str| attr(name).and_then(|v| v.parse::<f64>().ok()).map(|v| v / 12700.0);
    let w = emu("w").unwrap_or(6.0).clamp(0.0, 2000.0);
    let h = emu("h").unwrap_or(6.0).clamp(0.0, 2000.0);
    (w > 0.0).then(|| Bevel { w, h, preset: attr("prst").unwrap_or("circle").to_string(), lighting: Lighting::from_raw(raw) })
}

/// The light, pointing from the surface towards it (x right, y down, z towards the viewer).
#[cfg(test)]
fn light() -> [f64; 3] {
    let (az, el) = (240f64.to_radians(), 45f64.to_radians());
    [el.cos() * az.cos(), el.cos() * az.sin(), el.sin()]
}

/// The bevel surface's rise per unit inwards (in units of h / w) at `u` (0 at the outline, 1 at
/// the inner edge of the band), by preset.
fn slope(preset: &str, u: f64) -> f64 {
    let u = u.clamp(1e-3, 1.0);
    match preset {
        "angle" => 1.0,
        // A quarter circle, steep at the outline and flat where it meets the face; the default,
        // and used for the presets not modelled.
        _ => {
            let v = 1.0 - u;
            v / (1.0 - v * v).max(1e-6).sqrt()
        }
    }
}

/// Lighting factor and outward edge normal at each position, for a polygon set: distance to the
/// nearest edge and that edge's outward normal (edges oriented by each polygon's signed area).
fn nearest_edge(polys: &[Vec<Point>], areas: &[f64], p: Point) -> Option<(f64, kurbo::Vec2)> {
    let mut best: Option<(f64, kurbo::Vec2)> = None;
    for (poly, area) in polys.iter().zip(areas) {
        for w in poly.windows(2) {
            let (a, b) = (w[0], w[1]);
            let e = b - a;
            let l2 = e.hypot2();
            if l2 < 1e-12 {
                continue;
            }
            let t = ((p - a).dot(e) / l2).clamp(0.0, 1.0);
            let d = (p - (a + e * t)).hypot();
            if best.is_none_or(|(bd, _)| d < bd) {
                // Outward normal: for a polygon of positive (clockwise on screen, y down) area the
                // outside is to the left of each edge's direction.
                let n = kurbo::Vec2::new(e.y, -e.x) / l2.sqrt();
                let n = if *area > 0.0 { n } else { -n };
                best = Some((d, n));
            }
        }
    }
    best
}

/// Draw the lit face and bevel band over a shape filled with `fill` (a solid colour, or `None`
/// for any other fill, which gets the band's highlights and shading as light and dark veils)
/// inside `path` (shape space, laid out in `bounds`), `m` mapping it to the output.
#[allow(clippy::too_many_arguments)]
pub fn draw(ctx: &mut RenderContext, bevel: &Bevel, fill: Option<Rgba>, path: &BezPath, bounds: Rect, m: Affine, alpha: f64) {
    let lit = &bevel.lighting;
    if bounds.width() <= 0.0 || bounds.height() <= 0.0 || !bounds.is_finite() {
        return;
    }
    // No bevel: only the face, one lit colour (as PowerPoint lights a 3-D shape's front face,
    // whichever way the camera turns it, within about 10 levels).
    if bevel.w <= 0.0 {
        if let Some(c) = fill {
            ctx.set_transform(m);
            ctx.set_paint(crate::color(lit.shade(c, [0.0, 0.0, 1.0]), alpha));
            ctx.fill_path(path);
        }
        return;
    }
    let k = crate::scale_of(m);
    let k = if k.is_finite() && k > 0.0 { k } else { 1.0 };
    let longest = bounds.width().max(bounds.height()) * k;
    let s = (MAX_SIDE / longest).min(1.0) * k;
    let (pw, ph) = (((bounds.width() * s).ceil() as u32).clamp(1, 2048), ((bounds.height() * s).ceil() as u32).clamp(1, 2048));
    // Each pixel tests every edge, so a dense outline is simplified to the raster's edge budget.
    let pixel = (bounds.width() / f64::from(pw)).min(bounds.height() / f64::from(ph));
    let polys = crate::pathgrad::simplify(crate::pathgrad::polygons(path), pixel, crate::pathgrad::edge_budget(pw, ph));
    if polys.is_empty() {
        return;
    }
    let (pw, ph) = crate::pathgrad::fit_raster(polys.iter().map(|p| p.len().saturating_sub(1)).sum(), pw, ph);
    let areas: Vec<f64> = polys.iter().map(|p| p.windows(2).map(|w| w[0].x * w[1].y - w[1].x * w[0].y).sum::<f64>() / 2.0).collect();
    let face = lit.factor([0.0, 0.0, 1.0]);
    let ratio = if bevel.w > 0.0 { bevel.h / bevel.w } else { 0.0 };
    let a = alpha.clamp(0.0, 1.0);
    let mut data = Vec::with_capacity((pw * ph) as usize);
    for j in 0..ph {
        for i in 0..pw {
            let p = Point::new(bounds.x0 + (i as f64 + 0.5) / pw as f64 * bounds.width(), bounds.y0 + (j as f64 + 0.5) / ph as f64 * bounds.height());
            let n = match nearest_edge(&polys, &areas, p) {
                Some((d, e)) if d < bevel.w => {
                    let z = slope(&bevel.preset, d / bevel.w) * ratio;
                    let n = [e.x * z, e.y * z, 1.0];
                    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                    [n[0] / len, n[1] / len, n[2] / len]
                }
                _ => [0.0, 0.0, 1.0],
            };
            let f = lit.factor(n);
            let px = match fill {
                Some(c) => {
                    let lc = lit.shade(c, n);
                    let ca = c.a as f64 * a / 255.0;
                    let pm = |v: u8| (f64::from(v) * ca).round().clamp(0.0, 255.0) as u8;
                    vello_cpu::color::PremulRgba8 { r: pm(lc.r), g: pm(lc.g), b: pm(lc.b), a: (ca * 255.0).round() as u8 }
                }
                None => {
                    // A veil: white where the band is lighter than the face, black where darker.
                    let d = ((f - face) * 1.5).clamp(-1.0, 1.0);
                    let va = (d.abs() * a * 255.0).round().clamp(0.0, 255.0) as u8;
                    let v = if d > 0.0 { va } else { 0 };
                    vello_cpu::color::PremulRgba8 { r: v, g: v, b: v, a: va }
                }
            };
            data.push(px);
        }
    }
    let pm = Arc::new(Pixmap::from_parts(data, pw as u16, ph as u16));
    ctx.set_transform(m);
    ctx.set_paint(vello_cpu::Image {
        image: vello_cpu::ImageSource::Pixmap(pm),
        sampler: peniko::ImageSampler { quality: peniko::ImageQuality::Medium, ..Default::default() },
    });
    ctx.set_paint_transform(
        Affine::translate(bounds.origin().to_vec2()) * Affine::scale_non_uniform(bounds.width() / pw as f64, bounds.height() / ph as f64),
    );
    ctx.fill_path(path);
    ctx.reset_paint_transform();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bevels_are_read_from_kept_3d_xml() {
        let raw = r#"<a:scene3d><a:camera prst="orthographicFront"/></a:scene3d><a:sp3d><a:bevelT w="50800" h="16510" prst="angle"/></a:sp3d>"#;
        let b = from_raw(raw).unwrap();
        assert_eq!((b.w, b.h, b.preset.as_str()), (4.0, 1.3, "angle"));
        assert_eq!(b.lighting, Lighting::from_raw(raw));
        // Defaults: 6 pt square, circle.
        let raw = r#"<a:scene3d/><a:sp3d><a:bevelT/></a:sp3d>"#;
        assert!(from_raw(raw).is_some_and(|b| (b.w, b.h, b.preset.as_str()) == (6.0, 6.0, "circle")));
        // Another rig or material is lit by it.
        let raw = r#"<a:scene3d><a:lightRig rig="flood" dir="t"/></a:scene3d><a:sp3d prstMaterial="metal"><a:bevelT/></a:sp3d>"#;
        assert!(from_raw(raw).is_some_and(|b| b.lighting.specular > 0.0));
        // No scene: flat in PowerPoint.
        assert_eq!(from_raw(r#"<a:sp3d><a:bevelT w="50800"/></a:sp3d>"#), None);
        // A 3-D shape with no bevel: a bevel of no width (its face is lit).
        assert!(from_raw(r#"<a:scene3d/><a:sp3d extrusionH="10"/>"#).is_some_and(|b| b.w == 0.0));
        assert_eq!(from_raw(r#"<a:scene3d/>"#), None);
    }

    #[test]
    fn the_light_comes_from_the_upper_left() {
        let l = light();
        assert!(l[0] < 0.0 && l[1] < 0.0 && l[2] > 0.0);
        // An upward-facing edge (outward normal up) is lit more than a downward one.
        let lit = |e: [f64; 2]| {
            let z = slope("circle", 0.3) * 0.5;
            let n = [e[0] * z, e[1] * z, 1.0];
            let len = (n[0] * n[0] + n[1] * n[1] + 1.0).sqrt();
            (n[0] * l[0] + n[1] * l[1] + n[2] * l[2]) / len
        };
        assert!(lit([0.0, -1.0]) > lit([-1.0, 0.0]) && lit([-1.0, 0.0]) > lit([1.0, 0.0]) && lit([1.0, 0.0]) > lit([0.0, 1.0]));
    }
}
