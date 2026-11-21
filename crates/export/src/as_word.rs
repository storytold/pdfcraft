//! The pages as a `craft_wordmodel::Document`, the model craft-libs' word-processing writers take
//! (storytold/craft-libs#793): paragraphs with their runs and heading levels, tables, pictures,
//! and a section per page with that page's size. RTF writes it with craft-rtf's full writer, every
//! page flowing ([`flow_doc`]); Word writes it with craft-docx, pages that can't flow kept as
//! positioned text boxes and pictures ([`word_doc`]).

mod pictures;

use super::as_docx::{rgb8, room_for};
use super::as_html::{allowed, language_tag};
use super::layout::{Image, Item, Page, Table, body_size, items, reading_order};
use craft_wordmodel::para::{Anchor, Float, ShapeKind, Wrap};
use craft_wordmodel::props::{Border, BorderStyle, Borders, CellProps, LineSpacing, TableProps};
use craft_wordmodel::{Block, CharProps, Document, InlineObject, ParaProps, Paragraph, PartKind, Rgb, SectionProps, Style, StyleSheet, TextColor};

/// A run's formatting: its size, bold, italic and colour (none for black, the default).
fn chr(size: f64, bold: bool, italic: bool, color: [f64; 3]) -> CharProps {
    let [r, g, b] = rgb8(color);
    // Half points, as Word and RTF store sizes; one that is not finite gets the 12 pt default.
    let half_points = if size.is_finite() { (size * 2.0).round().clamp(2.0, 3276.0) } else { 24.0 };
    CharProps {
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
/// cell.
pub(crate) fn table(t: &Table) -> craft_wordmodel::Table {
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
                push_text(&mut p, &c.text, chr(c.size, c.bold, c.italic, c.color));
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

/// A page's section: its size, with margins of an eighth of each side, at most an inch.
fn section(page: &Page) -> SectionProps {
    let twips = |pt: f64| if pt.is_finite() { (pt * 20.0).round().clamp(144.0, 31680.0) as i64 } else { 12240 };
    let (w, h) = (twips(page.width), twips(page.height));
    let (mx, my) = ((w / 8).min(1440), (h / 8).min(1440));
    let pt = |t: i64| t as f32 / 20.0;
    SectionProps {
        page_w: pt(w),
        page_h: pt(h),
        landscape: false,
        margin_top: pt(my),
        margin_bottom: pt(my),
        margin_left: pt(mx),
        margin_right: pt(mx),
        header: pt(my / 2),
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
                    push_text(&mut p, &r.text, chr(r.block.size, r.block.bold, r.block.italic, r.block.color));
                }
                self.body.push(Block::Para(p));
            }
            Item::Table(t) => {
                // Tables that touch become one table: a paragraph keeps them apart.
                if matches!(self.body.last(), Some(Block::Table(_))) {
                    self.body.push(Block::Para(Paragraph::new()));
                }
                self.body.push(Block::Table(table(&t)));
            }
            Item::Img(im) => {
                let Some(media) = self.add_media(im) else { return };
                self.body.push(Block::Para(picture(media, im, sec)));
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
/// to side and top to bottom.
fn picture(media: String, im: &Image, sec: &SectionProps) -> Paragraph {
    let text_w = f64::from((sec.page_w - sec.margin_left - sec.margin_right).max(1.0));
    let text_h = f64::from((sec.page_h - sec.margin_top - sec.margin_bottom).max(1.0));
    let (w, h) = ((im.rect[2] - im.rect[0]).max(1.0), (im.rect[3] - im.rect[1]).max(1.0));
    let k = (text_w / w).min(text_h / h).min(1.0);
    let pic = InlineObject::Image {
        media,
        w: (w * k) as f32,
        h: (h * k) as f32,
        alt: String::new(),
        float: Float::default(),
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
    let mut sec = pages.first().map(section).unwrap_or_default();
    for it in items(pages) {
        if matches!(it, Item::PageBreak) {
            b.end_page(sec);
            b.page += 1;
            sec = pages.get(b.page).map(section).unwrap_or_default();
        } else {
            b.flow(it, &sec);
        }
    }
    let omitted = std::mem::take(&mut b.omitted);
    (b.finish(sec), omitted)
}

/// Pages with rotated text, or text that actually overlaps an image or an oversized
/// (watermark-like) text block, cannot be put in paragraph flow without changing their layout.
/// Keep those pages as editable, absolutely positioned text boxes. Every other page, including
/// one with a logo or a big title that sits clear of the text, flows as paragraphs and tables.
pub(crate) fn positioned(page: &Page) -> bool {
    if page.blocks.iter().any(|b| b.rotation.abs() > 0.1) {
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

/// Whether two [x0, y0, x1, y1] boxes share more than a hairline (0.5 pt each way); boxes that
/// only touch, or hostile non-finite coordinates, do not count.
fn overlaps(a: [f64; 4], b: [f64; 4]) -> bool {
    let w = a[2].min(b[2]) - a[0].max(b[0]);
    let h = a[3].min(b[3]) - a[1].max(b[1]);
    w.is_finite() && h.is_finite() && w > 0.5 && h > 0.5
}

/// Bound every value written into a shape's position and size, including hostile PDF coordinates.
fn layout_number(n: f64) -> f64 {
    if n.is_finite() { n.clamp(-1584.0, 1584.0) } else { 0.0 }
}

/// The stacking order (`Float::z`, higher on top) of watermark-sized text, which is behind
/// everything, of the page's pictures, over it, and of its other text, over both.
const Z_WATERMARK: u32 = 1;
const Z_PICTURE: u32 = 1 << 20;
const Z_TEXT: u32 = 1 << 21;

/// A page that can't flow: one 1 pt paragraph anchors its text boxes and pictures, placed on the
/// page. Shapes never reserve body space, so even a page-sized watermark cannot push the text
/// after it onto another page.
fn positioned_page<'a>(b: &mut Builder<'a>, page: &'a Page) {
    let mut anchor = Paragraph::new();
    anchor.props = exact_1pt();
    let body = body_size(std::slice::from_ref(page)).max(1.0);
    // The text boxes in reading order, which a screen reader follows; the file's block order isn't
    // the reading order.
    let blocks = reading_order(page, page.blocks.iter().map(|bl| (bl.rect, bl.size, bl)).collect());
    for (i, bl) in blocks.into_iter().enumerate() {
        let rotation = layout_number(-bl.rotation).rem_euclid(360.0);
        let (sin, cos) = rotation.to_radians().sin_cos();
        let (bw, bh) = (layout_number(bl.rect[2] - bl.rect[0]).max(1.0), layout_number(bl.rect[3] - bl.rect[1]).max(1.0));
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
        let (x, y) = (layout_number(bl.rect[0] + (bw - w) / 2.0), layout_number(page.height - bl.rect[3] + (bh - h) / 2.0));
        let mut p = Paragraph::new();
        p.props = ParaProps { space_before: Some(0.0), space_after: Some(0.0), ..ParaProps::default() };
        push_text(&mut p, &bl.text, chr(bl.size, bl.bold, bl.italic, bl.color));
        let story = b.doc.add_part(PartKind::TextBox, vec![craft_wordmodel::para_block(p)]);
        if let Some(part) = b.doc.parts.get_mut(&story) {
            // No inset, and the box grows to fit its text.
            part.body.inset = Some([0; 4]);
            part.body.auto_fit = true;
        }
        // Each layer keeps to its range, however many blocks a hostile page has.
        let order = u32::try_from(i).unwrap_or(u32::MAX).min(Z_PICTURE - Z_WATERMARK - 1);
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
        let shape = InlineObject::Shape {
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
        };
        anchor.push_object(shape, &CharProps::default());
    }
    for (n, im) in page.images.iter().enumerate() {
        let Some(media) = b.add_media(im) else { continue };
        let (w, h) = (layout_number(im.size[0]).max(1.0), layout_number(im.size[1]).max(1.0));
        let (x, y) = (layout_number((im.rect[0] + im.rect[2] - w) / 2.0), layout_number(page.height - (im.rect[1] + im.rect[3] + h) / 2.0));
        let rotation = layout_number(-im.rotation).rem_euclid(360.0);
        let order = u32::try_from(n).unwrap_or(0).min(Z_TEXT - Z_PICTURE - 1);
        let float = Float {
            wrap: Wrap::BehindText,
            h_rel: Anchor::Page,
            v_rel: Anchor::Page,
            x: x as f32,
            y: y as f32,
            rot: rotation as f32,
            z: Z_PICTURE + order,
            ..Float::default()
        };
        let pic =
            InlineObject::Image { media, w: w as f32, h: h as f32, alt: String::new(), float, crop: [0.0; 4], fx: Default::default(), ole: None };
        anchor.push_object(pic, &CharProps::default());
    }
    b.body.push(Block::Para(anchor));
}

/// The pages as a Word document: a page that can't flow ([`positioned`]) keeps its layout as
/// positioned text boxes and pictures, and the others flow as paragraphs, tables and pictures.
/// Each page is a section of its own size, and its headings are told from its own body size.
pub(crate) fn word_doc(pages: &[Page], meta: &Meta) -> Document {
    let mut b = Builder::new(pages, meta, None);
    let mut last = SectionProps::default();
    for (i, page) in pages.iter().enumerate() {
        b.page = i;
        let sec = section(page);
        if positioned(page) {
            positioned_page(&mut b, page);
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
    b.finish(last)
}
