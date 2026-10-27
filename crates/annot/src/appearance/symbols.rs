use super::*;

/// The label and colour of a standard rubber stamp name (ISO 32000-2 Table 184), for a stamp
/// another program wrote without an appearance. Names are matched byte for byte, as PDF names
/// are compared (§7.3.5). Eight are stamps of PdfCraft's palette; the other six take a label of
/// their own and the palette's colour for their meaning.
fn standard_stamp(name: &[u8]) -> Option<(&'static str, Rgb)> {
    use crate::StampKind::*;
    let palette = [Approved, NotApproved, NotForPublicRelease, Confidential, Final, ForComment, Draft, ForPublicRelease];
    if let Some(kind) = palette.into_iter().find(|k| k.name().as_bytes() == name) {
        return Some((kind.label(), kind.color()));
    }
    let (label, like) = match name {
        b"Experimental" => ("EXPERIMENTAL", Draft),
        b"AsIs" => ("AS IS", Draft),
        b"Expired" => ("EXPIRED", NotApproved),
        b"Sold" => ("SOLD", Approved),
        b"Departmental" => ("DEPARTMENTAL", Draft),
        b"TopSecret" => ("TOP SECRET", Confidential),
        _ => return None,
    };
    Some((label, like.color()))
}

/// The neutral grey of a stamp whose name has no drawing.
const PLACEHOLDER_GREY: Rgb = [0.45, 0.45, 0.45];

/// The display appearance of a rubber stamp another program wrote without one: the stamp its
/// `/Name` names (Draft when there is none, Table 184's default) in its `/C` colour or the
/// stamp's own, or for any other name a thin grey frame with both diagonals, so the stamp stays
/// visible and can be selected. The frame is grey whatever the `/C`: it stands in for a stamp
/// that cannot be drawn, so it takes none of that stamp's colour. The name is never drawn as text.
///
/// For display only ([`crate::with_missing_appearances`]): [`build`] declines these stamps, so
/// edits (restyling, flattening) never write this drawing into the file. `None` for any other
/// annotation, one of PdfCraft's own stamps, a dictionary with an `/AP` (a stamp's own artwork is
/// never replaced; the display copy passes the dictionary without an `/AP` that has no normal
/// appearance), or a `/Rect` that isn't finite or is under a point wide or high.
pub(crate) fn display_stamp(d: &Dict) -> Option<Stream> {
    let own = matches!(d.get(b"PCStamp"), Some(Object::Bool(true))) || d.contains(b"PCPicture") || d.contains(b"PCOutline");
    if d.name(b"Subtype") != Some(b"Stamp") || own || d.contains(b"AP") {
        return None;
    }
    let rect = nums(d, b"Rect")?;
    let [a, b, c, e] = rect.as_slice() else { return None };
    let rect = [a.min(*c), b.min(*e), a.max(*c), b.max(*e)];
    if !rect.iter().all(|v| v.is_finite()) || rect[2] - rect[0] < 1.0 || rect[3] - rect[1] < 1.0 {
        return None;
    }
    let opacity = d.get(b"CA").and_then(|o| o.as_f64()).unwrap_or(1.0).clamp(0.0, 1.0);
    // A malformed `/C` falls back to the stamp's own colour rather than hiding the stamp.
    let stroke = color(d, b"C").flatten();
    let mut res = Dict::new();
    if let Some(gs) = ext_gstate(opacity, false) {
        res.set(b"ExtGState".to_vec(), Object::Dict(gs));
    }
    // A stamp whose intent (`/IT`, PDF 2.0) is an image or a snapshot has no `/Name` to default.
    let stamp_intent = d.name(b"IT").is_none_or(|it| it == b"Stamp");
    let name = d.name(b"Name").or(stamp_intent.then_some(b"Draft".as_slice()));
    if let Some((label, col)) = name.and_then(standard_stamp) {
        return Some(stamp(label, false, rect, stroke.unwrap_or(col), None, opacity, res));
    }
    let [x0, y0, x1, y1] = rect;
    let (a0, b0, a1, b1) = (x0 + 0.5, y0 + 0.5, x1 - 0.5, y1 - 0.5);
    let mut c = String::new();
    if opacity < 1.0 {
        c.push_str("/GS0 gs\n");
    }
    c.push_str(&format!(
        "{}1 w\n{} {} {} {} re\n{} {} m {} {} l\n{} {} m {} {} l\nS\n",
        // Grey whatever the `/C`: the placeholder is not the stamp, so it takes none of its colour.
        rg_stroke(PLACEHOLDER_GREY),
        n(a0),
        n(b0),
        n(a1 - a0),
        n(b1 - b0),
        n(a0),
        n(b0),
        n(a1),
        n(b1),
        n(a0),
        n(b1),
        n(a1),
        n(b0)
    ));
    Some(form(rect, c.as_bytes(), res))
}

