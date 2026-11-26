use super::{Dict, Document, Family, Hatch, Object, PdfString, Rect, Rounded, Stream};

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

/// The side of a hatch pattern's square cell, in points: each direction of line is drawn once
/// per cell, so this is the distance between neighbouring lines.
pub(super) const HATCH_CELL: f64 = 9.0;
/// The width of a hatch pattern's lines, in points.
pub(super) const HATCH_WIDTH: f64 = 0.0;

/// The content of one [`HATCH_CELL`]-square cell of a tiling pattern drawing `hatch` in
/// `color`. Lines run past the cell (the pattern's box clips them) and a diagonal is drawn in
/// the neighbouring cells' places too, so the tiles join into unbroken lines.
pub(super) fn hatch_cell(hatch: Hatch, color: [f64; 3]) -> String {
    let s = HATCH_CELL;
    let line = |a: (f64, f64), b: (f64, f64)| format!("{} {} m {} {} l ", n(a.0), n(a.1), n(b.0), n(b.1));
    let level = || line((-1.0, s / 2.0), (s + 1.0, s / 2.0));
    let upright = || line((s / 2.0, -1.0), (s / 2.0, s + 1.0));
    // `/` rises left to right (PDF y is up); `\` falls.
    let rising = || [-s, 0.0, s].map(|k| line((k - 1.0, -1.0), (k + s + 1.0, s + 1.0))).concat();
    let falling = || [-s, 0.0, s].map(|k| line((k - 1.0, s + 1.0), (k + s + 1.0, -1.0))).concat();
    let path = match hatch {
        Hatch::CrossHatch => level() + &upright(),
        Hatch::CrossDiagonal => rising() + &falling(),
        Hatch::DiagonalLeft => falling(),
        Hatch::DiagonalRight => rising(),
        Hatch::Horizontal => level(),
        Hatch::Vertical => upright(),
    };
    format!("{} RG {} w 0 J {path}S\n", rgb(color), n(HATCH_WIDTH))
}

/// The content that fills `path` (ending in a space) with the tiling pattern the page's
/// resources name `name`, in a graphics state of its own.
pub(super) fn hatched(name: &str, path: &str) -> String {
    format!("q /Pattern cs /{name} scn {path}f Q\n")
}

/// The content that draws rounded box `r` on a page `page_h` tall: its fill along its curves,
/// its pattern's lines over the fill inside the same curves (the tiling pattern the page names
/// `pattern`), then its edge inside it. A side a page break cut has no edge.
pub(crate) fn rounded(r: &Rounded, page_h: f64, pattern: Option<&str>) -> String {
    let mut c = String::new();
    if let Some(color) = r.fill {
        let path = outline(r.rect, r.radii, r.inverted, (false, false), page_h);
        c.push_str(&format!("{} rg {path}f\n", rgb(color)));
        if let Some(name) = pattern.filter(|_| r.pattern.is_some()) {
            c.push_str(&hatched(name, &path));
        }
    }
    if let Some((w, color, dashed)) = r.edge {
        // The edge's centre line, half its width inside the box, so the edge stays in: a corner
        // curving around the box tightens by as much, and one curving into it widens.
        let inset = Rect::new(r.rect.x + w / 2.0, r.rect.y + w / 2.0, (r.rect.w - w).max(0.0), (r.rect.h - w).max(0.0));
        let most = inset.w.min(inset.h) / 2.0;
        let radii: [f64; 4] = std::array::from_fn(|i| match (r.radii[i], r.inverted[i]) {
            (v, _) if v <= 0.0 => 0.0,
            (v, true) => (v + w / 2.0).min(most),
            (v, false) => (v - w / 2.0).clamp(0.0, most),
        });
        let path = outline(inset, radii, r.inverted, (r.open_top, r.open_bottom), page_h);
        c.push_str(&format!("q {} RG {} w {}{path}S Q\n", rgb(color), n(w), if dashed { "[2 2] 0 d " } else { "" }));
    }
    c
}

