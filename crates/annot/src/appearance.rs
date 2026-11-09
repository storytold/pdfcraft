//! Appearance streams (§12.5.5) for the comment types PdfCraft creates, execution plan M5.2.
//!
//! [`build`] draws a normal appearance (`/AP /N`) from the annotation dictionary and the
//! objects it points at, so the same code serves new comments and restyled ones. Drawings are in page space with
//! `/BBox = /Rect` (identity matrix), except note icons, which are drawn in a 20 × 20 box.
//! It returns `None` for anything it cannot draw faithfully (unknown line endings, a cloudy
//! callout), so callers never replace an appearance with a worse one. It resolves `/QuadPoints`
//! itself; for the other values, callers that hold the document follow references first
//! (`draw::with_direct_values`).
//!
//! The note icons are PdfCraft's own drawings (AGENTS.md §1). Text boxes use the standard
//! Helvetica font with WinAnsi encoding, and draw Thai from the craft-fonts Thai face as Type 3
//! glyphs (`pdfcraft_fonts::shaped_text`, the writer Add text uses); line breaking uses
//! [`text_width`], the standard-14 metrics and the Thai face's shaped advances.

use pdfcraft_cos::{Dict, Object, PdfString, Stream};
use pdfcraft_fonts::shaped_text;
use pdfcraft_fonts::{first_undrawable, helvetica_width, literal, win_ansi, wrap_with};

/// The width of `text` at `size` as appearances draw it: Helvetica, with Thai shaped in the Thai
/// face that text boxes draw it with.
pub fn text_width(text: &str, size: f64) -> f64 {
    shaped_text::thai_width(text, size, helvetica_width)
}

/// `text` wrapped `width` wide as [`text_width`] measures it; a Thai mark stays on its letter.
pub fn wrap(text: &str, size: f64, width: f64) -> Vec<String> {
    wrap_with(text, size, width, text_width)
}

/// How far a text box's `lines` reach, in em: above the first baseline (0.9 of the 1.2 em line)
/// and below the last (0.3), or further where Thai stacked marks or descenders reach, so the box
/// is sized to their ink and doesn't clip them.
fn reach(lines: &[String]) -> [f64; 2] {
    let ink = |line: Option<&String>| line.and_then(|l| shaped_text::thai_reach(l));
    [ink(lines.first()).map_or(0.9, |r| r[1].max(0.9)), ink(lines.last()).map_or(0.3, |r| (-r[0]).max(0.3))]
}

/// The height a text box's `lines` take at `size`, inside its padding: 1.2 em a line, and more
/// for Thai that reaches further ([`reach`]).
pub fn text_lines_height(lines: &[String], size: f64) -> f64 {
    let [up, down] = reach(lines);
    lines.len().max(1) as f64 * size * 1.2 + (up - 0.9 + down - 0.3) * size
}

use crate::{Document, NOTE_SIZE, Rgb, n};

mod symbols;

pub(crate) use symbols::display_stamp;
use symbols::{CAPTION_SIZE, attach_icon, ellipse, line_caption, line_layout, note_icon, stamp};

/// Length of an arrowhead for a line of width `w`.
pub fn arrow_size(w: f64) -> f64 {
    6.0 + 3.0 * w.max(0.0)
}

/// Nominal radius of a cloudy border's bumps for a line of width `w` (intensity 1). ISO 32000-2
/// Table 169 leaves the size open; this is the size PdfCraft's polygon clouds already had, so
/// squares, circles and text boxes match them.
pub fn cloud_radius(w: f64) -> f64 {
    6.0 + 1.5 * w.max(0.0)
}

/// One of the ten `/LE` names (ISO 32000-2 Table 217). Unknown names are not drawn.
pub(crate) fn known_ending(kind: &[u8]) -> bool {
    std::str::from_utf8(kind).ok().and_then(crate::LineEnding::parse).is_some()
}

/// Draw a line ending (`/LE`) at `tip`, for a line arriving from `from`.
///
/// Returns false for an unknown style or a non-finite direction, so the caller keeps the
/// previous appearance instead of writing `NaN`. `None` and a zero-length direction draw nothing.
fn line_end(c: &mut String, kind: &[u8], tip: (f64, f64), from: (f64, f64), w: f64) -> bool {
    if kind == b"None" {
        return true;
    }
    if !known_ending(kind) {
        return false;
    }
    let (dx, dy) = (from.0 - tip.0, from.1 - tip.1);
    if !dx.is_finite() || !dy.is_finite() {
        return false;
    }
    let len = dx.hypot(dy);
    if !len.is_finite() {
        return false;
    }
    if len == 0.0 {
        return true;
    }
    let s = arrow_size(w);
    if !s.is_finite() {
        return false;
    }
    let (ix, iy) = (dx / len, dy / len);
    match kind {
        b"OpenArrow" | b"ClosedArrow" | b"ROpenArrow" | b"RClosedArrow" => {
            // Open and closed arrows keep their original geometry. A reverse arrow uses the same
            // head, pointing back along the line.
            let (ux, uy) = if kind.starts_with(b"R") { (-ix, -iy) } else { (ix, iy) };
            let (cos, sin) = (30f64.to_radians().cos(), 30f64.to_radians().sin());
            let a = (tip.0 + s * (ux * cos - uy * sin), tip.1 + s * (ux * sin + uy * cos));
            let b = (tip.0 + s * (ux * cos + uy * sin), tip.1 + s * (-ux * sin + uy * cos));
            let op = if kind.ends_with(b"ClosedArrow") { "h B" } else { "S" };
            c.push_str(&format!("{} {} m {} {} l {} {} l {op}\n", n(a.0), n(a.1), n(tip.0), n(tip.1), n(b.0), n(b.1)));
        }
        b"Square" | b"Circle" | b"Diamond" => {
            let h = s / 2.0;
            if kind == b"Square" {
                c.push_str(&format!("{} {} {} {} re h B\n", n(tip.0 - h), n(tip.1 - h), n(s), n(s)));
            } else if kind == b"Circle" {
                c.push_str(&ellipse(tip.0 - h, tip.1 - h, tip.0 + h, tip.1 + h));
                c.push_str("B\n");
            } else {
                let (px, py) = (-iy, ix);
                let v = |along: f64, across: f64| (tip.0 + ix * along + px * across, tip.1 + iy * along + py * across);
                let (a, b, d, e) = (v(h, 0.0), v(0.0, h), v(-h, 0.0), v(0.0, -h));
                c.push_str(&format!("{} {} m {} {} l {} {} l {} {} l h B\n", n(a.0), n(a.1), n(b.0), n(b.1), n(d.0), n(d.1), n(e.0), n(e.1)));
            }
        }
        _ => {
            // Butt is perpendicular to the line. Slash is that stroke turned 45 degrees.
            let (px, py) = (-iy, ix);
            let (sx, sy) = if kind == b"Slash" {
                let q = std::f64::consts::FRAC_1_SQRT_2;
                (px * q - py * q, px * q + py * q)
            } else {
                (px, py)
            };
            let h = s / 2.0;
            c.push_str(&format!("{} {} m {} {} l S\n", n(tip.0 - sx * h), n(tip.1 - sy * h), n(tip.0 + sx * h), n(tip.1 + sy * h)));
        }
    }
    true
}

/// The cubic Bézier segments (first control point, second control point, end) of a smooth curve
/// through every point of `pts`, from the first: a Catmull-Rom spline, so the curve leaves each
/// point parallel to the chord between its neighbours (an end uses itself as the missing
/// neighbour). Ink is drawn so, as Acrobat draws it, rather than as straight segments. The curve
/// stays within its points and these control points.
pub fn smooth_segments(pts: &[(f64, f64)]) -> Vec<[(f64, f64); 3]> {
    let Some(&last) = pts.last() else { return Vec::new() };
    let at = |i: usize| pts.get(i).copied().unwrap_or(last);
    (0..pts.len().saturating_sub(1))
        .map(|i| {
            let (p0, p1, p2, p3) = (at(i.saturating_sub(1)), at(i), at(i + 1), at(i + 2));
            [(p1.0 + (p2.0 - p0.0) / 6.0, p1.1 + (p2.1 - p0.1) / 6.0), (p2.0 - (p3.0 - p1.0) / 6.0, p2.1 - (p3.1 - p1.1) / 6.0), p2]
        })
        .collect()
}

/// The points where the cubic Bézier segment from `p0` (control points `c1`, `c2`) to `p3` turns
/// back in x or in y, strictly between its ends: with its ends, they bound the segment.
fn cubic_extremes(p0: (f64, f64), c1: (f64, f64), c2: (f64, f64), p3: (f64, f64)) -> Vec<(f64, f64)> {
    let at = |t: f64| {
        let u = 1.0 - t;
        let w = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
        (w[0] * p0.0 + w[1] * c1.0 + w[2] * c2.0 + w[3] * p3.0, w[0] * p0.1 + w[1] * c1.1 + w[2] * c2.1 + w[3] * p3.1)
    };
    let mut out = Vec::new();
    for (q0, q1, q2, q3) in [(p0.0, c1.0, c2.0, p3.0), (p0.1, c1.1, c2.1, p3.1)] {
        // The derivative over 3: a t² + 2 b t + c.
        let (a, b, c) = (-q0 + 3.0 * q1 - 3.0 * q2 + q3, q0 - 2.0 * q1 + q2, q1 - q0);
        let roots = if a.abs() < 1e-12 {
            if b.abs() < 1e-12 { vec![] } else { vec![-c / (2.0 * b)] }
        } else {
            let disc = b * b - a * c;
            if disc < 0.0 { vec![] } else { vec![(-b + disc.sqrt()) / a, (-b - disc.sqrt()) / a] }
        };
        out.extend(roots.into_iter().filter(|t| *t > 0.0 && *t < 1.0).map(at));
    }
    out
}

/// `c` operators for [`smooth_segments`] (the path already starts at the first point).
fn smooth_curve(pts: &[(f64, f64)]) -> String {
    smooth_segments(pts).iter().map(|[a, b, e]| format!("{} {} {} {} {} {} c\n", n(a.0), n(a.1), n(b.0), n(b.1), n(e.0), n(e.1))).collect()
}