/// Size of a line's caption, as of a measurement's label.
pub(super) const CAPTION_SIZE: f64 = 10.0;

/// Characters of a line's caption drawn at most, as for a measurement's label.
const MAX_CAPTION_CHARS: usize = 256;

/// Leader lengths and caption offsets (Table 178) are held within this many points (200 inches,
/// the largest page side ISO 32000-2 Annex C allows), so an absurd value can't overflow.
const MAX_LINE_EXTENT: f64 = 14_400.0;

/// A Table 178 length or offset: finite and within [`MAX_LINE_EXTENT`], 0 when absent or not one.
fn line_length(v: Option<f64>) -> f64 {
    v.filter(|v| v.is_finite()).map_or(0.0, |v| v.clamp(-MAX_LINE_EXTENT, MAX_LINE_EXTENT))
}

/// The caption a Line with `/Cap true` shows (ISO 32000-2 Table 178): its `/Contents`, else the
/// plain text of its `/RC`, on one line and cut to [`MAX_CAPTION_CHARS`]. `None` without a
/// caption, and for PdfCraft's own measurements, whose label (`/PCMeasureValue`) is the caption.
pub(super) fn line_caption(d: &Dict) -> Option<String> {
    if !matches!(d.get(b"Cap"), Some(Object::Bool(true))) || d.contains(b"PCMeasureValue") {
        return None;
    }
    let text = |key: &[u8]| d.get(key).and_then(Object::as_string).map(PdfString::to_text);
    let raw = text(b"Contents").filter(|t| !t.trim().is_empty()).or_else(|| text(b"RC").map(|rc| rich_plain(&rc)))?;
    let words: Vec<&str> = raw.split_whitespace().collect();
    let caption: String = words.join(" ").chars().filter(|c| !c.is_control()).take(MAX_CAPTION_CHARS).collect();
    (!caption.is_empty()).then_some(caption)
}

/// The text of rich text (§12.7.4.3, XHTML): its markup dropped, the five XML entities and
/// numeric character references read, block ends read as spaces.
fn rich_plain(rc: &str) -> String {
    let mut out = String::new();
    let mut tag: Option<String> = None;
    // Rich text past this is not caption material; it bounds the work.
    for ch in rc.chars().take(MAX_CAPTION_CHARS * 64) {
        match (&mut tag, ch) {
            (None, '<') => tag = Some(String::new()),
            (Some(name), '>') => {
                let name =
                    name.trim_start_matches('/').split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or_default().to_ascii_lowercase();
                if matches!(name.as_str(), "p" | "br" | "div" | "li") {
                    out.push(' ');
                }
                tag = None;
            }
            (Some(name), c) => {
                if name.len() < 16 {
                    name.push(c);
                }
            }
            (None, c) => out.push(c),
        }
    }
    let mut text = String::new();
    let mut rest = out.as_str();
    while let Some(at) = rest.find('&') {
        text.push_str(rest.get(..at).unwrap_or_default());
        let after = rest.get(at + 1..).unwrap_or_default();
        let entity = after.find(';').filter(|&end| end <= 10).and_then(|end| Some((after.get(..end)?, end)));
        let decoded = entity.and_then(|(name, end)| {
            let c = match name {
                "amp" => '&',
                "lt" => '<',
                "gt" => '>',
                "quot" => '"',
                "apos" => '\'',
                _ => {
                    let num = name.strip_prefix('#')?;
                    let code = match num.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                        None => num.parse().ok()?,
                    };
                    char::from_u32(code)?
                }
            };
            Some((c, end))
        });
        match decoded {
            Some((c, end)) => {
                text.push(c);
                rest = after.get(end + 1..).unwrap_or_default();
            }
            None => {
                text.push('&');
                rest = after;
            }
        }
    }
    text.push_str(rest);
    text
}

