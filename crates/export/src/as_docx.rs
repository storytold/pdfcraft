use super::Zip;
use super::as_html::esc;
use super::layout::{Block, Field, FieldKind, Image, Item, Page, Shape, Table, body_size, items};

/// A colour's 8-bit sRGB components (NaN and out-of-range values clamp; NaN reads as 0).
pub(crate) fn rgb8(c: [f64; 3]) -> [u8; 3] {
    // The value is clamped to 0..=255 before the cast, so it cannot truncate.
    c.map(|v| if v.is_finite() { (v.clamp(0.0, 1.0) * 255.0).round() as u8 } else { 0 })
}

/// The colour as `RRGGBB`, or `None` for black (the default every format already uses).
pub(crate) fn hex(c: [f64; 3]) -> Option<String> {
    let [r, g, b] = rgb8(c);
    (r | g | b != 0).then(|| format!("{r:02X}{g:02X}{b:02X}"))
}

/// The font family Word knows for a PDF font's base name: without its subset tag (`ABCDEF+`),
/// style suffix (`-BoldMT`, `,Italic`) and PostScript tail (`MT`, `PSMT`), with a space before
/// each capital that starts a word of a run-together name (`TimesNewRoman` → `Times New
/// Roman`), and the standard 14 fonts by the families Word ships for them. `None` when nothing is
/// left; at most 31 characters, Word's limit.
pub(crate) fn word_font(base: &str) -> Option<String> {
    let bytes = base.as_bytes();
    let tagged = bytes.get(6) == Some(&b'+') && bytes.get(..6).is_some_and(|t| t.iter().all(u8::is_ascii_uppercase));
    let name = if tagged { base.get(7..).unwrap_or(base) } else { base };
    let name = name.split([',', '-']).next().unwrap_or(name).trim();
    let name = ["PSMT", "MT", "PS"].iter().find_map(|tail| name.strip_suffix(tail).filter(|n| !n.is_empty())).unwrap_or(name);
    let family = match name {
        "Helvetica" | "Arial" => "Arial".to_string(),
        "Times" | "TimesNewRoman" | "TimesRoman" => "Times New Roman".to_string(),
        "Courier" | "CourierNew" => "Courier New".to_string(),
        _ => {
            let mut out = String::new();
            let mut prev: Option<char> = None;
            let mut chars = name.chars().filter(|c| !c.is_control()).peekable();
            while let Some(c) = chars.next() {
                // A capital after a lower-case letter starts a word, as does the last capital of
                // an acronym before a lower-case letter (`SegoeUI`, `ITCAvantGarde`).
                let next_lower = chars.peek().is_some_and(|n| n.is_lowercase());
                if c.is_uppercase() && prev.is_some_and(|p| p.is_lowercase() || (p.is_uppercase() && next_lower)) {
                    out.push(' ');
                }
                out.push(c);
                prev = Some(c);
            }
            out
        }
    };
    let family: String = family.chars().take(31).collect();
    let family = family.trim();
    (!family.is_empty()).then(|| family.to_string())
}

/// `w:rFonts` for a PDF font's base name, or nothing when it has no family.
fn fonts_xml(base: &str) -> String {
    word_font(base).map_or_else(String::new, |f| {
        let f = esc(&f);
        format!("<w:rFonts w:ascii=\"{f}\" w:hAnsi=\"{f}\" w:cs=\"{f}\"/>")
    })
}

/// A formatted text run (shared by paragraphs and table cells), set in the family of the PDF
/// font `font`.
fn run_xml(text: &str, font: &str, size: f64, bold: bool, italic: bool, color: [f64; 3]) -> String {
    let mut rpr = fonts_xml(font);
    if bold {
        rpr.push_str("<w:b/>");
    }
    if italic {
        rpr.push_str("<w:i/>");
    }
    if let Some(h) = hex(color) {
        rpr.push_str(&format!("<w:color w:val=\"{h}\"/>"));
    }
    let half_points = if size.is_finite() { (size * 2.0).round().clamp(2.0, 3276.0) as i64 } else { 24 };
    rpr.push_str(&format!("<w:sz w:val=\"{half_points}\"/>"));
    format!("<w:r><w:rPr>{rpr}</w:rPr><w:t xml:space=\"preserve\">{}</w:t></w:r>", esc(text))
}

/// One Word table: explicit absent borders (alignment alone cannot prove PDF rules), a grid sized
/// from the detected column edges, `gridSpan` for spanning cells, empty cells for holes.
pub(crate) fn docx_table(t: &Table) -> String {
    let mut edges = t.cols.clone();
    edges.push(t.rect[2]);
    let widths: Vec<i64> = edges.windows(2).map(|w| ((w[1] - w[0]).max(20.0) * 20.0).round().clamp(60.0, 31680.0) as i64).collect();
    let mut s = String::from("<w:tbl><w:tblPr><w:tblW w:w=\"0\" w:type=\"auto\"/><w:tblBorders>");
    for edge in ["top", "left", "bottom", "right", "insideH", "insideV"] {
        s.push_str(&format!("<w:{edge} w:val=\"nil\"/>"));
    }
    s.push_str("</w:tblBorders></w:tblPr><w:tblGrid>");
    for w in &widths {
        s.push_str(&format!("<w:gridCol w:w=\"{w}\"/>"));
    }
    s.push_str("</w:tblGrid>");
    for row in &t.rows {
        s.push_str("<w:tr>");
        let mut column = 0usize;
        for c in row {
            let span = if c.span > 1 { format!("<w:gridSpan w:val=\"{}\"/>", c.span) } else { String::new() };
            let w: i64 = widths.iter().skip(column).take(c.span).sum();
            s.push_str(&format!(
                "<w:tc><w:tcPr><w:tcW w:w=\"{w}\" w:type=\"dxa\"/>{span}</w:tcPr><w:p>{}</w:p></w:tc>",
                run_xml(&c.text, &c.font, c.size, c.bold, c.italic, c.color)
            ));
            column = column.saturating_add(c.span);
        }
        // Pad the grid so every row covers all columns (Word rejects short rows).
        for _ in column..t.cols.len() {
            s.push_str("<w:tc><w:tcPr><w:tcW w:w=\"0\" w:type=\"auto\"/></w:tcPr><w:p/></w:tc>");
        }
        s.push_str("</w:tr>");
    }
    s.push_str("</w:tbl>");
    s
}

