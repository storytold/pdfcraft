use pdfcraft_fonts::shaped_text::{Piece, ShapedFonts, clusters, first_undrawable_in_line, is_thai, thai_message, thai_pieces};

use super::{Dict, Document, EditError, Object, Rgb, added};

/// The Thai of a header, footer or text watermark, drawn from the Thai face as Add text draws it:
/// Type 3 fonts added to the document once and to each page the mark goes on, beside the page
/// font that draws the rest.
#[derive(Default)]
pub(crate) struct MarkText {
    shaped: ShapedFonts,
    /// The fonts, by the resource names the marked pages give them.
    fonts: Dict,
}

impl MarkText {
    /// The fonts for the Thai in `lines` (every line the mark draws, on any page), named so that
    /// none of `pages` (of `all`) has the name already. Thai this build can't draw (it has no Thai
    /// face, or the face lacks a character) is refused, saying what to do.
    pub(crate) fn new<'a>(
        doc: &mut Document,
        all: &[pdfcraft_model::Page],
        pages: &[usize],
        lines: impl IntoIterator<Item = &'a str>,
    ) -> Result<Self, EditError> {
        let thai = |line: &str| thai_pieces(line).map_err(|e| EditError::Invalid(thai_message(e, line)));
        let pieces = lines.into_iter().filter(|l| l.chars().any(is_thai)).map(thai).collect::<Result<Vec<_>, _>>()?;
        if pieces.is_empty() {
            return Ok(Self::default());
        }
        let mut taken = Dict::new();
        for page in pages.iter().filter_map(|&i| all.get(i)) {
            for (name, font) in crate::place::page_fonts(doc, page).iter() {
                taken.set(name.clone(), font.clone());
            }
        }
        let mut fonts = Dict::new();
        let shaped =
            ShapedFonts::new(clusters(pieces.iter().map(Vec::as_slice)), pdfcraft_fonts::shaped_glyph, &taken, &mut fonts, "PCTh", &mut |o| {
                Object::Ref(doc.add(o))
            })
            .map_err(|(e, c)| EditError::Invalid(thai_message(e, &c.text)))?;
        Ok(Self { shaped, fonts })
    }

    /// Show `line` inside `BT … ET` from `x` on baseline `y` at `size`: its Thai from the fonts,
    /// the rest in the page font `latin` (selected when `current` is `None`), `width` wide.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn show(
        &self,
        out: &mut Vec<u8>,
        line: &str,
        x: f64,
        y: f64,
        size: f64,
        latin: &str,
        width: impl Fn(&str) -> f64,
        current: &mut Option<usize>,
    ) {
        let plain = || vec![Piece::Latin(line.into())];
        // `new` shaped every line drawn, so this shapes.
        let pieces = if self.fonts.is_empty() || !line.chars().any(is_thai) { plain() } else { thai_pieces(line).unwrap_or_else(|_| plain()) };
        self.shaped.show_line(out, &pieces, x, y, size, latin, pdfcraft_fonts::win_ansi, width, current);
    }

    /// Add the fonts to `page`'s own resources.
    pub(crate) fn add_to(&self, doc: &mut Document, page: &pdfcraft_model::Page) -> Result<(), EditError> {
        if self.fonts.is_empty() {
            return Ok(());
        }
        crate::place::add_page_fonts(doc, page, &self.fonts)
    }
}

/// The kinds of page marks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkKind {
    HeaderFooter,
    Watermark,
    Background,
    /// Printer marks: trim, bleed, registration, colour bars, page information.
    PrinterMarks,
}

impl MarkKind {
    pub(crate) fn tag(self) -> &'static str {
        match self {
            MarkKind::HeaderFooter => "HeaderFooter",
            MarkKind::Watermark => "Watermark",
            MarkKind::Background => "Background",
            MarkKind::PrinterMarks => "PrinterMarks",
        }
    }
}

