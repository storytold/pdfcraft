//! The pages as a `craft_wordmodel::Document`, the model craft-libs' word-processing writers take
//! (storytold/craft-libs#793): paragraphs with their runs and heading levels, tables, pictures,
//! and a section per page with that page's size. RTF writes it with craft-rtf's full writer, every
//! page flowing ([`flow_doc`]); Word writes it with craft-docx, pages that can't flow kept as
//! positioned text boxes, pictures and vector shapes, their form fields as content controls
//! ([`word_doc`]).

mod art;
mod fields;
mod fill;
mod pictures;

pub(crate) use fields::MAX_DOCX_LIST_ITEMS;
#[cfg(test)]
pub(crate) use fields::MAX_FIELD_NAME_CHARS;

use super::as_docx::{rgb8, room_for};
use super::as_html::{allowed, language_tag};
use super::clip::font_family;
use super::layout::{self, FieldKind, Image, Item, Page, Paint, Table, body_size, items, reading_order};
use craft_wordmodel::para::{Anchor, Float, ShapeKind, Wrap};
use craft_wordmodel::props::{Border, BorderStyle, Borders, CellProps, LineSpacing, TableProps};
use craft_wordmodel::{Block, CharProps, Document, InlineObject, ParaProps, Paragraph, PartKind, Rgb, SectionProps, Style, StyleSheet, TextColor};

/// The font family Word knows for a PDF font's base name: the first family [`font_family`]
/// reads from it, with known joined family words restored (Word has no CSS fallback list),
/// as Copy with formatting names it (`ABCDEF+Arial-BoldMT` → `Arial`,
/// `TimesNewRomanPSMT` → `Times New Roman`, `MS-Gothic` → `MS Gothic`), with the standard 14
/// fonts by the families Word ships for them (`Helvetica` → `Arial`). `None` when nothing is
/// left; at most 31 characters, Word's limit.
pub(crate) fn word_font(base: &str) -> Option<String> {
    let family = font_family(base).into_iter().next()?;
    let family = match family.as_str() {
        "Helvetica" => "Arial".to_string(),
        "Times" | "Times Roman" => "Times New Roman".to_string(),
        "Courier" => "Courier New".to_string(),
        "Sim Sun" => "SimSun".to_string(),
        "Microsoft Ya Hei" => "Microsoft YaHei".to_string(),
        "Deja Vu Sans" => "DejaVu Sans".to_string(),
        "Deja Vu Sans Mono" => "DejaVu Sans Mono".to_string(),
        "Deja Vu Serif" => "DejaVu Serif".to_string(),
        _ => family,
    };
    let family: String = family.chars().take(31).collect();
    let family = family.trim();
    (!family.is_empty()).then(|| family.to_string())
}

/// A run's formatting: its size, bold, italic and colour (none for black, the default), set in
/// the family of the PDF font `font` when one is given and has one.
fn chr(font: Option<&str>, size: f64, bold: bool, italic: bool, color: [f64; 3]) -> CharProps {
    let [r, g, b] = rgb8(color);
    // Half points, as Word and RTF store sizes; one that is not finite gets the 12 pt default.
    let half_points = if size.is_finite() { (size * 2.0).round().clamp(2.0, 3276.0) } else { 24.0 };
    let family = font.and_then(word_font);
    CharProps {
        font: family.clone(),
        font_ea: family.clone(),
        font_cs: family,
        size: Some(half_points as f32 / 2.0),
        bold: bold.then_some(true),
        italic: italic.then_some(true),
        color: (r | g | b != 0).then_some(TextColor::Rgb(Rgb(r, g, b))),
        ..CharProps::default()
    }
}

/// Appends `text` to `p` as a run (`Paragraph::push_text` drops U+FFFC, the model's object mark,
/// which in the PDF's text is a character, not an object), keeping only the characters Word and
/// RTF readers accept.
fn push_text(p: &mut Paragraph, text: &str, props: CharProps) {
    // A hard line ending in the text (LF, CR or CRLF) is a line break, the model's `\n`, which the
    // writers keep inside its paragraph or cell (storytold/pdfcraft#4723).
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let text: String = text.chars().filter(|c| allowed(*c)).collect();
    p.push_text(&text, &props);
}

