//! The open, solid polylines the model can represent as editable ink. This is DrawingML
//! geometry, not Office InkML. Unsupported geometry keeps the existing shape-import fallback.

use std::fmt::Write as _;

use gridcraft_model::{Anchor, Color, Shape, ShapeKind, Theme};

use super::xfrm;
use crate::tables::EMU_PER_PX;
use crate::xml::El;

const MAX_POINTS: usize = 20_000;
const MAX_COORD: f64 = 1e7;

fn bounded(v: f64) -> bool {
    v.is_finite() && v.abs() <= MAX_COORD
}

fn only_children(e: &El, names: &[&str]) -> bool {
    e.children.iter().all(|c| names.contains(&c.name.as_str()))
}

fn only_attrs(e: &El, names: &[&str]) -> bool {
    e.attrs.iter().all(|(k, _)| names.contains(&k.as_str()))
}

fn one<'a>(e: &'a El, name: &str) -> Option<&'a El> {
    let mut found = e.children.iter().filter(|c| c.name == name);
    let first = found.next()?;
    found.next().is_none().then_some(first)
}

fn validate_shape_obj(obj: &El) -> Option<&El> {
    if obj.name != "sp"
        || !only_children(obj, &["nvSpPr", "spPr"])
        || !only_attrs(obj, &["macro", "textlink"])
        || obj.attrs.iter().any(|(_, v)| !v.is_empty())
        || obj.path(&["nvSpPr", "cNvSpPr"]).is_some_and(|e| e.flag("txBox", false))
    {
        return None;
    }
    let sp = one(obj, "spPr")?;
    if !only_children(sp, &["xfrm", "custGeom", "noFill", "ln"]) || !sp.attrs.is_empty() {
        return None;
    }
    let no_fill = one(sp, "noFill")?;
    if !no_fill.attrs.is_empty() || !no_fill.children.is_empty() {
        return None;
    }
    Some(sp)
}

fn validate_xfrm(sp: &El, anchor: &Anchor) -> Option<()> {
    let xf = one(sp, "xfrm")?;
    if !xf.attrs.is_empty() || !only_children(xf, &["off", "ext"]) {
        return None;
    }
    let off = one(xf, "off")?;
    if off.attr_i64("x")? != 0 || off.attr_i64("y")? != 0 {
        return None;
    }
    let ext = one(xf, "ext")?;
    let ew = ext.attr_i64("cx")?;
    let eh = ext.attr_i64("cy")?;
    if ew <= 0 || eh <= 0 || anchor.width <= 0.0 || anchor.height <= 0.0 || !bounded(anchor.width as f64) || !bounded(anchor.height as f64) {
        return None;
    }
    let same_extent =
        |emu: i64, extent: f32| (emu as f64 / EMU_PER_PX - extent as f64).abs() <= 0.001_f64.max(extent as f64 * f32::EPSILON as f64 * 2.0);
    if !same_extent(ew, anchor.width) || !same_extent(eh, anchor.height) {
        return None;
    }
    Some(())
}

fn validate_cust_geom(sp: &El) -> Option<(&El, i64, i64)> {
    let geom = one(sp, "custGeom")?;
    if !geom.attrs.is_empty() || !only_children(geom, &["avLst", "gdLst", "ahLst", "cxnLst", "rect", "pathLst"]) {
        return None;
    }
    // Text bounds have no effect on this unfilled geometry, which has no caption.
    if geom.children.iter().any(|c| !matches!(c.name.as_str(), "pathLst" | "rect") && (!c.children.is_empty() || !c.attrs.is_empty())) {
        return None;
    }
    if geom.kids("rect").count() > 1 || geom.kids("rect").any(|r| !r.children.is_empty() || !only_attrs(r, &["l", "t", "r", "b"])) {
        return None;
    }
    let paths = one(geom, "pathLst")?;
    if paths.children.len() != 1 {
        return None;
    }
    let path = one(paths, "path")?;
    let w = path.attr_i64("w")?;
    let h = path.attr_i64("h")?;
    if w <= 0
        || h <= 0
        || path.attr("fill") != Some("none")
        || !only_attrs(path, &["w", "h", "fill", "stroke", "extrusionOk"])
        || path.attr("stroke").is_some_and(|_| path.attr_bool("stroke") != Some(true))
        || path.attr("extrusionOk").is_some_and(|_| path.attr_bool("extrusionOk") != Some(false))
        || !(2..=MAX_POINTS).contains(&path.children.len())
    {
        return None;
    }
    Some((path, w, h))
}

fn validate_line(sp: &El) -> Option<(i64, Color)> {
    let ln = one(sp, "ln")?;
    if !only_attrs(ln, &["w", "cap", "cmpd", "algn"])
        || ln.attr("cap") != Some("flat")
        || ln.attr("cmpd").is_some_and(|v| v != "sng")
        || ln.attr("algn").is_some_and(|v| v != "ctr")
        || !only_children(ln, &["solidFill", "miter"])
    {
        return None;
    }
    let width_emu = ln.attr_i64("w")?;
    let unit = EMU_PER_PX as i64;
    if width_emu % unit != 0 || !(1..=40).contains(&(width_emu / unit)) {
        return None;
    }
    let width = width_emu / unit;
    let join = one(ln, "miter")?;
    if !join.attrs.is_empty() || !join.children.is_empty() {
        return None;
    }
    let fill = one(ln, "solidFill")?;
    if fill.children.len() != 1 {
        return None;
    }
    let color = one(fill, "srgbClr")?;
    if !only_attrs(color, &["val"]) || !only_children(color, &["alpha"]) {
        return None;
    }
    let expected_alpha = if width > 8 { 45_000 } else { 100_000 };
    if color.children.is_empty() {
        if expected_alpha != 100_000 {
            return None;
        }
    } else {
        let alpha = one(color, "alpha")?;
        if !only_attrs(alpha, &["val"]) || !alpha.children.is_empty() || alpha.attr_i64("val")? != expected_alpha {
            return None;
        }
    }
    let line = Color::from_hex(color.attr("val")?)?;
    Some((width, line))
}

