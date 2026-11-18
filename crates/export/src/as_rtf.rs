//! Rich Text Format through craft-rtf's writer: this module maps the pages onto its small model
//! (storytold/craft-libs#793).

use super::as_docx::rgb8;
use super::as_html::allowed;
use super::layout::{Item, Page, Table, items};
use craft_rtf::{Block, Cell, CharFmt, Document, PAGE_BREAK, Para, Run};

/// The text as RTF may hold it: the characters Word refuses are left out.
fn clean(s: &str) -> String {
    s.chars().filter(|c| allowed(*c)).collect()
}

/// A run's formatting: its size in half points, bold, italic and colour (none for black, which
/// is the default).
fn char_fmt(size: f64, bold: bool, italic: bool, color: [f64; 3]) -> CharFmt {
    let color = Some(rgb8(color)).filter(|c| *c != [0, 0, 0]);
    // A size that is not finite or below zero becomes 0, as the cast saturates.
    CharFmt { bold, italic, half_points: Some((size * 2.0).round() as u32), color, ..CharFmt::default() }
}

/// A table's rows. Each cell ends at the right edge of the last grid column it spans, in twips.
fn table(t: &Table) -> craft_rtf::Table {
    let mut edges = t.cols.clone();
    edges.push(t.rect[2]);
    let cellx: Vec<i32> = edges.iter().skip(1).map(|e| (e * 20.0).round() as i32).collect();
    let rows = t
        .rows
        .iter()
        .map(|row| {
            let mut column = 0;
            row.iter()
                .map(|c| {
                    column += c.span.max(1);
                    let right = cellx.get(column.min(cellx.len()).saturating_sub(1)).copied().unwrap_or(0);
                    let run = Run { text: clean(&c.text), fmt: char_fmt(c.size, c.bold, c.italic, c.color) };
                    Cell { right, paras: vec![Para { runs: vec![run], ..Para::default() }] }
                })
                .collect()
        })
        .collect();
    craft_rtf::Table { rows }
}

/// Keep a break outside tables, including when no text follows it on its page.
fn standalone_page_break(blocks: &mut Vec<Block>) {
    let run = Run { text: PAGE_BREAK.to_string(), fmt: CharFmt::default() };
    blocks.push(Block::Para(Para { runs: vec![run], ..Para::default() }));
}

/// Rich Text Format: paragraphs with their sizes, bold, italic and colours, real table rows, page
/// breaks between pages (images are left out).
pub fn rtf(pages: &[Page]) -> String {
    let mut blocks = Vec::new();
    // A page break waits for what follows it: RTF's `\page` is a character in a paragraph, so it
    // starts the next one, or stands alone before a table.
    let mut page_break = false;
    for it in items(pages) {
        match it {
            Item::Para(runs, _) => {
                let mut runs: Vec<Run> = runs
                    .iter()
                    .map(|r| Run { text: clean(&r.text), fmt: char_fmt(r.block.size, r.block.bold, r.block.italic, r.block.color) })
                    .collect();
                let Some(first) = runs.first_mut() else { continue };
                if std::mem::take(&mut page_break) {
                    first.text.insert(0, PAGE_BREAK);
                }
                blocks.push(Block::Para(Para { runs, ..Para::default() }));
            }
            Item::Table(t) => {
                if std::mem::take(&mut page_break) {
                    standalone_page_break(&mut blocks);
                }
                blocks.push(Block::Table(table(&t)));
            }
            Item::Img(_) => {}
            Item::PageBreak => {
                if page_break {
                    standalone_page_break(&mut blocks);
                }
                page_break = true;
            }
        }
    }
    if page_break {
        standalone_page_break(&mut blocks);
    }
    craft_rtf::write(&Document { blocks, pictures_left_out: 0 }, "Helvetica")
}