/// Header and footer settings (Acrobat's Add Header and Footer dialog).
#[derive(Clone, Debug, PartialEq)]
pub struct HeaderFooter {
    /// Left, centre and right header text, then left, centre and right footer text. Tokens:
    /// `<<1>>` page number, `<<n>>` page count, `<<1 of n>>`, `<<1/n>>`, `<<Page 1>>`,
    /// `<<Page 1 of n>>`, dates `<<m/d/yyyy>>`, `<<mm/dd/yyyy>>`, `<<d/m/yyyy>>`, `<<yyyy-mm-dd>>`,
    /// `<<yyyy/mm/dd>>`, `<<yyyy/m/d>>`, `<<m/d/yy>>`, `<<mmmm d, yyyy>>`, the dotted day-first
    /// dates of German-speaking and much of Central and Eastern Europe `<<dd.mm.yyyy>>`,
    /// `<<d.m.yyyy>>`, `<<dd.mm.yy>>`, `<<d.m.yy>>`, and Bates numbers `<<Bates Number#6#1#PRE#SUF>>`
    /// (digits, start, prefix, suffix).
    pub text: [String; 6],
    /// The standard face the text is drawn in (#3754): Helvetica (the default), Times or Courier.
    pub font: added::Family,
    pub font_size: f64,
    pub color: Rgb,
    pub underline: bool,
    /// Margins in points: top, bottom, left, right (the text's distance from the page edges).
    pub margins: [f64; 4],
    /// The number `<<1>>` shows on the first page of the range.
    pub start_number: u32,
    /// Shrink the page content so the header and footer don't cover it (Acrobat's Appearance
    /// Options): the original content is scaled down evenly into the band the header and footer
    /// leave free, centred across the page. Remove and Update take the scaling away again.
    pub shrink: bool,
}

impl Default for HeaderFooter {
    /// Acrobat's defaults: 8 pt black, margins 0.5 in top and bottom, 1 in left and right.
    fn default() -> Self {
        Self {
            text: Default::default(),
            font: added::Family::Helvetica,
            font_size: 8.0,
            color: [0.0; 3],
            underline: false,
            margins: [36.0, 36.0, 72.0, 72.0],
            start_number: 1,
            shrink: false,
        }
    }
}

impl HeaderFooter {
    /// The first character [`add_header_footer`] would refuse, because neither the standard
    /// fonts nor (for Thai) the Thai face can draw it (#125). Tokens only add ASCII (Bates
    /// prefixes and suffixes are part of the template), so checking the templates, split as
    /// `expand`'s output is, covers every page (and a `\r` that would be drawn is refused too).
    /// The dialog asks this too, so its preview never offers text that applying refuses (#2013).
    pub fn first_undrawable(&self) -> Option<char> {
        self.text.iter().flat_map(|t| t.lines()).find_map(first_undrawable_in_line)
    }

    /// The room the header and the footer take at the top and the bottom of a page, in points:
    /// the margin, the lines of the tallest box, and a clearance of half the font size. A side
    /// without text takes none. This is the room [`HeaderFooter::shrink`] keeps free.
    pub fn bands(&self) -> (f64, f64) {
        let size = self.font_size;
        let line_h = size * 1.2;
        let lines = |boxes: &[String]| boxes.iter().filter(|t| !t.trim().is_empty()).map(|t| t.lines().count().max(1)).max();
        let (header, footer) = self.text.split_at(3);
        // Header lines hang below the top margin (a full line each); the footer's last line sits
        // on the bottom margin, so only its capitals (0.8 of the size) rise above it.
        let top = lines(header).map_or(0.0, |n| self.margins[0] + size + (n - 1) as f64 * line_h + size / 2.0);
        let bottom = lines(footer).map_or(0.0, |n| self.margins[1] + size * 0.8 + (n - 1) as f64 * line_h + size / 2.0);
        (top, bottom)
    }
}

/// One header and footer in a document: its settings and the pages (0-based, ascending) it is on.
/// Marks with the same settings count as one.
#[derive(Clone, Debug, PartialEq)]
pub struct HeaderFooterMark {
    pub pages: Vec<usize>,
    pub settings: HeaderFooter,
}

/// The entry of a header and footer's content streams that keeps its settings, so Update can tell
/// several apart and fill the dialog with the one chosen (#5785). Drawn text alone can't say
/// which tokens made it. Private to PdfCraft (like `/PCMark`); other readers ignore it.
pub(crate) const SETTINGS_KEY: &[u8] = b"PCHeaderFooter";

/// The most bytes one recorded text box may take. A dialog box never needs more, and it bounds
/// what listing the headers and footers of a hostile file decodes.
const MAX_RECORDED_TEXT: usize = 16 * 1024;