/// Bumps one cloudy outline draws at most (plus one per edge); past it the bumps grow.
pub(crate) const MAX_CLOUD_BUMPS: usize = 20_000;

/// A cloudy border's intensity (ISO 32000-2 §12.5.4, Table 169: `/BE /I`, "in the range 0 to 2.
/// Default value: 0"), or `None` for no cloud: no `/BE`, a style other than `/C`, or intensity 0,
/// which draws the border plain.
pub(crate) fn cloud_intensity(d: &Dict) -> Option<f64> {
    let be = d.get(b"BE").and_then(|b| b.as_dict())?;
    if be.name(b"S") != Some(b"C") {
        return None;
    }
    let i = be.get(b"I").and_then(|i| i.as_f64()).unwrap_or(0.0);
    (i.is_finite() && i > 0.0).then(|| i.min(2.0))
}

/// The bump radius for a cloudy border of width `w` at `intensity` (0–2].
pub(crate) fn cloud_bump(w: f64, intensity: f64) -> f64 {
    cloud_radius(w) * intensity.clamp(0.5, 2.0)
}

/// How far past its outline a cloud with bumps of radius `bump` around an outline `perimeter`
/// long can reach. `cloud_path` rounds each edge to whole bumps, so one may span up to three
/// radii and bulge 1.5 radii; it also grows the radius to keep within its bump budget.
pub(crate) fn cloud_reach(perimeter: f64, bump: f64) -> f64 {
    1.5 * bump.max(perimeter / (2.0 * MAX_CLOUD_BUMPS as f64))
}

/// `/RD` when it is four finite, non-negative numbers (Tables 170 and 180).
fn rd_of(d: &Dict) -> Option<Vec<f64>> {
    nums(d, b"RD").filter(|r| r.len() == 4 && r.iter().all(|x| x.is_finite() && *x >= 0.0))
}

/// `rect` less `rd`, whose four numbers are the left, top, right and bottom differences.
fn less_rd(rect: [f64; 4], rd: &[f64]) -> [f64; 4] {
    let at = |i: usize| rd.get(i).copied().unwrap_or(0.0);
    [rect[0] + at(0), rect[1] + at(3), rect[2] - at(2), rect[3] - at(1)]
}

/// The box a plain (not cloudy) square or circle is drawn in: `/Rect` less a usable `/RD` (Table
/// 180: the differences between `/Rect` and "the actual boundaries of the underlying square or
/// circle"), or `None` when there is no `/RD`, it isn't four finite non-negative numbers, or it
/// leaves less than a point either way, so the caller draws in `/Rect` as without one.
fn rect_less_rd(d: &Dict, rect: [f64; 4]) -> Option<[f64; 4]> {
    let inner = less_rd(rect, &rd_of(d)?);
    (inner[2] - inner[0] >= 1.0 && inner[3] - inner[1] >= 1.0).then_some(inner)
}

/// The shape a cloudy square, circle or text box draws its bumps around: `/Rect` less `/RD`
/// (Table 180: the differences between "the left, top, right, and bottom coordinates of Rect and
/// those of the square or circle, respectively"), but at least the bumps' greatest reach and half
/// the line from each side, so they stay inside `/Rect`; without a usable `/RD`, that margin.
fn cloud_inner(d: &Dict, rect: [f64; 4], bump: f64, w: f64) -> Option<[f64; 4]> {
    // The cloud's reach and half the line keep it inside `/Rect`; an `/RD` leaving more is
    // followed, one leaving less is not.
    let perimeter = 2.0 * ((rect[2] - rect[0]) + (rect[3] - rect[1]));
    let m = cloud_reach(perimeter, bump) + w / 2.0;
    let safe = [rect[0] + m, rect[1] + m, rect[2] - m, rect[3] - m];
    let inner = match rd_of(d) {
        Some(rd) => {
            let r = less_rd(rect, &rd);
            [r[0].max(safe[0]), r[1].max(safe[1]), r[2].min(safe[2]), r[3].min(safe[3])]
        }
        None => safe,
    };
    (inner[2] - inner[0] >= 1.0 && inner[3] - inner[1] >= 1.0).then_some(inner)
}

/// Points around the ellipse inscribed in `r`, counter-clockwise, one per bump of radius `bump`
/// (each edge of the outline becomes one bump), for a cloudy circle.
fn ellipse_points(r: [f64; 4], bump: f64) -> Vec<(f64, f64)> {
    let (cx, cy, rx, ry) = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0, (r[2] - r[0]) / 2.0, (r[3] - r[1]) / 2.0);
    // Ramanujan's approximation of the perimeter; the count stays within the bump budget.
    let perimeter = std::f64::consts::PI * (3.0 * (rx + ry) - ((3.0 * rx + ry) * (rx + 3.0 * ry)).max(0.0).sqrt());
    let count = (perimeter / (2.0 * bump.max(0.5))).round().clamp(8.0, MAX_CLOUD_BUMPS as f64) as u32;
    (0..count).map(|i| f64::from(i) * std::f64::consts::TAU / f64::from(count)).map(|a| (cx + rx * a.cos(), cy + ry * a.sin())).collect()
}

/// A closed cloudy outline through `pts` (§12.5.4 `/BE /S /C`): each edge becomes a row of
/// half-circle bumps of about `r`, bulging outwards.
pub fn cloud_path(pts: &[(f64, f64)], r: f64) -> String {
    let area: f64 = pts.iter().zip(pts.iter().cycle().skip(1)).map(|(a, b)| a.0 * b.1 - b.0 * a.1).sum();
    // Outward normal of an edge: to the right of travel for counter-clockwise outlines.
    let sign = if area >= 0.0 { 1.0 } else { -1.0 };
    let perimeter: f64 = pts.iter().zip(pts.iter().cycle().skip(1)).map(|(a, b)| (b.0 - a.0).hypot(b.1 - a.1)).sum();
    // Each edge rounds to at least one bump, so the total stays within the budget plus one per edge.
    let r = r.max(perimeter / (2.0 * MAX_CLOUD_BUMPS as f64));
    let mut c = String::new();
    for (i, (a, b)) in pts.iter().zip(pts.iter().cycle().skip(1)).enumerate() {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = dx.hypot(dy);
        if i == 0 {
            c.push_str(&format!("{} {} m\n", n(a.0), n(a.1)));
        }
        if len < 1e-6 {
            continue;
        }
        let k = ((len / (2.0 * r)).round() as usize).clamp(1, 10_000);
        let step = len / k as f64;
        let (ux, uy) = (dx / len, dy / len);
        let (nx, ny) = (sign * uy, -sign * ux);
        let h = 4.0 / 3.0 * step / 2.0;
        for j in 0..k {
            let p0 = (a.0 + ux * step * j as f64, a.1 + uy * step * j as f64);
            let p1 = (a.0 + ux * step * (j + 1) as f64, a.1 + uy * step * (j + 1) as f64);
            c.push_str(&format!("{} {} {} {} {} {} c\n", n(p0.0 + nx * h), n(p0.1 + ny * h), n(p1.0 + nx * h), n(p1.1 + ny * h), n(p1.0), n(p1.1)));
        }
    }
    c.push_str("h\n");
    c
}

fn nums(d: &Dict, key: &[u8]) -> Option<Vec<f64>> {
    d.get(key)?.as_array()?.iter().map(|o| o.as_f64()).collect()
}

/// The usable `/QuadPoints` quads, by the rule redaction apply reads them with
/// ([`pdfcraft_geom::quad_points::usable_quads`]): a malformed or non-finite quad is not drawn (it
/// would write `NaN` into the stream) and the others still are. The array and its entries are
/// resolved first, as the other readers resolve them, so an indirect quad is drawn too. `None`
/// when none is usable.
fn quads(context: &AppearanceContext<'_>) -> Option<Vec<[f64; 8]>> {
    let doc = context.doc;
    let q =
        pdfcraft_geom::quad_points::usable_quads(doc.resolve(context.dict.get(b"QuadPoints")?).as_array()?.iter().map(|x| doc.resolve(x).as_f64()));
    (!q.is_empty()).then_some(q)
}

/// `/C`-style colour arrays (ISO 32000-2 Table 166): none (transparent), gray, RGB or CMYK.
/// The entries should all be numbers; a four-entry array with a non-number in it, such as
/// `[1 0 0 null]`, is still read as CMYK with that entry 0, as PDFium, MuPDF and Poppler read it.
/// A reference is not such an entry: it stands for a value this builder can't see (§7.3.10), so
/// the colour is unreadable, as the other colour and number readers here treat one.
pub(crate) fn color(d: &Dict, key: &[u8]) -> Option<Option<Rgb>> {
    let c = match d.get(key) {
        None => return Some(None),
        Some(o) => match o.as_array()? {
            a if a.len() == 4 => a
                .iter()
                .map(|o| match o {
                    Object::Ref(_) => None,
                    o => Some(o.as_f64().unwrap_or(0.0)),
                })
                .collect::<Option<Vec<f64>>>()?,
            a => a.iter().map(|o| o.as_f64()).collect::<Option<Vec<f64>>>()?,
        },
    };
    Some(match c.as_slice() {
        [] => None,
        [g] => Some([*g; 3]),
        [r, g, b] => Some([*r, *g, *b]),
        [c, m, y, k] => Some([(1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)]),
        _ => return None,
    })
}

fn interior_color(d: &Dict, fallback: Option<Rgb>) -> Option<Rgb> {
    color(d, b"IC").flatten().or(fallback)
}

/// The border width (ISO 32000-2 §12.5.4): `/BS /W` when there is a border style dictionary, which
/// makes `/Border` ignored (Table 166), else the third element of `/Border [hr vr w [dash]]`, read
/// whether or not the optional fourth element, a dash array, follows. 1 when neither says, and
/// when the number given is negative or not finite.
pub(crate) fn border_width(d: &Dict) -> f64 {
    let width = match d.get(b"BS").and_then(Object::as_dict) {
        Some(bs) => bs.get(b"W").and_then(Object::as_f64),
        None => d.get(b"Border").and_then(Object::as_array).and_then(|b| b.get(2)).and_then(Object::as_f64),
    };
    width.filter(|w| w.is_finite() && *w >= 0.0).unwrap_or(1.0)
}

