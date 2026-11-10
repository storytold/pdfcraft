//! Comment summaries that show each page beside its comments, as Acrobat's Summarize Options
//! "Document and comments" layouts do: the page, scaled down, on the left half of a landscape
//! sheet, its comments listed in boxes on the right, and a connector line (or the comment's
//! number) tying each box to the comment's place on the page. A page whose comments don't fit
//! beside it continues on another sheet showing the same page again.

use super::imposition::page_form;
use super::settings::Content;
use super::{Dict, Document, Matrix, ObjRef, Object, PrintError, Stream};
use pdfcraft_cos::PdfString;
use pdfcraft_fonts::pdf::Metrics;
use std::io::Write as _;

/// One comment as a summary shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct SummaryComment {
    /// The page it is on (0-based).
    pub page: usize,
    /// Its number on the page, drawn on the page by [`Connect::Numbers`].
    pub number: usize,
    /// Where it is on its page, in user space (`[x0 y0 x1 y1]`).
    pub rect: [f64; 4],
    /// Its colour (`/C`), which its box and connector take; grey without one.
    pub color: Option<[f64; 3]>,
    /// What its box says: `(indent level, text)`, the heading first and replies indented under
    /// it. A line longer than the box is wrapped.
    pub lines: Vec<(usize, String)>,
}

/// How a comment's box is tied to its place on the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Connect {
    /// A connector line from the comment to its box ("with connector lines on single pages").
    Lines,
    /// The comment's number on the page, beside it ("on single pages").
    Numbers,
}

/// The sheet: US Letter, landscape.
const SHEET: (f64, f64) = (792.0, 612.0);
const MARGIN: f64 = 36.0;
/// The band at the top holding the title and the page number.
const HEADER: f64 = 24.0;
/// Between the page and the comment column.
const GAP: f64 = 24.0;
const FONT: f64 = 8.5;
const LEADING: f64 = 10.5;
const TITLE_FONT: f64 = 10.0;
const PAD: f64 = 4.0;
/// Between two comment boxes.
const BOX_GAP: f64 = 6.0;
/// Per reply level.
const INDENT: f64 = 10.0;
/// The sheets one summary may have: each comment's box makes at most a few, so this is only hit
/// by a document with an absurd number of comments, which gets an error rather than a huge file.
const MAX_SHEETS: usize = 20_000;
/// The colour of a comment without one.
const GREY: [f64; 3] = [0.45, 0.45, 0.45];