/// The path around `rect` (top-left based, on a page `page_h` tall) with the corners `radii`
/// (top-left, top-right, bottom-right, bottom-left), each curving around the box or, when
/// `inverted`, into it. `open` leaves out the top or bottom side (and doesn't close the path):
/// those are where a page break cut the box. Ends with a space, for the paint operator.
fn outline(rect: Rect, radii: [f64; 4], inverted: [bool; 4], open: (bool, bool), page_h: f64) -> String {
    let (x0, y0, x1, y1) = (rect.x, rect.y, rect.right(), rect.bottom());
    let pt = |q: (f64, f64)| format!("{} {}", n(q.0), n(page_h - q.1));
    // Corner `i`: its point, then where its curve starts and ends, going clockwise.
    let corner = |i: usize| {
        let r = radii.get(i).copied().unwrap_or(0.0);
        match i {
            0 => ((x0, y0), (x0, y0 + r), (x0 + r, y0)),
            1 => ((x1, y0), (x1 - r, y0), (x1, y0 + r)),
            2 => ((x1, y1), (x1, y1 - r), (x1 - r, y1)),
            _ => ((x0, y1), (x0 + r, y1), (x0, y1 - r)),
        }
    };
    // Corner `i`'s curve, from its start: a quarter circle as one Bézier curve.
    let curve = |i: usize| {
        if radii.get(i).copied().unwrap_or(0.0) <= 0.0 {
            return String::new();
        }
        let (p, a, b) = corner(i);
        const K: f64 = 0.5523;
        let (c1, c2) = if inverted.get(i).copied().unwrap_or(false) {
            // Centred on the corner itself.
            ((a.0 + (b.0 - p.0) * K, a.1 + (b.1 - p.1) * K), (b.0 + (a.0 - p.0) * K, b.1 + (a.1 - p.1) * K))
        } else {
            ((a.0 + (p.0 - a.0) * K, a.1 + (p.1 - a.1) * K), (b.0 + (p.0 - b.0) * K, b.1 + (p.1 - b.1) * K))
        };
        format!("{} {} {} c ", pt(c1), pt(c2), pt(b))
    };
    // Along corners `from`, joining them with straight sides.
    let along = |from: &[usize]| {
        let mut s = String::new();
        for &i in from {
            s.push_str(&format!("{} l {}", pt(corner(i).1), curve(i)));
        }
        s
    };
    match open {
        (false, false) => format!("{} m {}{}{}{}h ", pt(corner(0).1), curve(0), along(&[1]), along(&[2]), along(&[3])),
        (true, false) => format!("{} m {}{} l ", pt((x1, y0)), along(&[2, 3]), pt((x0, y0))),
        (false, true) => format!("{} m {}{} l ", pt((x0, y1)), along(&[0, 1]), pt((x1, y1))),
        (true, true) => format!("{} m {} l {} m {} l ", pt((x0, y0)), pt((x0, y1)), pt((x1, y0)), pt((x1, y1))),
    }
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

/// `s` as a JavaScript string literal, quotes included: a quote, a backslash or a line break in
/// it stays text instead of ending the string and running as code.
pub(super) fn js_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            c if c.is_control() || c == '\u{2028}' || c == '\u{2029}' => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub(super) fn js_action(js: &str) -> Object {
    let mut a = Dict::new();
    a.set(b"S".to_vec(), Object::name("JavaScript"));
    a.set(b"JS".to_vec(), Object::String(PdfString::literal(js.as_bytes().to_vec())));
    Object::Dict(a)
}

#[cfg(test)]
mod js_string_tests {
    use super::js_string;

    /// A date pattern that tries to close the string and run code stays one string literal.
    #[test]
    fn a_pattern_cannot_leave_its_string() {
        assert_eq!(js_string("yyyy-mm-dd"), "\"yyyy-mm-dd\"");
        assert_eq!(js_string("x\");app.alert(1);//"), "\"x\\\");app.alert(1);//\"");
        assert_eq!(js_string("a\\\nb\u{2028}"), "\"a\\\\\\u000ab\\u2028\"");
    }
}
