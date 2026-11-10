//! Comment summaries that show the pages with their comments, as two of Acrobat's Summarize
//! Options layouts do:
//! - [`PageLayout::LinesOnSinglePages`]: the page, scaled down, on the left half of a landscape
//!   sheet, its comments in boxes on the right, and a connector line from each comment to its box.
//!   Comments that don't fit beside the page continue on another sheet showing the page again.
//! - [`PageLayout::NumbersOnSeparatePages`]: the page on a sheet of its own with each comment's
//!   number on it, then its comments, numbered, on the sheet after it. Page and comment sheets
//!   take turns, so a two-page view shows each page beside its comments: a page without comments
//!   is followed by a sheet saying so, and when its comments need more than one sheet the page
//!   is shown again beside each further one. The summary opens in a two-page view.
//!
//! On each page's first sheet its comments (and their replies) are annotations again, placed
//! where the page shows them, so the summary's comment list lists them and goes to them.

use super::imposition::{appearance, appearance_matrix, form_xobject, page_form, page_form_where, prints};
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
    /// Its number on the page, drawn on the page by [`PageLayout::NumbersOnSeparatePages`].
    pub number: usize,
    /// Its position in the page's `/Annots`, when known: a comment that doesn't print (hidden,
    /// or without the Print flag) isn't drawn on the page, so nothing points at it. A comment
    /// with one is put on the page's first sheet as an annotation, with its replies.
    pub index: Option<usize>,
    /// Where it is on its page, in user space (`[x0 y0 x1 y1]`).
    pub rect: [f64; 4],
    /// Its colour (`/C`), which its box and connector take; grey without one.
    pub color: Option<[f64; 3]>,
    /// What its box says: `(indent level, text)`, the heading first and replies indented under
    /// it. A line longer than the box is wrapped.
    pub lines: Vec<(usize, String)>,
}

/// How a summary shows the pages with their comments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageLayout {
    /// "Document and comments with connector lines on single pages".
    LinesOnSinglePages,
    /// "Document and comments with sequence numbers on separate pages".
    NumbersOnSeparatePages,
}

/// What [`summary_with_pages`] makes.
#[derive(Clone, Debug, PartialEq)]
pub struct SummaryOptions<'a> {
    /// The title at the top of every sheet and in the document's properties.
    pub title: &'a str,
    pub layout: PageLayout,
    /// The paper (width, height in points, either way round): each sheet is turned to suit what
    /// it holds. A size outside 2 in to 200 in on a side is taken as US Letter.
    pub paper: (f64, f64),
    /// Pages without comments get their sheet too.
    pub pages_without_comments: bool,
}

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
/// The sheets one summary may have: a page makes one or two, and each comment's box at most a
/// few, so this is only hit by an absurd document, which gets an error rather than a huge file.
const MAX_SHEETS: usize = 100_000;
/// The colour of a comment without one.
const GREY: [f64; 3] = [0.45, 0.45, 0.45];