/// A table: its grid from the column edges, ruled where the PDF draws rules, one paragraph per
/// cell, its text in the cells' PDF fonts when `fonts`.
pub(crate) fn table(t: &Table, fonts: bool) -> craft_wordmodel::Table {
    let mut edges = t.cols.clone();
    edges.push(t.rect[2]);
    // Whole twips, at least 20 pt wide, and within Word's range of 3 pt to 22 inches.
    let widths: Vec<f32> = edges.windows(2).map(|w| (((w[1] - w[0]).max(20.0) * 20.0).round().clamp(60.0, 31680.0) / 20.0) as f32).collect();
    let [top, left, bottom, right] = t.ruled.unwrap_or_default();
    let inside = t.ruled.is_some();
    let rule = |drawn: bool| Some(if drawn { Border { style: BorderStyle::Single, width: 0.5, color: None, space: 0.0 } } else { Border::default() });
    let props = TableProps {
        borders: Some(Borders {
            top: rule(top),
            left: rule(left),
            bottom: rule(bottom),
            right: rule(right),
            between: rule(inside),
            inside_v: rule(inside),
        }),
        ..TableProps::default()
    };
    let rows = t
        .rows
        .iter()
        .map(|row| {
            let mut cells = Vec::new();
            let mut column = 0usize;
            for c in row {
                let span = c.span.max(1);
                let w: f32 = widths.iter().skip(column).take(span).sum();
                let mut p = Paragraph::new();
                push_text(&mut p, &c.text, chr(fonts.then_some(c.font.as_str()), c.size, c.bold, c.italic, c.color));
                cells.push(craft_wordmodel::Cell {
                    props: CellProps { width: Some(w), span: span as u32, ..CellProps::default() },
                    blocks: vec![craft_wordmodel::para_block(p)],
                    ..Default::default()
                });
                column = column.saturating_add(span);
            }
            // A short row is padded to the grid, as Word expects every row to fill it.
            for w in widths.iter().skip(column) {
                cells.push(craft_wordmodel::Cell {
                    props: CellProps { width: Some(*w), ..CellProps::default() },
                    blocks: vec![craft_wordmodel::para_block(Paragraph::new())],
                    ..Default::default()
                });
            }
            craft_wordmodel::Row { cells, ..Default::default() }
        })
        .collect();
    craft_wordmodel::Table { props, grid: widths, rows, ..Default::default() }
}

/// A page's section: its size, with margins of an eighth of each side, at most an inch. A page
/// that keeps its layout (`kept`) places everything itself, so its top margin is a quarter inch
/// (at most an eighth of the page): Word moved a field framed in the top margin down to the
/// margin.
fn section(page: &Page, kept: bool) -> SectionProps {
    let twips = |pt: f64| if pt.is_finite() { (pt * 20.0).round().clamp(144.0, 31680.0) as i64 } else { 12240 };
    let (w, h) = (twips(page.width), twips(page.height));
    let (mx, my) = ((w / 8).min(1440), (h / 8).min(1440));
    let top = if kept { (h / 8).min(360) } else { my };
    let pt = |t: i64| t as f32 / 20.0;
    SectionProps {
        page_w: pt(w),
        page_h: pt(h),
        landscape: false,
        margin_top: pt(top),
        margin_bottom: pt(my),
        margin_left: pt(mx),
        margin_right: pt(mx),
        header: pt(top / 2),
        footer: pt(my / 2),
        ..SectionProps::default()
    }
}

/// The document default formatting for text in `tag`. Word keeps one language per script:
/// Chinese, Japanese and Korean are its East Asian text, Arabic, Hebrew, Persian and Urdu its
/// right-to-left text, and any other language its Latin, Greek and Cyrillic text. A script the
/// tag names (`pa-Arab`, `zh-Latn`) decides over its language.
fn language(tag: &str) -> CharProps {
    let mut subtags = tag.split('-');
    let primary = subtags.next().unwrap_or_default().to_ascii_lowercase();
    // The script comes straight after the language and any three-letter extended languages; a
    // singleton before it starts an extension or private use, which names no script.
    let script = subtags
        .find(|s| s.len() != 3 || !s.bytes().all(|b| b.is_ascii_alphabetic()))
        .filter(|s| s.len() == 4 && s.bytes().all(|b| b.is_ascii_alphabetic()))
        .map(str::to_ascii_lowercase);
    let tag = Some(tag.to_string());
    match (script.as_deref(), primary.as_str()) {
        (Some("arab" | "hebr" | "syrc" | "thaa"), _) => CharProps { lang_bidi: tag, ..CharProps::default() },
        (Some("hans" | "hant" | "hani" | "jpan" | "kore" | "hang" | "hira" | "kana"), _) => CharProps { lang_ea: tag, ..CharProps::default() },
        (Some("latn" | "cyrl" | "grek"), _) => CharProps { lang: tag, ..CharProps::default() },
        (_, "zh" | "ja" | "ko") => CharProps { lang_ea: tag, ..CharProps::default() },
        (_, "ar" | "he" | "fa" | "ur") => CharProps { lang_bidi: tag, ..CharProps::default() },
        _ => CharProps { lang: tag, ..CharProps::default() },
    }
}

