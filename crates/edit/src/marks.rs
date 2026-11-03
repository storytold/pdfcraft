use super::{Document, EditError, Object, Rgb, added};

/// The kinds of page marks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkKind {
    HeaderFooter,
    Watermark,
    Background,
}

impl MarkKind {
    pub(crate) fn tag(self) -> &'static str {
        match self {
            MarkKind::HeaderFooter => "HeaderFooter",
            MarkKind::Watermark => "Watermark",
            MarkKind::Background => "Background",
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
    /// Document face for Syriac content; defaults to Estrangela, independently of the Latin face.
    pub syriac_family: pdfcraft_fonts::SyriacFamily,
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
            syriac_family: pdfcraft_fonts::SyriacFamily::default(),
            font_size: 8.0,
            color: [0.0; 3],
            underline: false,
            margins: [36.0, 36.0, 72.0, 72.0],
            start_number: 1,
            shrink: false,
        }
    }
}

/// Whether this line uses Arabic layout; any Syriac keeps its explicitly selected family path.
pub(crate) fn arabic_text(text: &str) -> bool {
    !pdfcraft_fonts::content_text::contains_syriac(text) && text.chars().any(pdfcraft_fonts::source_shaping::is_arabic)
}

/// The standard Latin face used beside Arabic in a header or footer.
pub(crate) fn latin_face(family: added::Family) -> pdfcraft_fonts::Std14 {
    match family {
        added::Family::Helvetica => pdfcraft_fonts::Std14::Helvetica,
        added::Family::Times => pdfcraft_fonts::Std14::TimesRoman,
        added::Family::Courier => pdfcraft_fonts::Std14::Courier,
    }
}

/// Shape exactly one Arabic mark line using the selected Latin metrics and bounded writer.
pub(crate) fn arabic_line(text: &str, size: f64, latin: pdfcraft_fonts::Std14) -> Result<pdfcraft_fonts::appearance_text::Line, EditError> {
    if text.contains(['\r', '\n']) {
        return Err(EditError::Invalid("an Arabic page-mark line cannot contain a line break".into()));
    }
    if let Some(ch) = pdfcraft_fonts::appearance_text::first_undrawable(text) {
        return Err(super::place::undrawable(ch));
    }
    pdfcraft_fonts::appearance_text::wrap_with_latin(text, size, f64::MAX, latin)
        .map_err(|error| EditError::Invalid(format!("Arabic page-mark shaping failed: {error:?}")))?
        .into_iter()
        .next()
        .ok_or_else(|| EditError::Invalid("the Arabic page-mark line is missing".into()))
}

/// Accept supported shaped text; otherwise report the first non-WinAnsi character.
/// The reported character identifies the fallback warning, not necessarily the glyph that made shaping fail.
fn content_undrawable(text: &str, family: pdfcraft_fonts::SyriacFamily) -> Option<char> {
    if pdfcraft_fonts::content_text::contains_syriac(text) && pdfcraft_fonts::content_text::wrap_for_family(text, 1.0, f64::MAX, family).is_ok() {
        return None;
    }
    if arabic_text(text) {
        if text.contains('\r') {
            return Some('\r');
        }
        if arabic_line(text, 1.0, pdfcraft_fonts::Std14::Helvetica).is_ok() {
            return None;
        }
        return pdfcraft_fonts::appearance_text::first_undrawable(text)
            .or_else(|| text.chars().find(|c| pdfcraft_fonts::source_shaping::is_arabic(*c)));
    }
    pdfcraft_fonts::first_non_win_ansi(text)
}