fn parse_points(path: &El, anchor: &Anchor, w: i64, h: i64) -> Option<String> {
    let mut text = String::new();
    for (i, command) in path.children.iter().enumerate() {
        if command.name != if i == 0 { "moveTo" } else { "lnTo" } || !command.attrs.is_empty() || command.children.len() != 1 {
            return None;
        }
        let pt = one(command, "pt")?;
        if !only_attrs(pt, &["x", "y"]) || !pt.children.is_empty() {
            return None;
        }
        let x = pt.attr_i64("x")? as f64 * anchor.width as f64 / w as f64;
        let y = pt.attr_i64("y")? as f64 * anchor.height as f64 / h as f64;
        if !bounded(x) || !bounded(y) {
            return None;
        }
        if i != 0 {
            text.push(';');
        }
        let _ = write!(text, "{},{}", x as f32, y as f32);
    }
    Some(text)
}

/// Do not partially import a path: losing a curve, subpath, transform or line effect changes
/// the drawing. Accept only the explicit subset emitted below, including its line styling.
pub(super) fn read(obj: &El, anchor: Anchor) -> Option<Shape> {
    let sp = validate_shape_obj(obj)?;
    validate_xfrm(sp, &anchor)?;
    let (path, w, h) = validate_cust_geom(sp)?;
    let (width, line) = validate_line(sp)?;
    let text = parse_points(path, &anchor, w, h)?;
    Some(Shape {
        id: 0,
        kind: ShapeKind::Ink,
        anchor,
        fill: Color::Rgb((width as u32) << 16),
        line,
        text,
        format: Default::default(),
        rotation: 0.0,
        math: Default::default(),
    })
}

/// Invalid/native out-of-range data uses the existing rectangle fallback. A successful
/// result is complete before it is appended, so malformed points never yield half a stroke.
pub(super) fn write(shape: &Shape, theme: &Theme, id: usize) -> Option<String> {
    if shape.kind != ShapeKind::Ink || shape.format.no_line {
        return None;
    }
    let width = match shape.fill {
        Color::Rgb(rgb) => ((rgb >> 16) & 0xff).max(1),
        _ => 2,
    };
    if !(1..=40).contains(&width) {
        return None;
    }
    let a = &shape.anchor;
    if !bounded(a.width as f64) || !bounded(a.height as f64) || a.width <= 0.0 || a.height <= 0.0 || !a.dx.is_finite() || !a.dy.is_finite() {
        return None;
    }
    let w = (a.width as f64 * EMU_PER_PX).round().max(1.0) as i64;
    let h = (a.height as f64 * EMU_PER_PX).round().max(1.0) as i64;
    let mut points = Vec::new();
    for point in shape.text.split(';') {
        if points.len() == MAX_POINTS {
            return None;
        }
        let (x, y) = point.split_once(',')?;
        let (x, y) = (x.parse::<f64>().ok()?, y.parse::<f64>().ok()?);
        if !bounded(x) || !bounded(y) {
            return None;
        }
        points.push(((x * EMU_PER_PX).round() as i64, (y * EMU_PER_PX).round() as i64));
    }
    if points.len() < 2 {
        return None;
    }
    let [r, g, b] = shape.line.resolve(theme)?;
    let mut xml = format!(
        "<xdr:sp macro=\"\" textlink=\"\"><xdr:nvSpPr><xdr:cNvPr id=\"{id}\" name=\"Ink {id}\"/><xdr:cNvSpPr/></xdr:nvSpPr><xdr:spPr>{}<a:custGeom><a:avLst/><a:gdLst/><a:ahLst/><a:cxnLst/><a:pathLst><a:path w=\"{w}\" h=\"{h}\" fill=\"none\">",
        xfrm(a, "a:xfrm")
    );
    for (i, (x, y)) in points.iter().enumerate() {
        let tag = if i == 0 { "moveTo" } else { "lnTo" };
        let _ = write!(xml, "<a:{tag}><a:pt x=\"{x}\" y=\"{y}\"/></a:{tag}>");
    }
    let _ = write!(
        xml,
        "</a:path></a:pathLst></a:custGeom><a:noFill/><a:ln w=\"{}\" cap=\"flat\"><a:solidFill><a:srgbClr val=\"{r:02X}{g:02X}{b:02X}\">",
        width as i64 * EMU_PER_PX as i64
    );
    if width > 8 {
        xml.push_str("<a:alpha val=\"45000\"/>");
    }
    xml.push_str("</a:srgbClr></a:solidFill><a:miter/></a:ln></xdr:spPr></xdr:sp>");
    Some(xml)
}