/// Pages with rotated text, or text that actually overlaps an image or an oversized
/// (watermark-like) text block, cannot be put in paragraph flow without changing their layout.
/// Nor can a form: its fields, boxes and comb cells only mean something where they stand.
/// Keep those pages as editable, absolutely positioned text boxes. Every other page, including
/// one with a logo or a big title that sits clear of the text, flows as paragraphs and tables.
fn positioned(page: &Page) -> bool {
    if page.blocks.iter().any(|b| b.rotation.abs() > 0.1) || !page.fields.is_empty() || framed(page) {
        return true;
    }
    let body = body_size(std::slice::from_ref(page)).max(1.0);
    let overlays = page
        .images
        .iter()
        .map(|im| (im.rect, None))
        .chain(page.blocks.iter().enumerate().filter(|(_, b)| b.size >= 2.0 * body).map(|(i, b)| (b.rect, Some(i))));
    // Bound the pairwise test on hostile pages (many overlays times many blocks): past the
    // budget, keep the default flowing layout.
    let mut budget: usize = 1_000_000;
    for (r, own) in overlays {
        for (i, b) in page.blocks.iter().enumerate() {
            if budget == 0 {
                return false;
            }
            budget -= 1;
            if Some(i) != own && overlaps(r, b.rect) {
                return true;
            }
        }
    }
    false
}

/// Shapes that frame text before a page counts as a ruled form or a bordered table.
pub(crate) const FRAMING_SHAPES: usize = 6;

/// Whether the page's text sits in boxes or between upright rules: at least [`FRAMING_SHAPES`]
/// painted outlines (or thin bars) taller than a hairline that reach within 6 pt of a text
/// block. A horizontal rule alone (an underline, a header line) does not frame anything, so a
/// page of running text with a few of them still flows.
fn framed(page: &Page) -> bool {
    let mut found = 0usize;
    // Bounded like `positioned`: past the budget the page keeps flowing.
    let mut budget: usize = 1_000_000;
    for s in &page.shapes {
        let Some(b) = shape_bounds(s) else { continue };
        let (w, h) = (b[2] - b[0], b[3] - b[1]);
        let outline = s.stroke.is_some() || (s.fill.is_some() && w.min(h) <= 3.0);
        if !outline || h < 4.0 {
            continue;
        }
        let near = [b[0] - 6.0, b[1] - 6.0, b[2] + 6.0, b[3] + 6.0];
        for blk in &page.blocks {
            if budget == 0 {
                return false;
            }
            budget -= 1;
            if overlaps(near, blk.rect) {
                found = found.saturating_add(1);
                break;
            }
        }
        if found >= FRAMING_SHAPES {
            return true;
        }
    }
    false
}

/// Whether two [x0, y0, x1, y1] boxes share more than a hairline (0.5 pt each way); boxes that
/// only touch, or hostile non-finite coordinates, do not count.
fn overlaps(a: [f64; 4], b: [f64; 4]) -> bool {
    let w = a[2].min(b[2]) - a[0].max(b[0]);
    let h = a[3].min(b[3]) - a[1].max(b[1]);
    w.is_finite() && h.is_finite() && w > 0.5 && h > 0.5
}

/// Bound every value written into a shape's style, including hostile PDF coordinates.
fn layout_number(n: f64) -> f64 {
    if n.is_finite() { n.clamp(-1584.0, 1584.0) } else { 0.0 }
}

/// Most images a Word export embeds, and most image bytes (the zip writer has 32-bit sizes and
/// a 16-bit entry count; a hostile PDF must not be able to overflow them).
pub(crate) const MAX_MEDIA: usize = 1000;

const MAX_MEDIA_BYTES: usize = 512 << 20;

/// Whether `im` can still be embedded: it has data, and the package stays within its limits.
fn room_for(media: &[(String, &Image)], im: &Image) -> bool {
    !im.bytes.is_empty()
        && media.len() < MAX_MEDIA
        && media.iter().map(|(_, m)| m.bytes.len()).fold(im.bytes.len(), usize::saturating_add) <= MAX_MEDIA_BYTES
}

/// Points as EMU (12700 per point); non-finite values are 0.
fn emu(pt: f64) -> i64 {
    if pt.is_finite() { (pt * 12700.0).round() as i64 } else { 0 }
}

/// The `a:graphic` of one picture (DrawingML), rotated by `rotation` degrees when given.
fn picture_xml(n: usize, name: &str, (cx, cy): (i64, i64), rotation: Option<f64>) -> String {
    let rot = rotation.map_or(String::new(), |r| format!(" rot=\"{}\"", (r * 60000.0).round() as i64));
    format!(
        "<wp:docPr id=\"{n}\" name=\"Picture {n}\"/><wp:cNvGraphicFramePr/>\
<a:graphic xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
<pic:pic xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\"><pic:nvPicPr><pic:cNvPr id=\"{n}\" name=\"{name}\"/><pic:cNvPicPr/></pic:nvPicPr>\
<pic:blipFill><a:blip r:embed=\"rIdImg{n}\"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>\
<pic:spPr><a:xfrm{rot}><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr></pic:pic>\
</a:graphicData></a:graphic>"
    )
}