/// Elements a dash array may have before it is drawn solid instead: the lengths are written into
/// the appearance as they are, so a file can't make it arbitrarily large.
const MAX_DASH: usize = 64;

/// A dash array (§8.4.3.6) as drawn: every element finite and non-negative, not all zero (that
/// pattern is invalid) and at most [`MAX_DASH`] long. `None` for anything else, which is drawn solid.
fn dash_array(o: &Object) -> Option<Vec<f64>> {
    let a = o.as_array().filter(|a| !a.is_empty() && a.len() <= MAX_DASH)?;
    let v: Vec<f64> = a.iter().map(Object::as_f64).collect::<Option<_>>()?;
    (v.iter().all(|x| x.is_finite() && *x >= 0.0) && v.iter().any(|x| *x > 0.0)).then_some(v)
}

/// Dash pattern operator for a dashed border, if any: `/BS /S /D` with its `/D` (default `[3]`,
/// Table 168), or, without `/BS`, the dash array that may follow the width in `/Border` (Table 166).
fn dash(d: &Dict) -> String {
    let arr = match d.get(b"BS").and_then(Object::as_dict) {
        Some(bs) if bs.name(b"S") != Some(b"D") => return String::new(),
        Some(bs) => match bs.get(b"D") {
            None => Some(vec![3.0]),
            Some(o) => dash_array(o),
        },
        None => d.get(b"Border").and_then(Object::as_array).and_then(|b| b.get(3)).and_then(dash_array),
    };
    let Some(arr) = arr else { return String::new() };
    format!("[{}] 0 d\n", arr.iter().map(|x| n(*x)).collect::<Vec<_>>().join(" "))
}

fn rg(c: Rgb) -> String {
    format!("{} {} {} rg\n", n(c[0]), n(c[1]), n(c[2]))
}

fn rg_stroke(c: Rgb) -> String {
    format!("{} {} {} RG\n", n(c[0]), n(c[1]), n(c[2]))
}

/// A FreeText's border colour where it has its own: the stroking colour in `/DA` (`RG`, or `G`
/// for grey). `None`: the border takes the text colour.
pub fn da_border(d: &Dict) -> Option<Rgb> {
    let da = d.get(b"DA").and_then(|o| o.as_string()).map(|s| String::from_utf8_lossy(&s.bytes).into_owned())?;
    let toks: Vec<&str> = da.split_whitespace().collect();
    let num = |i: usize| toks.get(i).and_then(|t| t.parse::<f64>().ok()).filter(|x| x.is_finite());
    let mut col = None;
    for (i, t) in toks.iter().enumerate() {
        match *t {
            "RG" if i >= 3 => {
                if let (Some(r), Some(g), Some(b)) = (num(i - 3), num(i - 2), num(i - 1)) {
                    col = Some([r, g, b].map(|x| x.clamp(0.0, 1.0)));
                }
            }
            "G" if i >= 1 => {
                if let Some(g) = num(i - 1) {
                    col = Some([g.clamp(0.0, 1.0); 3]);
                }
            }
            _ => {}
        }
    }
    col
}

/// `/DA` of a text box: (text colour, font size). Defaults to black 12 pt.
pub fn parse_da(d: &Dict) -> (Rgb, f64) {
    let da = d.get(b"DA").and_then(|o| o.as_string()).map(|s| String::from_utf8_lossy(&s.bytes).into_owned()).unwrap_or_default();
    let toks: Vec<&str> = da.split_whitespace().collect();
    let mut col = [0.0; 3];
    let mut size = 12.0;
    let num = |i: usize| toks.get(i).and_then(|t| t.parse::<f64>().ok());
    for (i, t) in toks.iter().enumerate() {
        match *t {
            "rg" if i >= 3 => {
                if let (Some(r), Some(g), Some(b)) = (num(i - 3), num(i - 2), num(i - 1)) {
                    col = [r, g, b];
                }
            }
            "g" if i >= 1 => {
                if let Some(g) = num(i - 1) {
                    col = [g; 3];
                }
            }
            "Tf" if i >= 1 => {
                if let Some(s) = num(i - 1).filter(|s| *s > 0.0) {
                    size = s;
                }
            }
            _ => {}
        }
    }
    (col.map(|x| x.clamp(0.0, 1.0)), size.min(400.0))
}

/// How opaque a comment's parts are drawn, 0–1: its line or border (`/CA`); its fill or
/// background (`/ca`, which PDF 2.0 gives the non-stroking operations; without it, as the line);
/// and a text box's text (`/PCTextOpacity`, PdfCraft's own; without it, as the fill, since text
/// is filled too). A comment with only `/CA` is drawn at that opacity throughout, as before.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PartOpacity {
    pub line: f64,
    pub fill: f64,
    pub text: f64,
}

/// The parts' opacities of the comment `d` (see [`PartOpacity`]).
pub fn part_opacity(d: &Dict) -> PartOpacity {
    let get = |key: &[u8]| d.get(key).and_then(|o| o.as_f64()).filter(|x| x.is_finite()).map(|x| x.clamp(0.0, 1.0));
    let line = get(b"CA").unwrap_or(1.0);
    let fill = get(b"ca").unwrap_or(line);
    let text = get(b"PCTextOpacity").unwrap_or(fill);
    PartOpacity { line, fill, text }
}

/// `/GS0`: the line's opacity for strokes and the fill's for everything filled; `/GS1`, where the
/// text's differs from the fill's: the text's for its glyphs.
fn ext_gstate(parts: PartOpacity, multiply: bool) -> Option<Dict> {
    if parts.line >= 1.0 && parts.fill >= 1.0 && parts.text >= 1.0 && !multiply {
        return None;
    }
    let gs = |stroke: f64, fill: f64| {
        let mut gs = Dict::new();
        gs.set(b"Type".to_vec(), Object::name("ExtGState"));
        if multiply {
            gs.set(b"BM".to_vec(), Object::name("Multiply"));
        }
        gs.set(b"CA".to_vec(), Object::Real(stroke));
        gs.set(b"ca".to_vec(), Object::Real(fill));
        Object::Dict(gs)
    };
    let mut res = Dict::new();
    res.set(b"GS0".to_vec(), gs(parts.line, parts.fill));
    if parts.text != parts.fill {
        res.set(b"GS1".to_vec(), gs(parts.line, parts.text));
    }
    Some(res)
}

pub(crate) fn form(bbox: [f64; 4], content: &[u8], resources: Dict) -> Stream {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Form"));
    d.set(b"FormType".to_vec(), Object::Int(1));
    d.set(b"BBox".to_vec(), Object::Array(bbox.iter().map(|x| Object::Real(*x)).collect()));
    d.set(b"Resources".to_vec(), Object::Dict(resources));
    Stream::flate(d, content)
}

/// The `cm` that draws a picture into `at`: an image (its unit square) or a form XObject whose
/// `/Matrix` maps it to `size` points (`None`, or a side that is not positive, draws nothing).
pub(crate) fn picture_place(at: [f64; 4], image: bool, size: Option<(f64, f64)>) -> Option<String> {
    let [x0, y0, x1, y1] = at;
    let (w, h) = (x1 - x0, y1 - y0);
    if image {
        return Some(format!("{} 0 0 {} {} {} cm", n(w), n(h), n(x0), n(y0)));
    }
    let (sw, sh) = size.filter(|(sw, sh)| *sw > 0.0 && *sh > 0.0)?;
    Some(format!("{} 0 0 {} {} {} cm", n(w / sw), n(h / sh), n(x0), n(y0)))
}

/// The page `/Rotate` an image signature's picture is turned back by (`/PCPictureRotate`, written
/// when the signature is added to a turned page): 0, 90, 180 or 270.
pub(crate) fn picture_turn(d: &Dict) -> i64 {
    d.get(b"PCPictureRotate").and_then(Object::as_int).unwrap_or(0).rem_euclid(360) / 90 * 90
}

/// The page `/Rotate` a rubber stamp's label is turned back by (`/PCStampRotate`).
fn stamp_turn(d: &Dict) -> i64 {
    d.get(b"PCStampRotate").and_then(Object::as_int).unwrap_or(0).rem_euclid(360) / 90 * 90
}

/// The page `/Rotate` a text box's or Fill & Sign text's lines are turned back by
/// (`/PCTextRotate`, written when it is added to a turned page).
pub(crate) fn text_turn(d: &Dict) -> i64 {
    d.get(b"PCTextRotate").and_then(Object::as_int).unwrap_or(0).rem_euclid(360) / 90 * 90
}

/// A FreeText's `/RD`: how far its text box lies inside `/Rect` from the left, bottom, right and
/// top. A distance a hair below 0 (rounding) reads as 0; `None` when one is negative or they can't
/// be read.
pub(crate) fn inset(d: &Dict) -> Option<[f64; 4]> {
    let rd: [f64; 4] = nums(d, b"RD")?.try_into().ok()?;
    rd.iter().all(|x| x.is_finite() && *x > -0.001).then(|| rd.map(|x| x.max(0.0)))
}

/// The first text [`build`] would draw in the standard fonts (Helvetica, WinAnsiEncoding) that
/// they can't, which would come out as `?`: what it is (for the message) and the character. The
/// text of a text box, callout or Fill & Sign text (whose Thai the Thai face draws, where this
/// build has one), a line's caption, a measurement's label, a
/// dynamic stamp's "By …" line, and a redaction mark's overlay text (drawn when it is applied). `None` when all of
/// it can be drawn.
pub fn undrawable_text(d: &Dict) -> Option<(&'static str, char)> {
    let text = |key: &[u8]| d.get(key).and_then(Object::as_string).map(PdfString::to_text);
    let (what, text) = match d.name(b"Subtype")? {
        // A text box draws Thai too.
        b"FreeText" => return Some(("the comment's text", shaped_text::first_undrawable(&text(b"Contents")?)?)),
        b"Line" if !d.contains(b"PCMeasureValue") => ("the line's caption", line_caption(d)?),
        b"Line" | b"PolyLine" | b"Polygon" => ("the measurement label", text(b"PCMeasureValue")?),
        b"Stamp" if matches!(d.get(b"PCStamp"), Some(Object::Bool(true))) => ("the stamp's \"By\" line", text(b"PCByLine")?),
        // Drawn in Helvetica when the redaction is applied: refused when it is marked.
        b"Redact" => ("the redaction's overlay text", text(b"OverlayText")?),
        _ => return None,
    };
    Some((what, first_undrawable(&text)?))
}