/// A summary of `comments` with their pages, as a new unencrypted document whose pages are the
/// sheets. The pages come in the order of their first comment, each page's comments in the order
/// given; pages without comments, when asked for, come in document order: among the others when
/// those are in document order too, else after them. Comments on pages `src` doesn't have are
/// left out. Without comments the summary is one sheet saying the document has none, whether or
/// not pages without comments are asked for.
///
/// The pages are drawn as they print ([`Content::DocumentAndMarkups`]): with the comments that
/// have the Print flag, without pop-ups and links. A comment that doesn't print still gets its
/// box, without a connector or number pointing at a place where nothing is drawn. On a page's
/// first sheet the listed comments are annotations rather than part of the drawing.
pub fn summary_with_pages(src: &Document, comments: &[SummaryComment], opts: &SummaryOptions<'_>) -> Result<Document, PrintError> {
    let pages = pdfcraft_model::pages(src);
    let mut doc = src.clone();
    if doc.trailer().contains(b"Encrypt") {
        doc.remove_encryption();
    }
    // A new file, not a version of the source: its own identifier.
    doc.require_full_save_with_new_id();
    let (pw, ph) = opts.paper;
    let usable = |v: f64| v.is_finite() && (144.0..=14_400.0).contains(&v);
    let paper = if usable(pw) && usable(ph) { (pw.min(ph), pw.max(ph)) } else { (612.0, 792.0) };
    let title = drawn(opts.title).replace(['\n', '\r'], " ");
    // Each comment's text as it is laid out and drawn.
    let texts: Vec<Vec<(usize, String)>> = comments.iter().map(|c| c.lines.iter().map(|(level, t)| (*level, drawn(t))).collect()).collect();
    let mut all_text: Vec<&str> = vec![&title, NO_COMMENTS, NO_PAGE_COMMENTS, "Page", "(continued)", "Comments on page", ELLIPSIS];
    all_text.extend(texts.iter().flatten().map(|(_, t)| t.as_str()));
    let font = SummaryFont::choose(&mut doc, &all_text)?;

    // Each page's comments, in the order given, and the pages in the order of their first.
    let mut by_page: std::collections::HashMap<usize, Vec<Listed<'_>>> = std::collections::HashMap::new();
    let mut order: Vec<usize> = Vec::new();
    for (c, lines) in comments.iter().zip(&texts).filter(|(c, _)| c.page < pages.len()) {
        if !by_page.contains_key(&c.page) {
            order.push(c.page);
        }
        by_page.entry(c.page).or_default().push((c, lines));
    }
    // A document without comments says so, rather than showing its pages beside nothing.
    if opts.pages_without_comments && !order.is_empty() {
        if order.is_sorted() {
            order = (0..pages.len()).collect();
        } else {
            order.extend((0..pages.len()).filter(|p| !by_page.contains_key(p)));
        }
    }

    let mut out = Out { font, sheets: Vec::new(), title };
    let mut promoted = std::collections::HashMap::new();
    let mut shown: std::collections::HashMap<usize, ShownPage> = std::collections::HashMap::new();
    for page in order {
        let list = by_page.get(&page).map_or(&[][..], Vec::as_slice);
        let annots = pages.get(page).and_then(|p| p.dict.get(b"Annots")).and_then(|a| doc.resolve(a).as_array().cloned()).unwrap_or_default();
        let printed = |c: &SummaryComment| {
            c.index.is_none_or(|i| {
                annots.get(i).and_then(|a| doc.resolve(a).as_dict().map(|d| prints(&doc, d, Content::DocumentAndMarkups))).unwrap_or(false)
            })
        };
        let items: Vec<Item> = list
            .iter()
            .map(|(c, lines)| Item {
                lines,
                color: c.color.filter(|c| c.iter().all(|v| v.is_finite())).map_or(GREY, |c| c.map(|v| v.clamp(0.0, 1.0))),
                anchor: printed(c).then_some(Anchor {
                    rect: c.rect,
                    number: c.number,
                    upright: c.index.and_then(|i| annots.get(i)).and_then(|a| doc.resolve(a).as_dict().map(|d| no_rotate(&doc, d))).unwrap_or(false),
                }),
            })
            .collect();
        // The comments that become annotations on the page's first sheet, drawn there by
        // themselves rather than as part of the page: those the page draws, as it prints, with
        // their threads. One that doesn't print stays a box with nothing pointing at it.
        let copied = with_replies(&doc, &annots, list.iter().filter(|(c, _)| printed(c)).filter_map(|(c, _)| c.index));
        let bare = page_form_where(&mut doc, &pages, page, Content::DocumentAndMarkups, &|i| !copied.contains(&i), &mut promoted)?;
        shown.insert(page, ShownPage { bare, full: copied.is_empty().then_some(bare), annots, copied });
        match opts.layout {
            PageLayout::LinesOnSinglePages => out.lines_on_single_pages(src, &pages, page, &items, paper)?,
            PageLayout::NumbersOnSeparatePages => out.numbers_on_separate_pages(src, &pages, page, &items, paper)?,
        }
    }
    let Out { font, sheets, title } = out;

    let pages_root = doc.add(Object::Null);
    let mut fonts = Dict::new();
    fonts.set(b"F1".to_vec(), Object::Ref(font.font));
    let mut kids = Vec::new();
    let mut add = |doc: &mut Document, page: ObjRef, size: (f64, f64), content: Vec<u8>, xobjects: Dict, annots: Vec<Object>| {
        let mut res = Dict::new();
        res.set(b"Font".to_vec(), Object::Dict(fonts.clone()));
        if !xobjects.is_empty() {
            res.set(b"XObject".to_vec(), Object::Dict(xobjects));
        }
        let contents = doc.add(Object::Stream(Stream::flate(Dict::new(), &content)));
        let mut p = Dict::new();
        p.set(b"Type".to_vec(), Object::name("Page"));
        p.set(b"Parent".to_vec(), Object::Ref(pages_root));
        p.set(b"MediaBox".to_vec(), Object::Array([0.0, 0.0, size.0, size.1].iter().map(|v| Object::Real(*v)).collect()));
        p.set(b"Resources".to_vec(), Object::Dict(res));
        p.set(b"Contents".to_vec(), Object::Ref(contents));
        if !annots.is_empty() {
            p.set(b"Annots".to_vec(), Object::Array(annots));
        }
        doc.set(page, Object::Dict(p));
        kids.push(Object::Ref(page));
    };
    if sheets.is_empty() {
        let size = paper;
        let mut c = Vec::new();
        let fitted = fit(&font, &title, TITLE_FONT, size.0 - 2.0 * MARGIN);
        text(&mut c, &font, &[0.0; 3], TITLE_FONT, MARGIN, size.1 - MARGIN - TITLE_FONT, &fitted)?;
        text(&mut c, &font, &[0.0; 3], FONT + 1.5, MARGIN, size.1 - MARGIN - HEADER - LEADING * 2.0, NO_COMMENTS)?;
        let page = doc.add(Object::Null);
        add(&mut doc, page, size, c, Dict::new(), Vec::new());
    }
    let opens_in = match opts.layout {
        PageLayout::LinesOnSinglePages => "OneColumn",
        PageLayout::NumbersOnSeparatePages => "TwoPageLeft",
    };
    let opens_page_sheets = !sheets.is_empty();
    for sheet in sheets {
        let page = doc.add(Object::Null);
        let mut xobjects = Dict::new();
        let mut annots = Vec::new();
        if let Some(on) = sheet.page
            && let Some(p) = shown.get_mut(&on.page)
        {
            // The page's first sheet carries its comments as annotations, so its drawing leaves
            // them out; a further sheet showing the page again draws them.
            let form = match (on.first, p.full) {
                (true, _) => p.bare,
                (false, Some(full)) => full,
                (false, None) => {
                    let full = page_form(&mut doc, &pages, on.page, Content::DocumentAndMarkups, &mut promoted)?;
                    p.full = Some(full);
                    full
                }
            };
            xobjects.set(b"P0".to_vec(), Object::Ref(form));
            if on.first {
                annots = copy_annotations(&mut doc, &p.annots, &p.copied, &on.m, page, &mut promoted);
            }
        }
        add(&mut doc, page, sheet.size, sheet.content, xobjects, annots);
    }

    let count = i64::try_from(kids.len()).map_err(|_| too_many())?;
    let mut tree = Dict::new();
    tree.set(b"Type".to_vec(), Object::name("Pages"));
    tree.set(b"Count".to_vec(), Object::Int(count));
    let first_sheet = kids.first().cloned();
    tree.set(b"Kids".to_vec(), Object::Array(kids));
    doc.set(pages_root, Object::Dict(tree));
    let mut cat = Dict::new();
    cat.set(b"Type".to_vec(), Object::name("Catalog"));
    cat.set(b"Pages".to_vec(), Object::Ref(pages_root));
    // The summary opens with each sheet whole in view; sequence numbers on separate pages in a
    // two-page view, each page beside its comments.
    if opens_page_sheets && let Some(first) = first_sheet {
        cat.set(b"PageLayout".to_vec(), Object::name(opens_in));
        cat.set(b"OpenAction".to_vec(), Object::Array(vec![first, Object::name("Fit")]));
    }
    // A layer that is off in the document stays off on the pages shown (as `impose` keeps it).
    let layers = src.trailer().get(b"Root").and_then(|r| src.resolve(r).as_dict().and_then(|root| root.get(b"OCProperties").cloned()));
    if let Some(layers) = layers {
        cat.set(b"OCProperties".to_vec(), layers);
    }
    let root = doc.add(Object::Dict(cat));
    doc.trailer_mut().set(b"Root".to_vec(), Object::Ref(root));
    let mut info = Dict::new();
    info.set(b"Title".to_vec(), PdfString::text(opts.title));
    info.set(b"Producer".to_vec(), PdfString::text("PdfCraft"));
    let info = doc.add(info);
    doc.trailer_mut().set(b"Info".to_vec(), Object::Ref(info));
    Ok(doc)
}

