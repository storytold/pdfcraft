use super::{Document, EditError, Object, Rgb, added, helvetica_width};

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
    /// The first character [`add_header_footer`] would refuse, because the standard fonts can't
    /// draw it (#125). Tokens only add ASCII (Bates prefixes and suffixes are part of the
    /// template), so checking the templates, split as `expand`'s output is, covers every page
    /// (and a `\r` that would be drawn is refused too). The dialog asks this too, so its preview
    /// never offers text that applying refuses (#2013).
    pub fn first_undrawable(&self) -> Option<char> {
        self.text.iter().flat_map(|t| t.lines()).find_map(pdfcraft_fonts::first_non_win_ansi)
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

impl MarkSource {
    /// A stand-in for a picture that isn't in the document yet: a one-point square. Its aspect
    /// ratio is a guess, so a layout made from it (a preview with [`watermark_centres`]) may not
    /// match the picture's: a repeat it refuses is a warning, and [`add_watermark`] decides.
    pub fn placeholder() -> Self {
        Self { xobject: pdfcraft_cos::ObjRef::new(0, 0), size: (1.0, 1.0), image: true }
    }
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
    /// Draw the watermark again and again across the page (`None`: once).
    pub repeat: Option<Repeat>,
}

impl Watermark {
    /// The first character of a text watermark [`add_watermark`] would refuse, because the
    /// standard fonts can't draw it (#125). A picture watermark draws no text.
    pub fn first_undrawable(&self) -> Option<char> {
        match self.source {
            Some(_) => None,
            None => self.text.trim().lines().find_map(pdfcraft_fonts::first_non_win_ansi),
        }
    }
}

/// A watermark drawn as a grid of copies. One copy sits at the watermark's centre (the page
/// centre plus its `offset`); the others follow at its size plus `spacing` in each direction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Repeat {
    /// Gap between neighbouring copies, in points (horizontal, vertical).
    pub spacing: [f64; 2],
    /// Shift every other row by half a step, like bricks.
    pub stagger: bool,
}

impl Default for Repeat {
    fn default() -> Self {
        Self { spacing: [36.0, 36.0], stagger: false }
    }
}

/// The most copies of a repeated watermark on one page. More is refused, not cut short.
pub const MAX_REPEATS: usize = 1000;
/// The largest spacing or offset a repeated watermark accepts, in points.
const MAX_REPEAT_EXTENT: f64 = 1.0e6;

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
            repeat: None,
        }
    }
}

/// Where the copies of a watermark go on one page.
pub(crate) struct Layout {
    /// The font size of a text watermark (0 for a picture).
    pub(crate) font_size: f64,
    /// The centre of every copy, in page points.
    pub(crate) centres: Vec<(f64, f64)>,
}

/// Where every copy of `wm` goes on a page of `page` points (width, height): the centre of each,
/// in points with y up. It is what `add_watermark` draws, so a dialog can preview it, and it
/// fails the same way (a repeat that needs too many copies).
pub fn watermark_centres(wm: &Watermark, page: (f64, f64)) -> Result<Vec<(f64, f64)>, EditError> {
    let lines: Vec<&str> = wm.text.trim().lines().collect();
    layout_for(wm, &lines, page).map(|l| l.centres)
}

/// The font size and the copies of the watermark on one page.
pub(crate) fn layout_for(wm: &Watermark, lines: &[&str], page: (f64, f64)) -> Result<Layout, EditError> {
    let (copy, font_size) = mark_box(wm, lines, page);
    let anchor = (page.0 / 2.0 + wm.offset[0], page.1 / 2.0 + wm.offset[1]);
    let centres = match wm.repeat {
        Some(rep) => {
            if !wm.offset.iter().all(|o| o.abs() <= MAX_REPEAT_EXTENT) {
                return Err(EditError::Invalid("invalid repeat settings".into()));
            }
            tile_centres(page, anchor, copy, rep)?
        }
        None => vec![anchor],
    };
    Ok(Layout { font_size, centres })
}

/// One copy of the watermark on a page of `page` points: the box it fills once turned by the
/// rotation, and the font size of a text watermark (0 for a picture).
fn mark_box(wm: &Watermark, lines: &[&str], page: (f64, f64)) -> ((f64, f64), f64) {
    if let Some(src) = &wm.source {
        let (dw, dh, _) = picture_size(src, page, wm.scale);
        return (rotated_box((dw, dh), wm.rotation), 0.0);
    }
    let widest = lines.iter().map(|l| helvetica_width(l, 1.0)).fold(0.0, f64::max).max(0.01);
    let (w, h) = page;
    let size = if wm.font_size > 0.0 { wm.font_size } else { ((w * w + h * h).sqrt() * 0.5 / widest).clamp(6.0, 300.0) };
    let block = (widest * size, size * 1.15 * lines.len() as f64);
    (rotated_box(block, wm.rotation), size)
}

/// The size of the box that turning a `size` rectangle by `degrees` fills.
fn rotated_box(size: (f64, f64), degrees: f64) -> (f64, f64) {
    let (s, c) = degrees.to_radians().sin_cos();
    (size.0 * c.abs() + size.1 * s.abs(), size.0 * s.abs() + size.1 * c.abs())
}