/// Where a Line annotation's parts go (ISO 32000-2 §12.5.6.7, Table 178).
#[derive(Debug, Default)]
pub(super) struct LineLayout {
    /// The line itself, start to end: `/L`, or `/L` moved out by its leader lines.
    pub(super) line: [(f64, f64); 2],
    /// Strokes along the line: the whole line, or the two pieces either side of an inline caption.
    pub(super) strokes: Vec<[(f64, f64); 2]>,
    /// The leader lines, from `/L` (past the `/LLO` gap) to the line and on by `/LLE`.
    pub(super) leaders: Vec<[(f64, f64); 2]>,
    /// A short stroke from the line to a caption `/CO` moved off it.
    pub(super) tick: Option<[(f64, f64); 2]>,
    /// The caption and its text matrix.
    pub(super) caption: Option<(String, [f64; 6])>,
}

/// Lay out a Line annotation from `/L` = `[a b]`, line width `w`, and its leader and caption
/// entries.
///
/// Leader lines: a positive `/LL` puts the line on the left of travel from `a` to `b` (above `/L`
/// for a line drawn left to right), so the leader lines, followed from the line to `/L`, turn
/// clockwise from the direction of travel; a negative one puts it on the right. The line sits
/// `|LL| + LLO` from `/L`. `/LLE` and `/LLO` are non-negative: a negative one counts as 0.
///
/// Caption (`/Cap true`): centred on the line's midpoint and turned with it so it reads left to
/// right (bottom to top for a vertical line). `/CP /Inline` (the default) centres it on the line,
/// which is broken around it; `/Top` sets it on top of the unbroken line. `/CO [h v]` moves it `h`
/// along the line and `v` across it (positive: up in reading direction); a caption moved off an
/// inline line gets a tick from the line to it.
///
/// A zero-length or overflowing `/L` has no direction: it gets neither leader lines nor caption.
/// Every point is finite; if anything derived from the entries is not, they are left out.
pub(super) fn line_layout(d: &Dict, [a, b]: [(f64, f64); 2], w: f64) -> LineLayout {
    let plain = LineLayout { line: [a, b], strokes: vec![[a, b]], ..LineLayout::default() };
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = dx.hypot(dy);
    if !len.is_finite() || len <= 0.0 {
        return plain;
    }
    let (ux, uy) = (dx / len, dy / len);
    let mut out = LineLayout { line: [a, b], ..LineLayout::default() };
    let ll = line_length(d.get(b"LL").and_then(Object::as_f64));
    if ll != 0.0 {
        // The unit normal from `/L` towards the line: left of travel for a positive /LL.
        let s = ll.signum();
        let (nx, ny) = (-uy * s, ux * s);
        let llo = line_length(d.get(b"LLO").and_then(Object::as_f64)).max(0.0);
        let lle = line_length(d.get(b"LLE").and_then(Object::as_f64)).max(0.0);
        let at = |p: (f64, f64), k: f64| (p.0 + nx * k, p.1 + ny * k);
        let off = ll.abs() + llo;
        out.line = [at(a, off), at(b, off)];
        out.leaders = vec![[at(a, llo), at(a, off + lle)], [at(b, llo), at(b, off + lle)]];
    }
    let [p, _] = out.line;
    let along = |t: f64| (p.0 + ux * t, p.1 + uy * t);
    let mut gap = None;
    if let Some(text) = line_caption(d) {
        let size = CAPTION_SIZE;
        let width = text_width(&text, size);
        // Reading direction, and "up" across it.
        let (tx, ty) = if ux < 0.0 || (ux == 0.0 && uy < 0.0) { (-ux, -uy) } else { (ux, uy) };
        let (vx, vy) = (-ty, tx);
        let co = nums(d, b"CO").filter(|c| c.len() == 2).unwrap_or_default();
        let h = line_length(co.first().copied());
        let v = line_length(co.get(1).copied());
        let top = d.name(b"CP") == Some(b"Top");
        // The caption's centre, as a distance along the line from `p`.
        let centre = len / 2.0 + h * (tx * ux + ty * uy);
        let mid = along(centre);
        // Baseline across the line: on top of it, or with the text centred on it.
        let base = if top { w.max(0.0) / 2.0 + 2.0 + v } else { v - size * 0.35 };
        let origin = (mid.0 - tx * width / 2.0 + vx * base, mid.1 - ty * width / 2.0 + vy * base);
        out.caption = Some((text, [tx, ty, -ty, tx, origin.0, origin.1]));
        if !top && v.abs() < size / 2.0 {
            let half = width / 2.0 + 2.0;
            gap = Some(((centre - half).clamp(0.0, len), (centre + half).clamp(0.0, len)));
        } else if !top {
            let reach = v - v.signum() * size * 0.4;
            out.tick = Some([mid, (mid.0 + vx * reach, mid.1 + vy * reach)]);
        }
    }
    out.strokes = match gap {
        Some((g0, g1)) => [(0.0, g0), (g1, len)].into_iter().filter(|(s, e)| e > s).map(|(s, e)| [along(s), along(e)]).collect(),
        None => vec![out.line],
    };
    let finite = |q: &(f64, f64)| q.0.is_finite() && q.1.is_finite();
    let all_finite =
        out.line.iter().chain(out.strokes.iter().flatten()).chain(out.leaders.iter().flatten()).chain(out.tick.iter().flatten()).all(finite)
            && out.caption.as_ref().is_none_or(|(_, m)| m.iter().all(|x| x.is_finite()));
    if all_finite { out } else { plain }
}

