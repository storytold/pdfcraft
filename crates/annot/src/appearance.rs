//! Appearance streams (§12.5.5) for the comment types PdfCraft creates, execution plan M5.2.
//!
//! [`build`] draws a normal appearance (`/AP /N`) from the annotation dictionary alone, so the
//! same code serves new comments and restyled ones. Drawings are in page space with
//! `/BBox = /Rect` (identity matrix), except note icons, which are drawn in a 20 × 20 box.
//! It returns `None` for anything it cannot draw faithfully (cloudy borders, unknown line
//! endings, indirect geometry), so callers never replace an appearance with a worse one.
//!
//! The note icons are PdfCraft's own drawings (AGENTS.md §1). Text boxes use the standard
//! Helvetica font with WinAnsi encoding; line breaking uses [`text_width`], an approximation of
//! Helvetica's proportions by character class (no font program or metrics file is bundled).

use pdfcraft_cos::{Dict, Object, PdfString, Stream};
use pdfcraft_fonts::{first_undrawable, literal, win_ansi};
pub use pdfcraft_fonts::{helvetica_width as text_width, wrap};

use crate::{NOTE_SIZE, Rgb, n};

mod symbols;

pub(crate) use symbols::display_stamp;
use symbols::{CAPTION_SIZE, attach_icon, ellipse, line_caption, line_layout, note_icon, stamp};

/// Length of an arrowhead for a line of width `w`.
pub fn arrow_size(w: f64) -> f64 {
    6.0 + 3.0 * w.max(0.0)
}

/// Nominal radius of a cloudy border's bumps for a line of width `w` (intensity 1).
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

/// `c` operators for [`smooth_segments`] (the path already starts at the first point).
fn smooth_curve(pts: &[(f64, f64)]) -> String {
    smooth_segments(pts).iter().map(|[a, b, e]| format!("{} {} {} {} {} {} c\n", n(a.0), n(a.1), n(b.0), n(b.1), n(e.0), n(e.1))).collect()
}

/// Bumps one cloudy outline draws at most (plus one per edge); past it the bumps grow.
pub(crate) const MAX_CLOUD_BUMPS: usize = 20_000;

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