/// A colour as Word's `#RRGGBB` (black included, unlike [`hex`]).
fn rgb_attr(c: [f64; 3]) -> String {
    let [r, g, b] = rgb8(c);
    format!("#{r:02X}{g:02X}{b:02X}")
}

/// The box of a shape's points, or `None` when it has none (or only hostile ones).
fn shape_bounds(s: &Shape) -> Option<[f64; 4]> {
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for p in s.paths.iter().flat_map(|(points, _)| points) {
        b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
    }
    b.iter().all(|v| v.is_finite()).then_some(b)
}

/// The corners of an axis-aligned rectangle a closed subpath traces (four points, or five with
/// the first repeated), as its box.
fn axis_rect(points: &[[f64; 2]]) -> Option<[f64; 4]> {
    let corners = match points {
        [a, b, c, d] => [*a, *b, *c, *d],
        [a, b, c, d, e] if (a[0] - e[0]).abs() < 0.01 && (a[1] - e[1]).abs() < 0.01 => [*a, *b, *c, *d],
        _ => return None,
    };
    let same = |u: f64, v: f64| (u - v).abs() < 0.01;
    let sides = [(corners[0], corners[1]), (corners[1], corners[2]), (corners[2], corners[3]), (corners[3], corners[0])];
    if !sides.iter().all(|(p, q)| same(p[0], q[0]) || same(p[1], q[1])) {
        return None;
    }
    let xs = corners.map(|p| p[0]);
    let ys = corners.map(|p| p[1]);
    Some([
        xs.iter().copied().fold(f64::INFINITY, f64::min),
        ys.iter().copied().fold(f64::INFINITY, f64::min),
        xs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        ys.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    ])
}

/// Most shapes one Word page draws (a VML shape each); a page with more keeps the first ones.
pub(crate) const MAX_PAGE_SHAPES: usize = 5000;

/// A subpath as drawn: an open path that ends where it started is closed (a box drawn as four
/// `l` segments back to its corner), and a filled one is closed as filling implies.
fn normalized(points: &[[f64; 2]], closed: bool, filled: bool) -> (Vec<[f64; 2]>, bool) {
    let same = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() < 0.01 && (a[1] - b[1]).abs() < 0.01;
    let mut points = points.to_vec();
    let mut closed = closed || (filled && points.len() > 2);
    if points.len() > 3 && points.first().zip(points.last()).is_some_and(|(a, b)| same(*a, *b)) {
        points.pop();
        closed = true;
    }
    (points, closed)
}

/// One painted path as VML shapes behind the text: a line for a single straight segment, a
/// rectangle for an axis-aligned box, otherwise a polyline per subpath (holes in a fill are not
/// kept). `None` for a shape without points or paint.
fn shape_xml(s: &Shape, page_height: f64, id: &mut usize, z: i64) -> Option<String> {
    if s.stroke.is_none() && s.fill.is_none() {
        return None;
    }
    shape_bounds(s)?;
    let y = |v: f64| layout_number(page_height - v);
    let x = layout_number;
    let mut stroke = String::new();
    let mut inner = String::new();
    match s.stroke {
        Some(st) => {
            // Word draws nothing for a 0 pt line; the PDF means the thinnest visible one.
            let w = if st.width.is_finite() { st.width.clamp(0.25, 100.0) } else { 0.25 };
            stroke.push_str(&format!(" stroked=\"t\" strokecolor=\"{}\" strokeweight=\"{w:.2}pt\"", rgb_attr(st.color)));
            if st.dashed {
                inner.push_str("<v:stroke dashstyle=\"dash\"/>");
            }
        }
        None => stroke.push_str(" stroked=\"f\""),
    }
    let paint = |filled: bool| match s.fill.filter(|_| filled) {
        Some(f) => format!("{stroke} filled=\"t\" fillcolor=\"{}\"", rgb_attr(f)),
        None => format!("{stroke} filled=\"f\""),
    };
    let place = "mso-position-horizontal-relative:page;mso-position-vertical-relative:page";
    let wrap = "<w10:wrap type=\"none\"/>";
    let subpaths: Vec<(Vec<[f64; 2]>, bool)> = s
        .paths
        .iter()
        .map(|(p, closed)| normalized(p, *closed, s.fill.is_some()))
        .filter(|(p, _)| p.iter().flatten().all(|v| v.is_finite()) && !p.is_empty())
        .collect();
    let mut out = String::new();
    for (points, closed) in &subpaths {
        *id = id.saturating_add(1);
        let n = *id;
        // A line runs from its top (then left) end down to the other: LibreOffice draws nothing
        // for a VML line whose `to` lies above or left of its `from`. A rising diagonal can't be
        // written that way and is a two-point polyline instead.
        let line = match points.as_slice() {
            [p, q] if !closed => {
                let (a, b) = ([x(p[0]), y(p[1])], [x(q[0]), y(q[1])]);
                let (a, b) = if (a[1], a[0]) <= (b[1], b[0]) { (a, b) } else { (b, a) };
                (a[0] <= b[0]).then_some((a, b))
            }
            _ => None,
        };
        if let Some((a, b)) = line {
            out.push_str(&format!(
                "<w:r><w:pict><v:line id=\"Shape{n}\" style=\"position:absolute;z-index:{z};{place}\" from=\"{:.3}pt,{:.3}pt\" to=\"{:.3}pt,{:.3}pt\"{}>{inner}{wrap}</v:line></w:pict></w:r>",
                a[0],
                a[1],
                b[0],
                b[1],
                paint(false)
            ));
        } else if let Some(r) = axis_rect(points).filter(|_| *closed) {
            out.push_str(&format!(
                "<w:r><w:pict><v:rect id=\"Shape{n}\" style=\"position:absolute;margin-left:{:.3}pt;margin-top:{:.3}pt;width:{:.3}pt;height:{:.3}pt;z-index:{z};{place}\"{}>{inner}{wrap}</v:rect></w:pict></w:r>",
                x(r[0]),
                y(r[3]),
                layout_number(r[2] - r[0]).max(0.01),
                layout_number(r[3] - r[1]).max(0.01),
                paint(true)
            ));
        } else if points.len() > 1 {
            let mut list: Vec<String> = points.iter().map(|p| format!("{:.3}pt,{:.3}pt", x(p[0]), y(p[1]))).collect();
            if *closed && let Some(first) = list.first().cloned() {
                list.push(first);
            }
            out.push_str(&format!(
                "<w:r><w:pict><v:polyline id=\"Shape{n}\" style=\"position:absolute;z-index:{z};{place}\" points=\"{}\"{}>{inner}{wrap}</v:polyline></w:pict></w:r>",
                list.join(","),
                paint(*closed)
            ));
        }
    }
    (!out.is_empty()).then_some(out)
}

