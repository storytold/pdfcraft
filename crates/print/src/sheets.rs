use super::error::PrintError;
use super::imposition::decoded_contents;
use super::settings::{Binding, BookletSubset, Layout, Orientation, PAPERS, PageOrder, Settings, SizeMode};
use super::{DecodeBudget, Dict, Document, Matrix, Object};

/// Whether most of page `index`'s text runs bottom-to-top as the page displays (a label laid
/// sideways on a letter or A4 sheet), counting each text-showing operator once; form XObjects
/// are followed a few levels deep.
pub fn text_reads_up(doc: &Document, index: usize) -> bool {
    let Some(page) = pdfcraft_model::pages(doc).into_iter().nth(index) else { return false };
    let to_display = Matrix(page.view_matrix(doc)).invert().unwrap_or_default();
    let res = page.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_default();
    let mut score = 0i64;
    let mut budget = DecodeBudget::page();
    // Content past the budget is left out: the answer is a guess from the text read so far.
    let data = decoded_contents(doc, &page.dict, &mut budget).unwrap_or_default();
    text_direction(doc, &data, &res, to_display, 0, &mut score, &mut budget);
    score > 0
}

pub(crate) fn text_direction(doc: &Document, data: &[u8], res: &Dict, base: Matrix, depth: usize, score: &mut i64, budget: &mut DecodeBudget) {
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
                if !budget.enter_form() {
                    continue;
                }
                if let Ok(body) = budget.decode(form) {
                    text_direction(doc, &body, &inner, m.then(&ctm), depth + 1, score, budget);
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
            sheets.extend(layout_labels(sizes, &pages, paper, settings.orientation, settings, flip));
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

fn layout_labels(sizes: &[(f64, f64)], pages: &[usize], paper: (f64, f64), orientation: Orientation, settings: &Settings, flip: bool) -> Vec<Sheet> {
    let size = oriented(paper, orientation, false);
    let cell = [LABEL_MARGIN, LABEL_MARGIN, size.0 - LABEL_MARGIN, size.1 - LABEL_MARGIN];
    pages
        .iter()
        .map(|&p| {
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
            Sheet { size, placed: vec![place_crop(p, rect, cell, s, quarter)], ..Sheet::default() }
        })
        .collect()
}