/// `f` of `o`, or of what it refers to: a direct value is read in place, never copied, so a large
/// one costs nothing until it has been checked. (`Document::resolve` copies a direct value whole.)
pub(crate) fn read<T>(doc: &Document, o: &Object, f: impl FnOnce(&Object) -> Option<T>) -> Option<T> {
    match o {
        Object::Ref(_) => f(&doc.resolve(o)),
        _ => f(o),
    }
}

/// An array of exactly `N` numbers, each resolved: a rewriting tool may make any of them indirect.
fn numbers<const N: usize>(doc: &Document, o: &Object) -> Option<[f64; N]> {
    read(doc, o, |array| {
        let items = array.as_array().filter(|a| a.len() == N)?;
        let values: Vec<f64> = items.iter().map(|v| read(doc, v, Object::as_f64)).collect::<Option<_>>()?;
        <[f64; N]>::try_from(values).ok()
    })
}

impl HeaderFooter {
    /// Why [`add_header_footer`] refuses these settings, if it does. A recorded header and footer
    /// is listed only when it passes too, so every one Update offers is one OK can apply.
    pub(crate) fn check(&self) -> Result<(), EditError> {
        if self.text.iter().all(|t| t.trim().is_empty()) {
            return Err(EditError::Invalid("type the header or footer text first".into()));
        }
        // Measured as recorded: a box the listing would refuse to decode must not be added, or it
        // could never be listed and updated, only replaced page-wide.
        if self.text.iter().any(|t| pdfcraft_cos::PdfString::text(t).bytes.len() > MAX_RECORDED_TEXT) {
            return Err(EditError::Invalid(format!("a header or footer box can hold at most {} KB of text", MAX_RECORDED_TEXT / 1024)));
        }
        if let Some(c) = self.first_undrawable() {
            return Err(super::place::undrawable(c));
        }
        if !(self.font_size.is_finite() && self.font_size > 0.0 && self.font_size <= 200.0 && self.margins.iter().all(|m| m.is_finite() && *m >= 0.0))
        {
            return Err(EditError::Invalid("invalid font size or margins".into()));
        }
        Ok(())
    }

    /// These settings as the [`SETTINGS_KEY`] entry. The colour is recorded as drawn (see `rgb`),
    /// so a header added with an out-of-range colour can still be listed and updated.
    pub(crate) fn to_object(&self) -> Object {
        let reals = |v: &[f64]| Object::Array(v.iter().copied().map(Object::Real).collect());
        let drawn = self.color.map(|c| if c.is_finite() { c.clamp(0.0, 1.0) } else { 0.0 });
        let mut d = Dict::new();
        d.set(b"Text".to_vec(), Object::Array(self.text.iter().map(|t| Object::String(pdfcraft_cos::PdfString::text(t))).collect()));
        d.set(
            b"Font".to_vec(),
            Object::name(match self.font {
                added::Family::Helvetica => "Helvetica",
                added::Family::Times => "Times",
                added::Family::Courier => "Courier",
            }),
        );
        d.set(b"Size".to_vec(), Object::Real(self.font_size));
        d.set(b"Color".to_vec(), reals(&drawn));
        d.set(b"Underline".to_vec(), Object::Bool(self.underline));
        d.set(b"Margins".to_vec(), reals(&self.margins));
        d.set(b"Start".to_vec(), Object::Int(i64::from(self.start_number)));
        d.set(b"Shrink".to_vec(), Object::Bool(self.shrink));
        Object::Dict(d)
    }

    /// Settings back from a [`SETTINGS_KEY`] entry. `None` unless every value is one the dialog
    /// could have written: a file can say anything here.
    pub(crate) fn from_object(doc: &Document, o: &Object) -> Option<Self> {
        let mut unlimited = usize::MAX;
        Self::from_object_within(doc, o, &mut unlimited)
    }