/// The styles: the body text in `font`, in the document's language, and two heading levels.
fn styles(font: &str, lang: Option<&str>) -> StyleSheet {
    let mut s = StyleSheet { styles: Vec::new(), ..StyleSheet::default() };
    s.default_chr = CharProps { font: Some(font.to_string()), ..language_tag(lang).map(language).unwrap_or_default() };
    // As `as_docx` set them: lines at 1.16 and 6 pt after each paragraph.
    s.default_para = ParaProps { line_spacing: Some(LineSpacing::Multiple(1.16)), ..ParaProps::default() };
    s.default_para_present = true;
    let normal = ParaProps { space_after: Some(6.0), ..ParaProps::default() };
    s.styles.push(Style { id: "Normal".into(), name: "Normal".into(), default: true, builtin: true, para: normal, ..Style::default() });
    for (lvl, before) in [(1u8, 12.0f32), (2, 10.0)] {
        s.styles.push(Style {
            id: format!("Heading{lvl}"),
            name: format!("heading {lvl}"),
            based_on: Some("Normal".into()),
            next: Some("Normal".into()),
            para: ParaProps { keep_next: Some(true), space_before: Some(before), outline_level: Some(lvl - 1), ..ParaProps::default() },
            chr: CharProps { bold: Some(true), ..CharProps::default() },
            builtin: true,
            ..Style::default()
        });
    }
    s
}

/// What the document holds besides its pages.
pub(crate) struct Meta<'a> {
    pub title: &'a str,
    /// The PDF's `/Lang`.
    pub language: Option<&'a str>,
    /// The body text's font.
    pub font: &'a str,
}

/// The document being built: the model, its body, and the pictures it has embedded so far (for
/// the export's limits on their count and size, [`room_for`]).
struct Builder<'a> {
    doc: Document,
    body: Vec<Block>,
    media: Vec<(String, &'a Image)>,
    /// For RTF, which repeats a picture's data at every placement and doesn't turn pictures: what
    /// its pictures may still cost, and the rotations baked into them ([`pictures::Budget`]).
    rtf: Option<pictures::Budget>,
    /// The page (0-based) being built, and the pages that lost a picture to a limit or to a
    /// picture that couldn't be prepared.
    page: usize,
    omitted: Vec<usize>,
}

impl<'a> Builder<'a> {
    fn new(pages: &[Page], meta: &Meta, rtf: Option<pictures::Budget>) -> Self {
        let mut doc = Document::new();
        doc.styles = std::sync::Arc::new(styles(meta.font, meta.language));
        doc.core.title = meta.title.to_string();
        // A background every page shares is the document's page colour (#2992): Word has one for
        // the whole document.
        let shared = pages.first().and_then(|p| p.background).map(rgb8);
        if let Some(c) = shared.filter(|c| *c != [255; 3] && pages.iter().all(|p| p.background.map(rgb8) == Some(*c))) {
            doc.settings.page_color = Some(Rgb(c[0], c[1], c[2]));
        }
        Builder { doc, body: Vec::new(), media: Vec::new(), rtf, page: 0, omitted: Vec::new() }
    }

    /// The media key for `im`, or `None` for an empty picture, and past the limits or for one that
    /// can't be prepared, which marks the page as having lost it.
    fn add_media(&mut self, im: &'a Image) -> Option<String> {
        if im.bytes.is_empty() {
            return None;
        }
        let prepared = if !room_for(&self.media, im) {
            None
        } else if let Some(budget) = &mut self.rtf {
            budget.prepare(im)
        } else {
            Some((im.bytes.to_vec(), im.ext))
        };
        let Some((bytes, ext)) = prepared else {
            if self.omitted.last() != Some(&self.page) {
                self.omitted.push(self.page);
            }
            return None;
        };
        let key = self.doc.add_media(bytes, ext);
        self.media.push((key.clone(), im));
        Some(key)
    }

    /// A block's run formatting. Word sets it in the family of the block's PDF font; RTF keeps
    /// the body font.
    fn chr(&self, bl: &layout::Block) -> CharProps {
        chr(self.rtf.is_none().then_some(bl.font.as_str()), bl.size, bl.bold, bl.italic, bl.color)
    }

