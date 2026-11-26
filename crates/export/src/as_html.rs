use super::as_docx::{rgb8, room_for};
use super::layout::{Image, Item, Page, items};
#[cfg(test)]
use super::layout::{Run, Table};
use craft_html::standalone::{self as html, HtmlBlock, HtmlCell, HtmlRun, HtmlTable};

pub(crate) use craft_html::standalone::allowed;

/// The longest language tag the exports write. BCP 47 sets no maximum (35 characters is only
/// the least a protocol must hold), so this bounds only what a PDF's `/Lang` can put in a file.
const LONGEST_TAG: usize = 255;

/// `lang` when it is a well formed language tag (BCP 47, as a PDF's `/Lang` holds one): a primary
/// language of two or three letters, then subtags of one to eight letters or digits, joined by
/// hyphens, [`LONGEST_TAG`] characters at most. A singleton (one letter or digit) starts an
/// extension, or with `x` private use, and needs a subtag after it. Anything else names no
/// language.
pub(crate) fn language_tag(lang: Option<&str>) -> Option<&str> {
    let tag = lang?.trim();
    let mut subtags = tag.split('-');
    let primary = subtags.next()?;
    if tag.len() > LONGEST_TAG || !(2..=3).contains(&primary.len()) || !primary.bytes().all(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    // In private use every subtag is data, one character long or not.
    let (mut private, mut singleton) = (false, false);
    for s in subtags {
        if !(1..=8).contains(&s.len()) || !s.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return None;
        }
        if s.len() == 1 && !private {
            if singleton {
                return None;
            }
            private = s.eq_ignore_ascii_case("x");
            singleton = true;
        } else {
            singleton = false;
        }
    }
    (!singleton).then_some(tag)
}

/// One HTML file: headings and paragraphs, tables as real `<table>` elements, images inline, a
/// rule between pages.
pub fn html(pages: &[Page], title: &str) -> String {
    html_with_language(pages, title, None)
}

/// A simple HTML export with no CSS; page layout and colours are omitted.
pub fn html_simple(pages: &[Page], title: &str) -> String {
    html_simple_with_language(pages, title, None)
}

/// [`html`] for a document in `language` (a BCP 47 tag such as `bg-BG`, as a PDF's `/Lang` gives
/// it), which the page names (`<html lang>`) for spell checking, speech and hyphenation. A
/// language that is not a well formed tag is left out.
pub fn html_with_language(pages: &[Page], title: &str, language: Option<&str>) -> String {
    html_with_mode(pages, title, false, language)
}

/// [`html_simple`] for a document in `language`, named as [`html_with_language`] names it: a
/// language is not styling, so the simple export keeps it.
pub fn html_simple_with_language(pages: &[Page], title: &str, language: Option<&str>) -> String {
    html_with_mode(pages, title, true, language)
}