/// `/C`-style colour arrays: none (transparent), gray, RGB or CMYK.
fn color(d: &Dict, key: &[u8]) -> Option<Option<Rgb>> {
    let c = match d.get(key) {
        None => return Some(None),
        Some(o) => o.as_array()?.iter().map(|o| o.as_f64()).collect::<Option<Vec<f64>>>()?,
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

fn ext_gstate(opacity: f64, multiply: bool) -> Option<Dict> {
    if opacity >= 1.0 && !multiply {
        return None;
    }
    let mut gs = Dict::new();
    gs.set(b"Type".to_vec(), Object::name("ExtGState"));
    if multiply {
        gs.set(b"BM".to_vec(), Object::name("Multiply"));
    }
    gs.set(b"CA".to_vec(), Object::Real(opacity));
    gs.set(b"ca".to_vec(), Object::Real(opacity));
    let mut res = Dict::new();
    res.set(b"GS0".to_vec(), Object::Dict(gs));
    Some(res)
}

fn form(bbox: [f64; 4], content: &[u8], resources: Dict) -> Stream {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Form"));
    d.set(b"FormType".to_vec(), Object::Int(1));
    d.set(b"BBox".to_vec(), Object::Array(bbox.iter().map(|x| Object::Real(*x)).collect()));
    d.set(b"Resources".to_vec(), Object::Dict(resources));
    Stream::flate(d, content)
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
fn text_turn(d: &Dict) -> i64 {
    d.get(b"PCTextRotate").and_then(Object::as_int).unwrap_or(0).rem_euclid(360) / 90 * 90
}

/// The first text [`build`] would draw in the standard fonts (Helvetica, WinAnsiEncoding) that
/// they can't, which would come out as `?`: what it is (for the message) and the character. The
/// text of a text box, callout or Fill & Sign text, a line's caption, a measurement's label, a
/// dynamic stamp's "By …" line, and a redaction mark's overlay text (drawn when it is applied). `None` when all of
/// it can be drawn.
pub fn undrawable_text(d: &Dict) -> Option<(&'static str, char)> {
    let text = |key: &[u8]| d.get(key).and_then(Object::as_string).map(PdfString::to_text);
    let (what, text) = match d.name(b"Subtype")? {
        b"FreeText" => ("the comment's text", text(b"Contents")?),
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
fn helvetica_resource() -> Object {
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("Type1"));
    font.set(b"BaseFont".to_vec(), Object::name("Helvetica"));
    font.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
    let mut fonts = Dict::new();
    fonts.set(b"Helv".to_vec(), Object::Dict(font));
    Object::Dict(fonts)
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
    dict: &'a Dict,
    subtype: &'a [u8],
    rect: [f64; 4],
    cloudy: bool,
    opacity: f64,
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
    fn new(context: &AppearanceContext<'_>) -> Self {
        let mut drawing = Self::default();
        if let Some(gs) = ext_gstate(context.opacity, context.subtype == b"Highlight") {
            drawing.resources.set(b"ExtGState".to_vec(), Object::Dict(gs));
            drawing.content.push_str("/GS0 gs\n");
        }
        drawing
    }
}

/// Draw the normal appearance of an annotation, or `None` if this subtype/variant isn't supported.
/// Text the standard fonts can't draw comes out as `?`: [`undrawable_text`] says which, and
/// [`crate::set_appearance`] marks the annotation with it (`pdfcraft_fonts::UNDRAWN_MARK`).
pub fn build(d: &Dict) -> Option<Stream> {
    let context = appearance_context(d)?;
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

fn appearance_context(d: &Dict) -> Option<AppearanceContext<'_>> {
    // Imported measurement appearances may include leaders, captions and formatting that
    // this builder cannot reproduce. Preserve them rather than silently losing detail.
    if d.contains(b"Measure") && !d.contains(b"PCMeasureValue") {
        return None;
    }
    let subtype = d.name(b"Subtype")?;
    let rect = nums(d, b"Rect").filter(|r| r.len() == 4)?;
    let rect = [rect[0].min(rect[2]), rect[1].min(rect[3]), rect[0].max(rect[2]), rect[1].max(rect[3])];
    // Cloudy borders (§12.5.4) are drawn for polygons only.
    let cloudy = d.get(b"BE").and_then(|b| b.as_dict()).is_some_and(|b| b.name(b"S") == Some(b"C"));
    if cloudy && subtype != b"Polygon" {
        return None;
    }
    Some(AppearanceContext {
        dict: d,
        subtype,
        rect,
        cloudy,
        opacity: d.get(b"CA").and_then(|o| o.as_f64()).unwrap_or(1.0).clamp(0.0, 1.0),
        stroke: color(d, b"C")?,
        width: border_width(d),
    })
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

fn draw_markup(context: &AppearanceContext<'_>, drawing: &mut AppearanceDrawing) -> Option<()> {
    let d = context.dict;
    let subtype = context.subtype;
    let stroke = context.stroke;
    let c = &mut drawing.content;
    let q = nums(d, b"QuadPoints").filter(|q| !q.is_empty() && q.len() % 8 == 0)?;
    let col = stroke?;
    for quad in q.as_chunks::<8>().0.iter() {
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
                let lw = (h * 0.07).clamp(0.5, 3.0);
                let t = if subtype == b"Underline" { 0.08 } else { 0.42 };
                let (a, b) = (at(p3, p1, t), at(p4, p2, t));
                c.push_str(&format!("{}{} w\n{} {} m {} {} l S\n", rg_stroke(col), n(lw), n(a.0), n(a.1), n(b.0), n(b.1)));
            }
            _ => {
                // Squiggly: a zigzag along the bottom of the quad.
                let lw = (h * 0.05).clamp(0.5, 2.0);
                let amp = (h * 0.06).max(0.75);
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
    let d = context.dict;
    let stroke = context.stroke;
    let c = &mut drawing.content;
    // While marked: the outline of each area (the fill comes when applied).
    let q = nums(d, b"QuadPoints").filter(|q| !q.is_empty() && q.len() % 8 == 0)?;
    let col = stroke.unwrap_or([0.89, 0.13, 0.13]);
    c.push_str(&format!("{}1 w\n", rg_stroke(col)));
    for quad in q.as_chunks::<8>().0.iter() {
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
    let inset = w / 2.0;
    let [x0, y0, x1, y1] = [rect[0] + inset, rect[1] + inset, rect[2] - inset, rect[3] - inset];
    if let Some(f) = fill {
        c.push_str(&rg(f));
    }
    if let Some(s) = stroke {
        c.push_str(&format!("{}{} w\n{}", rg_stroke(s), n(w), dash(d)));
    }
    if subtype == b"Square" {
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
    let cloudy = context.cloudy;
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
    if cloudy {
        let intensity = d.get(b"BE").and_then(|b| b.as_dict()).and_then(|b| b.get(b"I")).and_then(|i| i.as_f64()).unwrap_or(1.0);
        c.push_str(&cloud_path(&pts, cloud_radius(w) * intensity.clamp(0.5, 2.0)));
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
        let [x0, y0, x1, y1] = rect;
        let (w, h) = (x1 - x0, y1 - y0);
        let image = matches!(d.get(b"PCPictureImage"), Some(Object::Bool(true)));
        let place = if image {
            format!("{} 0 0 {} {} {} cm", n(w), n(h), n(x0), n(y0))
        } else {
            let size = nums(d, b"PCPictureSize").filter(|s| s.len() == 2 && s[0] > 0.0 && s[1] > 0.0)?;
            format!("{} 0 0 {} {} {} cm", n(w / size[0]), n(h / size[1]), n(x0), n(y0))
        };
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
    match name {
        b"PCCheck" => c.push_str(&format!(
            "{}{} w 1 J 1 j\n{} {} m {} {} l {} {} l S\n",
            rg_stroke(col),
            n(lw),
            n(x0 + w * 0.15),
            n(y0 + h * 0.5),
            n(x0 + w * 0.4),
            n(y0 + h * 0.2),
            n(x0 + w * 0.88),
            n(y0 + h * 0.85)
        )),
        b"PCCross" => c.push_str(&format!(
            "{}{} w 1 J\n{} {} m {} {} l {} {} m {} {} l S\n",
            rg_stroke(col),
            n(lw),
            n(x0 + w * 0.18),
            n(y0 + h * 0.18),
            n(x1 - w * 0.18),
            n(y1 - h * 0.18),
            n(x0 + w * 0.18),
            n(y1 - h * 0.18),
            n(x1 - w * 0.18),
            n(y0 + h * 0.18)
        )),
        b"PCDot" => {
            let r = w.min(h) * 0.3;
            c.push_str(&rg(col));
            c.push_str(&ellipse(x0 + w / 2.0 - r, y0 + h / 2.0 - r, x0 + w / 2.0 + r, y0 + h / 2.0 + r));
            c.push_str("f\n");
        }
        b"PCLine" => c.push_str(&format!(
            "{}{} w 1 J\n{} {} m {} {} l S\n",
            rg_stroke(col),
            n(h.clamp(0.5, 2.0)),
            n(x0),
            n(y0 + h / 2.0),
            n(x1),
            n(y0 + h / 2.0)
        )),
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

fn draw_free_text(context: &AppearanceContext<'_>, mut drawing: AppearanceDrawing) -> Option<Stream> {
    let d = context.dict;
    let rect = context.rect;
    let stroke = context.stroke;
    let mut c = std::mem::take(&mut drawing.content);
    let mut res = std::mem::take(&mut drawing.resources);
    let _tail = std::mem::take(&mut drawing.tail);
    let (text_color, size) = parse_da(d);
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
        let rd = nums(d, b"RD").filter(|r| r.len() == 4 && r.iter().all(|x| *x >= 0.0))?;
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
        c.push_str(&format!("{}{}{} w 1 J 1 j\n", rg_stroke(text_color), rg(bg.unwrap_or([1.0; 3])), n(lw)));
        for (i, p) in pts.iter().enumerate() {
            c.push_str(&format!("{} {} {}\n", n(p.0), n(p.1), if i == 0 { "m" } else { "l" }));
        }
        c.push_str("S\n");
        if !line_end(&mut c, &end, pts[0], pts[1], lw) {
            return None;
        }
    }
    if let Some(bg) = bg {
        c.push_str(&format!("{}{} {} {} {} re f\n", rg(bg), n(rect[0]), n(rect[1]), n(rect[2] - rect[0]), n(rect[3] - rect[1])));
    }
    if bw > 0.0 {
        let h = bw / 2.0;
        c.push_str(&format!(
            "{}{} w\n{}{} {} {} {} re S\n[] 0 d\n",
            rg_stroke(text_color),
            n(bw),
            dash(d),
            n(rect[0] + h),
            n(rect[1] + h),
            n(rect[2] - rect[0] - bw),
            n(rect[3] - rect[1] - bw)
        ));
    }
    let text = d.get(b"Contents").and_then(|o| o.as_string()).map(PdfString::to_text).unwrap_or_default();
    let pad = 2.0 + bw;
    let width = (rect[2] - rect[0] - 2.0 * pad).max(1.0);
    let q = d.int(b"Q").unwrap_or(0);
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
    let mut y = rect[3] - pad - size * 0.9;
    for line in wrap(&text, size, width) {
        if y < rect[1] - size {
            break;
        }
        let lw = text_width(&line, size);
        let x = match q {
            1 => rect[0] + pad + (width - lw) / 2.0,
            2 => rect[2] - pad - lw,
            _ => rect[0] + pad,
        };
        out.extend(format!("1 0 0 1 {} {} Tm ", n(x), n(y)).bytes());
        out.extend(literal(&win_ansi(&line)));
        out.extend_from_slice(b" Tj\n");
        y -= size * 1.2;
    }
    out.extend_from_slice(b"ET\n");
    res.set(b"Font".to_vec(), helvetica_resource());
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