/// A rubber stamp: a rounded frame (a pointed tag for sign-here stamps) with the label in bold
/// capitals, and the dynamic stamps' "By … at …" line.
pub(super) fn stamp(label: &str, sign_here: bool, rect: [f64; 4], col: Rgb, by: Option<&str>, opacity: f64, mut res: Dict) -> Stream {
    let [x0, y0, x1, y1] = rect;
    let h = y1 - y0;
    let mut c = String::new();
    if opacity < 1.0 {
        c.push_str("/GS0 gs\n");
    }
    let lw = (h * 0.07).clamp(1.0, 3.0);
    let inset = lw / 2.0 + 0.5;
    if sign_here {
        // A tag pointing left, filled, with white text.
        let tip = h * 0.45;
        c.push_str(&format!(
            "{}{}{} w 1 j\n{} {} m {} {} l {} {} l {} {} l {} {} l h B\n",
            rg(col),
            rg_stroke(col),
            n(lw),
            n(x0 + inset),
            n(y0 + h / 2.0),
            n(x0 + tip),
            n(y1 - inset),
            n(x1 - inset),
            n(y1 - inset),
            n(x1 - inset),
            n(y0 + inset),
            n(x0 + tip),
            n(y0 + inset)
        ));
    } else {
        // A rounded frame with a light tint.
        let r = (h * 0.2).min(8.0);
        let k = 0.552_284_75 * r;
        let (a0, b0, a1, b1) = (x0 + inset, y0 + inset, x1 - inset, y1 - inset);
        let tint = col.map(|v| 1.0 - (1.0 - v) * 0.12);
        c.push_str(&format!(
            "{}{}{} w\n{} {} m {} {} l {} {} {} {} {} {} c {} {} l {} {} {} {} {} {} c {} {} l {} {} {} {} {} {} c {} {} l {} {} {} {} {} {} c h B\n",
            rg(tint),
            rg_stroke(col),
            n(lw),
            n(a0 + r),
            n(b0),
            n(a1 - r),
            n(b0),
            n(a1 - r + k),
            n(b0),
            n(a1),
            n(b0 + r - k),
            n(a1),
            n(b0 + r),
            n(a1),
            n(b1 - r),
            n(a1),
            n(b1 - r + k),
            n(a1 - r + k),
            n(b1),
            n(a1 - r),
            n(b1),
            n(a0 + r),
            n(b1),
            n(a0 + r - k),
            n(b1),
            n(a0),
            n(b1 - r + k),
            n(a0),
            n(b1 - r),
            n(a0),
            n(b0 + r),
            n(a0),
            n(b0 + r - k),
            n(a0 + r - k),
            n(b0),
            n(a0 + r),
            n(b0)
        ));
    }
    let (title_h, by_h) = if by.is_some() { (h * 0.5, h * 0.22) } else { (h * 0.52, 0.0) };
    let text_area = if sign_here { (x0 + h * 0.45, x1 - lw * 2.0) } else { (x0 + lw * 2.0, x1 - lw * 2.0) };
    let mut size = title_h;
    let tw = text_width(label, size) * 1.12;
    if tw > text_area.1 - text_area.0 - 4.0 {
        size *= (text_area.1 - text_area.0 - 4.0) / tw;
    }
    let tw = text_width(label, size) * 1.12;
    let text_col = if sign_here { [1.0, 1.0, 1.0] } else { col };
    let ty = if by.is_some() { y0 + h * 0.48 } else { y0 + (h - size * 0.72) / 2.0 };
    let mut out = c.into_bytes();
    out.extend(
        format!("BT\n{}/HelvB {} Tf\n1 0 0 1 {} {} Tm ", rg(text_col), n(size), n(text_area.0 + (text_area.1 - text_area.0 - tw) / 2.0), n(ty))
            .bytes(),
    );
    out.extend(literal(&win_ansi(label)));
    out.extend_from_slice(b" Tj\n");
    if let Some(b) = by {
        let mut s = by_h;
        let bw = text_width(b, s);
        if bw > text_area.1 - text_area.0 - 4.0 {
            s *= (text_area.1 - text_area.0 - 4.0) / bw;
        }
        let bw = text_width(b, s);
        out.extend(
            format!("/Helv {} Tf\n1 0 0 1 {} {} Tm ", n(s), n(text_area.0 + (text_area.1 - text_area.0 - bw) / 2.0), n(y0 + h * 0.18)).bytes(),
        );
        out.extend(literal(&win_ansi(b)));
        out.extend_from_slice(b" Tj\n");
    }
    out.extend_from_slice(b"ET\n");
    let font = |base: &str| {
        let mut f = Dict::new();
        f.set(b"Type".to_vec(), Object::name("Font"));
        f.set(b"Subtype".to_vec(), Object::name("Type1"));
        f.set(b"BaseFont".to_vec(), Object::name(base));
        f.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
        Object::Dict(f)
    };
    let mut fonts = Dict::new();
    fonts.set(b"HelvB".to_vec(), font("Helvetica-Bold"));
    fonts.set(b"Helv".to_vec(), font("Helvetica"));
    res.set(b"Font".to_vec(), Object::Dict(fonts));
    form(rect, &out, res)
}