/// `/Font` resources naming `/Helv`: standard Helvetica with WinAnsiEncoding.
pub(crate) fn helvetica_resource() -> Object {
    Object::Dict(helvetica_fonts())
}

/// [`helvetica_resource`]'s dictionary.
fn helvetica_fonts() -> Dict {
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("Type1"));
    font.set(b"BaseFont".to_vec(), Object::name("Helvetica"));
    font.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
    let mut fonts = Dict::new();
    fonts.set(b"Helv".to_vec(), Object::Dict(font));
    fonts
}

/// Where a caret and its paragraph symbol go within `rect`: the box the caret is drawn in, and for
/// `/Sy /P` the symbol's baseline origin and font size (ISO 32000-2 Table 183).
///
/// The caret lies within `/Rect` less `/RD` (left, top, right, bottom), when `/RD` is four finite,
/// non-negative numbers that leave a box; otherwise within `/Rect`. The ¶ sits to the right of the
/// caret, its height a little over half the caret's, in the room `/RD` leaves on the right; when
/// that is too narrow, the caret gives up to half its width and the symbol shrinks to what is left,
/// so both stay inside `/Rect` (the form's `/BBox`).
fn caret_layout(d: &Dict, rect: [f64; 4]) -> ([f64; 4], Option<(f64, f64, f64)>) {
    let rd = nums(d, b"RD").filter(|r| r.len() == 4 && r.iter().all(|x| x.is_finite() && *x >= 0.0));
    let mut caret = match rd {
        Some(rd) => [rect[0] + rd[0], rect[1] + rd[3], rect[2] - rd[2], rect[3] - rd[1]],
        None => rect,
    };
    if !(caret[2] > caret[0] && caret[3] > caret[1]) {
        caret = rect;
    }
    if d.name(b"Sy") != Some(b"P") {
        return (caret, None);
    }
    let size = (caret[3] - caret[1]) * 0.6;
    let (gap, glyph) = (size * 0.1, text_width("\u{b6}", size));
    let need = gap + glyph;
    let room = rect[2] - caret[2];
    if room < need {
        caret[2] -= (need - room).min((caret[2] - caret[0]) * 0.5);
    }
    let room = rect[2] - caret[2];
    let scale = if need > room { room / need } else { 1.0 };
    let size = size * scale;
    if !(size.is_finite() && size > 0.0) {
        return (caret, None);
    }
    // The pilcrow descends about a sixth of the size below its baseline.
    (caret, Some((caret[2] + gap * scale, caret[1] + size * 0.18, size)))
}

#[derive(Clone, Copy)]
struct AppearanceContext<'a> {
    doc: &'a Document,
    dict: &'a Dict,
    subtype: &'a [u8],
    rect: [f64; 4],
    /// A cloudy border's intensity ([`cloud_intensity`]), or `None` for a plain one.
    cloud: Option<f64>,
    /// The line's opacity (`parts.line`).
    opacity: f64,
    parts: PartOpacity,
    stroke: Option<Rgb>,
    width: f64,
}

#[derive(Default)]
struct AppearanceDrawing {
    resources: Dict,
    content: String,
    tail: Vec<u8>,
}

impl AppearanceDrawing {
    /// Initialize an appearance drawing buffer and configure its graphics state for opacity and blend mode.
    fn new(context: &AppearanceContext<'_>) -> Self {
        let mut drawing = Self::default();
        if let Some(gs) = ext_gstate(
            context.parts,
            context.subtype == b"Highlight" || (context.subtype == b"Ink" && context.dict.name(b"IT") == Some(b"FreehandHighlight")),
        ) {
            drawing.resources.set(b"ExtGState".to_vec(), Object::Dict(gs));
            drawing.content.push_str("/GS0 gs\n");
        }
        drawing
    }
}

/// Draw the normal appearance of an annotation, or `None` if this subtype/variant isn't supported.
/// Text the standard fonts can't draw comes out as `?`: [`undrawable_text`] says which, and
/// [`crate::set_appearance`] marks the annotation with it (`pdfcraft_fonts::UNDRAWN_MARK`).
pub fn build(doc: &Document, d: &Dict) -> Option<Stream> {
    build_on_page(doc, d, None)
}

/// [`build`] for an annotation on a page whose visible region is `page` (`Page::crop` in
/// pdfcraft-model): a text markup, line, polyline, polygon or ink annotation whose points reach
/// past its `/Rect` is drawn through them as far as the page shows ([`point_reach`]), and its
/// appearance's `/BBox` says how far ([`widened_rect`]). Without a page it is drawn within `/Rect`.
pub(crate) fn build_on_page(doc: &Document, d: &Dict, page: Option<[f64; 4]>) -> Option<Stream> {
    let context = appearance_context(doc, d, page)?;
    let drawing = AppearanceDrawing::new(&context);
    match context.subtype {
        b"Text" => return draw_icon(&context, "Note", [1.0, 0.82, 0.0], drawing),
        b"FileAttachment" => return draw_icon(&context, "PushPin", [0.0, 0.47, 0.84], drawing),
        b"Stamp" => return draw_stamp(&context, drawing),
        b"FreeText" => return draw_free_text(&context, drawing),
        _ => {}
    }
    let mut drawing = drawing;
    draw_body(&context, &mut drawing)?;
    finish_appearance(&context, drawing)
}

fn appearance_context<'a>(doc: &'a Document, d: &'a Dict, page: Option<[f64; 4]>) -> Option<AppearanceContext<'a>> {
    // Imported measurement appearances may include leaders, captions and formatting that
    // this builder cannot reproduce. Preserve them rather than silently losing detail.
    if d.contains(b"Measure") && !d.contains(b"PCMeasureValue") {
        return None;
    }
    let subtype = d.name(b"Subtype")?;
    // A number too long for a double reads as infinite, and two near its limit are an infinite
    // width apart: nothing is drawn rather than "inf" written into the appearance (review on #4195).
    let rect = normalised_rect(d).filter(|r| r.iter().all(|x| x.is_finite()) && (r[2] - r[0]).is_finite() && (r[3] - r[1]).is_finite())?;
    let rect = page.and_then(|page| point_reach(doc, d, rect, page)).unwrap_or(rect);
    // Cloudy borders (§12.5.4, Table 169) apply to squares, circles, polygons and text boxes.
    let cloud = cloud_intensity(d);
    if cloud.is_some() && !matches!(subtype, b"Square" | b"Circle" | b"Polygon" | b"FreeText") {
        return None;
    }
    Some(AppearanceContext {
        doc,
        dict: d,
        subtype,
        rect,
        cloud,
        opacity: part_opacity(d).line,
        parts: part_opacity(d),
        stroke: color(d, b"C")?,
        width: border_width(d),
    })
}

/// `/Rect` as lower-left and upper-right corners (a rectangle may be written with either pair of
/// opposite corners, ISO 32000-2 §7.9.5); `None` unless it is four numbers.
fn normalised_rect(d: &Dict) -> Option<[f64; 4]> {
    let [x0, y0, x1, y1] = <[f64; 4]>::try_from(nums(d, b"Rect")?).ok()?;
    Some([x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)])
}

/// The subtypes drawn through points of their own rather than within `/Rect`: text markup over
/// its `/QuadPoints` (ISO 32000-2 §12.5.6.10), lines along `/L` (§12.5.6.7), polygons and
/// polylines through `/Vertices` (§12.5.6.9) and ink through `/InkList` (§12.5.6.13).
pub(crate) fn is_drawn_from_points(d: &Dict) -> bool {
    matches!(d.name(b"Subtype"), Some(b"Highlight" | b"Underline" | b"StrikeOut" | b"Squiggly" | b"Line" | b"PolyLine" | b"Polygon" | b"Ink"))
}

/// At most this many quadrilaterals widen a text markup's box ([`point_reach`]). Each takes in a
/// word or a group of contiguous words (ISO 32000-2 Table 182), usually a line, so even written
/// word by word a markup over a whole dense page stays well below it; a larger one is drawn
/// within its `/Rect`, as before the box was widened.
pub(crate) const MAX_MARKUP_QUADS: usize = 10_000;

/// At most this many points widen a line's, polyline's, polygon's or ink annotation's box
/// ([`point_reach`]): as many as [`MAX_MARKUP_QUADS`] quadrilaterals have. A larger one is drawn
/// within its `/Rect`.
pub(crate) const MAX_REACH_POINTS: usize = 4 * MAX_MARKUP_QUADS;