/// The run properties of field text in `size` points, spread `spacing` points apart.
fn field_rpr(size: f64, spacing: f64) -> String {
    let half_points = (size * 2.0).round().clamp(2.0, 3276.0) as i64;
    let mut rpr = String::from("<w:rPr><w:rFonts w:ascii=\"Arial\" w:hAnsi=\"Arial\" w:cs=\"Arial\"/>");
    let twips = (spacing * 20.0).round();
    if twips.is_finite() && twips != 0.0 {
        rpr.push_str(&format!("<w:spacing w:val=\"{}\"/>", twips.clamp(-31680.0, 31680.0) as i64));
    }
    rpr.push_str(&format!("<w:sz w:val=\"{half_points}\"/></w:rPr>"));
    rpr
}

/// The form fields a Word document has written: their names (Word's field names, which name
/// their bookmarks, must be unique) and how many there are (any at all protects the document for
/// filling in forms).
#[derive(Default)]
struct Form {
    names: std::collections::HashSet<String>,
    fields: usize,
}

impl Form {
    /// A Word form field name for `name`: letters, digits and `_`, starting with a letter, at
    /// most 20 characters (Word's limit), and not one this document already uses. `id` keeps a
    /// repeated name apart.
    fn name(&mut self, name: &str, id: usize) -> String {
        let mut base: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
        if !base.starts_with(|c: char| c.is_ascii_alphabetic()) {
            base.insert(0, 'F');
        }
        // ASCII only, so any byte length is a character boundary.
        base.truncate(20);
        let mut candidate = base.clone();
        if self.names.contains(&candidate) {
            let suffix = format!("_{id}");
            base.truncate(20usize.saturating_sub(suffix.len()));
            candidate = format!("{base}{suffix}");
        }
        self.names.insert(candidate.clone());
        candidate
    }
}

/// A paragraph framed over `r` (y up) on a page `page_height` tall: Word places the frame
/// exactly there, out of the text flow. `down` points move the frame's top down (and its height
/// with it) so that a single line sits in the middle of the box, rather than a space before the
/// paragraph, which readers treat differently at the top of a frame or page; `extra` adds
/// indentation or alignment.
fn frame_ppr(r: [f64; 4], page_height: f64, down: f64, extra: &str) -> String {
    let twips = |v: f64| (layout_number(v) * 20.0).round().clamp(0.0, 31680.0) as i64;
    let down = if down.is_finite() { down.clamp(0.0, (r[3] - r[1]).max(0.0)) } else { 0.0 };
    format!(
        "<w:pPr><w:framePr w:w=\"{}\" w:h=\"{}\" w:hRule=\"exact\" w:wrap=\"around\" w:vAnchor=\"page\" w:hAnchor=\"page\" w:x=\"{}\" w:y=\"{}\"/><w:spacing w:before=\"0\" w:after=\"0\" w:line=\"240\" w:lineRule=\"auto\"/>{extra}</w:pPr>",
        twips(r[2] - r[0]).max(20),
        twips(r[3] - r[1] - down).max(20),
        twips(r[0]),
        twips(page_height - r[3] + down)
    )
}

