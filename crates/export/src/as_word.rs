//! The pages as a `craft_wordmodel::Document`, the model craft-libs' word-processing writers take
//! (storytold/craft-libs#793): paragraphs with their runs and heading levels, tables, pictures,
//! and a section per page with that page's size. RTF writes it with craft-rtf's full writer, every
//! page flowing.

use super::as_docx::{MAX_MEDIA, rgb8};
use super::as_html::{allowed, language_tag};
use super::layout::{Image, Item, Page, Table, items};
use craft_wordmodel::para::{Float, OBJ};
use craft_wordmodel::props::{Border, BorderStyle, Borders, CellProps, LineSpacing, TableLook, TableProps};
use craft_wordmodel::{Block, CharProps, Document, InlineObject, ParaProps, Paragraph, Rgb, SectionProps, Style, StyleSheet, TextColor};

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

/// Appends `text` to `p` as a run, merged into the last one when the formatting is the same. Only
/// the characters Word and RTF readers accept, and never U+FFFC, the model's object mark: in the
/// PDF's text it is a character, not an object. Appending to the fields takes constant time, where
/// `Paragraph::insert_text` walks every run on each call.
fn push_text(p: &mut Paragraph, text: &str, props: CharProps) {
    // A hard line ending in the text (LF, CR or CRLF) is a line break, the model's `\n`, which the
    // writers keep inside its paragraph or cell (storytold/pdfcraft#4723).
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let text: String = text.chars().filter(|c| allowed(*c) && *c != OBJ).collect();
    if text.is_empty() {
        return;
    }
    p.text.push_str(&text);
    match p.runs.last_mut() {
        Some(last) if last.props == props => last.len += text.len(),
        _ => p.runs.push(craft_wordmodel::Run { len: text.len(), props }),
    }
}

/// Appends an object to `p` in constant time (`Paragraph::insert_object` counts the objects before
/// its offset).
fn push_object(p: &mut Paragraph, obj: InlineObject) {
    let props = CharProps::default();
    p.text.push(OBJ);
    p.objects.push(obj);
    let len = OBJ.len_utf8();
    match p.runs.last_mut() {
        Some(last) if last.props == props => last.len += len,
        _ => p.runs.push(craft_wordmodel::Run { len, props }),
    }
}

/// A table: its grid from the column edges, ruled where the PDF draws rules, one paragraph per
/// cell.
fn table(t: &Table) -> craft_wordmodel::Table {
    let mut edges = t.cols.clone();
    edges.push(t.rect[2]);
    // Whole twips, at least 3 pt wide (Word's narrowest column), as `as_docx` writes them.
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
        // No table style, so none of its options: no header row, bands or first column. (craft-flow
        // made the first row of a table with no style a header row from the default look.)
        look: TableLook { header_row: false, total_row: false, banded_rows: false, first_column: false, last_column: false, banded_columns: false },
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

/// The styles: the body text in `font`, in the document's language, and two heading levels.
fn styles(font: &str, language: Option<&str>) -> StyleSheet {
    let mut s = StyleSheet { styles: Vec::new(), ..StyleSheet::default() };
    s.default_chr = CharProps { font: Some(font.to_string()), lang: language_tag(language).map(str::to_string), ..CharProps::default() };
    // As `as_docx` sets them: lines at 1.16 and 6 pt after each paragraph.
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

/// A paragraph holding a picture, scaled down to fit between the margins of `sec`.
fn picture(doc: &mut Document, im: &Image, sec: &SectionProps) -> Paragraph {
    let text_w = f64::from((sec.page_w - sec.margin_left - sec.margin_right).max(1.0));
    let (w, h) = ((im.rect[2] - im.rect[0]).max(1.0), (im.rect[3] - im.rect[1]).max(1.0));
    let k = (text_w / w).min(1.0);
    let media = doc.add_media(im.bytes.to_vec(), im.ext);
    let mut p = Paragraph::new();
    push_object(
        &mut p,
        InlineObject::Image {
            media,
            w: (w * k) as f32,
            h: (h * k) as f32,
            alt: String::new(),
            float: Float::default(),
            crop: [0.0; 4],
            fx: Default::default(),
            ole: None,
        },
    );
    p
}

/// What the document holds besides its pages.
pub(crate) struct Meta<'a> {
    pub title: &'a str,
    /// The PDF's `/Lang`.
    pub language: Option<&'a str>,
    /// The body text's font.
    pub font: &'a str,
}

/// The pages as one flowing document: each page a section of its own size (so a page always
/// starts a new one, blank pages included), its paragraphs, tables and pictures in reading order.
pub(crate) fn flow_doc(pages: &[Page], meta: &Meta) -> Document {
    let mut doc = Document::new();
    doc.styles = std::sync::Arc::new(styles(meta.font, meta.language));
    doc.core.title = meta.title.to_string();
    let mut body: Vec<Block> = Vec::new();
    let mut media = 0usize;
    let mut page = 0usize;
    let mut sec = pages.first().map(section).unwrap_or_default();
    for it in items(pages) {
        match it {
            Item::Para(runs, lvl) => {
                let mut p = Paragraph::new();
                if matches!(lvl, 1 | 2) {
                    p.props.style = Some(format!("Heading{lvl}"));
                }
                for r in &runs {
                    push_text(&mut p, &r.text, chr(r.block.size, r.block.bold, r.block.italic, r.block.color));
                }
                body.push(Block::Para(p));
            }
            Item::Table(t) => {
                // Tables that touch become one table: a paragraph keeps them apart.
                if matches!(body.last(), Some(Block::Table(_))) {
                    body.push(Block::Para(Paragraph::new()));
                }
                body.push(Block::Table(table(&t)));
            }
            Item::Img(im) => {
                if im.bytes.is_empty() || media >= MAX_MEDIA {
                    continue;
                }
                media += 1;
                body.push(Block::Para(picture(&mut doc, im, &sec)));
            }
            Item::PageBreak => {
                // The page ends its section; the next page's section starts on a new page.
                let mut p = Paragraph::new();
                p.props = ParaProps {
                    space_before: Some(0.0),
                    space_after: Some(0.0),
                    line_spacing: Some(LineSpacing::Exactly(1.0)),
                    ..ParaProps::default()
                };
                p.section = Some(Box::new(sec));
                body.push(Block::Para(p));
                page += 1;
                sec = pages.get(page).map(section).unwrap_or_default();
            }
        }
    }
    doc.last_section = sec;
    // A background every page shares is the document's page colour.
    let shared = pages.first().and_then(|p| p.background).map(rgb8);
    if let Some(c) = shared.filter(|c| *c != [255; 3] && pages.iter().all(|p| p.background.map(rgb8) == Some(*c))) {
        doc.settings.page_color = Some(Rgb(c[0], c[1], c[2]));
    }
    doc.body = body.into_iter().map(std::sync::Arc::new).collect();
    doc.ensure_nonempty();
    doc
}