/// A summary of `comments` beside their pages, titled `title`, as a new unencrypted document
/// whose pages are the sheets. `comments` come grouped by page in the order they are listed;
/// a page's comments that come apart (another page's between them) start a new group. Comments
/// on pages `src` doesn't have are left out. With no comments the summary is one sheet saying so.
///
/// The pages are drawn as they print ([`Content::DocumentAndMarkups`]): with the comments that
/// have the Print flag, without pop-ups and links.
pub fn summary_with_pages(src: &Document, title: &str, comments: &[SummaryComment], connect: Connect) -> Result<Document, PrintError> {
    let pages = pdfcraft_model::pages(src);
    let mut doc = src.clone();
    if doc.trailer().contains(b"Encrypt") {
        doc.remove_encryption();
    }
    let empty = "This document has no comments.";
    let mut all_text: Vec<&str> = vec![title, empty, "Page", "(continued)"];
    all_text.extend(comments.iter().flat_map(|c| c.lines.iter().map(|(_, t)| t.as_str())));
    let font = SummaryFont::choose(&mut doc, &all_text)?;

    let col_x = MARGIN + (SHEET.0 - 2.0 * MARGIN - GAP) / 2.0 + GAP;
    let col_w = SHEET.0 - MARGIN - col_x;
    let top = SHEET.1 - MARGIN - HEADER;
    let mut promoted = std::collections::HashMap::new();
    let mut forms: std::collections::HashMap<usize, (ObjRef, Matrix, [f64; 4])> = std::collections::HashMap::new();
    let mut sheets: Vec<Sheet> = Vec::new();
    let mut groups: Vec<(usize, Vec<&SummaryComment>)> = Vec::new();
    for c in comments.iter().filter(|c| c.page < pages.len()) {
        match groups.last_mut() {
            Some((page, list)) if *page == c.page => list.push(c),
            _ => groups.push((c.page, vec![c])),
        }
    }
    for (page, list) in groups {
        let placed = match forms.get(&page) {
            Some(p) => *p,
            None => {
                let form = page_form(&mut doc, &pages, page, Content::DocumentAndMarkups, &mut promoted)?;
                let p = place(src, &pages, page, form);
                forms.insert(page, p);
                p
            }
        };
        let mut sheet = Sheet::new(page, placed, false);
        let mut y = top;
        for c in list {
            let color = c.color.filter(|c| c.iter().all(|v| v.is_finite())).map_or(GREY, |c| c.map(|v| v.clamp(0.0, 1.0)));
            let mut rows: Vec<(usize, String)> = Vec::new();
            for (level, text) in &c.lines {
                // Deep reply chains stop indenting at a quarter of the column.
                let indent = (*level as f64 * INDENT).min(col_w / 4.0);
                let width = (col_w - 2.0 * PAD - indent).max(FONT);
                let text = text.replace('\t', "    ");
                for row in pdfcraft_fonts::wrap_with(&text, FONT, width, |s, size| font.width(s, size)) {
                    rows.push((*level, row));
                }
            }
            if rows.is_empty() {
                rows.push((0, String::new()));
            }
            let mut first = true;
            let mut rest: &[(usize, String)] = &rows;
            while !rest.is_empty() {
                let room = ((y - MARGIN - 2.0 * PAD) / LEADING).floor().max(0.0) as usize;
                // A box goes whole onto the next sheet unless it is too tall for any sheet, or
                // part of it is already placed; then it is split where the sheet ends.
                let fresh = sheet.boxes.is_empty();
                let take = if room >= rest.len() {
                    rest.len()
                } else if fresh || !first {
                    room
                } else {
                    0
                };
                if take == 0 {
                    sheets.push(std::mem::replace(&mut sheet, Sheet::new(page, placed, true)));
                    if sheets.len() >= MAX_SHEETS {
                        return Err(PrintError::Invalid("the comment summary would have too many pages".into()));
                    }
                    y = top;
                    if fresh && room == 0 {
                        // Not even one line fits an empty sheet; can't happen with these constants.
                        return Err(PrintError::Invalid("the comment summary sheet is too small".into()));
                    }
                    continue;
                }
                let (now, later) = rest.split_at(take);
                let height = now.len() as f64 * LEADING + 2.0 * PAD;
                sheet.boxes.push(SummaryBox { top: y, height, rows: now.to_vec(), color, anchor: first.then_some((c.rect, c.number)) });
                y -= height + BOX_GAP;
                first = false;
                rest = later;
            }
        }
        sheets.push(sheet);
        if sheets.len() >= MAX_SHEETS {
            return Err(PrintError::Invalid("the comment summary would have too many pages".into()));
        }
    }

    let pages_root = doc.add(Object::Null);
    let mut kids = Vec::new();
    let mut fonts = Dict::new();
    fonts.set(b"F1".to_vec(), Object::Ref(font.font));
    let draw_sheet = |doc: &mut Document, content: Vec<u8>, xobjects: Dict| {
        let mut res = Dict::new();
        res.set(b"Font".to_vec(), Object::Dict(fonts.clone()));
        if !xobjects.is_empty() {
            res.set(b"XObject".to_vec(), Object::Dict(xobjects));
        }
        let contents = doc.add(Object::Stream(Stream::flate(Dict::new(), &content)));
        let mut p = Dict::new();
        p.set(b"Type".to_vec(), Object::name("Page"));
        p.set(b"Parent".to_vec(), Object::Ref(pages_root));
        p.set(b"MediaBox".to_vec(), Object::Array([0.0, 0.0, SHEET.0, SHEET.1].iter().map(|v| Object::Real(*v)).collect()));
        p.set(b"Resources".to_vec(), Object::Dict(res));
        p.set(b"Contents".to_vec(), Object::Ref(contents));
        doc.add(Object::Dict(p))
    };
    if sheets.is_empty() {
        let mut c: Vec<u8> = Vec::new();
        text(&mut c, &font, &[0.0; 3], TITLE_FONT, MARGIN, SHEET.1 - MARGIN - TITLE_FONT, title)?;
        text(&mut c, &font, &[0.0; 3], FONT + 1.5, MARGIN, top - LEADING * 2.0, empty)?;
        kids.push(Object::Ref(draw_sheet(&mut doc, c, Dict::new())));
    }
    for sheet in &sheets {
        let mut c: Vec<u8> = Vec::new();
        let mut xobjects = Dict::new();
        let (form, m, frame) = sheet.placed;
        // Header: the title on the left, the page on the right.
        let base = SHEET.1 - MARGIN - TITLE_FONT;
        text(&mut c, &font, &[0.0; 3], TITLE_FONT, MARGIN, base, title)?;
        let label = if sheet.continued { format!("Page {} (continued)", sheet.page + 1) } else { format!("Page {}", sheet.page + 1) };
        let label_x = SHEET.0 - MARGIN - font.width(&label, TITLE_FONT);
        text(&mut c, &font, &[0.0; 3], TITLE_FONT, label_x, base, &label)?;
        let _ = writeln!(
            c,
            "q 0.75 G 0.5 w {} {} m {} {} l S Q",
            n(MARGIN),
            n(top + HEADER / 2.0 - 4.0),
            n(SHEET.0 - MARGIN),
            n(top + HEADER / 2.0 - 4.0)
        );
        // The page, framed.
        xobjects.set(b"P0".to_vec(), Object::Ref(form));
        let [a, b, cc, d, e, f] = m.0;
        let _ = writeln!(c, "q {} {} {} {} {} {} cm /P0 Do Q", n(a), n(b), n(cc), n(d), n(e), n(f));
        let _ = writeln!(c, "q 0.6 G 0.5 w {} {} {} {} re S Q", n(frame[0]), n(frame[1]), n(frame[2] - frame[0]), n(frame[3] - frame[1]));
        for bx in &sheet.boxes {
            let [r, g, bl] = bx.color;
            let tint = bx.color.map(|v| 1.0 - (1.0 - v) * 0.12);
            let y0 = bx.top - bx.height;
            let _ = writeln!(
                c,
                "q {} {} {} rg {} {} {} RG 1 w {} {} {} {} re B Q",
                n(tint[0]),
                n(tint[1]),
                n(tint[2]),
                n(r),
                n(g),
                n(bl),
                n(col_x),
                n(y0),
                n(col_w),
                n(bx.height)
            );
            let mut ty = bx.top - PAD - FONT;
            for (level, row) in &bx.rows {
                let indent = (*level as f64 * INDENT).min(col_w / 4.0);
                text(&mut c, &font, &[0.0; 3], FONT, col_x + PAD + indent, ty, row)?;
                ty -= LEADING;
            }
            let Some((rect, number)) = bx.anchor else { continue };
            let Some(spot) = spot_on_page(&m, rect, frame, connect) else { continue };
            match connect {
                Connect::Lines => {
                    let end = (col_x, bx.top - PAD - FONT / 2.0);
                    let _ = writeln!(
                        c,
                        "q {} {} {} RG {} {} {} rg 0.6 w {} {} m {} {} l S {} Q",
                        n(r),
                        n(g),
                        n(bl),
                        n(r),
                        n(g),
                        n(bl),
                        n(spot.0),
                        n(spot.1),
                        n(end.0),
                        n(end.1),
                        circle(spot.0, spot.1, 1.8) + " f"
                    );
                }
                Connect::Numbers => {
                    let label = number.to_string();
                    let size = 7.5;
                    let radius = (font.width(&label, size) / 2.0 + 2.5).max(6.5);
                    // A white ring keeps the number apart from a comment of the same colour.
                    let _ = writeln!(c, "q {} {} {} rg 1 G 1.2 w {} b Q", n(r), n(g), n(bl), circle(spot.0, spot.1, radius));
                    let x = spot.0 - font.width(&label, size) / 2.0;
                    text(&mut c, &font, &[1.0; 3], size, x, spot.1 - size * 0.35, &label)?;
                }
            }
        }
        kids.push(Object::Ref(draw_sheet(&mut doc, c, xobjects)));
    }

    let count = i64::try_from(kids.len()).map_err(|_| PrintError::Invalid("the comment summary would have too many pages".into()))?;
    let mut tree = Dict::new();
    tree.set(b"Type".to_vec(), Object::name("Pages"));
    tree.set(b"Count".to_vec(), Object::Int(count));
    tree.set(b"Kids".to_vec(), Object::Array(kids));
    doc.set(pages_root, Object::Dict(tree));
    let mut cat = Dict::new();
    cat.set(b"Type".to_vec(), Object::name("Catalog"));
    cat.set(b"Pages".to_vec(), Object::Ref(pages_root));
    // A layer that is off in the document stays off on the pages shown (as `impose` keeps it).
    let layers = src.trailer().get(b"Root").and_then(|r| src.resolve(r).as_dict().and_then(|root| root.get(b"OCProperties").cloned()));
    if let Some(layers) = layers {
        cat.set(b"OCProperties".to_vec(), layers);
    }
    let root = doc.add(Object::Dict(cat));
    doc.trailer_mut().set(b"Root".to_vec(), Object::Ref(root));
    let mut info = Dict::new();
    info.set(b"Title".to_vec(), PdfString::text(title));
    info.set(b"Producer".to_vec(), PdfString::text("PdfCraft"));
    let info = doc.add(info);
    doc.trailer_mut().set(b"Info".to_vec(), Object::Ref(info));
    Ok(doc)
}