/// A form field as a framed paragraph over the field's box, which a document protected for
/// filling in forms lets its reader fill in: a text field is a Word form field, limited to the
/// PDF field's maximum length when it has one (a comb field also spreads its characters over its
/// cells); a check box is a content control that shows an X when checked (the PDF draws the box
/// itself); a list is a drop-down content control.
fn field_xml(f: &Field, page_height: f64, id: usize, form: &mut Form) -> Option<String> {
    let r = f.rect;
    if !r.iter().all(|v| v.is_finite()) {
        return None;
    }
    let (w, h) = (layout_number(r[2] - r[0]).max(1.0), layout_number(r[3] - r[1]).max(1.0));
    // Auto size fits the box's height, within a legible range.
    let size = if f.size.is_finite() && f.size > 0.0 { f.size.clamp(2.0, 72.0) } else { (h * 0.65).clamp(4.0, 12.0) };
    // One line of `size` points sits in the middle of the box.
    let middle = |size: f64| ((h - 1.15 * size) / 2.0).max(0.0);
    let name = esc(&f.name);
    let props = format!("<w:alias w:val=\"{name}\"/><w:tag w:val=\"{name}\"/>");
    // En spaces (half an em each) keep an empty field as wide as its box.
    let blank = |n: usize| "\u{2002}".repeat(n.clamp(1, 200));
    let fits = |advance: f64| if advance > 0.0 { (w / advance).floor() as usize } else { 1 };
    form.fields = form.fields.saturating_add(1);
    let paragraph = match &f.kind {
        FieldKind::Text { value, multiline, comb, max_len } => {
            // A comb cell holds one character, centred: the run is spaced one cell per character.
            let glyph = 0.55 * size;
            let cells = comb.filter(|n| *n > 0);
            // Word adds the spacing after the last character too, which ends the run a cell past
            // the box: the frame gets that cell more, or the last character wraps out of sight.
            let (spacing, left, frame) = match cells {
                Some(n) => {
                    let cell = w / n as f64;
                    (cell - glyph, ((cell - glyph) / 2.0).max(0.0), [r[0], r[1], r[2] + cell, r[3]])
                }
                None => (0.0, 1.0, r),
            };
            // An empty field shows Word's own placeholder of five en spaces, or one per comb cell.
            let text = if value.is_empty() { blank(cells.unwrap_or(5)) } else { value.clone() };
            let limit = max_len.filter(|n| *n > 0).map_or_else(String::new, |n| format!("<w:maxLength w:val=\"{}\"/>", n.min(32767)));
            let field_name = esc(&form.name(&f.name, id));
            let indent = format!("<w:ind w:left=\"{}\"/>", (left * 20.0).round().clamp(0.0, 31680.0) as i64);
            let before = if *multiline { 0.0 } else { middle(size) };
            format!(
                "<w:p>{}<w:r><w:fldChar w:fldCharType=\"begin\"><w:ffData><w:name w:val=\"{field_name}\"/><w:enabled/><w:calcOnExit w:val=\"0\"/><w:statusText w:type=\"text\" w:val=\"{}\"/><w:textInput>{limit}</w:textInput></w:ffData></w:fldChar></w:r><w:r><w:instrText xml:space=\"preserve\"> FORMTEXT </w:instrText></w:r><w:r><w:fldChar w:fldCharType=\"separate\"/></w:r><w:r>{}<w:t xml:space=\"preserve\">{}</w:t></w:r><w:r><w:fldChar w:fldCharType=\"end\"/></w:r></w:p>",
                frame_ppr(frame, page_height, before, &indent),
                esc(&f.name.chars().take(138).collect::<String>()),
                field_rpr(size, spacing),
                esc(&text)
            )
        }
        FieldKind::Check { checked } => {
            let size = (h.min(w) * 0.8).clamp(4.0, 24.0);
            format!(
                "<w:p>{}<w:sdt><w:sdtPr>{props}<w14:checkbox><w14:checked w14:val=\"{}\"/><w14:checkedState w14:val=\"0058\" w14:font=\"Arial\"/><w14:uncheckedState w14:val=\"2002\" w14:font=\"Arial\"/></w14:checkbox></w:sdtPr><w:sdtContent><w:r>{}<w:t xml:space=\"preserve\">{}</w:t></w:r></w:sdtContent></w:sdt></w:p>",
                frame_ppr(r, page_height, middle(size), "<w:jc w:val=\"center\"/>"),
                u8::from(*checked),
                field_rpr(size, 0.0),
                if *checked { "X" } else { "\u{2002}" }
            )
        }
        FieldKind::Choice { value, options } => {
            let mut items = String::new();
            for o in options.iter().take(500) {
                let o = esc(o);
                items.push_str(&format!("<w:listItem w:displayText=\"{o}\" w:value=\"{o}\"/>"));
            }
            let (text, placeholder) = if value.is_empty() { (blank(fits(0.5 * size)), "<w:showingPlcHdr/>") } else { (value.clone(), "") };
            format!(
                "<w:p>{}<w:sdt><w:sdtPr>{props}{placeholder}<w:dropDownList>{items}</w:dropDownList></w:sdtPr><w:sdtContent><w:r>{}<w:t xml:space=\"preserve\">{}</w:t></w:r></w:sdtContent></w:sdt></w:p>",
                frame_ppr(r, page_height, middle(size), "<w:ind w:left=\"20\"/>"),
                field_rpr(size, 0.0),
                esc(&text)
            )
        }
    };
    Some(paragraph)
}

/// A gap between two pieces of a line wider than this many times their size starts a new
/// text box ([`rows`]): about two word spaces.
const ROW_GAP: f64 = 0.6;

/// The lines grouped into rows of pieces that follow each other on one baseline, in order: same
/// (upright) direction, baselines within a fifth of the size, and the next piece starting at
/// most [`ROW_GAP`] times the size after the last (and not far before it).
fn rows(lines: &[Block]) -> Vec<Vec<&Block>> {
    let mut out: Vec<Vec<&Block>> = Vec::new();
    for b in lines {
        let joins = out.last().and_then(|row| row.last()).is_some_and(|prev| {
            let size = prev.size.max(b.size).max(1.0);
            let gap = b.rect[0] - prev.rect[2];
            prev.rotation.abs() <= 0.1
                && b.rotation.abs() <= 0.1
                && (prev.rect[1] - b.rect[1]).abs() < 0.2 * prev.size.min(b.size).max(1.0)
                && gap <= ROW_GAP * size
                && gap >= -0.2 * size
        });
        match out.last_mut() {
            Some(row) if joins => row.push(b),
            _ => out.push(vec![b]),
        }
    }
    out
}