const NO_COMMENTS: &str = "This document has no comments.";
/// Beside a page without comments, in [`PageLayout::NumbersOnSeparatePages`].
const NO_PAGE_COMMENTS: &str = "No comments on this page.";

/// A page the summary shows: its drawing without the comments its first sheet carries as
/// annotations (`bare`), its drawing with them (`full`, made when a further sheet shows the page
/// again), its `/Annots`, and the positions in them of the comments carried.
struct ShownPage {
    bare: ObjRef,
    full: Option<ObjRef>,
    annots: Vec<Object>,
    copied: std::collections::BTreeSet<usize>,
}

/// Where a sheet shows a page: which, whether it is the page's first sheet, and the matrix from
/// the page's user space to the sheet.
#[derive(Clone, Copy)]
struct OnSheet {
    page: usize,
    first: bool,
    m: Matrix,
}

/// The error for a summary that would pass [`MAX_SHEETS`].
fn too_many() -> PrintError {
    PrintError::Invalid("the comment summary would have too many pages".into())
}

/// A comment and its text as drawn ([`drawn`]).
type Listed<'a> = (&'a SummaryComment, &'a [(usize, String)]);

/// A comment as its box shows it.
struct Item<'a> {
    lines: &'a [(usize, String)],
    color: [f64; 3],
    /// Its rectangle and number, when the page draws it: where its connector or number goes.
    anchor: Option<Anchor>,
}