    /// One flowing item: a paragraph, a table, or a picture scaled to fit between the margins of
    /// `sec`.
    fn flow(&mut self, it: Item<'a>, sec: &SectionProps) {
        match it {
            Item::Para(runs, lvl) => {
                let mut p = Paragraph::new();
                if matches!(lvl, 1 | 2) {
                    p.props.style = Some(format!("Heading{lvl}"));
                }
                for r in &runs {
                    push_text(&mut p, &r.text, self.chr(r.block));
                }
                self.body.push(Block::Para(p));
            }
            Item::Table(t) => {
                // Tables that touch become one table: a paragraph keeps them apart.
                if matches!(self.body.last(), Some(Block::Table(_))) {
                    self.body.push(Block::Para(Paragraph::new()));
                }
                self.body.push(Block::Table(table(&t, self.rtf.is_none())));
            }
            Item::Img(im) => {
                let Some(media) = self.add_media(im) else { return };
                // Pictures flow upright: Word flips one the PDF mirrors or turns half round to
                // match (a bottom-up bitmap vertically, a left-right mirror horizontally).
                let flips = if self.rtf.is_none() { im.upright_flips() } else { (false, false) };
                self.body.push(Block::Para(picture(media, im, sec, flips)));
            }
            Item::PageBreak => {}
        }
    }

    /// End a page: its section ends here, and the next page's starts on a new page.
    fn end_page(&mut self, sec: SectionProps) {
        let mut p = Paragraph::new();
        p.props = exact_1pt();
        p.section = Some(Box::new(sec));
        self.body.push(Block::Para(p));
    }

    fn finish(mut self, last: SectionProps) -> Document {
        self.doc.last_section = last;
        self.doc.body = self.body.into_iter().map(std::sync::Arc::new).collect();
        self.doc.ensure_nonempty();
        self.doc
    }
}

/// A paragraph holding the picture `media`, scaled down to fit between the margins of `sec`, side
/// to side and top to bottom, and flipped (left to right, top to bottom) as `flips` says.
fn picture(media: String, im: &Image, sec: &SectionProps, (flip_h, flip_v): (bool, bool)) -> Paragraph {
    let text_w = f64::from((sec.page_w - sec.margin_left - sec.margin_right).max(1.0));
    let text_h = f64::from((sec.page_h - sec.margin_top - sec.margin_bottom).max(1.0));
    let (w, h) = ((im.rect[2] - im.rect[0]).max(1.0), (im.rect[3] - im.rect[1]).max(1.0));
    let k = (text_w / w).min(text_h / h).min(1.0);
    let pic = InlineObject::Image {
        media,
        w: (w * k) as f32,
        h: (h * k) as f32,
        alt: String::new(),
        float: Float { flip_h, flip_v, ..Float::default() },
        crop: [0.0; 4],
        fx: Default::default(),
        ole: None,
    };
    let mut p = Paragraph::new();
    p.push_object(pic, &CharProps::default());
    p
}

/// A paragraph that takes no room: no space around it and a 1 pt line.
fn exact_1pt() -> ParaProps {
    ParaProps { space_before: Some(0.0), space_after: Some(0.0), line_spacing: Some(LineSpacing::Exactly(1.0)), ..ParaProps::default() }
}

/// The pages as one flowing document: each page a section of its own size (so a page always
/// starts a new one, blank pages included), its paragraphs, tables and pictures in reading order.
/// Headings are told from the whole document's body size.
/// Its pictures are prepared for RTF ([`pictures::Budget`]); the pages (0-based) that lost one
/// come with it, their other content kept.
pub(crate) fn flow_doc(pages: &[Page], meta: &Meta) -> (Document, Vec<usize>) {
    flow_doc_with_budget(pages, meta, pictures::Budget::default())
}

fn flow_doc_with_budget(pages: &[Page], meta: &Meta, budget: pictures::Budget) -> (Document, Vec<usize>) {
    let mut b = Builder::new(pages, meta, Some(budget));
    let mut sec = pages.first().map(|p| section(p, false)).unwrap_or_default();
    for it in items(pages) {
        if matches!(it, Item::PageBreak) {
            b.end_page(sec);
            b.page += 1;
            sec = pages.get(b.page).map(|p| section(p, false)).unwrap_or_default();
        } else {
            b.flow(it, &sec);
        }
    }
    let omitted = std::mem::take(&mut b.omitted);
    (b.finish(sec), omitted)
}