/// The points `d` is drawn through, as `x y` pairs, and how far its strokes reach past them:
/// half the border width, and for a line ending (`/LE`) or a cloudy border (§12.5.4) the size
/// they are drawn at. `None` for other subtypes, more points than the caps allow, and points that
/// aren't all finite numbers.
fn drawn_points(doc: &Document, d: &Dict) -> Option<(Vec<f64>, f64)> {
    let w = border_width(d);
    let has_ending = || d.get(b"LE").and_then(Object::as_array).is_some_and(|e| e.iter().any(|e| e.as_name().is_some_and(|e| e != b"None")));
    let (points, margin) = match d.name(b"Subtype")? {
        subtype @ (b"Highlight" | b"Underline" | b"StrikeOut" | b"Squiggly") => {
            // The quads `draw_markup` draws (`quads`: resolved, malformed ones skipped), counted
            // before the numbers are read.
            let array = doc.resolve(d.get(b"QuadPoints")?);
            let entries = array.as_array()?;
            if entries.len() / 8 > MAX_MARKUP_QUADS {
                return None;
            }
            let quads = pdfcraft_geom::quad_points::usable_quads(entries.iter().map(|x| doc.resolve(x).as_f64()));
            if quads.is_empty() {
                return None;
            }
            // How far each quad's stroke reaches past it: half the line's width, and a squiggle's
            // peaks above a quad lower than they are (`draw_markup`).
            let margin = quads.iter().map(|q| markup_overreach(subtype, (q[0] - q[4]).hypot(q[1] - q[5]))).fold(0.0, f64::max);
            (quads.into_iter().flatten().collect(), margin)
        }
        b"Ink" => {
            let strokes = d.get(b"InkList")?.as_array()?;
            let count = strokes.iter().try_fold(0usize, |sum, s| sum.checked_add(s.as_array()?.len()))?;
            if count / 2 > MAX_REACH_POINTS {
                return None;
            }
            let mut points = Vec::with_capacity(count.saturating_mul(3));
            for s in strokes {
                let stroke: Vec<f64> = s.as_array()?.iter().map(Object::as_f64).collect::<Option<_>>()?;
                // A stroke's odd last number is no point (`draw_ink` skips it too).
                let pts: Vec<(f64, f64)> = stroke.as_chunks::<2>().0.iter().map(|p| (p[0], p[1])).collect();
                points.extend(pts.iter().flat_map(|p| [p.0, p.1]));
                // The smooth curve through three or more points (`smooth_segments`) can bulge
                // past them: its turning points count too.
                if pts.len() > 2 {
                    let mut start = pts.first().copied().unwrap_or_default();
                    for [c1, c2, end] in smooth_segments(&pts) {
                        points.extend(cubic_extremes(start, c1, c2, end).iter().flat_map(|p| [p.0, p.1]));
                        start = end;
                    }
                }
            }
            (points, w / 2.0)
        }
        subtype @ (b"PolyLine" | b"Polygon") => {
            let count = d.get(b"Vertices")?.as_array()?.len();
            if count < 4 || count % 2 != 0 || count / 2 > MAX_REACH_POINTS {
                return None;
            }
            let vertices = nums(d, b"Vertices")?;
            // A cloud's bumps (`draw_polygon`) reach as far past the outline as `cloud_reach` says.
            let reach = match (subtype, cloud_intensity(d)) {
                (b"Polygon", Some(intensity)) => {
                    let pts: Vec<(f64, f64)> = vertices.as_chunks::<2>().0.iter().map(|p| (p[0], p[1])).collect();
                    let perimeter: f64 = pts.iter().zip(pts.iter().cycle().skip(1)).map(|(a, b)| (b.0 - a.0).hypot(b.1 - a.1)).sum();
                    cloud_reach(if perimeter.is_finite() { perimeter } else { 0.0 }, cloud_bump(w, intensity))
                }
                (b"PolyLine", _) if has_ending() => arrow_size(w),
                _ => 0.0,
            };
            (vertices, w / 2.0 + reach)
        }
        b"Line" => {
            let [x0, y0, x1, y1] = <[f64; 4]>::try_from(nums(d, b"L")?).ok()?;
            if ![x0, y0, x1, y1].iter().all(|v| v.is_finite()) {
                return None;
            }
            // Leader lines move the line off `/L` (§12.5.6.7, Table 178).
            let layout = line_layout(d, [(x0, y0), (x1, y1)], w);
            let mut points = vec![x0, y0, x1, y1];
            for (x, y) in layout.line.iter().chain(layout.leaders.iter().flatten()).chain(layout.tick.iter().flatten()) {
                points.extend([*x, *y]);
            }
            // The caption's box: its width along the text matrix, from the descender to the
            // cap height and a little over (Helvetica, as `draw_line` sets it).
            if let Some((text, m)) = &layout.caption {
                let width = text_width(text, CAPTION_SIZE);
                for (x, y) in [(0.0, -0.25), (width, -0.25), (0.0, 1.0), (width, 1.0)].map(|(x, y)| (x, y * CAPTION_SIZE)) {
                    points.extend([m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]]);
                }
            }
            (points, w / 2.0 + if has_ending() { arrow_size(w) } else { 0.0 })
        }
        _ => return None,
    };
    (points.iter().all(|v| v.is_finite()) && margin.is_finite()).then_some((points, margin))
}

/// An annotation's `/Rect` (normalised, `rect`) grown to take in the points it is drawn through
/// ([`drawn_points`]) where they reach past it, as far as the page's visible region `page` goes;
/// `None` when they don't, aren't usable or number more than the caps allow, and for subtypes
/// drawn within `/Rect` ([`is_drawn_from_points`]). The points are what the annotation draws (ISO
/// 32000-2 Table 182 `/QuadPoints`, Table 178 `/L`, Table 181 `/Vertices`, Table 185 `/InkList`),
/// and the appearance is mapped onto `/Rect` (§12.5.5), so a `/Rect` that leaves them out would
/// cut off what is drawn: a highlight or ink stroke whose `/Rect` lies beside it drew nothing.
/// Once a point is outside, the box takes in the strokes' width and line endings too. Nothing
/// outside the crop box is shown or printed (§14.11.2), so the box grows no further than it; it
/// never shrinks `rect`, and a `/Rect` that holds every point is kept as it is.
fn point_reach(doc: &Document, d: &Dict, rect: [f64; 4], page: [f64; 4]) -> Option<[f64; 4]> {
    if !rect.iter().all(|v| v.is_finite()) {
        return None;
    }
    let (points, margin) = drawn_points(doc, d)?;
    let mut reach = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for p in points.as_chunks::<2>().0 {
        reach = [reach[0].min(p[0]), reach[1].min(p[1]), reach[2].max(p[0]), reach[3].max(p[1])];
    }
    let clip = |b: [f64; 4]| [b[0].max(page[0]), b[1].max(page[1]), b[2].min(page[2]), b[3].min(page[3])];
    let shown = clip(reach);
    // Strokes wholly off the page widen nothing. Points `/Rect` already holds don't either, and
    // points off the page whose strokes reach onto it only as far as `/Rect` holds.
    let stroked = clip([reach[0] - margin, reach[1] - margin, reach[2] + margin, reach[3] + margin]);
    if stroked[0] > stroked[2] || stroked[1] > stroked[3] {
        return None;
    }
    let inside = |b: [f64; 4]| rect[0] <= b[0] && rect[1] <= b[1] && b[2] <= rect[2] && b[3] <= rect[3];
    let on_page = shown[0] <= shown[2] && shown[1] <= shown[3];
    if inside(if on_page { shown } else { stroked }) {
        return None;
    }
    let reach = stroked;
    let out = [rect[0].min(reach[0]), rect[1].min(reach[1]), rect[2].max(reach[2]), rect[3].max(reach[3])];
    // A `/Rect` of its own can still be too wide for a width or height (`-1e308` to `1e308`).
    let usable = (out[2] - out[0]).is_finite() && (out[3] - out[1]).is_finite();
    (usable && out != rect).then_some(out)
}

/// The `/Rect` an annotation takes with its built appearance `stream`: the appearance's `/BBox`
/// where [`point_reach`] widened it past `/Rect`, as the appearance is mapped onto `/Rect`
/// (§12.5.5); `None` (keep `/Rect`) otherwise and for subtypes drawn within `/Rect`.
pub(crate) fn widened_rect(d: &Dict, stream: &Stream) -> Option<[f64; 4]> {
    if !is_drawn_from_points(d) {
        return None;
    }
    let bbox = <[f64; 4]>::try_from(nums(&stream.dict, b"BBox")?).ok()?;
    (bbox.iter().all(|v| v.is_finite()) && Some(bbox) != normalised_rect(d)).then_some(bbox)
}

fn draw_body(context: &AppearanceContext<'_>, drawing: &mut AppearanceDrawing) -> Option<()> {
    match context.subtype {
        b"Highlight" | b"Underline" | b"StrikeOut" | b"Squiggly" => draw_markup(context, drawing),
        b"Redact" => draw_redaction(context, drawing),
        b"Square" | b"Circle" => draw_box(context, drawing),
        b"Line" => draw_line(context, drawing),
        b"Polygon" | b"PolyLine" => draw_polygon(context, drawing),
        b"Caret" => draw_caret(context, drawing),
        b"Ink" => draw_ink(context, drawing),
        _ => None,
    }
}

fn finish_appearance(context: &AppearanceContext<'_>, mut drawing: AppearanceDrawing) -> Option<Stream> {
    // PdfCraft measurement captions are kept separate from the comment's free-form text.
    // A restyle regenerates the path and its value together.
    let mut out = drawing.content.into_bytes();
    out.extend(drawing.tail);
    if let Some(value) = context.dict.get(b"PCMeasureValue").and_then(Object::as_string) {
        let value = value.to_text();
        if value.chars().count() <= 256 && matches!(context.subtype, b"Line" | b"PolyLine" | b"Polygon") {
            let size = 10.0;
            let x = (context.rect[0] + context.rect[2] - text_width(&value, size)) * 0.5;
            let y = context.rect[3] - 12.0;
            let col = context.stroke.unwrap_or([0.0, 0.47, 0.84]);
            out.extend(format!("{}BT /Helv {} Tf {} {} Td ", rg(col), n(size), n(x), n(y)).bytes());
            // WinAnsi bytes (e.g. 0xB2 for "²") go into the stream as-is, not through UTF-8.
            out.extend(literal(&win_ansi(&value)));
            out.extend_from_slice(b" Tj ET\n");
            drawing.resources.set(b"Font".to_vec(), helvetica_resource());
        }
    }
    Some(form(context.rect, &out, drawing.resources))
}

/// The line width of an underline or strike-out over a quad `h` high.
fn markup_line_width(h: f64) -> f64 {
    (h * 0.07).clamp(0.5, 3.0)
}

/// A squiggle's line width and amplitude over a quad `h` high: its peaks stand twice the
/// amplitude above the quad's bottom edge.
fn squiggle(h: f64) -> (f64, f64) {
    ((h * 0.05).clamp(0.5, 2.0), (h * 0.06).max(0.75))
}

/// How far a text markup of `subtype` drawn over a quad `h` high reaches past the quad: half its
/// line's width, and for a squiggle, how far its peaks stand above a quad lower than they are.
fn markup_overreach(subtype: &[u8], h: f64) -> f64 {
    match subtype {
        b"Underline" | b"StrikeOut" => markup_line_width(h) / 2.0,
        b"Squiggly" => {
            let (lw, amp) = squiggle(h);
            lw / 2.0 + (amp * 2.0 - h).max(0.0)
        }
        _ => 0.0,
    }
}