/// An ellipse inscribed in a rectangle, as four Bézier arcs.
pub(super) fn ellipse(x0: f64, y0: f64, x1: f64, y1: f64) -> String {
    let k = 0.552_284_75;
    let (cx, cy, rx, ry) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0, (x1 - x0) / 2.0, (y1 - y0) / 2.0);
    let (ox, oy) = (rx * k, ry * k);
    let mut s = format!("{} {} m\n", n(cx + rx), n(cy));
    for [a, b, c, d, e, f] in [
        [cx + rx, cy + oy, cx + ox, cy + ry, cx, cy + ry],
        [cx - ox, cy + ry, cx - rx, cy + oy, cx - rx, cy],
        [cx - rx, cy - oy, cx - ox, cy - ry, cx, cy - ry],
        [cx + ox, cy - ry, cx + rx, cy - oy, cx + rx, cy],
    ] {
        s.push_str(&format!("{} {} {} {} {} {} c\n", n(a), n(b), n(c), n(d), n(e), n(f)));
    }
    s.push_str("h\n");
    s
}

/// PdfCraft's note icons, drawn in a 20 × 20 box: a speech bubble for `/Comment`, a page
/// with a folded corner for everything else, both filled with the note colour.
/// File attachment icons in a 20 × 20 box: PdfCraft's own drawings.
pub(super) fn attach_icon(name: &str, col: Rgb) -> String {
    let mut s = format!("{}{}1.2 w 1 j 1 J\n", rg(col), rg_stroke(col));
    match name {
        "Paperclip" => s.push_str("8 4 m 8 15 l 8 18 13 18 13 15 c 13 6 l 13 3 10 3 10 6 c 10 14 l S\n"),
        "Graph" => s.push_str("3 3 m 3 17 l 3 3 m 17 3 l S\n5 3 3 6 re 9 3 3 10 re 13 3 3 13 re f\n"),
        "Tag" => s.push_str("3 10 m 8 15 l 17 15 l 17 5 l 8 5 l h S\n7.3 10 m 7.3 10.7 6.7 11.3 6 11.3 c 5.3 11.3 4.7 10.7 4.7 10 c 4.7 9.3 5.3 8.7 6 8.7 c 6.7 8.7 7.3 9.3 7.3 10 c f\n"),
        _ => {
            // Push pin: a round head on a short needle.
            s.push_str("10 2 m 10 9 l S\n6 9 m 14 9 l 14 11 l 12 12 l 12 16 l 13.5 17 l 13.5 18.5 l 6.5 18.5 l 6.5 17 l 8 16 l 8 12 l 6 11 l h f\n");
        }
    }
    s
}