fn positioned_page<'a>(page: &'a Page, media: &mut Vec<(String, &'a Image)>, shape_id: &mut usize, form: &mut Form) -> String {
    // One 1 pt paragraph anchors the whole page. Shapes never reserve body space, so even a
    // page-sized watermark cannot push the invoice footer onto another page.
    let mut s = String::from("<w:p><w:pPr><w:spacing w:before=\"0\" w:after=\"0\" w:line=\"20\" w:lineRule=\"exact\"/></w:pPr>");
    let body = body_size(std::slice::from_ref(page)).max(1.0);
    // Vector art goes behind everything else, in painting order: the first shape is furthest
    // back, the last just behind watermark-sized text (z-index -2) and the text (1).
    let shapes = page.shapes.len().min(MAX_PAGE_SHAPES);
    for (i, shape) in page.shapes.iter().take(MAX_PAGE_SHAPES).enumerate() {
        let z = i64::try_from(i).unwrap_or(i64::MAX).saturating_sub(i64::try_from(shapes).unwrap_or(i64::MAX)).saturating_sub(3);
        if let Some(xml) = shape_xml(shape, page.height, shape_id, z) {
            s.push_str(&xml);
        }
    }
    // Lines are placed in rows: the pieces of one visual line that follow each other within
    // a couple of spaces (a bold word inside a sentence) share a text box, so the reader sets
    // the space between them; a wider gap (a tab stop, a column) starts a box of its own.
    let groups: Vec<Vec<&Block>> = if page.lines.is_empty() { page.blocks.iter().map(|b| vec![b]).collect() } else { rows(&page.lines) };
    for group in &groups {
        let Some(b) = group.first().copied() else { continue };
        *shape_id = shape_id.saturating_add(1);
        // `+ 0.0` turns the -0 of an upright block into 0, so its style reads `rotation:0.000`.
        let rotation = layout_number(-b.rotation).rem_euclid(360.0) + 0.0;
        let (sin, cos) = rotation.to_radians().sin_cos();
        let rect = group.iter().map(|g| g.rect).fold(b.rect, |a, r| [a[0].min(r[0]), a[1].min(r[1]), a[2].max(r[2]), a[3].max(r[3])]);
        let (bw, bh) = (layout_number(rect[2] - rect[0]).max(1.0), layout_number(rect[3] - rect[1]).max(1.0));
        let h = if b.rotation.abs() > 0.1 { layout_number(b.size).max(1.0) } else { bh };
        // Recover the unrotated width from the axis-aligned box. Use the larger axis so
        // quarter turns and 45-degree watermarks do not divide by (nearly) zero.
        let w = if b.rotation.abs() <= 0.1 {
            bw
        } else if cos.abs() >= sin.abs() {
            (bw - h * sin.abs()) / cos.abs()
        } else {
            (bh - h * cos.abs()) / sin.abs()
        };
        let w = layout_number(w).max(1.0);
        let (x, y) = (layout_number(rect[0] + (bw - w) / 2.0), layout_number(page.height - rect[3] + (bh - h) / 2.0));
        let z = if b.size >= 2.0 * body { -2 } else { 1 };
        // One line of text gets room to spare, so that the reader's font (a little wider, or
        // without the PDF's tightened spacing) doesn't wrap it: the box is transparent and its
        // text starts at the left edge, so a wider box changes nothing else. It reaches at most
        // the page's edge. The width is fixed: Word caps a box that sizes itself to its text
        // (`mso-wrap-style:none`) at the width between the page margins, which wrapped the
        // form's longer lines.
        let size = group.iter().map(|g| layout_number(g.size)).fold(1.0, f64::max);
        let one_line = b.rotation.abs() <= 0.1 && bh < 1.6 * size;
        let w = if one_line { layout_number((w * 1.25 + 2.0).min(page.width - x)).max(w) } else { w };
        let style = format!(
            "position:absolute;margin-left:{x:.3}pt;margin-top:{y:.3}pt;width:{w:.3}pt;height:{h:.3}pt;rotation:{rotation:.3};z-index:{z};mso-position-horizontal-relative:page;mso-position-vertical-relative:page"
        );
        let mut runs = String::new();
        for (i, g) in group.iter().enumerate() {
            // The source's own space between two pieces, or a visible gap, is a space.
            let spaced = i > 0
                && group
                    .get(i - 1)
                    .is_some_and(|prev| prev.space_around[1] || g.space_around[0] || g.rect[0] - prev.rect[2] > 0.12 * g.size.max(1.0));
            let text = if spaced { format!(" {}", g.text) } else { g.text.clone() };
            runs.push_str(&run_xml(&text, &g.font, g.size, g.bold, g.italic, g.color));
        }
        s.push_str(&format!(
            "<w:r><w:pict><v:rect id=\"Text{}\" style=\"{style}\" filled=\"f\" stroked=\"f\"><v:textbox inset=\"0,0,0,0\" style=\"mso-fit-shape-to-text:t\"><w:txbxContent><w:p><w:pPr><w:spacing w:before=\"0\" w:after=\"0\"/></w:pPr>{runs}</w:p></w:txbxContent></v:textbox><w10:wrap type=\"none\"/></v:rect></w:pict></w:r>",
            shape_id
        ));
    }
    for im in &page.images {
        if !room_for(media, im) {
            continue;
        }
        *shape_id = shape_id.saturating_add(1);
        let n = media.len().saturating_add(1);
        let (w, h) = (layout_number(im.size[0]).max(1.0), layout_number(im.size[1]).max(1.0));
        let (x, y) = (layout_number((im.rect[0] + im.rect[2] - w) / 2.0), layout_number(page.height - (im.rect[1] + im.rect[3] + h) / 2.0));
        let rotation = layout_number(-im.rotation).rem_euclid(360.0);
        let name = format!("image{n}.{}", im.ext);
        let pic = picture_xml(n, &name, (emu(w), emu(h)), Some(rotation));
        // A DrawingML anchor, the way Word itself stores a picture placed behind text: a VML
        // rectangle with an image does not show the picture.
        s.push_str(&format!(
            "<w:r><w:drawing><wp:anchor distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\" simplePos=\"0\" relativeHeight=\"{}\" behindDoc=\"1\" locked=\"0\" layoutInCell=\"1\" allowOverlap=\"1\">\
<wp:simplePos x=\"0\" y=\"0\"/><wp:positionH relativeFrom=\"page\"><wp:posOffset>{}</wp:posOffset></wp:positionH><wp:positionV relativeFrom=\"page\"><wp:posOffset>{}</wp:posOffset></wp:positionV>\
<wp:extent cx=\"{}\" cy=\"{}\"/><wp:effectExtent l=\"0\" t=\"0\" r=\"0\" b=\"0\"/><wp:wrapNone/>",
            n.saturating_add(1),
            emu(x),
            emu(y),
            emu(w),
            emu(h),
        ));
        s.push_str(&pic);
        s.push_str("</wp:anchor></w:drawing></w:r>");
        media.push((name, im));
    }
    s.push_str("</w:p>");
    // Fields follow as framed paragraphs over their boxes (a form field in a text box can't be
    // filled in once the document is protected), in front of the shapes behind the text.
    for f in page.fields.iter().take(MAX_PAGE_SHAPES) {
        *shape_id = shape_id.saturating_add(1);
        if let Some(xml) = field_xml(f, page.height, *shape_id, form) {
            s.push_str(&xml);
        }
    }
    s
}