#[cfg(test)]
fn para_html(runs: &[Run<'_>], lvl: u8, simple: bool) -> Option<String> {
    let html_runs: Vec<HtmlRun<'_>> =
        runs.iter().map(|r| HtmlRun { text: &r.text, bold: r.block.bold, italic: r.block.italic, color: rgb8(r.block.color) }).collect();
    html::render_para(&html_runs, lvl, simple)
}

#[cfg(test)]
fn table_html(t: &Table, simple: bool) -> String {
    let table = HtmlTable {
        rows: t
            .rows
            .iter()
            .map(|row| row.iter().map(|c| HtmlCell { text: &c.text, bold: c.bold, italic: c.italic, color: rgb8(c.color), span: c.span }).collect())
            .collect(),
    };
    html::render_table(&table, simple)
}

fn html_with_mode(pages: &[Page], title: &str, simple: bool, language: Option<&str>) -> String {
    let mut embedded: Vec<(String, &Image)> = Vec::new();
    let mut blocks: Vec<HtmlBlock<'_>> = Vec::new();
    let items: Vec<Item<'_>> = items(pages);
    for it in &items {
        match it {
            Item::Para(runs, lvl) => {
                blocks.push(HtmlBlock::Para {
                    runs: runs
                        .iter()
                        .map(|r| HtmlRun { text: &r.text, bold: r.block.bold, italic: r.block.italic, color: rgb8(r.block.color) })
                        .collect(),
                    level: *lvl,
                });
            }
            Item::Table(t) => {
                blocks.push(HtmlBlock::Table(HtmlTable {
                    rows: t
                        .rows
                        .iter()
                        .map(|row| {
                            row.iter()
                                .map(|c| HtmlCell { text: &c.text, bold: c.bold, italic: c.italic, color: rgb8(c.color), span: c.span })
                                .collect()
                        })
                        .collect(),
                }));
            }
            // HTML embeds each placement in full, so repeated pictures also consume the budget.
            Item::Img(im) if room_for(&embedded, im) => {
                let mime = if im.ext == "jpg" { "image/jpeg" } else { "image/png" };
                blocks.push(HtmlBlock::Image { mime, bytes: &im.bytes });
                embedded.push((String::new(), im));
            }
            Item::Img(_) => {}
            Item::PageBreak => blocks.push(HtmlBlock::PageBreak),
        }
    }
    html::render(&blocks, title, simple, language_tag(language))
}

#[cfg(test)]
mod tests {
    use super::{html, para_html, table_html};
    use crate::layout::{Block, Cell, Page, Run, Table};

    fn block(text: &str, x0: f64, y0: f64, x1: f64, y1: f64) -> Block {
        Block {
            text: text.into(),
            rect: [x0, y0, x1, y1],
            size: 12.0,
            bold: false,
            italic: false,
            color: [0.0; 3],
            rotation: 0.0,
            space_around: [false; 2],
        }
    }

    #[test]
    fn empty_or_control_only_runs_and_blocks_do_not_emit_empty_tags() {
        let page = Page {
            width: 612.0,
            height: 792.0,
            blocks: vec![
                Block { bold: true, italic: true, color: [1.0, 0.0, 0.0], ..block("\u{1}\u{FFFF}", 72.0, 700.0, 90.0, 712.0) },
                Block { color: [0.0, 0.5, 0.0], ..block("Hello", 90.5, 700.0, 140.0, 712.0) },
                Block { bold: true, ..block("\u{0}\u{2}", 72.0, 660.0, 120.0, 672.0) },
            ],
            images: Vec::new(),
            background: None,
            rules: Vec::new(),
        };
        let out = html(&[page], "Clean");
        assert!(out.contains("<p style=\"color:#008000\">Hello</p>"), "{out}");
        assert!(!out.contains("<strong></strong>"), "{out}");
        assert!(!out.contains("<em></em>"), "{out}");
        assert_eq!(out.matches("<p").count(), 1, "{out}");

        // Styled whitespace-only paragraphs are omitted, and leading whitespace runs do not set
        // the paragraph colour or emit emphasis/colour wrappers.
        let ws_block = Block { bold: true, italic: true, color: [1.0, 0.0, 0.0], ..block("   ", 72.0, 700.0, 80.0, 712.0) };
        let vis_block = Block { color: [0.0, 0.5, 0.0], ..block("Visible", 80.0, 700.0, 120.0, 712.0) };
        assert_eq!(para_html(&[Run { text: "   \t\n".into(), block: &ws_block }], 0, false), None);
        assert_eq!(
            para_html(&[Run { text: " ".into(), block: &ws_block }, Run { text: "Visible".into(), block: &vis_block }], 0, false).as_deref(),
            Some("<p style=\"color:#008000\"> Visible</p>\n")
        );
    }

    #[test]
    fn heading_level_is_clamped_and_empty_styled_cells_stay_clean() {
        let b = block("Deep Heading", 72.0, 700.0, 200.0, 712.0);
        let runs = [Run { text: "Deep Heading".into(), block: &b }];
        assert_eq!(para_html(&runs, 9, true).as_deref(), Some("<h6>Deep Heading</h6>\n"));

        let table = Table {
            rect: [0.0, 0.0, 100.0, 20.0],
            cols: vec![0.0, 50.0],
            rows: vec![vec![
                Cell { text: "\u{1}".into(), size: 11.0, bold: true, italic: true, color: [0.0; 3], span: 1 },
                Cell { text: "Val".into(), size: 11.0, bold: true, italic: false, color: [0.0; 3], span: 1 },
            ]],
            ruled: None,
        };
        assert_eq!(table_html(&table, true), "<table>\n<tr><td></td><td><strong>Val</strong></td></tr>\n</table>\n");
    }
}