    /// [`from_object`](Self::from_object), converting at most `budget` bytes of text, which it
    /// takes from: `None` once that is spent. Bounds what one scan of a document converts.
    pub(crate) fn from_object_within(doc: &Document, o: &Object, budget: &mut usize) -> Option<Self> {
        #[cfg(test)]
        DECODED.with(|n| n.set(n.get() + 1));
        // Every value resolved, as a rewriting tool may make any of them indirect, and read in
        // place: nothing is copied before it is checked.
        read(doc, o, |o| {
            let d = o.as_dict()?;
            let flag = |key: &[u8]| {
                read(doc, d.get(key)?, |v| match v {
                    Object::Bool(b) => Some(*b),
                    _ => None,
                })
            };
            // The count and each size are checked before anything is decoded.
            let text: Vec<String> = read(doc, d.get(b"Text")?, |texts| {
                let texts = texts.as_array().filter(|a| a.len() == 6)?;
                texts
                    .iter()
                    .map(|t| {
                        read(doc, t, |t| {
                            let s = t.as_string().filter(|s| s.bytes.len() <= MAX_RECORDED_TEXT)?;
                            *budget = budget.checked_sub(s.bytes.len())?;
                            Some(s.to_text())
                        })
                    })
                    .collect()
            })?;
            let hf = Self {
                text: text.try_into().ok()?,
                font: read(doc, d.get(b"Font")?, |v| match v.as_name()? {
                    b"Helvetica" => Some(added::Family::Helvetica),
                    b"Times" => Some(added::Family::Times),
                    b"Courier" => Some(added::Family::Courier),
                    _ => None,
                })?,
                font_size: read(doc, d.get(b"Size")?, Object::as_f64)?,
                color: numbers(doc, d.get(b"Color")?)?,
                underline: flag(b"Underline")?,
                margins: numbers(doc, d.get(b"Margins")?)?,
                start_number: read(doc, d.get(b"Start")?, |v| u32::try_from(v.as_int()?).ok())?,
                shrink: flag(b"Shrink")?,
            };
            let sane = hf.check().is_ok() && hf.color.iter().all(|c| (0.0..=1.0).contains(c));
            sane.then_some(hf)
        })
    }
}

#[cfg(test)]
thread_local! {
    /// How many settings entries [`HeaderFooter::from_object_within`] has read on this thread.
    pub(crate) static DECODED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The header and footer settings a [`SETTINGS_KEY`] entry records, if it records ones that
/// [`add_header_footer`](crate::add_header_footer) would accept: what redaction matches a
/// header's streams by, however each stores them.
pub fn recorded_header_footer(doc: &Document, value: &Object) -> Option<HeaderFooter> {
    HeaderFooter::from_object(doc, value)
}

/// The `cm` that shrinks a page's original content (user space) into the band `hf` leaves free,
/// for a page shown `size` (display space) with view matrix `view` (display to user space): an
/// even scale, centred across, the content's bottom on the footer's band.
pub(crate) fn shrink_matrix(hf: &HeaderFooter, size: (f64, f64), view: [f64; 6]) -> Result<[f64; 6], EditError> {
    let (w, h) = size;
    let (top, bottom) = hf.bands();
    let free = h - top - bottom;
    if !(free.is_finite() && free >= 1.0 && w.is_finite() && w > 0.0) {
        return Err(EditError::Invalid("the header and footer margins leave no room for the page content to shrink into".into()));
    }
    let k = free / h;
    let v = kurbo::Affine::new(view);
    let det = v.determinant();
    if !(det.is_finite() && det.abs() > 1e-12) {
        return Err(EditError::Invalid("the page can't be shrunk: its view matrix is degenerate".into()));
    }
    let shrink = kurbo::Affine::new([k, 0.0, 0.0, k, (w - w * k) / 2.0, bottom]);
    // User space to display space, shrink there, and back.
    Ok((v * shrink * v.inverse()).as_coeffs())
}

/// A picture for a background or watermark (Acrobat: Source ▸ File): an XObject already in
/// the document, an image (drawn into a unit square) or a form (a page of a PDF).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MarkSource {
    pub xobject: pdfcraft_cos::ObjRef,
    /// Its natural size in points.
    pub size: (f64, f64),
    /// An image XObject (unit square); otherwise a form whose `/Matrix` maps it to `size`.
    pub image: bool,
}