/// One sheet: a page and the comment boxes beside it.
struct Sheet {
    page: usize,
    /// The page's form, the matrix from its user space to the sheet, and its frame on the sheet.
    placed: (ObjRef, Matrix, [f64; 4]),
    /// The page's comments continue from the sheet before.
    continued: bool,
    boxes: Vec<SummaryBox>,
}

impl Sheet {
    fn new(page: usize, placed: (ObjRef, Matrix, [f64; 4]), continued: bool) -> Self {
        Self { page, placed, continued, boxes: Vec::new() }
    }
}

/// A comment's box (or the part of it on this sheet).
struct SummaryBox {
    top: f64,
    height: f64,
    rows: Vec<(usize, String)>,
    color: [f64; 3],
    /// The comment's rectangle and number, on the part with its heading: where its connector or
    /// number goes.
    anchor: Option<([f64; 4], usize)>,
}

/// Page `index` fitted into the left half of the sheet, centred: its form, the matrix from its
/// user space to the sheet (through its display space, so a turned page shows upright), and its
/// frame on the sheet.
fn place(src: &Document, pages: &[pdfcraft_model::Page], index: usize, form: ObjRef) -> (ObjRef, Matrix, [f64; 4]) {
    let area_w = (SHEET.0 - 2.0 * MARGIN - GAP) / 2.0;
    let area_h = SHEET.1 - 2.0 * MARGIN - HEADER;
    let Some(page) = pages.get(index) else { return (form, Matrix::IDENTITY, [MARGIN, MARGIN, MARGIN + area_w, MARGIN + area_h]) };
    let (w, h) = page.display_size(src);
    let (w, h) = (if w.is_finite() && w > 0.0 { w } else { 612.0 }, if h.is_finite() && h > 0.0 { h } else { 792.0 });
    let s = (area_w / w).min(area_h / h);
    let (pw, ph) = (w * s, h * s);
    let (x, y) = (MARGIN + (area_w - pw) / 2.0, MARGIN + (area_h - ph) / 2.0);
    let to_display = Matrix(page.view_matrix(src)).invert().unwrap_or_default();
    let m = to_display.then(&Matrix([s, 0.0, 0.0, s, x, y]));
    (form, m, [x, y, x + pw, y + ph])
}