fn word_page_size(page: &Page) -> (i64, i64) {
    let twips = |pt: f64| if pt.is_finite() { (pt * 20.0).round().clamp(144.0, 31680.0) as i64 } else { 12240 };
    (twips(page.width), twips(page.height))
}

/// A page's section: its size and margins. A page that keeps its layout places everything
/// itself, so its top margin is a quarter inch (at most an eighth of the page): Word moved a
/// field framed in the top margin down to the margin.
fn section(page: &Page, kept: bool) -> String {
    let (pw, ph) = word_page_size(page);
    let (mx, my) = ((pw / 8).min(1440), (ph / 8).min(1440));
    let top = if kept { (ph / 8).min(360) } else { my };
    format!(
        "<w:sectPr><w:type w:val=\"nextPage\"/><w:pgSz w:w=\"{pw}\" w:h=\"{ph}\"/><w:pgMar w:top=\"{top}\" w:right=\"{mx}\" w:bottom=\"{my}\" w:left=\"{mx}\" w:header=\"{}\" w:footer=\"{}\" w:gutter=\"0\"/></w:sectPr>",
        top / 2,
        my / 2
    )
}

/// The page colour of a Word document: Word has one background for the whole document, so it
/// is written only when every page has the same, non-white background (#2992).
fn word_background(pages: &[Page]) -> Option<String> {
    let first = rgb8(pages.first()?.background?);
    let same = pages.iter().all(|p| p.background.map(rgb8) == Some(first));
    (same && first != [255, 255, 255]).then(|| format!("{:02X}{:02X}{:02X}", first[0], first[1], first[2]))
}