/// A watermark: text, or a picture from a file (`source`; the text is then ignored).
#[derive(Clone, Debug, PartialEq)]
pub struct Watermark {
    pub text: String,
    pub source: Option<MarkSource>,
    /// Pictures: size relative to the page (1.0 fits the page).
    pub scale: f64,
    /// 0 = fit: about half the page diagonal.
    pub font_size: f64,
    pub color: Rgb,
    /// 0–1.
    pub opacity: f64,
    /// Degrees counter-clockwise (Acrobat's 45° runs from bottom-left to top-right).
    pub rotation: f64,
    /// Draw behind the page content instead of on top.
    pub behind: bool,
    /// Offset of the centre from the page centre, in points (right, up).
    pub offset: [f64; 2],
}

impl Watermark {
    /// The first character of a text watermark [`add_watermark`] would refuse, because neither
    /// the standard fonts nor (for Thai) the Thai face can draw it (#125). A picture watermark
    /// draws no text.
    pub fn first_undrawable(&self) -> Option<char> {
        match self.source {
            Some(_) => None,
            None => self.text.trim().lines().find_map(first_undrawable_in_line),
        }
    }
}

impl Default for Watermark {
    fn default() -> Self {
        Self {
            text: String::new(),
            source: None,
            scale: 0.5,
            font_size: 0.0,
            color: [0.6, 0.6, 0.6],
            opacity: 0.5,
            rotation: 45.0,
            behind: false,
            offset: [0.0; 2],
        }
    }
}

/// A background behind the page content: a solid colour, or a picture from a file.
#[derive(Clone, Debug, PartialEq)]
pub struct Background {
    pub color: Rgb,
    pub opacity: f64,
    pub source: Option<MarkSource>,
    /// Pictures: size relative to the page (1.0 fits the page).
    pub scale: f64,
    /// Replace a white backdrop the page paints itself, which would hide the background
    /// ([`backdrop`](crate::backdrop)). Removing the background turns it back on where that is
    /// still safe ([`backdrops_left_off`](crate::backdrops_left_off) are the pages where it isn't).
    pub replace_backdrop: bool,
}

impl Default for Background {
    /// A light yellow that still reads as a colour on screen (a near-white default looked as if
    /// nothing had happened).
    fn default() -> Self {
        Self { color: [1.0, 0.95, 0.7], opacity: 1.0, source: None, scale: 1.0, replace_backdrop: true }
    }
}

/// Drawing a picture mark: fitted to the page at `scale`, centred (plus `offset`), turned by
/// `rotation` degrees. Returns the content and the resource name it uses.
pub(crate) fn picture(src: &MarkSource, page: (f64, f64), scale: f64, rotation: f64, offset: [f64; 2]) -> String {
    let (w, h) = page;
    let (sw, sh) = (src.size.0.max(0.01), src.size.1.max(0.01));
    let k = (w / sw).min(h / sh) * if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let (dw, dh) = (sw * k, sh * k);
    let (s, c) = rotation.to_radians().sin_cos();
    let (cx, cy) = (w / 2.0 + offset[0], h / 2.0 + offset[1]);
    // Centre, rotate, then place the picture's box (unit square for images).
    let place = if src.image {
        format!("{} 0 0 {} {} {} cm", n(dw), n(dh), n(-dw / 2.0), n(-dh / 2.0))
    } else {
        format!("{} 0 0 {} {} {} cm", n(k), n(k), n(-dw / 2.0), n(-dh / 2.0))
    };
    format!("{} {} {} {} {} {} cm\n{place}\n/PCPic{} Do\n", n(c), n(s), n(-s), n(c), n(cx), n(cy), src.xobject.num)
}

/// Register a mark picture in the page's own resources (`/PCPic<num>`).
pub(crate) fn add_picture_resource(doc: &mut Document, page: &pdfcraft_model::Page, src: &MarkSource) -> Result<(), EditError> {
    let p = doc.get(page.obj).as_dict().cloned().unwrap_or_default();
    let mut res = p.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let mut xo = res.get(b"XObject").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default();
    xo.set(format!("PCPic{}", src.xobject.num).into_bytes(), Object::Ref(src.xobject));
    res.set(b"XObject".to_vec(), Object::Dict(xo));
    doc.update_dict(page.obj, |d| d.set(b"Resources".to_vec(), Object::Dict(res)))?;
    Ok(())
}

/// Values tokens need that come from outside the document.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Context {
    /// Today's date: (year, month 1–12, day 1–31).
    pub date: (i64, u32, u32),
}