fn draw_markup(context: &AppearanceContext<'_>, drawing: &mut AppearanceDrawing) -> Option<()> {
    let subtype = context.subtype;
    let stroke = context.stroke;
    let c = &mut drawing.content;
    let q = quads(context)?;
    let col = stroke?;
    for quad in q.iter() {
        let p = |i: usize| (quad[2 * i], quad[2 * i + 1]);
        let (p1, p2, p3, p4) = (p(0), p(1), p(2), p(3));
        let h = (p1.0 - p3.0).hypot(p1.1 - p3.1);
        // A point a fraction `t` of the way from the bottom edge to the top edge.
        let at = |bottom: (f64, f64), top: (f64, f64), t: f64| (bottom.0 + (top.0 - bottom.0) * t, bottom.1 + (top.1 - bottom.1) * t);
        match subtype {
            b"Highlight" => {
                c.push_str(&rg(col));
                c.push_str(&format!("{} {} m {} {} l {} {} l {} {} l h f\n", n(p1.0), n(p1.1), n(p2.0), n(p2.1), n(p4.0), n(p4.1), n(p3.0), n(p3.1)));
            }
            b"Underline" | b"StrikeOut" => {
                let lw = markup_line_width(h);
                let t = if subtype == b"Underline" { 0.08 } else { 0.42 };
                let (a, b) = (at(p3, p1, t), at(p4, p2, t));
                c.push_str(&format!("{}{} w\n{} {} m {} {} l S\n", rg_stroke(col), n(lw), n(a.0), n(a.1), n(b.0), n(b.1)));
            }
            _ => {
                // Squiggly: a zigzag along the bottom of the quad.
                let (lw, amp) = squiggle(h);
                let len = (p4.0 - p3.0).hypot(p4.1 - p3.1);
                let step = (h / 4.0).max(1.5);
                let steps = ((len / step).ceil() as usize).clamp(1, 10_000);
                c.push_str(&format!("{}{} w 1 j\n", rg_stroke(col), n(lw)));
                for i in 0..=steps {
                    let t = i as f64 / steps as f64;
                    let base = (p3.0 + (p4.0 - p3.0) * t, p3.1 + (p4.1 - p3.1) * t);
                    let up = if i % 2 == 0 { amp * 2.0 } else { 0.0 };
                    let (ux, uy) = if h > 0.0 { ((p1.0 - p3.0) / h, (p1.1 - p3.1) / h) } else { (0.0, 1.0) };
                    let (x, y) = (base.0 + ux * up, base.1 + uy * up);
                    c.push_str(&format!("{} {} {}\n", n(x), n(y), if i == 0 { "m" } else { "l" }));
                }
                c.push_str("S\n");
            }
        }
    }
    Some(())
}

fn draw_redaction(context: &AppearanceContext<'_>, drawing: &mut AppearanceDrawing) -> Option<()> {
    let stroke = context.stroke;
    let c = &mut drawing.content;
    // While marked: the outline of each area (the fill comes when applied).
    let q = quads(context)?;
    let col = stroke.unwrap_or([0.89, 0.13, 0.13]);
    c.push_str(&format!("{}1 w\n", rg_stroke(col)));
    for quad in q.iter() {
        c.push_str(&format!(
            "{} {} m {} {} l {} {} l {} {} l h S\n",
            n(quad[0]),
            n(quad[1]),
            n(quad[2]),
            n(quad[3]),
            n(quad[6]),
            n(quad[7]),
            n(quad[4]),
            n(quad[5])
        ));
    }
    Some(())
}

fn draw_box(context: &AppearanceContext<'_>, drawing: &mut AppearanceDrawing) -> Option<()> {
    let d = context.dict;
    let subtype = context.subtype;
    let rect = context.rect;
    let stroke = context.stroke;
    let w = context.width;
    let c = &mut drawing.content;
    let fill = interior_color(d, None);
    if stroke.is_none() && fill.is_none() {
        return Some(());
    }
    // A plain square or circle lies within /Rect less /RD (Table 180), its stroke centred
    // half the width inside that box.
    let shape = rect_less_rd(d, rect).unwrap_or(rect);
    let inset = w / 2.0;
    let [x0, y0, x1, y1] = [shape[0] + inset, shape[1] + inset, shape[2] - inset, shape[3] - inset];
    if let Some(f) = fill {
        c.push_str(&rg(f));
    }
    if let Some(s) = stroke {
        c.push_str(&format!("{}{} w\n{}", rg_stroke(s), n(w), dash(d)));
    }
    if let Some(intensity) = context.cloud {
        let bump = cloud_bump(w, intensity);
        let inner = cloud_inner(d, rect, bump, w)?;
        let pts = if subtype == b"Square" {
            vec![(inner[0], inner[1]), (inner[2], inner[1]), (inner[2], inner[3]), (inner[0], inner[3])]
        } else {
            ellipse_points(inner, bump)
        };
        c.push_str(&format!("1 j\n{}", cloud_path(&pts, bump)));
    } else if subtype == b"Square" {
        c.push_str(&format!("{} {} {} {} re\n", n(x0), n(y0), n(x1 - x0), n(y1 - y0)));
    } else {
        c.push_str(&ellipse(x0, y0, x1, y1));
    }
    c.push_str(match (fill.is_some(), stroke.is_some() && w > 0.0) {
        (true, true) => "B\n",
        (true, false) => "f\n",
        (false, true) => "S\n",
        (false, false) => "n\n",
    });
    Some(())
}

fn draw_line(context: &AppearanceContext<'_>, drawing: &mut AppearanceDrawing) -> Option<()> {
    let d = context.dict;
    let stroke = context.stroke;
    let w = context.width;
    let c = &mut drawing.content;
    let res = &mut drawing.resources;
    let tail = &mut drawing.tail;
    let l = nums(d, b"L").filter(|l| l.len() == 4 && l.iter().all(|x| x.is_finite()))?;
    let col = stroke?;
    let ends: Vec<Vec<u8>> = match d.get(b"LE") {
        None => vec![b"None".to_vec(), b"None".to_vec()],
        Some(o) => o.as_array()?.iter().map(|e| e.as_name().map(<[u8]>::to_vec)).collect::<Option<_>>()?,
    };
    if ends.len() != 2 || ends.iter().any(|e| !known_ending(e)) {
        return None;
    }
    let fill = interior_color(d, Some(col)).unwrap_or(col);
    let layout = line_layout(d, [(l[0], l[1]), (l[2], l[3])], w);
    let [p, q] = layout.line;
    c.push_str(&format!("{}{}{} w 1 J 1 j\n{}", rg_stroke(col), rg(fill), n(w), dash(d)));
    for [s, e] in layout.leaders.iter().chain(&layout.strokes).chain(&layout.tick) {
        c.push_str(&format!("{} {} m {} {} l S\n", n(s.0), n(s.1), n(e.0), n(e.1)));
    }
    c.push_str("[] 0 d\n");
    for (end, (tip, from)) in ends.iter().zip([(p, q), (q, p)]) {
        if !line_end(c, end, tip, from, w) {
            return None;
        }
    }
    if let Some((text, m)) = layout.caption {
        // The caption is black, in the standard font the measurement label uses.
        tail.extend(
            format!("q 0 g BT /Helv {} Tf {} {} {} {} {} {} Tm ", n(CAPTION_SIZE), n(m[0]), n(m[1]), n(m[2]), n(m[3]), n(m[4]), n(m[5])).bytes(),
        );
        // WinAnsi bytes go into the stream as-is, not through UTF-8.
        tail.extend(literal(&win_ansi(&text)));
        tail.extend_from_slice(b" Tj ET Q\n");
        res.set(b"Font".to_vec(), helvetica_resource());
    }
    Some(())
}

fn draw_polygon(context: &AppearanceContext<'_>, drawing: &mut AppearanceDrawing) -> Option<()> {
    let d = context.dict;
    let subtype = context.subtype;
    let stroke = context.stroke;
    let w = context.width;
    let c = &mut drawing.content;
    let v = nums(d, b"Vertices").filter(|v| v.len() >= 4 && v.len() % 2 == 0 && v.iter().all(|x| x.is_finite()))?;
    let pts: Vec<(f64, f64)> = v.as_chunks::<2>().0.iter().map(|p| (p[0], p[1])).collect();
    let col = stroke;
    let closed = subtype == b"Polygon";
    let fill = if closed { interior_color(d, None) } else { None };
    let ends = if subtype == b"PolyLine" {
        match d.get(b"LE") {
            None => None,
            Some(o) => {
                let names: Vec<Vec<u8>> =
                    o.as_array().filter(|a| a.len() == 2)?.iter().map(|e| e.as_name().map(<[u8]>::to_vec)).collect::<Option<_>>()?;
                if names.iter().any(|e| !known_ending(e)) {
                    return None;
                }
                Some(names)
            }
        }
    } else {
        None
    };
    if let Some(f) = fill {
        c.push_str(&rg(f));
    }
    if let Some(s) = col {
        c.push_str(&format!("{}{} w 1 J 1 j\n{}", rg_stroke(s), n(w), dash(d)));
    }
    if let Some(intensity) = context.cloud {
        c.push_str(&cloud_path(&pts, cloud_bump(w, intensity)));
    } else {
        for (i, p) in pts.iter().enumerate() {
            c.push_str(&format!("{} {} {}\n", n(p.0), n(p.1), if i == 0 { "m" } else { "l" }));
        }
        if closed {
            c.push_str("h\n");
        }
    }
    c.push_str(match (fill.is_some(), col.is_some() && w > 0.0) {
        (true, true) => "B\n",
        (true, false) => "f\n",
        (false, true) => "S\n",
        (false, false) => "n\n",
    });
    if let Some(ends) = ends {
        let col = col?;
        let (Some(start), Some(end)) = (ends.first(), ends.get(1)) else { return None };
        let (Some(&tip0), Some(&from0)) = (pts.first(), pts.get(1)) else { return None };
        let (Some(&tip1), Some(&from1)) = (pts.last(), pts.get(pts.len().saturating_sub(2))) else { return None };
        let fill = interior_color(d, Some(col)).unwrap_or(col);
        c.push_str(&format!("[] 0 d\n{}", rg(fill)));
        if !line_end(c, start, tip0, from0, w) || !line_end(c, end, tip1, from1, w) {
            return None;
        }
    }
    Some(())
}