/// A comment's box (or the part of it on one sheet).
struct SummaryBox {
    top: f64,
    height: f64,
    rows: Vec<(usize, String)>,
    color: [f64; 3],
    /// On the part with its heading: the comment's rectangle and number.
    anchor: Option<Anchor>,
}

/// One finished sheet. A sheet showing a page draws it as `/P0`.
struct Sheet {
    size: (f64, f64),
    content: Vec<u8>,
    page: Option<OnSheet>,
}

/// The sheets made so far, and what they are set with.
struct Out {
    font: SummaryFont,
    sheets: Vec<Sheet>,
    title: String,
}

impl Out {
    /// Adds a sheet, or fails once the summary would pass [`MAX_SHEETS`].
    fn push(&mut self, sheet: Sheet) -> Result<(), PrintError> {
        if self.sheets.len() >= MAX_SHEETS {
            return Err(too_many());
        }
        self.sheets.push(sheet);
        Ok(())
    }

    /// The title on the left of the header band and `label` on the right, over a rule.
    fn header(&self, c: &mut Vec<u8>, size: (f64, f64), label: &str) -> Result<(), PrintError> {
        let base = size.1 - MARGIN - TITLE_FONT;
        let label_x = size.0 - MARGIN - self.font.width(label, TITLE_FONT);
        // A long document name is cut short before the label rather than run under it.
        let fitted = fit(&self.font, &self.title, TITLE_FONT, label_x - MARGIN - GAP);
        text(c, &self.font, &[0.0; 3], TITLE_FONT, MARGIN, base, &fitted)?;
        text(c, &self.font, &[0.0; 3], TITLE_FONT, label_x, base, label)?;
        let rule = size.1 - MARGIN - HEADER / 2.0 - 4.0;
        let _ = writeln!(c, "q 0.75 G 0.5 w {} {} m {} {} l S Q", n(MARGIN), n(rule), n(size.0 - MARGIN), n(rule));
        Ok(())
    }

