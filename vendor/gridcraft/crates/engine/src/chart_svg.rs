//! Charts as inline SVG for Save as Web Page: the chart crate's drawing primitives, text included.

use std::fmt::Write as _;

use gridcraft_chart::{HAlign, Prim, Rgba, VAlign};
use gridcraft_model::{Chart, Workbook};

use crate::io::esc;

/// Widest or tallest chart written; larger anchors are clamped.
const MAX_SIDE: f32 = 16_384.0;

/// Most bytes of SVG written for the charts of one web page together. A chart past what is
/// left makes the export fail rather than grow without bound or be dropped without a word.
pub(crate) const MAX_PAGE_SVG_BYTES: usize = 64 * 1024 * 1024;

struct HelveticaMeasure;

impl gridcraft_chart::Measure for HelveticaMeasure {
    fn text_width(&self, text: &str, size: f32, bold: bool) -> f32 {
        gridcraft_pdf::text_width(if bold { gridcraft_pdf::Font::HelveticaBold } else { gridcraft_pdf::Font::Helvetica }, size, text)
    }
}

/// `chart` of sheet `sheet` as an `<svg>` element placed by `style`, `None` for a degenerate size,
/// or an error for an element longer than `budget` bytes.
pub(crate) fn chart_svg(wb: &Workbook, sheet: usize, chart: &Chart, style: &str, budget: usize) -> crate::Result<Option<String>> {
    let (w, h) = (chart.anchor.width.min(MAX_SIDE), chart.anchor.height.min(MAX_SIDE));
    if !(w >= 1.0 && h >= 1.0) {
        return Ok(None);
    }
    let data = gridcraft_chart::resolve(wb, sheet, chart);
    let prims = gridcraft_chart::render(chart, &data, w, h, &HelveticaMeasure);
    match prims_svg_within(&prims, w, h, style, budget) {
        Some(svg) => Ok(Some(svg)),
        None => Err(crate::EngineError::Other(format!(
            "the charts would make the web page over {} MiB. Save as .xlsx or PDF instead.",
            MAX_PAGE_SVG_BYTES / (1024 * 1024)
        ))),
    }
}

#[cfg(test)]
fn prims_svg(prims: &[Prim], w: f32, h: f32, style: &str) -> String {
    prims_svg_within(prims, w, h, style, usize::MAX).unwrap_or_default()
}