impl HeaderFooter {
    /// Shared baseline metrics for a column, including Syriac vowel and dot ink.
    pub(crate) fn column_metrics(
        text: &str,
        family: pdfcraft_fonts::SyriacFamily,
        latin: pdfcraft_fonts::Std14,
    ) -> Result<(f64, f64, f64), EditError> {
        if arabic_text(text) {
            if text.len() > pdfcraft_fonts::content_text::MAX_TEXT_BYTES || text.lines().take(1025).count() > 1024 {
                return Err(EditError::Invalid("Arabic page marks need at most 262144 UTF-8 bytes and 1024 lines per text box".into()));
            }
            let (mut ascent, mut descent) = (0.8f64, -0.2f64);
            for line in text.lines() {
                let row = arabic_line(line, 1.0, latin)?;
                ascent = ascent.max(row.ascent);
                descent = descent.min(-row.descent);
            }
            return Ok((ascent, descent, (ascent - descent + 0.1).max(1.2)));
        }
        if !pdfcraft_fonts::content_text::contains_syriac(text) {
            return Ok((0.8, 0.0, 1.2));
        }
        let rows = pdfcraft_fonts::content_text::wrap_for_family(text, 1.0, f64::MAX, family).map_err(EditError::Invalid)?;
        let (mut ascent, mut descent) = (0.8f64, -0.2f64);
        for row in rows {
            let (a, d) = row.vertical_extents().map_err(EditError::Invalid)?;
            ascent = ascent.max(a);
            descent = descent.min(d);
        }
        Ok((ascent, descent, (ascent - descent + 0.1).max(1.2)))
    }

    /// Preflight header/footer template lines with the selected Syriac face or standard-font
    /// WinAnsi coverage. A line that fails this check reports its first non-WinAnsi character
    /// as a refusal marker, not necessarily the glyph that failed. Bates prefixes and suffixes
    /// are checked as part of the template. `None` does not certify placement or outline budgets;
    /// [`add_header_footer`](crate::add_header_footer) validates the expanded text and geometry when applying the mark.
    pub fn first_undrawable(&self) -> Option<char> {
        self.text.iter().flat_map(|t| t.lines()).find_map(|text| content_undrawable(text, self.syriac_family))
    }

    /// The room the header and the footer take at the top and the bottom of a page, in points:
    /// the margin, the lines of the tallest box, and a clearance of half the font size. A side
    /// without text takes none. This is the room [`HeaderFooter::shrink`] keeps free.
    pub fn bands(&self) -> (f64, f64) {
        let size = self.font_size;
        let room = |boxes: &[String], header: bool| {
            boxes
                .iter()
                .filter(|t| !t.trim().is_empty())
                .map(|t| {
                    let (ascent, descent, pitch) = Self::column_metrics(t, self.syriac_family, latin_face(self.font)).unwrap_or((0.8, 0.0, 1.2));
                    let rows = t.lines().count().max(1);
                    let ink = if header { (ascent - descent).max(1.0) } else { ascent - descent };
                    (ink + rows.saturating_sub(1) as f64 * pitch + 0.5) * size
                })
                .reduce(f64::max)
        };
        let (header, footer) = self.text.split_at(3);
        // Header lines hang below the top margin (a full line each); the footer's last line sits
        // on the bottom margin, so only its capitals (0.8 of the size) rise above it.
        let top = room(header, true).map_or(0.0, |height| self.margins[0] + height);
        let bottom = room(footer, false).map_or(0.0, |height| self.margins[1] + height);
        (top, bottom)
    }
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
    /// Document face for Syriac text watermarks; defaults to Estrangela. Pictures do not use it.
    pub syriac_family: pdfcraft_fonts::SyriacFamily,
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
    /// Preflight trimmed watermark lines with the selected Syriac face or standard-font
    /// WinAnsi coverage. On a failed coverage check, return the first non-WinAnsi character as a
    /// refusal marker, not necessarily the missing glyph. Picture sources return `None`;
    /// [`add_watermark`](crate::add_watermark) still validates placement and outline budgets for text marks.
    pub fn first_undrawable(&self) -> Option<char> {
        match self.source {
            Some(_) => None,
            None => self.text.trim().lines().find_map(|text| content_undrawable(text, self.syriac_family)),
        }
    }
}

impl Default for Watermark {
    /// Use an empty Estrangela text watermark with gray half-opacity, 45-degree rotation and centered placement.
    fn default() -> Self {
        Self {
            text: String::new(),
            syriac_family: pdfcraft_fonts::SyriacFamily::default(),
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
            _ => format!("<<{token}>>"),
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