/// A Word document (.docx, Office Open XML). Ordinary pages flow as paragraphs and tables;
/// pages with rotated text, text that overlaps artwork, form fields or text framed by boxes and
/// rules use floating shapes to retain their layout, and so does every page of a document where
/// most pages do.
pub fn docx(pages: &[Page], title: &str) -> Vec<u8> {
    let mut body = String::new();
    let mut media: Vec<(String, &Image)> = Vec::new();
    // Word needs a paragraph between adjacent tables and after the last one in the body.
    let mut after_table = false;
    let mut shape_id = 0usize;
    let mut form = Form::default();
    let layouts: Vec<bool> = pages.iter().map(positioned).collect();
    // A document most of whose pages keep their layout (a form) keeps it on every page: its
    // pages of notes then sit where they did instead of flowing past the page's end.
    let all_kept = layouts.iter().filter(|kept| **kept).count().saturating_mul(2) > pages.len();
    for (page_index, page) in pages.iter().enumerate() {
        let (pw, _) = word_page_size(page);
        let text_w = (pw - 2 * (pw / 8).min(1440)) as f64 / 20.0;
        let kept = all_kept || layouts.get(page_index).copied().unwrap_or(false);
        if kept {
            body.push_str(&positioned_page(page, &mut media, &mut shape_id, &mut form));
            after_table = false;
        } else {
            for it in items(std::slice::from_ref(page)) {
                match &it {
                    Item::Para(runs, lvl) => {
                        let style = match lvl {
                            1 => "<w:pPr><w:pStyle w:val=\"Heading1\"/></w:pPr>",
                            2 => "<w:pPr><w:pStyle w:val=\"Heading2\"/></w:pPr>",
                            _ => "",
                        };
                        body.push_str("<w:p>");
                        body.push_str(style);
                        for r in runs {
                            body.push_str(&run_xml(&r.text, &r.block.font, r.block.size, r.block.bold, r.block.italic, r.block.color));
                        }
                        body.push_str("</w:p>");
                    }
                    Item::Table(t) => {
                        if after_table {
                            body.push_str("<w:p/>");
                        }
                        body.push_str(&docx_table(t));
                    }
                    Item::Img(im) => {
                        if room_for(&media, im) {
                            let n = media.len() + 1;
                            let name = format!("image{n}.{}", im.ext);
                            // Size on the page, at most the text width.
                            let (w, h) = ((im.rect[2] - im.rect[0]).max(1.0), (im.rect[3] - im.rect[1]).max(1.0));
                            let k = (text_w / w).min(1.0);
                            let (cx, cy) = (emu(w * k), emu(h * k));
                            body.push_str(&format!("<w:p><w:r><w:drawing><wp:inline><wp:extent cx=\"{cx}\" cy=\"{cy}\"/>"));
                            body.push_str(&picture_xml(n, &name, (cx, cy), None));
                            body.push_str("</wp:inline></w:drawing></w:r></w:p>");
                            media.push((name, im));
                        }
                    }
                    Item::PageBreak => body.push_str("<w:p><w:r><w:br w:type=\"page\"/></w:r></w:p>"),
                }
                after_table = matches!(it, Item::Table(_));
            }
        }
        // Section breaks carry this source page's size and create exactly one next page.
        // The last section must be a direct child of the body (OOXML 17.6.17).
        if page_index + 1 < pages.len() {
            body.push_str(&format!(
                "<w:p><w:pPr><w:spacing w:before=\"0\" w:after=\"0\" w:line=\"20\" w:lineRule=\"exact\"/>{}</w:pPr></w:p>",
                section(page, kept)
            ));
        } else {
            if after_table || page.blocks.is_empty() && page.images.is_empty() {
                body.push_str("<w:p><w:pPr><w:spacing w:after=\"0\" w:line=\"20\" w:lineRule=\"exact\"/></w:pPr></w:p>");
            }
            body.push_str(&section(page, kept));
        }
        after_table = false;
    }
    // Word shows a document background only when its settings say so (`settings.xml` below).
    let page_colour = word_background(pages);
    // A document with form fields is protected for filling in forms, without a password: only
    // the fields can be changed, and a text field takes no more characters than its PDF field
    // allowed. Review ▸ Restrict Editing turns it off.
    let protect = form.fields > 0;
    let background = page_colour.as_deref().map(|c| format!("<w:background w:color=\"{c}\"/>")).unwrap_or_default();
    let doc = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" xmlns:v=\"urn:schemas-microsoft-com:vml\" xmlns:w10=\"urn:schemas-microsoft-com:office:word\" \
xmlns:w14=\"http://schemas.microsoft.com/office/word/2010/wordml\" xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\" mc:Ignorable=\"w14\">{background}<w:body>{body}\
</w:body></w:document>"
    );
    let types = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/>\
<Default Extension=\"png\" ContentType=\"image/png\"/><Default Extension=\"jpg\" ContentType=\"image/jpeg\"/>\
<Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>\
<Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/>\
<Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.openxmlformats-package.core-properties+xml\"/>\
<Override PartName=\"/word/settings.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml\"/></Types>";
    let rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/>\
<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties\" Target=\"docProps/core.xml\"/></Relationships>";
    let mut doc_rels = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rIdStyles\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>\
<Relationship Id=\"rIdSettings\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings\" Target=\"settings.xml\"/>",
    );
    for (i, (name, _)) in media.iter().enumerate() {
        doc_rels.push_str(&format!(
            "<Relationship Id=\"rIdImg{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"media/{name}\"/>",
            i + 1
        ));
    }
    doc_rels.push_str("</Relationships>");
    let styles = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
<w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/><w:pPr><w:spacing w:after=\"120\"/></w:pPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading1\"><w:name w:val=\"heading 1\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:pPr><w:keepNext/><w:spacing w:before=\"240\"/><w:outlineLvl w:val=\"0\"/></w:pPr><w:rPr><w:b/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading2\"><w:name w:val=\"heading 2\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:pPr><w:keepNext/><w:spacing w:before=\"200\"/><w:outlineLvl w:val=\"1\"/></w:pPr><w:rPr><w:b/></w:rPr></w:style>\
</w:styles>";
    let core = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><dc:title>{}</dc:title></cp:coreProperties>",
        esc(title)
    );
    let mut z = Zip::default();
    z.add("[Content_Types].xml", types.as_bytes(), true);
    z.add("_rels/.rels", rels.as_bytes(), true);
    z.add("docProps/core.xml", core.as_bytes(), true);
    z.add("word/document.xml", doc.as_bytes(), true);
    z.add("word/styles.xml", styles.as_bytes(), true);
    z.add("word/_rels/document.xml.rels", doc_rels.as_bytes(), true);
    // Children in the schema's order: the background setting, the protection, then the
    // compatibility mode. Word 2013 and later (15) lay the document out as current Word does
    // and run its check boxes; without it Word opens the file in compatibility mode.
    let background = if page_colour.is_some() { "<w:displayBackgroundShape/>" } else { "" };
    let protection = if protect { "<w:documentProtection w:edit=\"forms\" w:enforcement=\"1\"/>" } else { "" };
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:settings xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">{background}{protection}<w:compat><w:compatSetting w:name=\"compatibilityMode\" w:uri=\"http://schemas.microsoft.com/office/word\" w:val=\"15\"/></w:compat></w:settings>"
    );
    z.add("word/settings.xml", xml.as_bytes(), true);
    for (name, im) in &media {
        z.add(&format!("word/media/{name}"), &im.bytes, false);
    }
    z.finish()
}