fn draw_caret(context: &AppearanceContext<'_>, drawing: &mut AppearanceDrawing) -> Option<()> {
    let d = context.dict;
    let rect = context.rect;
    let stroke = context.stroke;
    let c = &mut drawing.content;
    // A filled caret: two curved flanks meeting at the top centre.
    let col = stroke.unwrap_or([0.0, 0.47, 0.84]);
    let (caret, pilcrow) = caret_layout(d, rect);
    let [x0, y0, x1, y1] = caret;
    let (cx, h) = ((x0 + x1) / 2.0, y1 - y0);
    c.push_str(&format!(
        "{}{} {} m {} {} {} {} {} {} c {} {} {} {} {} {} c h f\n",
        rg(col),
        n(x0),
        n(y0),
        n(cx - (x1 - x0) * 0.1),
        n(y0 + h * 0.2),
        n(cx),
        n(y1 - h * 0.25),
        n(cx),
        n(y1),
        n(cx),
        n(y1 - h * 0.25),
        n(cx + (x1 - x0) * 0.1),
        n(y0 + h * 0.2),
        n(x1),
        n(y0)
    ));
    if let Some((x, y, size)) = pilcrow {
        // `/Sy /P`: a new paragraph symbol beside the caret (Table 183), in its colour.
        // 0xB6 is the pilcrow in WinAnsiEncoding.
        drawing.tail.extend(format!("BT /Helv {} Tf {} {} Td (", n(size), n(x), n(y)).bytes());
        drawing.tail.push(0xB6);
        drawing.tail.extend_from_slice(b") Tj ET\n");
        drawing.resources.set(b"Font".to_vec(), helvetica_resource());
    }
    Some(())
}

fn draw_ink(context: &AppearanceContext<'_>, drawing: &mut AppearanceDrawing) -> Option<()> {
    let d = context.dict;
    let stroke = context.stroke;
    let w = context.width;
    let c = &mut drawing.content;
    let col = stroke?;
    let list = d.get(b"InkList")?.as_array()?;
    // `/BS` gives an ink stroke its width and dash pattern (Table 182).
    c.push_str(&format!("{}{} w 1 J 1 j\n{}", rg_stroke(col), n(w), dash(d)));
    for s in list {
        let pts: Vec<f64> = s.as_array()?.iter().map(|o| o.as_f64()).collect::<Option<_>>()?;
        let pts: Vec<(f64, f64)> = pts.as_chunks::<2>().0.iter().map(|p| (p[0], p[1])).collect();
        let Some(first) = pts.first() else { continue };
        c.push_str(&format!("{} {} m\n", n(first.0), n(first.1)));
        match pts.as_slice() {
            [_] => c.push_str(&format!("{} {} l\n", n(first.0 + 0.01), n(first.1))),
            [_, end] => c.push_str(&format!("{} {} l\n", n(end.0), n(end.1))),
            _ => c.push_str(&smooth_curve(&pts)),
        }
        c.push_str("S\n");
    }
    Some(())
}

fn draw_stamp(context: &AppearanceContext<'_>, mut drawing: AppearanceDrawing) -> Option<Stream> {
    let d = context.dict;
    let rect = context.rect;
    let stroke = context.stroke;
    let opacity = context.opacity;
    let c = &mut drawing.content;
    let mut res = std::mem::take(&mut drawing.resources);
    // A typed signature: filled outlines normalised to the rectangle.
    if let Some(outline) = d.get(b"PCOutline").and_then(|o| o.as_array()) {
        let [x0, y0, x1, y1] = rect;
        let (w, h) = (x1 - x0, y1 - y0);
        c.push_str(&rg(stroke.unwrap_or([0.0; 3])));
        for contour in outline {
            let v: Vec<f64> = contour.as_array().map(|a| a.iter().filter_map(Object::as_f64).collect()).unwrap_or_default();
            for (i, p) in v.as_chunks::<2>().0.iter().enumerate() {
                c.push_str(&format!("{} {} {}\n", n(x0 + p[0] * w), n(y0 + p[1] * h), if i == 0 { "m" } else { "l" }));
            }
            if v.len() >= 6 {
                c.push_str("h\n");
            }
        }
        c.push_str("f\n");
        return Some(form(rect, c.as_bytes(), res));
    }
    // A custom stamp: its picture (an image, or a form mapped to /PCPictureSize) fills
    // the rectangle. An image signature added to a turned page (`/PCPictureRotate`) gets
    // the standard counter-rotation as its form `/Matrix`: viewers fit the turned
    // bounding box to `/Rect` (ISO 32000-2 §12.5.5), so it reads upright as displayed.
    if let Some(pic) = d.get(b"PCPicture").and_then(Object::as_ref) {
        let image = matches!(d.get(b"PCPictureImage"), Some(Object::Bool(true)));
        let size = nums(d, b"PCPictureSize").and_then(|s| match s.as_slice() {
            [w, h] if *w > 0.0 && *h > 0.0 => Some((*w, *h)),
            _ => None,
        });
        let place = picture_place(rect, image, size)?;
        c.push_str(&format!("q {place} /Pic Do Q\n"));
        let mut xo = Dict::new();
        xo.set(b"Pic".to_vec(), Object::Ref(pic));
        res.set(b"XObject".to_vec(), Object::Dict(xo));
        let mut stream = form(rect, c.as_bytes(), res);
        let turn = picture_turn(d);
        if image && turn != 0 {
            // The page's view matrix without its offset: display axes in user space.
            let m = pdfcraft_model::view_matrix_for(turn, [0.0; 4]);
            let matrix = [m[0], m[1], m[2], m[3], 0.0, 0.0];
            stream.dict.set(b"Matrix".to_vec(), Object::Array(matrix.iter().map(|x| Object::Real(*x)).collect()));
        }
        return Some(stream);
    }
    // Only PdfCraft's own Fill & Sign marks are drawn here.
    let name = d.name(b"Name")?;
    let col = stroke.unwrap_or([0.0; 3]);
    // A mark or stamp placed on a turned page is drawn upright in its displayed box, then
    // mapped into `/Rect` the way the page maps display space into user space. Viewers
    // fit that box to `/Rect`; the page rotation then shows it the right way up.
    let turn = stamp_turn(d);
    let draw = if turn == 0 {
        rect
    } else {
        let (rw, rh) = (rect[2] - rect[0], rect[3] - rect[1]);
        let (dw, dh) = if matches!(turn, 90 | 270) { (rh, rw) } else { (rw, rh) };
        if dw < 1.0 || dh < 1.0 {
            return None;
        }
        [0.0, 0.0, dw, dh]
    };
    let turned = |mut stream: Stream| {
        if turn != 0 {
            let m = pdfcraft_model::view_matrix_for(turn, rect);
            stream.dict.set(b"Matrix".to_vec(), Object::Array(m.iter().copied().map(Object::Real).collect()));
        }
        stream
    };
    let [x0, y0, x1, y1] = draw;
    let (w, h) = (x1 - x0, y1 - y0);
    let lw = w.min(h) * 0.12;
    // The strokes come from `FillMark::strokes`, which a copy's outline draws too.
    let path = |mark: crate::FillMark| {
        let ops: Vec<String> = mark
            .strokes(draw)
            .iter()
            .flat_map(|s| s.iter().enumerate().map(|(i, p)| format!("{} {} {}", n(p[0]), n(p[1]), if i == 0 { "m" } else { "l" })))
            .collect();
        format!("{} S\n", ops.join(" "))
    };
    match name {
        b"PCCheck" => c.push_str(&format!("{}{} w 1 J 1 j\n{}", rg_stroke(col), n(lw), path(crate::FillMark::Check))),
        b"PCCross" => c.push_str(&format!("{}{} w 1 J\n{}", rg_stroke(col), n(lw), path(crate::FillMark::Cross))),
        b"PCDot" => {
            let r = w.min(h) * 0.3;
            c.push_str(&rg(col));
            c.push_str(&ellipse(x0 + w / 2.0 - r, y0 + h / 2.0 - r, x0 + w / 2.0 + r, y0 + h / 2.0 + r));
            c.push_str("f\n");
        }
        b"PCLine" => c.push_str(&format!("{}{} w 1 J\n{}", rg_stroke(col), n(h.clamp(0.5, 2.0)), path(crate::FillMark::Line))),
        other => {
            // Only stamps PdfCraft made: others keep their own artwork.
            if !matches!(d.get(b"PCStamp"), Some(Object::Bool(true))) {
                return None;
            }
            let kind = crate::StampKind::from_name(other)?;
            let by = d.get(b"PCByLine").and_then(|o| o.as_string()).map(PdfString::to_text);
            let sign_here = kind.group() == crate::StampGroup::SignHere;
            return Some(turned(stamp(kind.label(), sign_here, draw, stroke.unwrap_or(kind.color()), by.as_deref(), opacity, res)));
        }
    }
    if turn != 0 {
        return Some(turned(form(draw, c.as_bytes(), res)));
    }
    Some(form(rect, c.as_bytes(), res))
}

/// User space into the form frame of [`pdfcraft_model::view_matrix_for`]. Those rotation
/// matrices have determinant 1, so the inverse needs no division.
fn user_to_form([a, b, c, d, e, f]: [f64; 6], p: (f64, f64)) -> (f64, f64) {
    (d * (p.0 - e) - c * (p.1 - f), a * (p.1 - f) - b * (p.0 - e))
}