// Model-derived transforms can contain tiny scale factors; retain their PDF decimal precision.
pub(crate) fn matrix_number(value: f64) -> String {
    let mut bytes = Vec::new();
    pdfcraft_cos::serialize(&Object::Real(value), &mut bytes);
    String::from_utf8_lossy(&bytes).into_owned()
}

pub(crate) fn n(v: f64) -> String {
    let s = format!("{:.3}", if v.abs() < 5e-4 { 0.0 } else { v });
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

pub(crate) fn rgb(c: Rgb) -> String {
    let c = c.map(|x| x.clamp(0.0, 1.0));
    format!("{} {} {} rg", n(c[0]), n(c[1]), n(c[2]))
}

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

/// Expand `<<…>>` tokens for page `index` (0-based within the range) of `count`.
pub fn expand(template: &str, page_number: u32, count: usize, bates: u64, cx: &Context) -> String {
    let (y, m, d) = cx.date;
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("<<") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find(">>") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let token = &after[..end];
        let p = page_number;
        let value = match token {
            "1" => p.to_string(),
            "n" => count.to_string(),
            "1 of n" => format!("{p} of {count}"),
            "1/n" => format!("{p}/{count}"),
            "Page 1" => format!("Page {p}"),
            "Page 1 of n" => format!("Page {p} of {count}"),
            "m/d" => format!("{m}/{d}"),
            "m/d/yy" => format!("{m}/{d}/{:02}", y.rem_euclid(100)),
            "m/d/yyyy" => format!("{m}/{d}/{y}"),
            "mm/dd/yy" => format!("{m:02}/{d:02}/{:02}", y.rem_euclid(100)),
            "mm/dd/yyyy" => format!("{m:02}/{d:02}/{y}"),
            "d/m/yy" => format!("{d}/{m}/{:02}", y.rem_euclid(100)),
            "d/m/yyyy" => format!("{d}/{m}/{y}"),
            "dd/mm/yyyy" => format!("{d:02}/{m:02}/{y}"),
            "d.m.yy" => format!("{d}.{m}.{:02}", y.rem_euclid(100)),
            "d.m.yyyy" => format!("{d}.{m}.{y}"),
            "dd.mm.yy" => format!("{d:02}.{m:02}.{:02}", y.rem_euclid(100)),
            "dd.mm.yyyy" => format!("{d:02}.{m:02}.{y}"),
            "yyyy-mm-dd" => format!("{y}-{m:02}-{d:02}"),
            "yyyy/mm/dd" => format!("{y}/{m:02}/{d:02}"),
            "yyyy/m/d" => format!("{y}/{m}/{d}"),
            "mmmm d, yyyy" => format!("{} {d}, {y}", MONTHS[(m.clamp(1, 12) - 1) as usize]),
            t if t.starts_with("Bates Number") => {
                // Bates Number#digits#start#prefix#suffix (start is applied by the caller).
                let parts: Vec<&str> = t.split('#').collect();
                let digits = parts.get(1).and_then(|d| d.parse::<usize>().ok()).unwrap_or(6).clamp(1, 15);
                format!("{}{:0digits$}{}", parts.get(3).unwrap_or(&""), bates, parts.get(4).unwrap_or(&""))
            }
            _ => {
                // Not a token: emit one `<` and retry, so a later token is still found.
                out.push('<');
                rest = &rest[start + 1..];
                continue;
            }
        };
        out.push_str(&value);
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

/// The Bates start number in a template (`<<Bates Number#digits#start#…>>`), if any.
pub(crate) fn bates_start(template: &str) -> Option<u64> {
    let i = template.find("<<Bates Number")?;
    let t = &template[i + 2..];
    let t = &t[..t.find(">>")?];
    t.split('#').nth(2).and_then(|s| s.parse().ok()).or(Some(1))
}

#[cfg(test)]
mod expand_tests {
    use super::*;

    #[test]
    fn expand_token_after_extra_angle_brackets() {
        let cx = Context::default();
        assert_eq!(expand("<<<1>>>", 3, 9, 1, &cx), "<3>");
        assert_eq!(expand("<< <<1>>", 3, 9, 1, &cx), "<< 3");
        assert_eq!(expand("<<x>> <<1>>", 3, 9, 1, &cx), "<<x>> 3");
    }
}