/// Pages with rotated text, or text that actually overlaps an image or an oversized
/// (watermark-like) text block, cannot be put in paragraph flow without changing their layout.
/// Nor can a form: its fields, boxes and comb cells only mean something where they stand.
/// Keep those pages as editable, absolutely positioned text boxes. Every other page, including
/// one with a logo or a big title that sits clear of the text, flows as paragraphs and tables.
pub(crate) fn positioned(page: &Page) -> bool {
    if page.blocks.iter().any(|b| b.rotation.abs() > 0.1) || !page.fields.is_empty() || page.fields_truncated || page.art_framed || framed(page) {
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
        for (points, _) in &s.paths {
            let Some(b) = art::point_bounds(points.iter()) else { continue };
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
    }
    false
}

impl Page {
    /// Retain whole shapes within a shared document point budget, preserving the original
    /// framing decision and recording omitted artwork. Call as each page is read, before
    /// collecting it with the other pages, to bound retained shape memory across the export.
    pub fn limit_shape_points(&mut self, remaining_points: &mut usize) {
        self.art_framed |= framed(self);
        let mut remap = Vec::with_capacity(self.shapes.len());
        let mut retained = 0usize;
        self.shapes.retain(|shape| {
            let points = shape.paths.iter().fold(0usize, |n, (path, _)| n.saturating_add(path.len()));
            if points <= *remaining_points {
                *remaining_points -= points;
                remap.push(Some(retained));
                retained += 1;
                true
            } else {
                self.art_truncated = true;
                remap.push(None);
                false
            }
        });
        self.paint_order.retain_mut(|paint| match paint {
            Paint::Shape(index) => match remap.get(*index).copied().flatten() {
                Some(new) => {
                    *index = new;
                    true
                }
                None => false,
            },
            Paint::Image(_) => true,
        });
        // Vec::retain drops each removed shape's points but keeps the outer allocation. Shrink
        // that too: many pages beyond the limit must not retain a page's full shape capacity.
        self.shapes.shrink_to_fit();
    }
}

/// Whether two [x0, y0, x1, y1] boxes share more than a hairline (0.5 pt each way); boxes that
/// only touch, or hostile non-finite coordinates, do not count.
fn overlaps(a: [f64; 4], b: [f64; 4]) -> bool {
    let w = a[2].min(b[2]) - a[0].max(b[0]);
    let h = a[3].min(b[3]) - a[1].max(b[1]);
    w.is_finite() && h.is_finite() && w > 0.5 && h > 0.5
}

/// The furthest a position or size written into a shape reaches, in points: Word's 22 inches.
const LAYOUT_LIMIT: f64 = 1584.0;

/// Bound every value written into a shape's position and size, including hostile PDF coordinates.
fn layout_number(n: f64) -> f64 {
    if n.is_finite() { n.clamp(-LAYOUT_LIMIT, LAYOUT_LIMIT) } else { 0.0 }
}

/// Most shapes (one per subpath a path draws), and most fields, one Word page holds. A page with
/// more keeps the first ones and is listed by [`crate::docx_checked`].
pub const MAX_DOCX_PAGE_SHAPES: usize = 5000;

/// Most points all of one Word page's shapes hold together (a hostile path of millions of
/// segments); past it a page keeps the shapes that fit and is listed by [`crate::docx_checked`].
pub const MAX_DOCX_PAGE_POINTS: usize = 200_000;

/// The stacking order (`Float::z`, higher on top) of watermark-sized text, which is behind
/// everything, of the page's paths and pictures, over it in the order the PDF paints them, of
/// its fields' borders and fills, over those (a scanned form's thin check box lines get the
/// solid box the PDF's fields draw over them), and of its other text, over them all.
const Z_WATERMARK: u32 = 1;
const Z_ART: u32 = 1 << 20;
const Z_FIELD: u32 = Z_ART + (1 << 19);
const Z_TEXT: u32 = 1 << 21;

/// A gap between two pieces of a line wider than this many times their size starts a new
/// text box ([`rows`]): about two word spaces.
const ROW_GAP: f64 = 0.6;

/// The lines grouped into rows of pieces that follow each other on one baseline, in order: same
/// (upright) direction, baselines within a fifth of the size of the row's first piece (the row
/// is placed at one baseline, so pieces that each sit a little lower must not drift away from
/// it), and the next piece starting at most [`ROW_GAP`] times the size after the last (and not
/// far before it).
fn rows(lines: &[layout::Block]) -> Vec<Vec<&layout::Block>> {
    let mut out: Vec<Vec<&layout::Block>> = Vec::new();
    for b in lines {
        let joins = out.last().and_then(|row| row.first().zip(row.last())).is_some_and(|(first, prev)| {
            let size = prev.size.max(b.size).max(1.0);
            let gap = b.rect[0] - prev.rect[2];
            prev.rotation.abs() <= 0.1
                && b.rotation.abs() <= 0.1
                && (first.rect[1] - b.rect[1]).abs() < 0.2 * first.size.min(b.size).max(1.0)
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

/// A text box of `w` by `h` points holding `blocks`, with no inset, in front of the text unless
/// `float` says otherwise; `auto_fit` lets it grow to fit its text.
fn text_box(b: &mut Builder<'_>, blocks: Paragraph, (w, h): (f64, f64), float: Float, auto_fit: bool) -> InlineObject {
    let story = b.doc.add_part(PartKind::TextBox, vec![craft_wordmodel::para_block(blocks)]);
    if let Some(part) = b.doc.parts.get_mut(&story) {
        part.body.inset = Some([0; 4]);
        part.body.auto_fit = auto_fit;
    }
    InlineObject::Shape {
        kind: ShapeKind::TextBox,
        w: w as f32,
        h: h as f32,
        fill: None,
        stroke: None,
        stroke_width: 0.0,
        float,
        story: Some(story),
        freeform: None,
        effects: Default::default(),
        extra: Default::default(),
    }
}

/// A page that keeps its layout: one 1 pt paragraph anchors its text boxes, vector paths,
/// pictures and its fields' boxes, placed on the page, and its fields follow as paragraphs
/// framed over their boxes. Shapes never reserve body space, so even a page-sized watermark
/// cannot push the text after it onto another page. Says whether it had to leave shapes, fields
/// or list items out to stay within [`MAX_DOCX_PAGE_SHAPES`], [`MAX_DOCX_PAGE_POINTS`] and
/// [`MAX_DOCX_LIST_ITEMS`], or a radio group's editability.
fn positioned_page<'a>(b: &mut Builder<'a>, page: &'a Page) -> bool {
    let mut anchor = Paragraph::new();
    anchor.props = exact_1pt();
    let body = body_size(std::slice::from_ref(page)).max(1.0);
    // The text boxes in reading order, which a screen reader follows; the file's block order isn't
    // the reading order. Lines are placed in rows: the pieces of one visual line that follow
    // each other within a couple of spaces (a bold word inside a sentence) share a text box, so
    // the reader sets the space between them; a wider gap (a tab stop, a column) starts a box of
    // its own.
    let groups: Vec<Vec<&layout::Block>> = if page.lines.is_empty() {
        reading_order(page, page.blocks.iter().map(|bl| (bl.rect, bl.size, bl)).collect()).into_iter().map(|bl| vec![bl]).collect()
    } else {
        let union = |a: [f64; 4], r: [f64; 4]| [a[0].min(r[0]), a[1].min(r[1]), a[2].max(r[2]), a[3].max(r[3])];
        reading_order(
            page,
            rows(&page.lines)
                .into_iter()
                .filter_map(|group| {
                    let first = group.first()?;
                    let rect = group.iter().fold(first.rect, |a, g| union(a, g.rect));
                    Some((rect, first.size, group))
                })
                .collect(),
        )
    };
    let mut boxes = 0u32;
    for group in &groups {
        let Some(bl) = group.first().copied() else { continue };
        let rotation = layout_number(-bl.rotation).rem_euclid(360.0);
        let (sin, cos) = rotation.to_radians().sin_cos();
        let rect = group.iter().fold(bl.rect, |a, g| [a[0].min(g.rect[0]), a[1].min(g.rect[1]), a[2].max(g.rect[2]), a[3].max(g.rect[3])]);
        let (bw, bh) = (layout_number(rect[2] - rect[0]).max(1.0), layout_number(rect[3] - rect[1]).max(1.0));
        let h = if bl.rotation.abs() > 0.1 { layout_number(bl.size).max(1.0) } else { bh };
        // Recover the unrotated width from the axis-aligned box. Use the larger axis so quarter
        // turns and 45-degree watermarks do not divide by (nearly) zero.
        let w = if bl.rotation.abs() <= 0.1 {
            bw
        } else if cos.abs() >= sin.abs() {
            (bw - h * sin.abs()) / cos.abs()
        } else {
            (bh - h * cos.abs()) / sin.abs()
        };
        let w = layout_number(w).max(1.0);
        let (x, y) = (layout_number(rect[0] + (bw - w) / 2.0), layout_number(page.height - rect[3] + (bh - h) / 2.0));
        // One line of text gets room to spare, so that the reader's font (a little wider, or
        // without the PDF's tightened spacing) doesn't wrap it: the box is transparent and its
        // text starts at the left edge, so a wider box changes nothing else. It reaches at most
        // the page's edge.
        let size = group.iter().map(|g| layout_number(g.size)).fold(1.0, f64::max);
        let one_line = bl.rotation.abs() <= 0.1 && bh < 1.6 * size;
        let w = if one_line { layout_number((w * 1.25 + 2.0).min(page.width - x)).max(w) } else { w };
        let mut p = Paragraph::new();
        p.props = ParaProps { space_before: Some(0.0), space_after: Some(0.0), ..ParaProps::default() };
        for (i, g) in group.iter().enumerate() {
            // The source's own space between two pieces, or a visible gap, is a space.
            let spaced = i > 0
                && group
                    .get(i - 1)
                    .is_some_and(|prev| prev.space_around[1] || g.space_around[0] || g.rect[0] - prev.rect[2] > 0.12 * g.size.max(1.0));
            let text = if spaced { format!(" {}", g.text) } else { g.text.clone() };
            push_text(&mut p, &text, b.chr(g));
        }
        // Each layer keeps to its range, however many blocks a hostile page has.
        let order = boxes.min(Z_ART - Z_WATERMARK - 1);
        boxes = boxes.saturating_add(1);
        let watermark = bl.size >= 2.0 * body;
        let float = Float {
            wrap: if watermark { Wrap::BehindText } else { Wrap::InFrontOfText },
            h_rel: Anchor::Page,
            v_rel: Anchor::Page,
            x: x as f32,
            y: y as f32,
            rot: rotation as f32,
            z: if watermark { Z_WATERMARK } else { Z_TEXT }.saturating_add(order),
            ..Float::default()
        };
        // The box grows to fit its text.
        let shape = text_box(b, p, (w, h), float, true);
        anchor.push_object(shape, &CharProps::default());
    }
    // Paths and pictures share the order the PDF paints them in. A page built without one
    // keeps its paths behind its pictures.
    let layers: std::collections::HashMap<Paint, usize> =
        page.paint_order.iter().enumerate().map(|(i, paint)| (*paint, i.saturating_add(1))).collect();
    let shapes = page.shapes.len();
    let mut budget = art::ArtBudget::new();
    let mut artwork: Vec<(usize, InlineObject)> = Vec::new();
    let mut drawn = Vec::new();
    for (i, shape) in page.shapes.iter().enumerate() {
        let layer = layers.get(&Paint::Shape(i)).copied().unwrap_or(i.saturating_add(1));
        art::shape_objects(shape, page.height, &mut budget, &mut drawn);
        artwork.extend(drawn.drain(..).map(|o| (layer, o)));
        if budget.dropped && budget.shapes == 0 {
            break;
        }
    }
    for (n, im) in page.images.iter().enumerate() {
        let Some(media) = b.add_media(im) else { continue };
        let (w, h) = (layout_number(im.size[0]).max(1.0), layout_number(im.size[1]).max(1.0));
        let (x, y) = (layout_number((im.rect[0] + im.rect[2] - w) / 2.0), layout_number(page.height - (im.rect[1] + im.rect[3] + h) / 2.0));
        let rotation = layout_number(-im.rotation).rem_euclid(360.0);
        // A mirrored picture is flipped top to bottom in its frame, before it is turned.
        let float = Float { rot: rotation as f32, flip_v: im.flip, ..art::behind_at(x, y) };
        let pic =
            InlineObject::Image { media, w: w as f32, h: h as f32, alt: String::new(), float, crop: [0.0; 4], fx: Default::default(), ole: None };
        artwork.push((layers.get(&Paint::Image(n)).copied().unwrap_or(shapes.saturating_add(n).saturating_add(1)), pic));
    }
    artwork.sort_by_key(|(layer, _)| *layer);
    for (i, (_, mut o)) in artwork.into_iter().enumerate() {
        if let Some(float) = o.float_mut() {
            float.z = Z_ART + u32::try_from(i).unwrap_or(u32::MAX).min(Z_FIELD - Z_ART - 1);
        }
        anchor.push_object(o, &CharProps::default());
    }
    // The fields' borders and fills, behind the text but over the page's own art and pictures.
    let mut outline_work = 1_000_000usize;
    for (i, f) in page.fields.iter().take(MAX_DOCX_PAGE_SHAPES).filter(|f| f.valid_rect()).enumerate() {
        let outlined = f.round
            && budget.outlines.iter().take(outline_work).any(|r| {
                outline_work = outline_work.saturating_sub(1);
                r.iter().zip(f.rect).all(|(a, b)| (*a - b).abs() <= 1.5)
            });
        if let Some(mut look) = fields::look(f, page.height, outlined) {
            if let Some(float) = look.float_mut() {
                float.z = Z_FIELD + u32::try_from(i).unwrap_or(u32::MAX).min(Z_TEXT - Z_FIELD - 1);
            }
            anchor.push_object(look, &CharProps::default());
        }
    }
    // Fields follow as framed paragraphs over their boxes, in front of the shapes behind the
    // text. A paragraph frame is moved onto the page by some readers: a field that reaches off
    // the page is in a text box instead, which keeps its place and is cut at the page's edge.
    let mut left_out =
        page.fields.iter().take(MAX_DOCX_PAGE_SHAPES).any(|f| !f.valid_rect() || matches!(f.kind, FieldKind::Radio { .. }) && !f.read_only);
    let mut framed_fields = Vec::new();
    for f in page.fields.iter().take(MAX_DOCX_PAGE_SHAPES).filter(|f| f.valid_rect()) {
        let r = f.rect;
        let clipped = r[0] < 0.0 || r[1] < 0.0 || r[2] > page.width || r[3] > page.height;
        let Some(p) = fields::paragraph(f, page.height, clipped) else { continue };
        left_out |= matches!(&f.kind, FieldKind::Choice { options, .. } if options.len() > MAX_DOCX_LIST_ITEMS);
        if !clipped {
            framed_fields.push(Block::Para(p));
            continue;
        }
        let (w, h) = (layout_number(r[2] - r[0]).max(1.0), layout_number(r[3] - r[1]).max(1.0));
        // A comb field's run ends a cell past its box (see `fields::paragraph`).
        let w = match &f.kind {
            FieldKind::Text { comb: Some(n), .. } if *n > 0 => layout_number(w + w / *n as f64),
            _ => w,
        };
        let float = Float {
            wrap: Wrap::InFrontOfText,
            h_rel: Anchor::Page,
            v_rel: Anchor::Page,
            x: layout_number(r[0]) as f32,
            y: layout_number(page.height - r[3]) as f32,
            z: Z_TEXT.saturating_add(boxes),
            ..Float::default()
        };
        boxes = boxes.saturating_add(1);
        let shape = text_box(b, p, (w, h), float, false);
        anchor.push_object(shape, &CharProps::default());
    }
    b.body.push(Block::Para(anchor));
    b.body.extend(framed_fields);
    budget.dropped || page.fields.len() > MAX_DOCX_PAGE_SHAPES || left_out
}

/// The pages as a Word document: a page that can't flow ([`positioned`]) keeps its layout as
/// positioned text boxes, shapes and pictures, its fields as content controls, and the others
/// flow as paragraphs, tables and pictures. A document most of whose pages keep their layout (a
/// form) keeps it on every page: its pages of notes then sit where they did instead of flowing
/// past the page's end. Each page is a section of its own size, and its headings are told from
/// its own body size. The pages (0-based) that kept their layout but lost shapes, fields or list
/// items to a limit, or whose paths were not all read, come with it, and so do the pages, kept
/// or flowing, that lost a picture to the document's picture limits.
pub(crate) fn word_doc(pages: &[Page], meta: &Meta) -> (Document, Vec<usize>) {
    let mut b = Builder::new(pages, meta, None);
    let mut last = SectionProps::default();
    let mut dropped = Vec::new();
    let layouts: Vec<bool> = pages.iter().map(positioned).collect();
    let all_kept = layouts.iter().filter(|kept| **kept).count().saturating_mul(2) > pages.len();
    for (i, page) in pages.iter().enumerate() {
        b.page = i;
        let kept = all_kept || layouts.get(i).copied().unwrap_or(false);
        let sec = section(page, kept);
        if kept {
            let left_out = positioned_page(&mut b, page);
            // Paths the reader left out are missing only where the page keeps its layout.
            if left_out || page.art_truncated || page.fields_truncated {
                dropped.push(i);
            }
        } else {
            for it in items(std::slice::from_ref(page)) {
                b.flow(it, &sec);
            }
        }
        if i + 1 < pages.len() {
            b.end_page(sec);
        } else {
            last = sec;
        }
    }
    // A picture past the document's picture limits is missing from its page, kept or flowing.
    dropped.append(&mut b.omitted);
    dropped.sort_unstable();
    dropped.dedup();
    (b.finish(last), dropped)
}
