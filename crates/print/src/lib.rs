//! Printing (execution plan M10.5). Layer L4.
//!
//! [`impose`] turns the pages to print into a print-ready PDF of *sheets*, laid out the way
//! Acrobat's Print dialog describes them:
//! - **Size**: one page per sheet, Fit, Actual size, Shrink oversized pages or a custom scale,
//!   centred;
//! - **Multiple**: pages per sheet in a grid (2, 4, 6, 9, 16 or custom), four page orders,
//!   optional page borders and auto-rotation;
//! - **Booklet**: saddle-stitched spreads (both sides, front or back only; left or right
//!   binding);
//! - **Poster**: each page enlarged across several sheets with overlap and cut marks;
//! - orientation (auto, portrait, landscape) and what to print (the document, with markups,
//!   with stamps, or form fields only; annotations print only with their Print flag).
//!
//! The sheets are written as a new, unencrypted, garbage-collected PDF; [`spool`] hands it to
//! the system's print spooler (CUPS on macOS and Linux).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use pdfcraft_content::Matrix;
use pdfcraft_cos::{Dict, Document, ObjRef, Object, SaveOptions, Stream, write_full};

pub mod range;
pub mod spool;
#[cfg(test)]
mod tests;

pub use range::{Subset, select_listed, select_pages};

/// What can go wrong when printing.
///
/// The three things a *person* can be wrong about — there is nothing to print, a page number past
/// the end, a range that is not a page — each have their own variant with the numbers apart from
/// the words, so an interface can say them in its own language with [`PrintError::said`]. The rest
/// are faults rather than mistakes and carry their text as it is.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum PrintError {
    #[error("there are no pages to print")]
    NoPages,
    /// A page number past the end of the document: the number asked for, and the page count.
    #[error("page {0} is out of range (1–{1})")]
    OutOfRange(usize, usize),
    /// A range token that is neither a page number nor a page label.
    #[error("{0:?} is not a page number or label")]
    NotAPage(String),
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Spool(String),
    #[error(transparent)]
    Cos(#[from] pdfcraft_cos::CosError),
}

/// How an interface says one of our wordings in its own language, holes and all: given the English
/// in [`WORDINGS`], the same wording in that language. [`in_english`] keeps the English.
pub type Words<'a> = &'a dyn Fn(&str) -> String;

/// The [`Words`] that translate nothing: the wording as it is written here.
pub fn in_english(wording: &str) -> String {
    wording.to_owned()
}

/// What a person can get wrong about what to print, in English, with their holes still in them.
///
/// An interface that reports these in another language translates these three, which is the whole
/// reason they are gathered here; `wordings_are_said` keeps them in step with the errors.
pub const WORDINGS: &[&str] = &["there are no pages to print", "page {n} is out of range (1–{count})", "{page} is not a page number or label"];

impl PrintError {
    /// This error as one sentence, said in the language `words` translates into.
    ///
    /// The wording is translated and the numbers go in afterwards, in a single pass, so what the
    /// person typed cannot be read as a hole. With [`in_english`] this is the error's own
    /// `Display`, which is what the command line and scripts report.
    pub fn said(&self, words: Words) -> String {
        match self {
            PrintError::NoPages => words("there are no pages to print"),
            PrintError::OutOfRange(n, count) => {
                say(&words("page {n} is out of range (1–{count})"), &[("{n}", n.to_string()), ("{count}", count.to_string())])
            }
            // The token goes in quoted, the way it is typed, as the error itself prints it.
            PrintError::NotAPage(token) => say(&words("{page} is not a page number or label"), &[("{page}", format!("{token:?}"))]),
            fault => fault.to_string(),
        }
    }
}