/// Where on the sheet a comment at `rect` (user space) is marked, kept inside the page's
/// `frame` (a comment can sit partly off its page): a connector starts at the middle of its right
/// edge, toward the comment list; a number sits on its top-right corner, clear of what it marks.
/// `None` for a rectangle that doesn't map to finite numbers.
fn spot_on_page(m: &Matrix, rect: [f64; 4], frame: [f64; 4], connect: Connect) -> Option<(f64, f64)> {
    let b = m.bbox(rect);
    if !b.iter().all(|v| v.is_finite()) {
        return None;
    }
    let y = match connect {
        Connect::Lines => (b[1] + b[3]) / 2.0,
        Connect::Numbers => b[3],
    };
    Some((b[2].clamp(frame[0], frame[2]), y.clamp(frame[1], frame[3])))
}

/// A circle of radius `r` around `(x, y)` as a path (four Bézier quarters).
fn circle(x: f64, y: f64, r: f64) -> String {
    let k = r * 0.552_284_75;
    format!(
        "{} {} m {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c h",
        n(x + r),
        n(y),
        n(x + r),
        n(y + k),
        n(x + k),
        n(y + r),
        n(x),
        n(y + r),
        n(x - k),
        n(y + r),
        n(x - r),
        n(y + k),
        n(x - r),
        n(y),
        n(x - r),
        n(y - k),
        n(x - k),
        n(y - r),
        n(x),
        n(y - r),
        n(x + k),
        n(y - r),
        n(x + r),
        n(y - k),
        n(x + r),
        n(y)
    )
}