/// The `<svg>` element, or `None` as soon as it grows past `budget` bytes.
fn prims_svg_within(prims: &[Prim], w: f32, h: f32, style: &str, budget: usize) -> Option<String> {
    let mut out = String::new();
    let _ = write!(
        out,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\" font-family=\"Helvetica,Arial,sans-serif\" style=\"{style}\">"
    );
    for p in prims {
        if out.len() > budget {
            return None;
        }
        match p {
            Prim::Rect { x, y, w, h, fill, stroke, radius } => {
                let _ = write!(out, "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"", num(*x), num(*y), num(*w), num(*h));
                if *radius > 0.0 {
                    let _ = write!(out, " rx=\"{}\"", num(*radius));
                }
                paint(&mut out, *fill, *stroke);
                out.push_str("/>");
            }
            Prim::Line { pts, color, width, dash } => {
                let _ = write!(out, "<polyline points=\"{}\"", points(pts));
                paint(&mut out, None, Some((*color, *width)));
                if *dash {
                    out.push_str(" stroke-dasharray=\"3 2\"");
                }
                out.push_str("/>");
            }
            Prim::Polygon { pts, fill, stroke } => {
                let _ = write!(out, "<polygon points=\"{}\"", points(pts));
                paint(&mut out, Some(*fill), *stroke);
                out.push_str("/>");
            }
            Prim::Wedge { cx, cy, r_outer, r_inner, a0, a1, fill, stroke } => {
                let _ = write!(out, "<path d=\"{}\"", wedge_path(*cx, *cy, *r_outer, *r_inner, *a0, *a1));
                paint(&mut out, Some(*fill), *stroke);
                out.push_str("/>");
            }
            Prim::Circle { cx, cy, r, fill, stroke } => {
                let _ = write!(out, "<circle cx=\"{}\" cy=\"{}\" r=\"{}\"", num(*cx), num(*cy), num(*r));
                paint(&mut out, *fill, *stroke);
                out.push_str("/>");
            }
            Prim::Text { x, y, text, size, color, bold, align, valign, rotation } => {
                // Baseline offsets as in the PDF export.
                let dy = match valign {
                    VAlign::Top => size * 0.78,
                    VAlign::Middle => size * 0.35,
                    VAlign::Bottom => -size * 0.21,
                };
                let anchor = match align {
                    HAlign::Left => "start",
                    HAlign::Center => "middle",
                    HAlign::Right => "end",
                };
                let _ = write!(out, "<text x=\"{}\" y=\"{}\" font-size=\"{}\" text-anchor=\"{anchor}\"", num(*x), num(y + dy), num(*size));
                if *bold {
                    out.push_str(" font-weight=\"bold\"");
                }
                if rotation.is_finite() && *rotation != 0.0 {
                    let _ = write!(out, " transform=\"rotate({} {} {})\"", num(rotation.to_degrees()), num(*x), num(*y));
                }
                let _ = write!(out, " fill=\"{}\"{}>{}</text>", hex(*color), opacity("fill", *color), esc(text));
            }
        }
    }
    out.push_str("</svg>");
    (out.len() <= budget).then_some(out)
}

/// Fill and stroke attributes of an open shape element; the caller closes it.
fn paint(out: &mut String, fill: Option<Rgba>, stroke: Option<(Rgba, f32)>) {
    match fill {
        Some(c) => {
            let _ = write!(out, " fill=\"{}\"{}", hex(c), opacity("fill", c));
        }
        None => out.push_str(" fill=\"none\""),
    }
    if let Some((c, w)) = stroke {
        let _ = write!(out, " stroke=\"{}\" stroke-width=\"{}\"{}", hex(c), num(w), opacity("stroke", c));
    }
}

/// A pie or doughnut sector. Angles run clockwise from 12 o'clock, like the chart crate's.
fn wedge_path(cx: f32, cy: f32, r_outer: f32, r_inner: f32, a0: f32, a1: f32) -> String {
    let span = (a1 - a0).clamp(-std::f32::consts::TAU + 0.001, std::f32::consts::TAU - 0.001);
    let a1 = a0 + span;
    let at = |a: f32, r: f32| format!("{} {}", num(cx + r * a.sin()), num(cy - r * a.cos()));
    let large = u8::from(span.abs() > std::f32::consts::PI);
    let sweep = u8::from(span >= 0.0);
    let mut d = format!("M{} A{r} {r} 0 {large} {sweep} {}", at(a0, r_outer), at(a1, r_outer), r = num(r_outer));
    if r_inner > 0.0 {
        let _ = write!(d, " L{} A{r} {r} 0 {large} {} {} Z", at(a1, r_inner), 1 - sweep, at(a0, r_inner), r = num(r_inner));
    } else {
        let _ = write!(d, " L{} {} Z", num(cx), num(cy));
    }
    d
}

fn points(pts: &[[f32; 2]]) -> String {
    pts.iter().map(|p| format!("{},{}", num(p[0]), num(p[1]))).collect::<Vec<_>>().join(" ")
}

fn hex(c: Rgba) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

fn opacity(attr: &str, c: Rgba) -> String {
    if c[3] == 255 { String::new() } else { format!(" {attr}-opacity=\"{:.2}\"", f32::from(c[3]) / 255.0) }
}

/// A coordinate with two decimals; a non-finite value is 0 so the SVG stays well-formed.
fn num(v: f32) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_svg_past_its_byte_budget_is_left_out() {
        let rect = Prim::Rect { x: 1.0, y: 2.0, w: 3.0, h: 4.0, fill: Some([255, 0, 0, 255]), stroke: None, radius: 0.0 };
        let prims = vec![rect; 10_000];
        let full = prims_svg(&prims, 100.0, 50.0, "");
        assert!(full.len() > 100_000);
        assert_eq!(prims_svg_within(&prims, 100.0, 50.0, "", full.len()), Some(full.clone()));
        assert_eq!(prims_svg_within(&prims, 100.0, 50.0, "", full.len() - 1), None);
        assert_eq!(prims_svg_within(&prims, 100.0, 50.0, "", 1_000), None);
    }

    #[test]
    fn every_primitive_becomes_an_svg_element() {
        let red = [255, 0, 0, 255];
        let prims = vec![
            Prim::Rect { x: 1.0, y: 2.0, w: 3.0, h: 4.0, fill: Some(red), stroke: None, radius: 0.0 },
            Prim::Line { pts: vec![[0.0, 0.0], [5.0, 5.5]], color: red, width: 1.0, dash: true },
            Prim::Polygon { pts: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]], fill: red, stroke: Some(([0, 0, 0, 255], 1.0)) },
            Prim::Wedge { cx: 10.0, cy: 10.0, r_outer: 5.0, r_inner: 2.0, a0: 0.0, a1: 1.0, fill: red, stroke: None },
            Prim::Circle { cx: 1.0, cy: 1.0, r: 2.0, fill: Some([0, 0, 255, 128]), stroke: None },
            Prim::Text {
                x: 5.0,
                y: 6.0,
                text: "A<B".into(),
                size: 10.0,
                color: red,
                bold: true,
                align: HAlign::Center,
                valign: VAlign::Top,
                rotation: -1.5,
            },
        ];
        let svg = prims_svg(&prims, 100.0, 50.0, "left:1px");
        for part in [
            "<rect x=\"1\" y=\"2\" width=\"3\" height=\"4\" fill=\"#ff0000\"/>",
            "<polyline points=\"0,0 5,5.5\" fill=\"none\" stroke=\"#ff0000\" stroke-width=\"1\" stroke-dasharray=\"3 2\"/>",
            "<polygon ",
            "<path d=\"M10 5 A5 5 0 0 1 ",
            "fill-opacity=\"0.50\"",
            "text-anchor=\"middle\"",
            "font-weight=\"bold\"",
            "transform=\"rotate(-85.94 5 6)\"",
            ">A&lt;B</text>",
        ] {
            assert!(svg.contains(part), "{part} in {svg}");
        }
        assert!(svg.starts_with("<svg ") && svg.ends_with("</svg>") && svg.contains("style=\"left:1px\""), "{svg}");
    }

    #[test]
    fn non_finite_numbers_do_not_break_the_markup() {
        let prims = [Prim::Circle { cx: f32::NAN, cy: f32::INFINITY, r: 1.0, fill: None, stroke: None }];
        assert!(prims_svg(&prims, 10.0, 10.0, "").contains("<circle cx=\"0\" cy=\"0\" r=\"1\" fill=\"none\"/>"));
    }

    #[test]
    fn a_full_circle_wedge_keeps_a_drawable_arc() {
        let d = wedge_path(0.0, 0.0, 10.0, 0.0, 0.0, std::f32::consts::TAU);
        assert!(d.contains("A10 10 0 1 1 ") && d.ends_with("L0 0 Z"), "{d}");
    }
}