/// A closed circle path (four Bézier quarters) centred on (`x`, `y`).
fn circle_path(x: f64, y: f64, r: f64) -> String {
    let k = r * 0.5523;
    let p = |v: f64| n(v);
    format!(
        "{} {} m {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c h\n",
        p(x + r),
        p(y),
        p(x + r),
        p(y + k),
        p(x + k),
        p(y + r),
        p(x),
        p(y + r),
        p(x - k),
        p(y + r),
        p(x - r),
        p(y + k),
        p(x - r),
        p(y),
        p(x - r),
        p(y - k),
        p(x - k),
        p(y - r),
        p(x),
        p(y - r),
        p(x + k),
        p(y - r),
        p(x + r),
        p(y - k),
        p(x + r),
        p(y)
    )
}

/// A note's icon in a 20 × 20 box: filled in `col`, outlined in dark grey. Each name of
/// [`crate::NoteIcon`] has its own shape, so the icon a note names is the one it shows (ISO
/// 32000-2 §12.5.6.4, Table 175); a name PdfCraft doesn't know draws as `Note`.
pub(super) fn note_icon(name: &str, col: Rgb) -> String {
    let mut s = format!("{}0.25 0.25 0.25 RG 0.8 w 1 j 1 J\n", rg(col));
    match name {
        "Comment" => {
            s.push_str("3 18.5 m 17 18.5 l 18.5 18.5 18.5 17 18.5 17 c 18.5 8 l 18.5 6.5 17 6.5 17 6.5 c 9.5 6.5 l 5 2 l 5.5 6.5 l 3 6.5 l 1.5 6.5 1.5 8 1.5 8 c 1.5 17 l 1.5 18.5 3 18.5 3 18.5 c h B\n");
            s.push_str("4.5 15 m 15.5 15 l 4.5 12.5 m 15.5 12.5 l 4.5 10 m 11.5 10 l S\n");
        }
        // A key lying flat: a round bow with a hole, a shaft and two bits.
        "Key" => {
            s.push_str(&circle_path(5.5, 10.0, 4.5));
            s.push_str(&circle_path(5.0, 10.0, 1.5));
            s.push_str(
                "9.5 11.2 m 18.5 11.2 l 18.5 8.8 l 17 8.8 l 17 6 l 15 6 l 15 8.8 l 13.5 8.8 l 13.5 6.8 l 11.5 6.8 l 11.5 8.8 l 9.5 8.8 l h B*\n",
            );
        }
        // A disc with a question mark.
        "Help" => {
            s.push_str(&circle_path(10.0, 10.0, 8.5));
            s.push_str("B\n1.6 w 7 12.8 m 7 15.6 13 15.6 13 12.8 c 13 10.6 10 10.8 10 8.2 c S\n10 5 m 10 5.1 l S\n");
        }
        // An insertion caret.
        "Insert" => s.push_str("1.5 3 m 10 18 l 18.5 3 l 14.5 3 l 10 11 l 5.5 3 l h B\n"),
        // A caret over a bar: a new paragraph goes here.
        "NewParagraph" => s.push_str("3.5 9 m 10 18.5 l 16.5 9 l h B\n3 2 14 4 re B\n"),
        // A pilcrow: a filled bowl and two stems.
        "Paragraph" => {
            s.push_str("11 18.5 m 7 18.5 4 16.3 4 13.2 c 4 10.1 7 8 11 8 c h B\n");
            s.push_str("11 18.5 m 16 18.5 l 16 1.5 l 14 1.5 l 14 16.5 l 13 16.5 l 13 1.5 l 11 1.5 l h B\n");
        }
        // A tick.
        "Check" => s.push_str("1.5 10.5 m 4.5 13.5 l 8 9.5 l 16 18.5 l 18.5 15.5 l 8 3.5 l h B\n"),
        // A ring.
        "Circle" => {
            s.push_str(&circle_path(10.0, 10.0, 8.5));
            s.push_str(&circle_path(10.0, 10.0, 5.0));
            s.push_str("B*\n");
        }
        // A thick X.
        "Cross" => s.push_str("4 1.5 m 10 7.5 l 16 1.5 l 18.5 4 l 12.5 10 l 18.5 16 l 16 18.5 l 10 12.5 l 4 18.5 l 1.5 16 l 7.5 10 l 1.5 4 l h B\n"),
        // A ring with sighting lines through it.
        "CrossHairs" => {
            s.push_str(&circle_path(10.0, 10.0, 7.0));
            s.push_str(&circle_path(10.0, 10.0, 4.5));
            s.push_str("B*\n1.2 w 10 0.5 m 10 19.5 l 0.5 10 m 19.5 10 l S\n");
        }
        // A five-pointed star.
        "Star" => {
            let points: Vec<String> = (0..10)
                .map(|i| {
                    let r = if i % 2 == 0 { 9.5 } else { 3.9 };
                    let a = std::f64::consts::FRAC_PI_2 + f64::from(i) * std::f64::consts::PI / 5.0;
                    format!("{} {}", n(10.0 + r * a.cos()), n(9.5 + r * a.sin()))
                })
                .collect();
            s.push_str(&format!("{} m {} l h B\n", points[0], points[1..].join(" l ")));
        }
        // Block arrows.
        "RightArrow" => s.push_str("1.5 7 m 11 7 l 11 2.5 l 18.5 10 l 11 17.5 l 11 13 l 1.5 13 l h B\n"),
        "UpArrow" => s.push_str("7 1.5 m 7 11 l 2.5 11 l 10 18.5 l 17.5 11 l 13 11 l 13 1.5 l h B\n"),
        _ => {
            s.push_str("3 19 m 13 19 l 17 15 l 17 1 l 3 1 l h B\n13 19 m 13 15 l 17 15 l S\n");
            s.push_str("5.5 12 m 14.5 12 l 5.5 9 m 14.5 9 l 5.5 6 m 11.5 6 l S\n");
        }
    }
    s
}