/// Show `s` at `(x, y)` in `rgb`.
fn text(c: &mut Vec<u8>, font: &SummaryFont, rgb: &[f64; 3], size: f64, x: f64, y: f64, s: &str) -> Result<(), PrintError> {
    if s.is_empty() {
        return Ok(());
    }
    let _ = write!(c, "BT {} {} {} rg /F1 {} Tf {} {} Td ", n(rgb[0]), n(rgb[1]), n(rgb[2]), n(size), n(x), n(y));
    c.extend(font.show(s)?);
    c.extend_from_slice(b" Tj ET\n");
    Ok(())
}

fn n(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" || s.is_empty() { "0".into() } else { s.into() }
}

/// The font a summary is set in, chosen as `pdfcraft_create::from_text_escaped` chooses it:
/// standard Helvetica for text it draws; the bundled Inter face for text Inter draws all of;
/// otherwise Helvetica with each character it can't draw written as `[U+XXXX]`, so nothing is
/// lost or shown as `?`.
struct SummaryFont {
    font: ObjRef,
    /// Inter's metrics; `None` for Helvetica.
    inter: Option<Metrics>,
}

impl SummaryFont {
    fn choose(doc: &mut Document, text: &[&str]) -> Result<Self, PrintError> {
        let helvetica_draws = text.iter().all(|t| pdfcraft_fonts::first_undrawable(t).is_none());
        if !helvetica_draws
            && let Some(face) = pdfcraft_fonts::document_latin_fonts(false).into_iter().next()
            && text.iter().all(|t| !t.chars().any(char::is_control) && pdfcraft_fonts::embedding::covers(face.bytes, t))
        {
            let mut fonts = Dict::new();
            let name = pdfcraft_fonts::embedding::embed(doc, &mut fonts, face.bytes, face.family).map_err(PrintError::Invalid)?;
            let font = fonts.reference(name.as_bytes()).ok_or_else(|| PrintError::Invalid("the bundled face could not be added".into()))?;
            let dict = doc.dict(&Object::Ref(font)).ok_or_else(|| PrintError::Invalid("the bundled face could not be read".into()))?;
            return Ok(Self { font, inter: Some(Metrics::from_dict(doc, &dict)) });
        }
        let mut font = Dict::new();
        font.set(b"Type".to_vec(), Object::name("Font"));
        font.set(b"Subtype".to_vec(), Object::name("Type1"));
        font.set(b"BaseFont".to_vec(), Object::name("Helvetica"));
        font.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
        Ok(Self { font: doc.add(font), inter: None })
    }

    /// `s` as Helvetica draws it: characters it can't are written as `[U+XXXX]`.
    fn escaped(s: &str) -> std::borrow::Cow<'_, str> {
        let mut buf = [0u8; 4];
        if pdfcraft_fonts::first_undrawable(s).is_none() {
            return std::borrow::Cow::Borrowed(s);
        }
        let mut out = String::with_capacity(s.len());
        for c in s.chars() {
            if pdfcraft_fonts::first_undrawable(c.encode_utf8(&mut buf)).is_none() {
                out.push(c);
            } else {
                out.push_str(&format!("[U+{:04X}]", u32::from(c)));
            }
        }
        std::borrow::Cow::Owned(out)
    }

    fn width(&self, s: &str, size: f64) -> f64 {
        match &self.inter {
            None => pdfcraft_fonts::helvetica_width(&Self::escaped(s), size),
            Some(m) => m.encode(s).map_or(0.0, |codes| m.codes(&codes).iter().map(|(code, _)| m.width(*code)).sum::<f64>() * size),
        }
    }

    /// The string operand that shows `s`.
    fn show(&self, s: &str) -> Result<Vec<u8>, PrintError> {
        let Some(m) = &self.inter else { return Ok(pdfcraft_fonts::literal(&pdfcraft_fonts::win_ansi(&Self::escaped(s)))) };
        let codes = m.encode(s).ok_or_else(|| PrintError::Invalid(format!("the bundled Inter face can't draw \"{}\"", s.escape_debug())))?;
        let mut out = Vec::with_capacity(codes.len().saturating_mul(2).saturating_add(2));
        out.push(b'<');
        for b in codes {
            out.extend_from_slice(format!("{b:02X}").as_bytes());
        }
        out.push(b'>');
        Ok(out)
    }
}