/// A wording with its holes filled.
///
/// A hole is a name in braces, such as `{n}`; what is not in `values` is left as it is written.
/// Values go in during a single pass, so a value with braces in it is never read as a hole.
fn say(wording: &str, values: &[(&str, String)]) -> String {
    let mut out = String::with_capacity(wording.len() + 8);
    let mut rest = wording;
    while let Some(open) = rest.find('{') {
        let (before, from_brace) = rest.split_at(open);
        out.push_str(before);
        // `find` counts from the brace and `}` is one byte, so this is a char boundary.
        let Some(close) = from_brace.find('}') else {
            // A brace with nothing closing it is just text.
            out.push_str(from_brace);
            return out;
        };
        let Some(hole) = from_brace.get(..=close) else { return out + from_brace };
        match values.iter().find(|(name, _)| *name == hole) {
            Some((_, value)) => out.push_str(value),
            None => out.push_str(hole),
        }
        rest = from_brace.get(close + 1..).unwrap_or_default();
    }
    out.push_str(rest);
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Orientation {
    #[default]
    Auto,
    Portrait,
    Landscape,
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum SizeMode {
    #[default]
    Fit,
    Actual,
    Shrink,
    /// Percent.
    Custom(f64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PageOrder {
    #[default]
    Horizontal,
    HorizontalReversed,
    Vertical,
    VerticalReversed,
    /// Single-sided sheets: cut into cell piles, then stack them in row-major order.
    CutStack,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BookletSubset {
    #[default]
    BothSides,
    FrontOnly,
    BackOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Binding {
    #[default]
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Layout {
    Size(SizeMode),
    Multiple {
        cols: usize,
        rows: usize,
        /// Source pages on each sheet, at most `cols × rows`: three per sheet uses a 2 × 2 grid
        /// with one cell left blank.
        per_sheet: usize,
        order: PageOrder,
        border: bool,
        auto_rotate: bool,
    },
    Booklet {
        subset: BookletSubset,
        binding: Binding,
    },
    /// Tile scale (percent), overlap (points), cut marks.
    Poster {
        scale: f64,
        overlap: f64,
        cut_marks: bool,
    },
    /// Shipping label: each page cut to its [`Settings::crops`] box, turned so its text reads
    /// upright on the sheet and fitted with [`LABEL_MARGIN`]; `flip` turns it a further 180°.
    Label {
        flip: bool,
    },
}

impl Default for Layout {
    fn default() -> Self {
        Layout::Size(SizeMode::Fit)
    }
}

impl Layout {
    /// Acrobat's pages-per-sheet choices as a grid (columns × rows on a portrait sheet).
    pub fn multiple(per_sheet: usize) -> Layout {
        let (cols, rows) = match per_sheet {
            2 => (1, 2),
            4 => (2, 2),
            6 => (2, 3),
            9 => (3, 3),
            16 => (4, 4),
            n => {
                let c = (n as f64).sqrt().ceil().max(1.0) as usize;
                (c, n.div_ceil(c).max(1))
            }
        };
        Layout::Multiple { cols, rows, per_sheet: per_sheet.max(1), order: PageOrder::Horizontal, border: false, auto_rotate: true }
    }
}

/// Acrobat's Comments & Forms choices.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Content {
    /// Page content and form fields.
    Document,
    /// Page content, form fields and every printable comment.
    #[default]
    DocumentAndMarkups,
    /// Page content, form fields and stamps.
    DocumentAndStamps,
    /// Form fields only.
    FormFieldsOnly,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// Source pages (0-based) in print order; see [`select_pages`].
    pub pages: Vec<usize>,
    /// Paper (width, height) in points, portrait.
    pub paper: (f64, f64),
    pub orientation: Orientation,
    pub layout: Layout,
    pub content: Content,
    /// For [`Layout::Label`], per source page: the area to keep (display space) and whether its
    /// text runs bottom-to-top (so a quarter turn clockwise makes it upright). `None` or a
    /// missing entry keeps the whole page.
    pub crops: Vec<Option<Crop>>,
    /// Print as image (Acrobat's Advanced Print Setup): every page rendered to a picture at this
    /// resolution before it is placed on the paper. The engine does the rendering; `impose` is
    /// then given the image-only document.
    pub raster: Option<Raster>,
}

/// How pages are rendered when printing as image.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Raster {
    /// Dots per inch, 72–600 (Acrobat offers 150, 300 and 600).
    pub dpi: f64,
    /// Render the pictures in grey, so the print-ready PDF itself is grayscale.
    pub grayscale: bool,
}

impl Default for Raster {
    fn default() -> Self {
        Raster { dpi: 300.0, grayscale: false }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crop {
    pub rect: [f64; 4],
    pub text_up: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            pages: Vec::new(),
            paper: PAPERS[0].1,
            orientation: Orientation::Auto,
            layout: Layout::default(),
            content: Content::default(),
            raster: None,
            crops: Vec::new(),
        }
    }
}

/// Common paper sizes (points, portrait).
pub const PAPERS: [(&str, (f64, f64)); 21] = [
    ("US Letter", (612.0, 792.0)),
    ("US Legal", (612.0, 1008.0)),
    ("Tabloid", (792.0, 1224.0)),
    ("A3", (841.89, 1190.55)),
    ("A4", (595.28, 841.89)),
    ("A5", (419.53, 595.28)),
    ("A6", (297.64, 419.53)),
    ("ARCH A (9 x 12 in)", (648.0, 864.0)),
    ("ARCH B (12 x 18 in)", (864.0, 1296.0)),
    ("ARCH C (18 x 24 in)", (1296.0, 1728.0)),
    ("ARCH D (24 x 36 in)", (1728.0, 2592.0)),
    ("ARCH E1 (30 x 42 in)", (2160.0, 3024.0)),
    ("ARCH E (36 x 48 in)", (2592.0, 3456.0)),
    ("ANSI C (17 x 22 in)", (1224.0, 1584.0)),
    ("ANSI D (22 x 34 in)", (1584.0, 2448.0)),
    ("ANSI E (34 x 44 in)", (2448.0, 3168.0)),
    ("A2", (1190.55, 1683.78)),
    ("A1", (1683.78, 2383.94)),
    ("A0", (2383.94, 3370.39)),
    ("4 x 6 in label (101.6 x 152.4 mm)", (288.0, 432.0)),
    ("US #10 Envelope (4.125 x 9.5 in)", (297.0, 684.0)),
];

/// `PAPERS` index of the 4 × 6 in shipping label.
pub const LABEL_PAPER: usize = 19;

/// The CUPS media name for `PAPERS[index]`, so the spooler feeds the sheet the PDF was laid out
/// on rather than the queue's default (PPD keywords; `w288h432` is the usual 4×6 label, `Env10`
/// the US #10 envelope).
pub fn cups_media(index: usize) -> Option<&'static str> {
    match PAPERS.get(index)?.0 {
        "US Letter" => Some("Letter"),
        "US Legal" => Some("Legal"),
        "Tabloid" => Some("Tabloid"),
        "A3" => Some("A3"),
        "A4" => Some("A4"),
        "A5" => Some("A5"),
        "US #10 Envelope (4.125 x 9.5 in)" => Some("Env10"),
        _ if index == LABEL_PAPER => Some("w288h432"),
        _ => None,
    }
}

/// Whether most of page `index`'s text runs bottom-to-top as the page displays (a label laid
/// sideways on a letter or A4 sheet), counting each text-showing operator once; form XObjects
/// are followed a few levels deep.
pub fn text_reads_up(doc: &Document, index: usize) -> bool {
    let Some(page) = pdfcraft_model::pages(doc).into_iter().nth(index) else { return false };
    let to_display = Matrix(page.view_matrix(doc)).invert().unwrap_or_default();
    let res = page.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_default();
    let mut score = 0i64;
    text_direction(doc, &decoded_contents(doc, &page.dict), &res, to_display, 0, &mut score);
    score > 0
}

fn text_direction(doc: &Document, data: &[u8], res: &Dict, base: Matrix, depth: usize, score: &mut i64) {
    let (mut ctm, mut tm, mut stack) = (base, Matrix::IDENTITY, Vec::new());
    for op in pdfcraft_content::parse(data).ops {
        match op.op.as_slice() {
            // Bounded like the renderer's graphics-state stack: deeper saves are ignored.
            b"q" if stack.len() < 256 => stack.push(ctm),
            b"Q" => ctm = stack.pop().unwrap_or(base),
            b"cm" => ctm = Matrix::from_operands(&op.operands).unwrap_or_default().then(&ctm),
            b"BT" => tm = Matrix::IDENTITY,
            b"Tm" => tm = Matrix::from_operands(&op.operands).unwrap_or_default(),
            b"Tj" | b"TJ" | b"'" | b"\"" => {
                let [a, b, ..] = tm.then(&ctm).0;
                *score += i64::from(b > a.abs()) - i64::from(-b > a.abs());
            }
            b"Do" if depth < 4 => {
                let Some(name) = op.name(0) else { continue };
                let Some(xobj) = res.get(b"XObject").and_then(|x| doc.resolve(x).as_dict().and_then(|x| x.get(name).cloned())) else { continue };
                let Object::Stream(form) = &*doc.resolve(&xobj) else { continue };
                if form.dict.get(b"Subtype").and_then(|s| doc.resolve(s).as_name().map(<[u8]>::to_vec)).as_deref() != Some(b"Form".as_slice()) {
                    continue;
                }
                let m = form.dict.get(b"Matrix").and_then(|m| doc.resolve(m).as_array().and_then(|a| Matrix::from_operands(a))).unwrap_or_default();
                let inner = form.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_else(|| res.clone());
                if let Ok(body) = form.decoded() {
                    text_direction(doc, &body, &inner, m.then(&ctm), depth + 1, score);
                }
            }
            _ => {}
        }
    }
}

/// Margin kept around a label: thermal heads miss the very edge, and many 4×6 drivers feed
/// 100 × 150 mm (283 × 425 pt), a little short of the 288 × 432 pt sheet.
pub const LABEL_MARGIN: f64 = 8.0;

/// Place `crop` (display space) of `page` centred in `cell`, scaled by `s` and turned by
/// `quarter` quarter turns counter-clockwise (0–3).
fn place_crop(page: usize, crop: [f64; 4], cell: [f64; 4], s: f64, quarter: u8) -> Placement {
    let (w, h) = (crop[2] - crop[0], crop[3] - crop[1]);
    let (pw, ph) = if quarter % 2 == 1 { (h * s, w * s) } else { (w * s, h * s) };
    let (x, y) = (cell[0] + (cell[2] - cell[0] - pw) / 2.0, cell[1] + (cell[3] - cell[1] - ph) / 2.0);
    let [a, b, c, d, e, f] = match quarter % 4 {
        0 => [s, 0.0, 0.0, s, x, y],
        1 => [0.0, s, -s, 0.0, x + pw, y],
        2 => [-s, 0.0, 0.0, -s, x + pw, y + ph],
        _ => [0.0, -s, s, 0.0, x, y + ph],
    };
    // Move the crop's corner to the origin first.
    Placement { page, matrix: Matrix([a, b, c, d, e - a * crop[0] - c * crop[1], f - b * crop[0] - d * crop[1]]), clip: crop }
}

/// Match a displayed PDF page to a standard sheet, independent of orientation.
/// Half a point allows rounded metric PDF dimensions without confusing distinct sizes.
pub fn matching_paper(size: (f64, f64)) -> Option<usize> {
    if !size.0.is_finite() || !size.1.is_finite() {
        return None;
    }
    let size = (size.0.min(size.1), size.0.max(size.1));
    PAPERS.iter().position(|(_, p)| (p.0 - size.0).abs() <= 0.5 && (p.1 - size.1).abs() <= 0.5)
}

/// The unprintable margin assumed around the sheet for Fit, Multiple and Booklet.
pub const MARGIN: f64 = 18.0;

/// One page as placed on a sheet: the source page, the matrix from its *display space* to the
/// sheet, and the visible part of it in display space (clipping).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub page: usize,
    pub matrix: Matrix,
    pub clip: [f64; 4],
}

/// One output sheet.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sheet {
    pub size: (f64, f64),
    pub placed: Vec<Placement>,
    /// Page borders (Multiple) and cut marks (Poster), in sheet space: rectangles and lines.
    pub borders: Vec<[f64; 4]>,
    pub lines: Vec<[f64; 4]>,
}

fn oriented(paper: (f64, f64), o: Orientation, landscape_wanted: bool) -> (f64, f64) {
    let (w, h) = (paper.0.min(paper.1), paper.0.max(paper.1));
    match o {
        Orientation::Portrait => (w, h),
        Orientation::Landscape => (h, w),
        Orientation::Auto => {
            if landscape_wanted {
                (h, w)
            } else {
                (w, h)
            }
        }
    }
}

/// Place a page of display size `(dw, dh)` inside `cell` (sheet space), scaled by `s`
/// (centred), optionally rotated 90° (counter-clockwise).
fn place_in(page: usize, (dw, dh): (f64, f64), cell: [f64; 4], s: f64, rotate: bool) -> Placement {
    let (cw, ch) = (cell[2] - cell[0], cell[3] - cell[1]);
    let (pw, ph) = if rotate { (dh * s, dw * s) } else { (dw * s, dh * s) };
    let (x, y) = (cell[0] + (cw - pw) / 2.0, cell[1] + (ch - ph) / 2.0);
    let matrix = if rotate { Matrix([0.0, s, -s, 0.0, x + pw, y]) } else { Matrix([s, 0.0, 0.0, s, x, y]) };
    Placement { page, matrix, clip: [0.0, 0.0, dw, dh] }
}

fn fit_scale((dw, dh): (f64, f64), (cw, ch): (f64, f64)) -> f64 {
    if dw <= 0.0 || dh <= 0.0 { 1.0 } else { (cw / dw).min(ch / dh) }
}

/// Lay out the sheets for `settings` given each source page's display size.
pub fn layout(sizes: &[(f64, f64)], settings: &Settings) -> Result<Vec<Sheet>, PrintError> {
    let pages: Vec<usize> = settings.pages.iter().copied().filter(|p| *p < sizes.len()).collect();
    if pages.is_empty() {
        return Err(PrintError::NoPages);
    }
    let paper = settings.paper;
    if !(paper.0 >= 72.0 && paper.1 >= 72.0 && paper.0.is_finite() && paper.1.is_finite()) {
        return Err(PrintError::Invalid("invalid paper size".into()));
    }
    let mut sheets = Vec::new();
    match settings.layout {
        Layout::Size(mode) => {
            for &p in &pages {
                let d = sizes[p];
                let size = oriented(paper, settings.orientation, d.0 > d.1);
                let fit = fit_scale(d, (size.0 - 2.0 * MARGIN, size.1 - 2.0 * MARGIN));
                let s = match mode {
                    SizeMode::Fit => fit,
                    SizeMode::Actual => 1.0,
                    SizeMode::Shrink => fit.min(1.0),
                    SizeMode::Custom(pct) if pct.is_finite() && pct > 0.0 => pct / 100.0,
                    SizeMode::Custom(_) => return Err(PrintError::Invalid("the scale must be more than 0%".into())),
                };
                sheets.push(Sheet { size, placed: vec![place_in(p, d, [0.0, 0.0, size.0, size.1], s, false)], ..Sheet::default() });
            }
        }
        Layout::Label { flip } => {
            let size = oriented(paper, settings.orientation, false);
            let cell = [LABEL_MARGIN, LABEL_MARGIN, size.0 - LABEL_MARGIN, size.1 - LABEL_MARGIN];
            for &p in &pages {
                let d = sizes[p];
                let crop = settings.crops.get(p).copied().flatten();
                let rect = crop
                    .map(|c| [c.rect[0].max(0.0), c.rect[1].max(0.0), c.rect[2].min(d.0), c.rect[3].min(d.1)])
                    .filter(|r| r[2] - r[0] > 1.0 && r[3] - r[1] > 1.0)
                    .unwrap_or([0.0, 0.0, d.0, d.1]);
                let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
                let turn = (w > h) != (cell[2] - cell[0] > cell[3] - cell[1]);
                let mut quarter = match (turn, crop.is_some_and(|c| c.text_up)) {
                    (false, _) => 0,
                    (true, true) => 3,
                    (true, false) => 1,
                };
                if flip {
                    quarter = (quarter + 2) % 4;
                }
                let s = if turn {
                    fit_scale((h, w), (cell[2] - cell[0], cell[3] - cell[1]))
                } else {
                    fit_scale((w, h), (cell[2] - cell[0], cell[3] - cell[1]))
                };
                sheets.push(Sheet { size, placed: vec![place_crop(p, rect, cell, s, quarter)], ..Sheet::default() });
            }
        }
        Layout::Multiple { cols, rows, per_sheet, order, border, auto_rotate } => {
            let cells = cols.checked_mul(rows).filter(|&n| n > 0 && n <= 256);
            let Some(cells) = cells else {
                return Err(PrintError::Invalid("invalid number of pages per sheet".into()));
            };
            // The grid can have more cells than the pages asked for on each sheet; the rest stay blank.
            let per_sheet = per_sheet.min(cells);
            if per_sheet == 0 {
                return Err(PrintError::Invalid("invalid number of pages per sheet".into()));
            }
            // A grid with more columns than rows wants a landscape sheet, and the reverse.
            let first = pages.first().and_then(|&p| sizes.get(p)).copied().ok_or(PrintError::NoPages)?;
            let page_landscape = first.0 > first.1;
            let (gc, gr, sheet_landscape) = if cols == rows {
                (cols, rows, page_landscape)
            } else if settings.orientation == Orientation::Landscape || (settings.orientation == Orientation::Auto && cols < rows) {
                // 2 per sheet (1 × 2 on portrait) prints side by side on landscape paper.
                (rows, cols, true)
            } else {
                (cols, rows, false)
            };
            let size = oriented(paper, settings.orientation, sheet_landscape);
            let gap = 6.0;
            let (aw, ah) = (size.0 - 2.0 * MARGIN, size.1 - 2.0 * MARGIN);
            let (cw, ch) = ((aw - gap * (gc - 1) as f64) / gc as f64, (ah - gap * (gr - 1) as f64) / gr as f64);
            if cw <= 0.0 || ch <= 0.0 {
                return Err(PrintError::Invalid("the page grid does not fit on the paper".into()));
            }
            let sheet_count = pages.len().div_ceil(per_sheet);
            for sheet_index in 0..sheet_count {
                let mut sheet = Sheet { size, ..Sheet::default() };
                if order == PageOrder::CutStack {
                    // Marks align across every sheet, including unoccupied cells. Cut in the
                    // middle of each gutter; marks stay in the printable area, clear of cells.
                    for col in 1..gc {
                        let x = MARGIN + col as f64 * (cw + gap) - gap / 2.0;
                        sheet.lines.extend([[x, MARGIN, x, MARGIN + 8.0], [x, size.1 - MARGIN - 8.0, x, size.1 - MARGIN]]);
                    }
                    for row in 1..gr {
                        let y = size.1 - MARGIN - row as f64 * (ch + gap) + gap / 2.0;
                        sheet.lines.extend([[MARGIN, y, MARGIN + 8.0, y], [size.0 - MARGIN - 8.0, y, size.0 - MARGIN, y]]);
                    }
                }
                for k in 0..per_sheet {
                    // A cell's pile gets consecutive pages. Missing pages leave blank cells,
                    // rather than shifting later piles left on the final sheets.
                    let index = if order == PageOrder::CutStack {
                        k.checked_mul(sheet_count).and_then(|n| n.checked_add(sheet_index))
                    } else {
                        sheet_index.checked_mul(per_sheet).and_then(|n| n.checked_add(k))
                    };
                    let Some(p) = index.and_then(|i| pages.get(i)).copied() else { continue };

                    let (col, row) = match order {
                        PageOrder::Horizontal | PageOrder::CutStack => (k % gc, k / gc),
                        PageOrder::HorizontalReversed => (gc - 1 - k % gc, k / gc),
                        PageOrder::Vertical => (k / gr, k % gr),
                        PageOrder::VerticalReversed => (gc - 1 - k / gr, k % gr),
                    };
                    let x0 = MARGIN + col as f64 * (cw + gap);
                    let y1 = size.1 - MARGIN - row as f64 * (ch + gap);
                    let cell = [x0, y1 - ch, x0 + cw, y1];
                    let d = sizes.get(p).copied().ok_or_else(|| PrintError::Invalid("page size is missing".into()))?;
                    if !(d.0.is_finite() && d.1.is_finite() && d.0 > 0.0 && d.1 > 0.0) {
                        return Err(PrintError::Invalid("invalid page size".into()));
                    }
                    let rotate = auto_rotate && (d.0 > d.1) != (cw > ch) && (d.0 - d.1).abs() > 1.0;
                    let s = fit_scale(if rotate { (d.1, d.0) } else { d }, (cw, ch));
                    if !s.is_finite() || s <= 0.0 {
                        return Err(PrintError::Invalid("page size cannot be scaled to the grid".into()));
                    }
                    let pl = place_in(p, d, cell, s, rotate);
                    if border {
                        let b = pl.matrix.bbox(pl.clip);
                        sheet.borders.push(b);
                    }
                    sheet.placed.push(pl);
                }
                sheets.push(sheet);
            }
        }
        Layout::Booklet { subset, binding } => {
            // Pad to a multiple of 4 with blank pages (None).
            let n4 = pages.len().div_ceil(4) * 4;
            let at = |i: usize| pages.get(i).copied();
            let size = oriented(paper, Orientation::Landscape, true);
            let half = (size.0 - 2.0 * MARGIN) / 2.0;
            let cells = [[MARGIN, MARGIN, MARGIN + half, size.1 - MARGIN], [MARGIN + half, MARGIN, size.0 - MARGIN, size.1 - MARGIN]];
            for k in 0..n4 / 4 {
                let front = (at(n4 - 1 - 2 * k), at(2 * k));
                let back = (at(2 * k + 1), at(n4 - 2 - 2 * k));
                let mut sides = Vec::new();
                if subset != BookletSubset::BackOnly {
                    sides.push(front);
                }
                if subset != BookletSubset::FrontOnly {
                    sides.push(back);
                }
                for (l, r) in sides {
                    let (l, r) = if binding == Binding::Right { (r, l) } else { (l, r) };
                    let mut sheet = Sheet { size, ..Sheet::default() };
                    for (p, cell) in [(l, cells[0]), (r, cells[1])] {
                        if let Some(p) = p {
                            let d = sizes[p];
                            let s = fit_scale(d, (cell[2] - cell[0], cell[3] - cell[1]));
                            sheet.placed.push(place_in(p, d, cell, s, false));
                        }
                    }
                    sheets.push(sheet);
                }
            }
        }
        Layout::Poster { scale, overlap, cut_marks } => {
            if !(scale.is_finite() && scale > 0.0 && overlap.is_finite() && overlap >= 0.0) {
                return Err(PrintError::Invalid("invalid tile scale or overlap".into()));
            }
            let s = scale / 100.0;
            for &p in &pages {
                let d = sizes[p];
                let size = oriented(paper, settings.orientation, d.0 > d.1);
                // Each tile shows `step` of the scaled page plus `overlap` shared with the next.
                let (tw, th) = (size.0 - 2.0 * MARGIN, size.1 - 2.0 * MARGIN);
                if overlap * 2.0 >= tw.min(th) {
                    return Err(PrintError::Invalid("the overlap is larger than the tile".into()));
                }
                let (sw, sh) = (d.0 * s, d.1 * s);
                let (stepx, stepy) = (tw - overlap, th - overlap);
                let nx = (((sw - overlap) / stepx).ceil() as usize).max(1);
                let ny = (((sh - overlap) / stepy).ceil() as usize).max(1);
                // The counts saturate at usize::MAX for a huge scale, so their product can overflow.
                if nx.checked_mul(ny).is_none_or(|n| n > 1024) {
                    return Err(PrintError::Invalid("the poster would need more than 1024 sheets".into()));
                }
                // Tiles from the top-left, row by row.
                for ty in 0..ny {
                    for tx in 0..nx {
                        // The window into the scaled page (scaled display space, y up).
                        let wx0 = tx as f64 * stepx;
                        let wy1 = sh - ty as f64 * stepy;
                        let matrix = Matrix([s, 0.0, 0.0, s, MARGIN - wx0, size.1 - MARGIN - wy1]);
                        let clip = [wx0 / s, (wy1 - th) / s, (wx0 + tw) / s, wy1 / s];
                        let mut sheet = Sheet { size, placed: vec![Placement { page: p, matrix, clip }], ..Sheet::default() };
                        if cut_marks {
                            let (x0, y0, x1, y1) = (MARGIN, MARGIN, size.0 - MARGIN, size.1 - MARGIN);
                            for (x, y) in [(x0, y0), (x1, y0), (x0, y1), (x1, y1)] {
                                let dx = if x == x0 { -1.0 } else { 1.0 };
                                let dy = if y == y0 { -1.0 } else { 1.0 };
                                sheet.lines.push([x + dx * 2.0, y, x + dx * (MARGIN - 2.0), y]);
                                sheet.lines.push([x, y + dy * 2.0, x, y + dy * (MARGIN - 2.0)]);
                            }
                        }
                        sheets.push(sheet);
                    }
                }
            }
        }
    }
    Ok(sheets)
}

// Model-derived transforms and clipping bounds need their full PDF decimal precision.
fn geometry_number(value: f64) -> String {
    let mut bytes = Vec::new();
    pdfcraft_cos::serialize(&Object::Real(value), &mut bytes);
    String::from_utf8_lossy(&bytes).into_owned()
}

fn n(v: f64) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

fn decoded_contents(doc: &Document, page: &Dict) -> Vec<u8> {
    let list: Vec<Object> = match page.get(b"Contents") {
        None => Vec::new(),
        Some(c) => match &*doc.resolve(c) {
            Object::Array(a) => a.clone(),
            _ => vec![c.clone()],
        },
    };
    let mut out = Vec::new();
    for o in list {
        if let Object::Stream(s) = &*doc.resolve(&o)
            && let Ok(d) = s.decoded()
        {
            out.extend_from_slice(&d);
            out.push(b'\n');
        }
    }
    out
}

/// Reduce `doc`'s pages to what [`impose`] draws of them under `content`: the annotations that
/// don't print are dropped, and with [`Content::FormFieldsOnly`] the page content too. For
/// printing pages as pictures, where a renderer draws each page as it stands.
pub fn keep_printed(doc: &mut Document, content: Content) {
    for page in pdfcraft_model::pages(doc) {
        let annots = page.dict.get(b"Annots").and_then(|a| doc.resolve(a).as_array().cloned()).unwrap_or_default();
        let kept: Vec<Object> = annots.into_iter().filter(|a| doc.resolve(a).as_dict().is_some_and(|d| prints(doc, d, content))).collect();
        // Fails only when the page object isn't a dictionary, and such a page draws nothing.
        let _ = doc.update_dict(page.obj, |d| {
            if content == Content::FormFieldsOnly {
                d.remove(b"Contents");
            }
            if d.contains(b"Annots") {
                d.set(b"Annots".to_vec(), Object::Array(kept));
            }
        });
    }
}

/// Does annotation `d` print under `content`?
fn prints(doc: &Document, d: &Dict, content: Content) -> bool {
    const HIDDEN: i64 = 2;
    const PRINT: i64 = 4;
    let flags = d.get(b"F").and_then(|f| doc.resolve(f).as_int()).unwrap_or(0);
    if flags & PRINT == 0 || flags & HIDDEN != 0 {
        return false;
    }
    let subtype = d.name(b"Subtype").unwrap_or(b"");
    match subtype {
        b"Widget" => true,
        b"Link" | b"Popup" => false,
        b"Stamp" => content != Content::Document && content != Content::FormFieldsOnly,
        _ => content == Content::DocumentAndMarkups,
    }
}

/// The normal appearance stream to draw for an annotation (honouring `/AS`).
fn appearance(doc: &Document, d: &Dict) -> Option<ObjRef> {
    let ap = doc.resolve(d.get(b"AP")?);
    let normal = ap.as_dict()?.get(b"N")?.clone();
    match &*doc.resolve(&normal) {
        Object::Stream(_) => normal.as_ref(),
        Object::Dict(states) => {
            let state = d.name(b"AS")?;
            states.get(state)?.as_ref()
        }
        _ => None,
    }
}

/// Algorithm 8.1: the matrix placing an appearance form (BBox × Matrix) onto the annotation rect.
fn appearance_matrix(doc: &Document, form: &Dict, rect: [f64; 4]) -> Option<Matrix> {
    let nums =
        |k: &[u8]| -> Option<Vec<f64>> { Some(doc.resolve(form.get(k)?).as_array()?.iter().filter_map(|x| doc.resolve(x).as_f64()).collect()) };
    let bbox = nums(b"BBox").filter(|b| b.len() == 4)?;
    let m = nums(b"Matrix").filter(|m| m.len() == 6).map(|m| Matrix([m[0], m[1], m[2], m[3], m[4], m[5]])).unwrap_or_default();
    let b = m.bbox([bbox[0], bbox[1], bbox[2], bbox[3]]);
    let (bw, bh) = (b[2] - b[0], b[3] - b[1]);
    if bw <= 0.0 || bh <= 0.0 {
        return None;
    }
    let a = Matrix([
        (rect[2] - rect[0]) / bw,
        0.0,
        0.0,
        (rect[3] - rect[1]) / bh,
        rect[0] - b[0] * (rect[2] - rect[0]) / bw,
        rect[1] - b[1] * (rect[3] - rect[1]) / bh,
    ]);
    // The Do that draws the form applies its /Matrix itself, so only the placement goes here.
    Some(a)
}

/// A Form XObject holding page `index` as it prints (user space; BBox = crop box).
fn page_form(
    doc: &mut Document,
    index: usize,
    content: Content,
    promoted: &mut std::collections::HashMap<ObjRef, ObjRef>,
) -> Result<ObjRef, PrintError> {
    let page = pdfcraft_model::pages(doc).swap_remove(index);
    let crop = page.crop(doc);
    let mut res = page.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_default();
    let mut body = if content == Content::FormFieldsOnly { Vec::new() } else { decoded_contents(doc, &page.dict) };
    // Wrap the page content so a state it leaves behind can't affect the annotations.
    if !body.is_empty() {
        let mut wrapped = b"q\n".to_vec();
        wrapped.extend_from_slice(&body);
        wrapped.extend_from_slice(b"\nQ\n");
        body = wrapped;
    }
    let annots = page.dict.get(b"Annots").and_then(|a| doc.resolve(a).as_array().cloned()).unwrap_or_default();
    let mut xobjects = res.get(b"XObject").and_then(|x| doc.resolve(x).as_dict().cloned()).unwrap_or_default();
    let mut k = 0;
    for a in annots {
        let Some(d) = doc.resolve(&a).as_dict().cloned() else { continue };
        if !prints(doc, &d, content) {
            continue;
        }
        let rect: Vec<f64> =
            d.get(b"Rect").and_then(|r| doc.resolve(r).as_array().map(|a| a.iter().filter_map(Object::as_f64).collect())).unwrap_or_default();
        if rect.len() != 4 {
            continue;
        }
        let rect = [rect[0].min(rect[2]), rect[1].min(rect[3]), rect[0].max(rect[2]), rect[1].max(rect[3])];
        let Some(ap) = appearance(doc, &d) else { continue };
        let form = doc.get(ap).as_dict().cloned().unwrap_or_default();
        let Some(m) = appearance_matrix(doc, &form, rect) else { continue };
        let ap = if !form.contains(b"Subtype")
            && let Object::Stream(stream) = &*doc.get(ap)
        {
            // Some existing annotation appearances omit Subtype, but Do dispatch needs a Form XObject.
            // Keep the original intact: other document objects can share this appearance.
            if let Some(&cached) = promoted.get(&ap) {
                cached
            } else {
                let mut stream = stream.clone();
                stream.dict.set(b"Subtype".to_vec(), Object::name("Form"));
                let copy = doc.add(Object::Stream(stream));
                promoted.insert(ap, copy);
                copy
            }
        } else {
            ap
        };
        let name = format!("PCPrA{k}");
        k += 1;
        let [a0, b0, c0, d0, e0, f0] = m.0;
        body.extend(format!("q {} {} {} {} {} {} cm /{name} Do Q\n", n(a0), n(b0), n(c0), n(d0), n(e0), n(f0)).bytes());
        xobjects.set(name.into_bytes(), Object::Ref(ap));
    }
    if !xobjects.is_empty() {
        res.set(b"XObject".to_vec(), Object::Dict(xobjects));
    }
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Form"));
    d.set(b"BBox".to_vec(), Object::Array(crop.iter().map(|v| Object::Real(*v)).collect()));
    d.set(b"Resources".to_vec(), Object::Dict(res));
    if let Some(g) = page.dict.get(b"Group") {
        d.set(b"Group".to_vec(), g.clone());
    }
    Ok(doc.add(Object::Stream(Stream::flate(d, &body))))
}

/// Build the print-ready PDF for `settings`. Encryption is not carried over (the file goes to a
/// printer or is the user's own copy); callers must check the print permission first.
pub fn impose(src: &Document, settings: &Settings) -> Result<Vec<u8>, PrintError> {
    let pages = pdfcraft_model::pages(src);
    let sizes: Vec<(f64, f64)> = pages.iter().map(|p| p.display_size(src)).collect();
    let sheets = layout(&sizes, settings)?;
    let mut doc = src.clone();
    // One form per source page used (a page may appear on several sheets: posters).
    let mut forms: std::collections::HashMap<usize, ObjRef> = std::collections::HashMap::new();
    let mut promoted = std::collections::HashMap::new();
    let views: Vec<[f64; 6]> = pages.iter().map(|p| p.view_matrix(src)).collect();
    let pages_root = doc.add(Object::Null);
    let mut kids = Vec::new();
    for sheet in &sheets {
        let mut c = String::new();
        let mut xo = Dict::new();
        for (i, pl) in sheet.placed.iter().enumerate() {
            let form = match forms.get(&pl.page) {
                Some(f) => *f,
                None => {
                    let f = page_form(&mut doc, pl.page, settings.content, &mut promoted)?;
                    forms.insert(pl.page, f);
                    f
                }
            };
            let name = format!("P{i}");
            xo.set(name.clone().into_bytes(), Object::Ref(form));
            // Display space → sheet, then clip to the visible part, then user → display.
            let [a, b, cc, d, e, f] = pl.matrix.0;
            let to_display = Matrix(views[pl.page]).invert().unwrap_or_default();
            let [ua, ub, uc, ud, ue, uf] = to_display.0;
            let [x0, y0, x1, y1] = pl.clip;
            c.push_str(&format!(
                "q {} {} {} {} {} {} cm {} {} {} {} re W n {} {} {} {} {} {} cm /{name} Do Q\n",
                geometry_number(a),
                geometry_number(b),
                geometry_number(cc),
                geometry_number(d),
                geometry_number(e),
                geometry_number(f),
                geometry_number(x0),
                geometry_number(y0),
                geometry_number(x1 - x0),
                geometry_number(y1 - y0),
                geometry_number(ua),
                geometry_number(ub),
                geometry_number(uc),
                geometry_number(ud),
                geometry_number(ue),
                geometry_number(uf)
            ));
        }
        if !sheet.borders.is_empty() || !sheet.lines.is_empty() {
            c.push_str("q 0 G 0.5 w\n");
            for r in &sheet.borders {
                c.push_str(&format!("{} {} {} {} re S\n", n(r[0]), n(r[1]), n(r[2] - r[0]), n(r[3] - r[1])));
            }
            for l in &sheet.lines {
                c.push_str(&format!("{} {} m {} {} l S\n", n(l[0]), n(l[1]), n(l[2]), n(l[3])));
            }
            c.push_str("Q\n");
        }
        let contents = doc.add(Object::Stream(Stream::flate(Dict::new(), c.as_bytes())));
        let mut res = Dict::new();
        res.set(b"XObject".to_vec(), Object::Dict(xo));
        let mut p = Dict::new();
        p.set(b"Type".to_vec(), Object::name("Page"));
        p.set(b"Parent".to_vec(), Object::Ref(pages_root));
        p.set(b"MediaBox".to_vec(), Object::Array([0.0, 0.0, sheet.size.0, sheet.size.1].iter().map(|v| Object::Real(*v)).collect()));
        p.set(b"Resources".to_vec(), Object::Dict(res));
        p.set(b"Contents".to_vec(), Object::Ref(contents));
        kids.push(Object::Ref(doc.add(Object::Dict(p))));
    }
    let mut tree = Dict::new();
    tree.set(b"Type".to_vec(), Object::name("Pages"));
    tree.set(b"Count".to_vec(), Object::Int(kids.len() as i64));
    tree.set(b"Kids".to_vec(), Object::Array(kids));
    doc.set(pages_root, Object::Dict(tree));
    let mut cat = Dict::new();
    cat.set(b"Type".to_vec(), Object::name("Catalog"));
    cat.set(b"Pages".to_vec(), Object::Ref(pages_root));
    // Keep the layers' configuration, or a layer that is off in the document would print: the
    // pages' marked content still refers to its groups, and without this nothing says they are off.
    let layers = src.trailer().get(b"Root").and_then(|r| src.resolve(r).as_dict().and_then(|root| root.get(b"OCProperties").cloned()));
    if let Some(layers) = layers {
        cat.set(b"OCProperties".to_vec(), layers);
    }
    let root = doc.add(Object::Dict(cat));
    doc.trailer_mut().set(b"Root".to_vec(), Object::Ref(root));
    doc.trailer_mut().remove(b"Info");
    if doc.trailer().contains(b"Encrypt") {
        doc.remove_encryption();
    }
    Ok(write_full(&doc, &SaveOptions::default())?)
}

/// Which source pages land on each sheet (for previews and the sheet count).
pub fn sheet_pages(sheets: &[Sheet]) -> Vec<Vec<usize>> {
    sheets.iter().map(|s| s.placed.iter().map(|p| p.page).collect()).collect()
}
