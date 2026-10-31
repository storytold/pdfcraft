use super::{CornerShape, Cornered, Dict, Document, Family, Object, PdfString, Pen, Rect, Stream};

/// Width and height of a JPEG from its first frame header (SOF0–SOF15, not the DNL/JPG/DAC/DHT
/// markers). `None` for anything else.
pub fn jpeg_size(data: &[u8]) -> Option<(u32, u32)> {
    jpeg_frame(data).map(|(w, h, _)| (w, h))
}

fn jpeg_frame(data: &[u8]) -> Option<(u32, u32, u8)> {
    jpeg_info(data).map(|(w, h, c, _)| (w, h, c))
}

/// The SHA-256 of a picture's bytes, to find its XObject again.
pub(super) fn content_hash(data: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(data).into()
}

pub(super) fn hex32(h: &[u8; 32]) -> Vec<u8> {
    h.iter().flat_map(|b| format!("{b:02x}").into_bytes()).collect()
}

pub(super) fn parse_hex32(s: &[u8]) -> Option<[u8; 32]> {
    let mut out = [0u8; 32];
    if s.len() != 64 || !s.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    for (o, pair) in out.iter_mut().zip(s.as_chunks::<2>().0) {
        *o = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(out)
}

/// Width, height, components and whether an Adobe APP14 segment is present (such CMYK JPEGs
/// store inverted values).
pub(super) fn jpeg_info(data: &[u8]) -> Option<(u32, u32, u8, bool)> {
    if data.get(0..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut i = 2usize;
    let mut guard = 0;
    let mut adobe = false;
    while i + 4 <= data.len() && guard < 10_000 {
        guard += 1;
        if *data.get(i)? != 0xFF {
            i += 1;
            continue;
        }
        let marker = *data.get(i + 1)?;
        if marker == 0xFF {
            i += 1;
            continue;
        }
        if (0xD0..=0xD9).contains(&marker) || marker == 0x01 {
            i += 2;
            continue;
        }
        let len = u16::from_be_bytes([*data.get(i + 2)?, *data.get(i + 3)?]) as usize;
        if marker == 0xEE && data.get(i + 4..i + 9) == Some(b"Adobe") {
            adobe = true;
        }
        let sof = matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if sof {
            let h = u16::from_be_bytes([*data.get(i + 5)?, *data.get(i + 6)?]) as u32;
            let w = u16::from_be_bytes([*data.get(i + 7)?, *data.get(i + 8)?]) as u32;
            let comps = *data.get(i + 9)?;
            return (w > 0 && h > 0).then_some((w, h, comps, adobe));
        }
        i = i.checked_add(2)?.checked_add(len.max(2))?;
    }
    None
}

/// Format a number for content streams.
pub(super) fn n(v: f64) -> String {
    let v = if v.is_finite() { v } else { 0.0 };
    let s = format!("{:.3}", if v.abs() < 5e-4 { 0.0 } else { v });
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

pub(super) fn rgb(c: [f64; 3]) -> String {
    format!("{} {} {}", n(c[0]), n(c[1]), n(c[2]))
}

pub(super) fn rgb_array(c: [f64; 3]) -> Object {
    Object::Array(c.iter().map(|v| Object::Real(v.clamp(0.0, 1.0))).collect())
}

pub(super) fn standard_font(base: &str) -> Object {
    let mut f = Dict::new();
    f.set(b"Type".to_vec(), Object::name("Font"));
    f.set(b"Subtype".to_vec(), Object::name("Type1"));
    f.set(b"BaseFont".to_vec(), Object::name(base));
    f.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
    Object::Dict(f)
}

/// Every face the content streams or `/DA` strings may name.
pub(super) fn all_faces() -> Vec<(&'static str, &'static str)> {
    let mut out = Vec::new();
    for fam in [Family::Helvetica, Family::Courier, Family::Times] {
        for (b, i) in [(false, false), (true, false), (false, true), (true, true)] {
            out.push((fam.resource_name(b, i), fam.base_font(b, i)));
        }
    }
    out
}

/// The content that draws box `r` (see [`Cornered`]) on a page `page_h` tall: its fill along its
/// corners, then each shown edge between its corners, then each shown corner with its own line,
/// the lines half their width inside the box.
pub(crate) fn cornered(r: &Cornered, page_h: f64) -> String {
    let pt = |q: (f64, f64)| format!("{} {}", n(q.0), n(page_h - q.1));
    let step = |s: &Step| match s {
        Step::Line(p) => format!("{} l ", pt(*p)),
        Step::Curve(a, b, p) => format!("{} {} {} c ", pt(*a), pt(*b), pt(*p)),
    };
    let stroke = |p: &Pen, path: &str| format!("q {} RG {} w {}{path}S Q\n", rgb(p.color), n(p.width), if p.dashed { "[2 2] 0 d " } else { "" });
    let mut c = String::new();
    if let Some(color) = r.fill {
        // Clockwise from the top left: the top and bottom right corners go from their side edge,
        // the others to it.
        let mut path = String::new();
        for (i, corner) in r.corners.iter().enumerate() {
            let (start, steps) = placed(r.rect, i, corner, 0.0, i % 2 == 1);
            path.push_str(&format!("{} {} ", pt(start), if i == 0 { "m" } else { "l" }));
            path.extend(steps.iter().map(step));
        }
        c.push_str(&format!("{} rg {path}h f\n", rgb(color)));
    }
    let (x0, y0, x1, y1) = (r.rect.x, r.rect.y, r.rect.right(), r.rect.bottom());
    let [tl, tr, br, bl] = r.corners.map(|k| k.radius);
    for (i, edge) in r.edges.iter().enumerate() {
        let Some(p) = edge else { continue };
        let h = p.width / 2.0;
        // Top, right, bottom, left, each between the corners at its ends.
        let (from, to) = match i {
            0 => ((x0 + tl, y0 + h), (x1 - tr, y0 + h)),
            1 => ((x1 - h, y0 + tr), (x1 - h, y1 - br)),
            2 => ((x0 + bl, y1 - h), (x1 - br, y1 - h)),
            _ => ((x0 + h, y0 + tl), (x0 + h, y1 - bl)),
        };
        if to.0 - from.0 + to.1 - from.1 > 0.0 {
            c.push_str(&stroke(p, &format!("{} m {} l ", pt(from), pt(to))));
        }
    }
    for (i, corner) in r.corners.iter().enumerate() {
        let Some(p) = corner.line.filter(|_| corner.radius > 0.0) else { continue };
        let (start, steps) = placed(r.rect, i, corner, p.width / 2.0, false);
        let path: String = steps.iter().map(step).collect();
        c.push_str(&stroke(&p, &format!("{} m {path}", pt(start))));
    }
    c
}

/// One step of a path: a line, or a cubic Bézier curve (its two control points), to a point.
#[derive(Clone, Copy, Debug)]
enum Step {
    Line((f64, f64)),
    Curve((f64, f64), (f64, f64), (f64, f64)),
}

/// The path of corner `c` in its own frame, where the box's corner is (0, 0), `u` runs along the
/// top or bottom edge into the box and `v` along the side edge: from its point on the side edge
/// to its point on the other. A right angle, a square notch `radius` deep, or a quarter circle,
/// around the box or (inverted) centred on its corner. `h` moves a corner around the box that far
/// into it, for a line twice as wide drawn inside, as the edges are; an inverted corner's line is
/// centred on its outline, as another viewer draws it.
fn corner_path(c: &CornerShape, h: f64) -> ((f64, f64), Vec<Step>) {
    // A quarter circle's control points lie this fraction of the radius from its ends.
    const K: f64 = 0.5523;
    let r = c.radius;
    if r <= 0.0 {
        return ((h, h), Vec::new());
    }
    match (c.round, c.inverted) {
        (false, false) => ((h, r), vec![Step::Line((h, h)), Step::Line((r, h))]),
        (false, true) => ((0.0, r), vec![Step::Line((r, r)), Step::Line((r, 0.0))]),
        (true, false) => {
            // Centred `r` in from both edges; a line inside it curves `h` tighter.
            let k = (r - h).max(0.0) * K;
            ((h, r), vec![Step::Curve((h, r - k), (r - k, h), (r, h))])
        }
        (true, true) => ((0.0, r), vec![Step::Curve((r * K, r), (r, r * K), (r, 0.0))]),
    }
}

/// Corner `i` (top-left, top-right, bottom-right, bottom-left) of `rect` as [`corner_path`]
/// draws it, on the page (top-left based): its start and steps, walked backwards when `back`.
fn placed(rect: Rect, i: usize, c: &CornerShape, h: f64, back: bool) -> ((f64, f64), Vec<Step>) {
    let (x0, y0, x1, y1) = (rect.x, rect.y, rect.right(), rect.bottom());
    let ((px, py), (sx, sy)) = match i {
        0 => ((x0, y0), (1.0, 1.0)),
        1 => ((x1, y0), (-1.0, 1.0)),
        2 => ((x1, y1), (-1.0, -1.0)),
        _ => ((x0, y1), (1.0, -1.0)),
    };
    let at = |(u, v): (f64, f64)| (px + sx * u, py + sy * v);
    let (start, steps) = corner_path(c, h);
    let start = at(start);
    let steps: Vec<Step> = steps
        .into_iter()
        .map(|s| match s {
            Step::Line(p) => Step::Line(at(p)),
            Step::Curve(a, b, p) => Step::Curve(at(a), at(b), at(p)),
        })
        .collect();
    if !back {
        return (start, steps);
    }
    // Backwards, each step ends where it began, its control points swapped.
    let mut begins = vec![start];
    begins.extend(steps.iter().map(|s| match s {
        Step::Line(p) | Step::Curve(_, _, p) => *p,
    }));
    let end = begins.pop().unwrap_or(start);
    let back = steps
        .iter()
        .rev()
        .zip(begins.iter().rev())
        .map(|(s, b)| match s {
            Step::Line(_) => Step::Line(*b),
            Step::Curve(c1, c2, _) => Step::Curve(*c2, *c1, *b),
        })
        .collect();
    (end, back)
}

/// A PDF appearance-state name for a check button's on value.
pub(super) fn state_name(on: &str) -> String {
    let s: String = on.trim().chars().map(|c| if c.is_whitespace() || "()<>[]{}/%#".contains(c) || c.is_control() { '_' } else { c }).collect();
    if s.is_empty() || s == "Off" { "Yes".into() } else { s }
}

pub(super) fn empty_form(doc: &mut Document, w: f64, h: f64) -> Object {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Form"));
    d.set(b"BBox".to_vec(), Object::Array([0.0, 0.0, w, h].iter().map(|v| Object::Real(*v)).collect()));
    Object::Ref(doc.add(Object::Stream(Stream::from_raw(d, Vec::new()))))
}

pub(super) fn js_action(js: &str) -> Object {
    let mut a = Dict::new();
    a.set(b"S".to_vec(), Object::name("JavaScript"));
    a.set(b"JS".to_vec(), Object::String(PdfString::literal(js.as_bytes().to_vec())));
    Object::Dict(a)
}