/// Where the copies of a repeated mark go: the centres, in page points (y up), of the copies
/// whose `size` box touches the page. One copy is centred on `anchor`; the others are `size` plus
/// the spacing away, with every other row shifted by half a step when `rep.stagger` is set.
/// More than [`MAX_REPEATS`] copies is an error, because dropping some would change what the
/// user asked for.
pub(crate) fn tile_centres(page: (f64, f64), anchor: (f64, f64), size: (f64, f64), rep: Repeat) -> Result<Vec<(f64, f64)>, EditError> {
    let bad = || EditError::Invalid("invalid repeat settings".into());
    let finite = |v: f64, max: f64| v.is_finite() && v.abs() <= max;
    // The anchor is the page centre plus the offset, which `layout_for` has already held to
    // `MAX_REPEAT_EXTENT`; half a page more is room for the centre itself.
    if !(rep.spacing.iter().all(|s| finite(*s, MAX_REPEAT_EXTENT) && *s >= 0.0)
        && finite(page.0, MAX_REPEAT_EXTENT)
        && finite(page.1, MAX_REPEAT_EXTENT)
        && finite(anchor.0, MAX_REPEAT_EXTENT + page.0)
        && finite(anchor.1, MAX_REPEAT_EXTENT + page.1)
        && size.0.is_finite()
        && size.1.is_finite())
    {
        return Err(bad());
    }
    // A box of nothing would need endless copies; the cap below catches the small ones too.
    let (bw, bh) = (size.0.max(0.01), size.1.max(0.01));
    let (px, py) = (bw + rep.spacing[0], bh + rep.spacing[1]);
    let too_many = || {
        EditError::Invalid(format!("the repeated watermark would need more than {MAX_REPEATS} copies on a page; increase the spacing or the size"))
    };
    // The rows (and, for each, the columns) whose copies touch the page.
    let first_row = ((-bh / 2.0 - anchor.1) / py).ceil();
    let last_row = ((page.1 + bh / 2.0 - anchor.1) / py).floor();
    // Both ends are within a few hundred million after the checks above, so they fit an i64.
    let (first_row, last_row) = (first_row as i64, last_row as i64);
    let columns = |row: i64| -> (i64, i64) {
        let shift = if rep.stagger && row.rem_euclid(2) == 1 { px / 2.0 } else { 0.0 };
        let first = ((-bw / 2.0 - anchor.0 - shift) / px).ceil();
        let last = ((page.0 + bw / 2.0 - anchor.0 - shift) / px).floor();
        (first as i64, last as i64)
    };
    let in_row = |row: i64| -> usize {
        let (a, b) = columns(row);
        usize::try_from((b - a + 1).max(0)).unwrap_or(usize::MAX)
    };
    // Rows come in at most two kinds (shifted or not), so the copies are counted from the number
    // of rows of each kind: the rows themselves are not the count, since staggered rows can be
    // empty in turn, and there may be far more rows than copies.
    let rows = usize::try_from((last_row - first_row + 1).max(0)).unwrap_or(usize::MAX);
    let odd_rows = usize::try_from((last_row + 1).div_euclid(2) - first_row.div_euclid(2)).unwrap_or(0).min(rows);
    let (even, odd) = (in_row(0), if rep.stagger { in_row(1) } else { in_row(0) });
    let total = (rows - odd_rows).saturating_mul(even).saturating_add(odd_rows.saturating_mul(odd));
    if total > MAX_REPEATS {
        return Err(too_many());
    }
    let mut centres = Vec::with_capacity(total);
    if total == 0 {
        return Ok(centres);
    }
    // Some kind of row holds a copy and the kinds alternate, so there are at most twice as many
    // rows as copies and this loop is as short as the cap.
    for row in first_row..=last_row {
        let shift = if rep.stagger && row.rem_euclid(2) == 1 { px / 2.0 } else { 0.0 };
        let (a, b) = columns(row);
        for col in a..=b {
            centres.push((anchor.0 + shift + col as f64 * px, anchor.1 + row as f64 * py));
        }
    }
    Ok(centres)
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

/// The size a picture mark is drawn at, in points: fitted to the page at `scale`. Returns its
/// width, height and the factor from its natural size.
pub(crate) fn picture_size(src: &MarkSource, page: (f64, f64), scale: f64) -> (f64, f64, f64) {
    let (sw, sh) = (src.size.0.max(0.01), src.size.1.max(0.01));
    let k = (page.0 / sw).min(page.1 / sh) * if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    (sw * k, sh * k, k)
}

/// Drawing a picture mark: fitted to the page at `scale`, centred (plus `offset`), turned by
/// `rotation` degrees. Returns the content and the resource name it uses.
pub(crate) fn picture(src: &MarkSource, page: (f64, f64), scale: f64, rotation: f64, offset: [f64; 2]) -> String {
    let (w, h) = page;
    let (dw, dh, k) = picture_size(src, page, scale);
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