/// The oval or circle (`shape`) around a text box whose whole box is `full`: its background
/// and border go into `c`; returns the text box inside it.
#[allow(clippy::too_many_arguments)]
fn shaped_border(
    d: &Dict,
    shape: crate::BorderShape,
    full: [f64; 4],
    turn: i64,
    bw: f64,
    bg: Option<Rgb>,
    line_color: Rgb,
    c: &mut String,
) -> Option<[f64; 4]> {
    // Laid out as displayed: a quarter turn swaps the sides' differences.
    let rect = match inset(d) {
        Some(rd) => {
            let (dx, dy) = if matches!(turn, 90 | 270) { (rd[1], rd[0]) } else { (rd[0], rd[1]) };
            [full[0] + dx, full[1] + dy, full[2] - dx, full[3] - dy]
        }
        None => shape.inside(full, bw, full),
    };
    if rect[2] - rect[0] < 1.0 || rect[3] - rect[1] < 1.0 {
        return None;
    }
    let h = bw / 2.0;
    let [x0, y0, x1, y1] = [full[0] + h, full[1] + h, full[2] - h, full[3] - h];
    let path = match shape {
        crate::BorderShape::Circle => {
            let (cx, cy, r) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0, (x1 - x0).min(y1 - y0) / 2.0);
            ellipse(cx - r, cy - r, cx + r, cy + r)
        }
        _ => ellipse(x0, y0, x1, y1),
    };
    if let Some(bg) = bg {
        c.push_str(&format!("{}{path}f\n", rg(bg)));
    }
    if bw > 0.0 {
        c.push_str(&format!("{}{} w\n{}{path}S\n[] 0 d\n", rg_stroke(line_color), n(bw), dash(d)));
    }
    Some(rect)
}

fn draw_free_text(context: &AppearanceContext<'_>, mut drawing: AppearanceDrawing) -> Option<Stream> {
    let d = context.dict;
    let parts = context.parts;
    let rect = context.rect;
    let stroke = context.stroke;
    let mut c = std::mem::take(&mut drawing.content);
    let mut res = std::mem::take(&mut drawing.resources);
    let _tail = std::mem::take(&mut drawing.tail);
    let (text_color, size) = parse_da(d);
    // The box's border and a callout's leader line: their own colour, else the text's.
    let line_color = da_border(d).unwrap_or(text_color);
    let bg = stroke;
    let bw = if d.contains(b"BS") || d.contains(b"Border") { border_width(d) } else { 0.0 };
    // A callout: the leader line (arrowhead at its first point), and the text box inside
    // `/Rect` by `/RD`.
    // Text added to a turned page is drawn upright in its displayed box, which the form
    // `/Matrix` then maps into `/Rect` (as for stamps). A callout's leader and box are
    // stored in user space and mapped into that displayed frame.
    let turn = text_turn(d);
    let user = rect;
    let (rect, matrix) = if turn == 0 {
        (rect, None)
    } else {
        let (rw, rh) = (rect[2] - rect[0], rect[3] - rect[1]);
        let (dw, dh) = if matches!(turn, 90 | 270) { (rh, rw) } else { (rw, rh) };
        if dw < 1.0 || dh < 1.0 {
            return None;
        }
        ([0.0, 0.0, dw, dh], Some(pdfcraft_model::view_matrix_for(turn, rect)))
    };
    let full = rect;
    let mut rect = rect;
    if d.contains(b"CL") {
        let cl = nums(d, b"CL").filter(|l| l.len() == 4 || l.len() == 6)?;
        // `/RD` is optional (Table 177; null is absent, §7.3.9): without it the text box is `/Rect`.
        let rd = match d.get(b"RD") {
            None | Some(Object::Null) => vec![0.0; 4],
            Some(_) => nums(d, b"RD").filter(|r| r.len() == 4 && r.iter().all(|x| *x >= 0.0))?,
        };
        // User space to the form's displayed frame (the inverse of `/Matrix`).
        let to_form = |p: (f64, f64)| match matrix {
            None => p,
            Some(m) => user_to_form(m, p),
        };
        let tb = [user[0] + rd[0], user[1] + rd[1], user[2] - rd[2], user[3] - rd[3]];
        rect = if matrix.is_some() {
            let (p, q) = (to_form((tb[0], tb[1])), to_form((tb[2], tb[3])));
            [p.0.min(q.0), p.1.min(q.1), p.0.max(q.0), p.1.max(q.1)]
        } else {
            [full[0] + rd[0], full[1] + rd[1], full[2] - rd[2], full[3] - rd[3]]
        };
        if rect[2] - rect[0] < 1.0 || rect[3] - rect[1] < 1.0 {
            return None;
        }
        let end = match d.get(b"LE") {
            None => b"None".to_vec(),
            Some(o) => o.as_name()?.to_vec(),
        };
        if !known_ending(&end) {
            return None;
        }
        let lw = bw.max(0.5);
        let pts: Vec<(f64, f64)> = cl.as_chunks::<2>().0.iter().map(|p| (p[0], p[1])).collect();
        if pts.len() < 2 || pts.iter().any(|p| !p.0.is_finite() || !p.1.is_finite()) {
            return None;
        }
        let pts: Vec<(f64, f64)> = pts.into_iter().map(to_form).collect();
        c.push_str(&format!("{}{}{} w 1 J 1 j\n", rg_stroke(line_color), rg(bg.unwrap_or([1.0; 3])), n(lw)));
        for (i, p) in pts.iter().enumerate() {
            c.push_str(&format!("{} {} {}\n", n(p.0), n(p.1), if i == 0 { "m" } else { "l" }));
        }
        c.push_str("S\n");
        if !line_end(&mut c, &end, pts[0], pts[1], lw) {
            return None;
        }
    }
    // An oval or a circle (`/PCBorderShape`) around the text box, which is `/Rect` inset by
    // `/RD` and keeps its place: background and border take the shape, the text is laid out
    // in the text box as in a rectangle.
    let shape = crate::BorderShape::of(d);
    // A cloudy box (not a callout): its background and border follow the cloud around
    // the text, which keeps to the cloud's inner rectangle.
    if let Some(intensity) = context.cloud.filter(|_| !d.contains(b"CL")) {
        let cw = if bw > 0.0 { bw } else { 1.0 };
        let bump = cloud_bump(cw, intensity);
        let inner = cloud_inner(d, rect, bump, cw)?;
        let pts = [(inner[0], inner[1]), (inner[2], inner[1]), (inner[2], inner[3]), (inner[0], inner[3])];
        c.push_str(&format!("{}{}{} w 1 j\n{}", rg_stroke(line_color), rg(bg.unwrap_or([1.0; 3])), n(cw), dash(d)));
        c.push_str(&cloud_path(&pts, bump));
        c.push_str(if bg.is_some() { "B\n[] 0 d\n" } else { "S\n[] 0 d\n" });
        rect = inner;
    } else if context.cloud.is_some() {
        return None;
    } else if shape != crate::BorderShape::Rectangle {
        rect = shaped_border(d, shape, full, turn, bw, bg, line_color, &mut c)?;
    } else {
        if let Some(bg) = bg {
            c.push_str(&format!("{}{} {} {} {} re f\n", rg(bg), n(rect[0]), n(rect[1]), n(rect[2] - rect[0]), n(rect[3] - rect[1])));
        }
        if bw > 0.0 {
            let h = bw / 2.0;
            c.push_str(&format!(
                "{}{} w\n{}{} {} {} {} re S\n[] 0 d\n",
                rg_stroke(line_color),
                n(bw),
                dash(d),
                n(rect[0] + h),
                n(rect[1] + h),
                n(rect[2] - rect[0] - bw),
                n(rect[3] - rect[1] - bw)
            ));
        }
    }
    let text = d.get(b"Contents").and_then(|o| o.as_string()).map(PdfString::to_text).unwrap_or_default();
    let pad = 2.0 + bw;
    let width = (rect[2] - rect[0] - 2.0 * pad).max(1.0);
    // In an oval or a circle the lines are centred in the text box, up and down too.
    let round = shape != crate::BorderShape::Rectangle;
    let q = if round { 1 } else { d.int(b"Q").unwrap_or(0) };
    // The text at its own opacity, where it has one.
    if parts.text != parts.fill {
        c.push_str("/GS1 gs\n");
    }
    c.push_str(&format!(
        "{} {} {} {} re W n\nBT\n/Helv {} Tf\n{}",
        n(rect[0]),
        n(rect[1]),
        n(rect[2] - rect[0]),
        n(rect[3] - rect[1]),
        n(size),
        rg(text_color)
    ));
    let mut out = c.into_bytes();
    let lines = wrap(&text, size, width);
    let mut y = rect[3] - pad - size * reach(&lines)[0];
    if round {
        y -= ((rect[3] - rect[1] - 2.0 * pad - text_lines_height(&lines, size)) / 2.0).max(0.0);
    }
    // Thai is shaped and drawn from the Thai face (Type 3 fonts in the form's resources, whose
    // streams `apply_appearance` stores in the document); the rest, and Thai it can't draw (as
    // `?`, which `undrawable_text` reports), in Helvetica.
    let mut fonts = helvetica_fonts();
    let (pieces, shaped) = shaped_text::appearance_lines(&lines, &mut fonts);
    let mut current = None;
    for pieces in &pieces {
        if y < rect[1] - size {
            break;
        }
        let lw = shaped_text::pieces_width(pieces, size, 0.0, helvetica_width);
        let x = match q {
            1 => rect[0] + pad + (width - lw) / 2.0,
            2 => rect[2] - pad - lw,
            _ => rect[0] + pad,
        };
        shaped.show_line(&mut out, pieces, x, y, size, "Helv", win_ansi, |s| helvetica_width(s, size), &mut current);
        y -= size * 1.2;
    }
    out.extend_from_slice(b"ET\n");
    res.set(b"Font".to_vec(), Object::Dict(fonts));
    let mut stream = form(full, &out, res);
    if let Some(m) = matrix {
        stream.dict.set(b"Matrix".to_vec(), Object::Array(m.iter().copied().map(Object::Real).collect()));
    }
    Some(stream)
}

fn draw_icon(context: &AppearanceContext<'_>, name: &str, fallback: Rgb, mut drawing: AppearanceDrawing) -> Option<Stream> {
    let d = context.dict;
    let col = context.stroke.unwrap_or(fallback);
    let icon = d.name(b"Name").map(|n| String::from_utf8_lossy(n).into_owned()).unwrap_or_else(|| name.into());
    let content = if name == "Note" { note_icon(&icon, col) } else { attach_icon(&icon, col) };
    drawing.content.push_str(&content);
    Some(form([0.0, 0.0, NOTE_SIZE, NOTE_SIZE], drawing.content.as_bytes(), drawing.resources))
}