    /// Lay `items` out in a column `width` wide, from `top` down to the margin, sheet after
    /// sheet: a box goes whole onto the next sheet unless it is too tall for any sheet, or part of
    /// it is already placed; then it is split where the sheet ends. At least one sheet, empty
    /// when there are no items.
    fn flow(&self, items: &[Item<'_>], width: f64, top: f64) -> Result<Vec<Vec<SummaryBox>>, PrintError> {
        let mut sheets: Vec<Vec<SummaryBox>> = vec![Vec::new()];
        let mut y = top;
        for item in items {
            let mut rows: Vec<(usize, String)> = Vec::new();
            for (level, text) in item.lines {
                // Deep reply chains stop indenting at a quarter of the column.
                let indent = (*level as f64 * INDENT).min(width / 4.0);
                let room = (width - 2.0 * PAD - indent).max(FONT);
                for row in pdfcraft_fonts::wrap_with(text, FONT, room, |s, size| self.font.width(s, size)) {
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
                let fresh = sheets.last().is_none_or(Vec::is_empty);
                let take = if room >= rest.len() {
                    rest.len()
                } else if fresh || !first {
                    room
                } else {
                    0
                };
                if take == 0 {
                    if fresh {
                        // Not even one line fits an empty sheet; can't happen with these sizes.
                        return Err(PrintError::Invalid("the comment summary sheet is too small".into()));
                    }
                    if sheets.len() >= MAX_SHEETS {
                        return Err(too_many());
                    }
                    sheets.push(Vec::new());
                    y = top;
                    continue;
                }
                let (now, later) = rest.split_at(take);
                let height = now.len() as f64 * LEADING + 2.0 * PAD;
                if let Some(sheet) = sheets.last_mut() {
                    sheet.push(SummaryBox { top: y, height, rows: now.to_vec(), color: item.color, anchor: if first { item.anchor } else { None } });
                }
                y -= height + BOX_GAP;
                first = false;
                rest = later;
            }
        }
        Ok(sheets)
    }

    /// Draw `boxes` in the column at `x`, `width` wide.
    fn boxes(&self, c: &mut Vec<u8>, boxes: &[SummaryBox], x: f64, width: f64) -> Result<(), PrintError> {
        for bx in boxes {
            let [r, g, b] = bx.color;
            let tint = bx.color.map(|v| 1.0 - (1.0 - v) * 0.12);
            let _ = writeln!(
                c,
                "q {} {} {} rg {} {} {} RG 1 w {} {} {} {} re B Q",
                n(tint[0]),
                n(tint[1]),
                n(tint[2]),
                n(r),
                n(g),
                n(b),
                n(x),
                n(bx.top - bx.height),
                n(width),
                n(bx.height)
            );
            let mut ty = bx.top - PAD - FONT;
            for (level, row) in &bx.rows {
                let indent = (*level as f64 * INDENT).min(width / 4.0);
                text(c, &self.font, &[0.0; 3], FONT, x + PAD + indent, ty, row)?;
                ty -= LEADING;
            }
        }
        Ok(())
    }

    /// The page beside its comments on landscape sheets, each comment's box tied to its place on
    /// the page by a connector line.
    fn lines_on_single_pages(
        &mut self,
        src: &Document,
        pages: &[pdfcraft_model::Page],
        page: usize,
        items: &[Item<'_>],
        paper: (f64, f64),
    ) -> Result<(), PrintError> {
        let size = (paper.1, paper.0);
        let top = size.1 - MARGIN - HEADER;
        let half = (size.0 - 2.0 * MARGIN - GAP) / 2.0;
        let area = [MARGIN, MARGIN, MARGIN + half, top];
        let (col_x, col_w) = (MARGIN + half + GAP, half);
        let (m, frame) = place(src, pages, page, area);
        for (k, boxes) in self.flow(items, col_w, top)?.into_iter().enumerate() {
            let mut c = Vec::new();
            let label = if k == 0 { format!("Page {}", page + 1) } else { format!("Page {} (continued)", page + 1) };
            self.header(&mut c, size, &label)?;
            draw_page(&mut c, &m, frame);
            self.boxes(&mut c, &boxes, col_x, col_w)?;
            for bx in &boxes {
                let Some(b) = bx.anchor.and_then(|a| a.on_sheet(&m)) else { continue };
                // From the middle of the comment's right edge, toward the column.
                let spot = (b[2].clamp(frame[0], frame[2]), ((b[1] + b[3]) / 2.0).clamp(frame[1], frame[3]));
                let end = (col_x, bx.top - PAD - FONT / 2.0);
                let [r, g, bl] = bx.color;
                let _ = writeln!(
                    c,
                    "q {} {} {} RG {} {} {} rg 0.6 w {} {} m {} {} l S {} f Q",
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
                    circle(spot.0, spot.1, 1.8)
                );
            }
            self.push(Sheet { size, content: c, page: Some(OnSheet { page, first: k == 0, m }) })?;
        }
        Ok(())
    }

    /// The page on a sheet of its own, turned to suit it, with each comment's number on it; then
    /// its comments, numbered, on a portrait sheet after it, or a sheet saying it has none. When
    /// the comments need more sheets the page comes again before each, so page and comment
    /// sheets keep taking turns.
    fn numbers_on_separate_pages(
        &mut self,
        src: &Document,
        pages: &[pdfcraft_model::Page],
        page: usize,
        items: &[Item<'_>],
        paper: (f64, f64),
    ) -> Result<(), PrintError> {
        let shown = pages.get(page).map_or((612.0, 792.0), |p| p.display_size(src));
        let size = if shown.0 > shown.1 { (paper.1, paper.0) } else { paper };
        let (m, frame) = place(src, pages, page, [MARGIN, MARGIN, size.0 - MARGIN, size.1 - MARGIN - HEADER]);
        let width = paper.0 - 2.0 * MARGIN;
        let top = paper.1 - MARGIN - HEADER;
        let comment_sheets = if items.is_empty() { vec![Vec::new()] } else { self.flow(items, width, top)? };
        for (k, boxes) in comment_sheets.into_iter().enumerate() {
            let more = if k == 0 { "" } else { " (continued)" };
            let mut c = Vec::new();
            self.header(&mut c, size, &format!("Page {}{more}", page + 1))?;
            draw_page(&mut c, &m, frame);
            self.numbers(&mut c, items, &m, frame)?;
            self.push(Sheet { size, content: c, page: Some(OnSheet { page, first: k == 0, m }) })?;

            let mut c = Vec::new();
            self.header(&mut c, paper, &format!("Comments on page {}{more}", page + 1))?;
            if boxes.is_empty() {
                text(&mut c, &self.font, &[0.0; 3], FONT + 1.5, MARGIN, top - LEADING, NO_PAGE_COMMENTS)?;
            } else {
                self.boxes(&mut c, &boxes, MARGIN, width)?;
            }
            self.push(Sheet { size: paper, content: c, page: None })?;
        }
        Ok(())
    }

    /// Each comment's number in a coloured circle on the comment's top-right corner, clear of
    /// what it marks, on a page placed with `m` inside `frame`.
    fn numbers(&self, c: &mut Vec<u8>, items: &[Item<'_>], m: &Matrix, frame: [f64; 4]) -> Result<(), PrintError> {
        for item in items {
            let Some(anchor) = item.anchor else { continue };
            let Some(b) = anchor.on_sheet(m) else { continue };
            let number = anchor.number;
            let spot = (b[2].clamp(frame[0], frame[2]), b[3].clamp(frame[1], frame[3]));
            let [r, g, bl] = item.color;
            let label = number.to_string();
            let size = 7.5;
            let radius = (self.font.width(&label, size) / 2.0 + 2.5).max(6.5);
            // A white ring keeps the number apart from a comment of the same colour.
            let _ = writeln!(c, "q {} {} {} rg 1 G 1.2 w {} b Q", n(r), n(g), n(bl), circle(spot.0, spot.1, radius));
            let x = spot.0 - self.font.width(&label, size) / 2.0;
            // Black on a light colour (a yellow note), white on a dark one.
            let ink = if 0.299 * r + 0.587 * g + 0.114 * bl > 0.6 { [0.0; 3] } else { [1.0; 3] };
            text(c, &self.font, &ink, size, x, spot.1 - size * 0.35, &label)?;
        }
        Ok(())
    }
}

/// Draw the page (the sheet's `/P0`) with `m`, framed.
fn draw_page(c: &mut Vec<u8>, m: &Matrix, frame: [f64; 4]) {
    let [a, b, cc, d, e, f] = m.0;
    let _ = writeln!(c, "q {} {} {} {} {} {} cm /P0 Do Q", n(a), n(b), n(cc), n(d), n(e), n(f));
    let _ = writeln!(c, "q 0.6 G 0.5 w {} {} {} {} re S Q", n(frame[0]), n(frame[1]), n(frame[2] - frame[0]), n(frame[3] - frame[1]));
}

/// The positions in `annots` of the comments at `listed`, with their replies and the replies to
/// those (an annotation whose `/IRT` is one of them), so a comment comes with its thread. One pass
/// gathers each annotation's replies; the walk visits each annotation once.
fn with_replies(doc: &Document, annots: &[Object], listed: impl Iterator<Item = usize>) -> std::collections::BTreeSet<usize> {
    let mut replies: std::collections::HashMap<ObjRef, Vec<usize>> = std::collections::HashMap::new();
    for (i, a) in annots.iter().enumerate() {
        if let Some(parent) = doc.resolve(a).as_dict().and_then(|d| d.get(b"IRT").and_then(Object::as_ref)) {
            replies.entry(parent).or_default().push(i);
        }
    }
    let mut set = std::collections::BTreeSet::new();
    let mut todo: Vec<usize> = listed.filter(|i| *i < annots.len()).collect();
    while let Some(i) = todo.pop() {
        if !set.insert(i) {
            continue;
        }
        if let Some(r) = annots.get(i).and_then(Object::as_ref) {
            todo.extend(replies.get(&r).into_iter().flatten().filter(|j| !set.contains(j)));
        }
    }
    set
}

/// The annotations at `copied` in `annots`, as new annotations on sheet `page` where the page
/// placed with `m` shows them (a NoRotate one upright): their rectangle and points moved there, their normal appearance
/// drawn there by a form placing the original, a reply's `/IRT` pointing at its comment's copy.
/// Pop-ups, actions and structure links, which would point back into the original, are left
/// out. An annotation without a usable rectangle is skipped.
fn copy_annotations(
    doc: &mut Document,
    annots: &[Object],
    copied: &std::collections::BTreeSet<usize>,
    m: &Matrix,
    page: ObjRef,
    promoted: &mut std::collections::HashMap<ObjRef, ObjRef>,
) -> Vec<Object> {
    // Each copy's reference first, so a reply can point at its comment's copy.
    let mut copies: std::collections::HashMap<ObjRef, ObjRef> = std::collections::HashMap::new();
    let mut todo = Vec::new();
    for &i in copied {
        let Some(a) = annots.get(i) else { continue };
        let new = doc.add(Object::Null);
        if let Some(r) = a.as_ref() {
            copies.insert(r, new);
        }
        todo.push((a, new));
    }
    let scale = m.0[0].hypot(m.0[1]);
    let mut out = Vec::new();
    for (a, new) in todo {
        let Some(mut d) = doc.resolve(a).as_dict().cloned() else { continue };
        let nums = |doc: &Document, o: Option<&Object>| -> Option<Vec<f64>> {
            let v: Vec<f64> = doc.resolve(o?).as_array()?.iter().filter_map(|x| doc.resolve(x).as_f64()).collect();
            v.iter().all(|x| x.is_finite()).then_some(v)
        };
        let Some(rect) = nums(doc, d.get(b"Rect")).filter(|r| r.len() == 4) else { continue };
        let rect = [rect[0].min(rect[2]), rect[1].min(rect[3]), rect[0].max(rect[2]), rect[1].max(rect[3])];
        let place = placement(m, rect, no_rotate(doc, &d));
        let Some(placed) = on_sheet(&place, rect) else { continue };
        let normal = appearance(doc, &d);
        for key in [&b"P"[..], b"Popup", b"Parent", b"StructParent", b"A", b"AA", b"AP", b"AS"] {
            d.remove(key);
        }
        let reals = |v: &[f64]| Object::Array(v.iter().map(|x| Object::Real(*x)).collect());
        d.set(b"P".to_vec(), Object::Ref(page));
        d.set(b"Rect".to_vec(), reals(&placed));
        match d.get(b"IRT").and_then(Object::as_ref).and_then(|r| copies.get(&r)) {
            Some(parent) => d.set(b"IRT".to_vec(), Object::Ref(*parent)),
            None => {
                d.remove(b"IRT");
                d.remove(b"RT");
            }
        }
        let moved = |v: &[f64]| v.as_chunks::<2>().0.iter().flat_map(|[x, y]| <[f64; 2]>::from(m.apply(*x, *y))).collect::<Vec<f64>>();
        for key in [&b"QuadPoints"[..], b"Vertices", b"L", b"CL"] {
            if let Some(v) = nums(doc, d.get(key)).filter(|v| v.len() % 2 == 0) {
                d.set(key.to_vec(), reals(&moved(&v)));
            }
        }
        if let Some(ink) = d.get(b"InkList").and_then(|o| doc.resolve(o).as_array().cloned()) {
            let paths: Vec<Object> = ink.iter().filter_map(|p| nums(doc, Some(p))).map(|v: Vec<f64>| reals(&moved(&v))).collect();
            d.set(b"InkList".to_vec(), Object::Array(paths));
        }
        if let Some(rd) = nums(doc, d.get(b"RD")).filter(|v| v.len() == 4) {
            d.set(b"RD".to_vec(), reals(&rd.iter().map(|v| v * scale).collect::<Vec<_>>()));
        }
        if let Some(ap) = normal
            && let Some(form) = doc.get(ap).as_dict().cloned()
            && let Some(fit) = appearance_matrix(doc, &form, rect)
        {
            // A form the size of the new rectangle, drawing the original appearance as the page
            // drew it, then placed as the page is on the sheet.
            let ap = form_xobject(doc, ap, promoted);
            let [a0, b0, c0, d0, e0, f0] = fit.then(&place).0;
            let body = format!("q {} {} {} {} {} {} cm /A Do Q\n", n(a0), n(b0), n(c0), n(d0), n(e0), n(f0));
            let mut xobjects = Dict::new();
            xobjects.set(b"A".to_vec(), Object::Ref(ap));
            let mut res = Dict::new();
            res.set(b"XObject".to_vec(), Object::Dict(xobjects));
            let mut f = Dict::new();
            f.set(b"Type".to_vec(), Object::name("XObject"));
            f.set(b"Subtype".to_vec(), Object::name("Form"));
            f.set(b"BBox".to_vec(), reals(&placed));
            f.set(b"Resources".to_vec(), Object::Dict(res));
            let wrapper = doc.add(Object::Stream(Stream::flate(f, body.as_bytes())));
            let mut apd = Dict::new();
            apd.set(b"N".to_vec(), Object::Ref(wrapper));
            d.set(b"AP".to_vec(), Object::Dict(apd));
        }
        doc.set(new, Object::Dict(d));
        out.push(Object::Ref(new));
    }
    out
}

/// Page `index` fitted into `area` (`[x0 y0 x1 y1]` on the sheet), centred: the matrix from its
/// user space to the sheet (through its display space, so a turned page shows upright), and its
/// frame on the sheet.
fn place(src: &Document, pages: &[pdfcraft_model::Page], index: usize, area: [f64; 4]) -> (Matrix, [f64; 4]) {
    let (area_w, area_h) = (area[2] - area[0], area[3] - area[1]);
    let Some(page) = pages.get(index) else { return (Matrix::IDENTITY, area) };
    let (w, h) = page.display_size(src);
    let (w, h) = (if w.is_finite() && w > 0.0 { w } else { 612.0 }, if h.is_finite() && h > 0.0 { h } else { 792.0 });
    let s = (area_w / w).min(area_h / h);
    let (pw, ph) = (w * s, h * s);
    let (x, y) = (area[0] + (area_w - pw) / 2.0, area[1] + (area_h - ph) / 2.0);
    let to_display = Matrix(page.view_matrix(src)).invert().unwrap_or_default();
    (to_display.then(&Matrix([s, 0.0, 0.0, s, x, y])), [x, y, x + pw, y + ph])
}

/// Where a comment's connector or number points: its rectangle (user space), its number, and
/// whether it stays upright on a turned page ([`placement`]).
#[derive(Clone, Copy)]
struct Anchor {
    rect: [f64; 4],
    number: usize,
    upright: bool,
}

impl Anchor {
    /// Its rectangle on a sheet showing the page with `m`, as drawn there.
    fn on_sheet(&self, m: &Matrix) -> Option<[f64; 4]> {
        on_sheet(&placement(m, self.rect, self.upright), self.rect)
    }
}

/// Whether annotation `d` has the NoRotate flag.
fn no_rotate(doc: &Document, d: &Dict) -> bool {
    const NO_ROTATE: i64 = 16;
    d.get(b"F").and_then(|f| doc.resolve(f).as_int()).is_some_and(|f| f & NO_ROTATE != 0)
}

/// The matrix placing an annotation at `rect` on a sheet showing its page with `m`. An `upright`
/// (NoRotate) one, such as a note's icon, stays upright where its top-left corner is, as a viewer
/// shows it on a turned page (ISO 32000-2 §12.5.3): scaled and moved, not turned.
fn placement(m: &Matrix, rect: [f64; 4], upright: bool) -> Matrix {
    if !upright {
        return *m;
    }
    let scale = m.0[0].hypot(m.0[1]);
    let (x, y) = m.apply(rect[0], rect[3]);
    Matrix([scale, 0.0, 0.0, scale, x - rect[0] * scale, y - rect[3] * scale])
}

/// A comment's rectangle (user space) on the sheet; `None` when it doesn't map to finite numbers.
fn on_sheet(m: &Matrix, rect: [f64; 4]) -> Option<[f64; 4]> {
    let b = m.bbox(rect);
    b.iter().all(|v| v.is_finite()).then_some(b)
}

/// What is drawn for `s`: tabs as four spaces, a soft hyphen as a hyphen (as Helvetica draws
/// it; the bundled face has no glyph for it). Line breaks stay: they lay the text out.
fn drawn(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\t', "    ").replace('\u{ad}', "-")
}

const ELLIPSIS: &str = "\u{2026}";

/// `s`, cut short with an ellipsis to fit `width` at `size`.
fn fit(font: &SummaryFont, s: &str, size: f64, width: f64) -> String {
    if font.width(s, size) <= width {
        return s.to_string();
    }
    let room = width - font.width(ELLIPSIS, size);
    let mut out = String::new();
    let mut used = 0.0;
    let mut buf = [0u8; 4];
    for ch in s.chars() {
        let w = font.width(ch.encode_utf8(&mut buf), size);
        if used + w > room {
            break;
        }
        used += w;
        out.push(ch);
    }
    out.push_str(ELLIPSIS);
    out
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

/// A number for a content stream: three decimals at most, without trailing zeros.
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
    /// The font for `text` ([`drawn`] already): line breaks lay it out and are not drawn, so
    /// each line is checked on its own.
    fn choose(doc: &mut Document, text: &[&str]) -> Result<Self, PrintError> {
        let lines = || text.iter().flat_map(|t| t.split(['\n', '\r']));
        let helvetica_draws = lines().all(|l| pdfcraft_fonts::first_undrawable(l).is_none());
        if !helvetica_draws
            && let Some(face) = pdfcraft_fonts::document_latin_fonts(false).into_iter().next()
            && lines().all(|l| !l.chars().any(char::is_control) && pdfcraft_fonts::embedding::covers(face.bytes, l))
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

    /// The width of `s` at `size` points, as [`show`](Self::show) draws it.
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
