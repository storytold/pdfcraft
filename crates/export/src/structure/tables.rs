//! What the export's tables hold beyond their cells: their header rows, the cells that span
//! rows, their captions and footnotes, and the table on an earlier page each continues.

use super::figures::{Caption, CaptionKind, pair_captions};
use crate::{Cell, Page, Table};

/// A table of a page, as the other exports detect it, and its structure.
#[derive(Clone, Debug, PartialEq)]
pub struct TableStructure {
    pub table: Table,
    /// How many of its first rows are its header.
    pub header_rows: usize,
    /// Per row of `table.rows`, per cell: how many rows it spans down; 0 for an empty cell that
    /// the cell above it spans.
    pub row_spans: Vec<Vec<usize>>,
    /// Its caption in [`PageStructure::captions`](super::PageStructure::captions).
    pub caption: Option<usize>,
    /// The blocks of [`Page::blocks`] under it that are its notes ("Note 1:", "(2)", "*"), in
    /// order.
    pub notes: Vec<usize>,
    /// The table it continues on the page before: that page's index, and the table's index in
    /// its [`PageStructure::tables`](super::PageStructure::tables).
    pub continues: Option<(usize, usize)>,
    /// Whether its header rows repeat the header of the table it continues.
    pub repeats_header: bool,
}

/// A note is at most this far (in its font size) below the table or the note before it.
const NOTE_GAP: f64 = 1.5;

/// A table has at most this many notes.
const MAX_NOTES: usize = 32;

/// Column edges of a table and of the one it continues differ by at most this much (points).
const EDGE_TOL: f64 = 8.0;

/// Comparisons a page may spend finding its tables' captions and notes; past it the tables that
/// are left have none.
const MAX_TABLE_WORK: usize = 1 << 20;

/// The structure of a page's `tables`: their header rows and row spans, the table captions and
/// the notes under them. `consumed` and `running` say per block whether a table holds it and
/// whether it is a running header or footer.
pub(super) fn page_tables(page: &Page, mut tables: Vec<Table>, captions: &[Caption], running: &[bool], consumed: &[bool]) -> Vec<TableStructure> {
    let mut budget = MAX_TABLE_WORK;
    let inner: Vec<Option<usize>> = tables.iter_mut().map(|t| caption_row(page, t, captions, &mut budget)).collect();
    let inner_notes: Vec<Vec<usize>> = tables.iter_mut().map(|t| note_rows(page, t, &mut budget)).collect();
    let rects: Vec<[f64; 4]> = tables.iter().map(|t| t.rect).collect();
    let wanted = |ci: usize, c: &Caption| c.kind == CaptionKind::Table && !inner.contains(&Some(ci));
    let pairs = pair_captions(page, captions, wanted, &rects, false, &mut budget).unwrap_or_default();
    let mut caption_blocks = vec![false; page.blocks.len()];
    for c in captions {
        if let Some(b) = caption_blocks.get_mut(c.block) {
            *b = true;
        }
    }
    let mut noted = vec![false; page.blocks.len()];
    tables
        .into_iter()
        .enumerate()
        .map(|(k, table)| {
            let header_rows = header_rows(&table.rows, table.cols.len());
            let row_spans = row_spans(&table, header_rows);
            let caption = inner.get(k).copied().flatten().or_else(|| pairs.iter().find(|p| p.1 == k).map(|p| p.0));
            let free = |bi: usize| {
                let flag = |v: &[bool]| v.get(bi).copied().unwrap_or(false);
                !flag(running) && !flag(consumed) && !flag(&noted) && !flag(&caption_blocks)
            };
            let mut notes = inner_notes.get(k).cloned().unwrap_or_default();
            notes.extend(notes_under(page, &table.rect, free, MAX_NOTES.saturating_sub(notes.len()), &mut budget));
            for &bi in &notes {
                if let Some(n) = noted.get_mut(bi) {
                    *n = true;
                }
            }
            TableStructure { table, header_rows, row_spans, caption, notes, continues: None, repeats_header: false }
        })
        .collect()
}

/// The table caption that the detection took into `table` as its first or last row (a caption
/// aligned with the first column, as wide as two, reads as a band over the table): that row is
/// taken out, and the table's box ends at the caption.
fn caption_row(page: &Page, table: &mut Table, captions: &[Caption], budget: &mut usize) -> Option<usize> {
    let only_text = |row: &Vec<Cell>| {
        let mut texts = row.iter().filter(|c| !c.text.is_empty());
        texts.next().filter(|_| texts.next().is_none()).map(|c| c.text.clone())
    };
    let ends = [(0, true), (table.rows.len().checked_sub(1)?, false)];
    for (r, first) in ends {
        let Some(text) = table.rows.get(r).and_then(only_text) else { continue };
        *budget = budget.checked_sub(captions.len())?;
        let found = captions.iter().enumerate().find(|(_, c)| {
            c.kind == CaptionKind::Table
                && page
                    .blocks
                    .get(c.block)
                    .is_some_and(|b| b.text.trim() == text && b.rect[1] >= table.rect[1] - 1.0 && b.rect[3] <= table.rect[3] + 1.0)
        });
        if let Some((ci, c)) = found
            && let Some(b) = page.blocks.get(c.block)
            && table.rows.len() > 2
        {
            table.rows.remove(r);
            if first {
                table.rect[3] = b.rect[1];
            } else {
                table.rect[1] = b.rect[3];
            }
            return Some(ci);
        }
    }
    None
}

/// The notes that the detection took into `table` as its last rows (a note aligned with the
/// first column, as wide as two): those rows are taken out, and the table's box ends above
/// them. Their blocks, top to bottom.
fn note_rows(page: &Page, table: &mut Table, budget: &mut usize) -> Vec<usize> {
    let mut out = Vec::new();
    while table.rows.len() > 2 && out.len() < MAX_NOTES {
        let Some(row) = table.rows.last() else { break };
        let mut texts = row.iter().filter(|c| !c.text.is_empty());
        let (Some(cell), None) = (texts.next(), texts.next()) else { break };
        let Some(left) = budget.checked_sub(page.blocks.len()) else { break };
        *budget = left;
        let found =
            page.blocks.iter().enumerate().find(|(bi, b)| {
                !out.contains(bi) && b.text.trim() == cell.text && b.rect[1] >= table.rect[1] - 1.0 && b.rect[3] <= table.rect[3] + 1.0
            });
        let Some((bi, b)) = found.filter(|(_, b)| is_note(&b.text)) else { break };
        table.rect[1] = b.rect[3];
        table.rows.pop();
        out.insert(0, bi);
    }
    out
}

/// How many of `rows` (a table of `ncols` grid columns) are its header: the first rows whose
/// text is all bold, while a later row is not; or else a first row with a cell spanning some
/// but not all columns (a group over its sub-columns, "Limits" over "Min Typ Max") and the row
/// under it, while a row follows them.
fn header_rows(rows: &[Vec<Cell>], ncols: usize) -> usize {
    let bold = |row: &Vec<Cell>| row.iter().any(|c| !c.text.is_empty()) && row.iter().filter(|c| !c.text.is_empty()).all(|c| c.bold);
    let leading = rows.iter().take_while(|r| bold(r)).count();
    if leading > 0 && leading < rows.len() {
        return leading;
    }
    let grouped = rows.first().is_some_and(|r| r.iter().any(|c| c.span > 1 && c.span < ncols));
    if grouped && rows.len() > 2 { 2 } else { 0 }
}

/// Per row, per cell: the rows it spans (see [`TableStructure::row_spans`]). Below the header
/// of an unruled table, an empty one-column cell is spanned by the cell above it when every cell
/// before it on its row is spanned that way too and a cell after it has text: the rows of one
/// parameter's conditions under its name. A ruled table's rules separate every row, so none
/// spans.
fn row_spans(table: &Table, header_rows: usize) -> Vec<Vec<usize>> {
    let mut spans: Vec<Vec<usize>> = table.rows.iter().map(|r| vec![1; r.len()]).collect();
    if table.ruled.is_some() {
        return spans;
    }
    let ncols = table.cols.len();
    // Per grid column, the cell (row, index) that a cell under it may extend.
    let mut open: Vec<Option<(usize, usize)>> = vec![None; ncols];
    for (r, row) in table.rows.iter().enumerate() {
        let mut column = 0;
        let mut all_spanned = true;
        for (k, cell) in row.iter().enumerate() {
            let rest_has_text = row.get(k + 1..).unwrap_or_default().iter().any(|c| !c.text.is_empty());
            let owner = open.get(column).copied().flatten();
            match owner {
                Some((orow, ocell)) if r >= header_rows && all_spanned && cell.span == 1 && cell.text.is_empty() && rest_has_text => {
                    if let Some(s) = spans.get_mut(orow).and_then(|s| s.get_mut(ocell)) {
                        *s += 1;
                    }
                    if let Some(s) = spans.get_mut(r).and_then(|s| s.get_mut(k)) {
                        *s = 0;
                    }
                }
                _ => {
                    all_spanned = false;
                    let starts = r >= header_rows && cell.span == 1 && !cell.text.is_empty();
                    for g in column..column.saturating_add(cell.span).min(ncols) {
                        if let Some(o) = open.get_mut(g) {
                            *o = (starts && g == column).then_some((r, k));
                        }
                    }
                }
            }
            column = column.saturating_add(cell.span);
        }
        // Columns past the row's last cell are not continued.
        for o in open.iter_mut().skip(column) {
            *o = None;
        }
    }
    spans
}

/// At most `max` notes under a table at `rect`: the blocks `free` allows that start with a note
/// mark, each at most [`NOTE_GAP`] lines below the table or the note before it and starting
/// across it.
fn notes_under(page: &Page, rect: &[f64; 4], free: impl Fn(usize) -> bool, max: usize, budget: &mut usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut bottom = rect[1];
    while out.len() < max {
        let Some(left) = budget.checked_sub(page.blocks.len()) else { break };
        *budget = left;
        let next = page
            .blocks
            .iter()
            .enumerate()
            .filter(|&(bi, b)| {
                let gap = bottom - b.rect[3];
                free(bi)
                    && !out.contains(&bi)
                    && b.rect[0] >= rect[0] - EDGE_TOL
                    && b.rect[0] < rect[2]
                    && (-2.0..=NOTE_GAP * b.size.max(1.0)).contains(&gap)
            })
            .min_by(|a, b| (bottom - a.1.rect[3]).total_cmp(&(bottom - b.1.rect[3])));
        match next {
            Some((bi, b)) if is_note(&b.text) => {
                out.push(bi);
                bottom = b.rect[1];
            }
            _ => break,
        }
    }
    out
}

/// Whether `text` starts with a note's mark: "Note" or "Notes" and a separator, a number or a
/// letter in parentheses or followed by `.` or `)`, or `*`, `†` or `‡`.
fn is_note(text: &str) -> bool {
    let t = text.trim_start();
    if t.starts_with(['*', '\u{2020}', '\u{2021}']) {
        return true;
    }
    let word = t.get(..4).is_some_and(|w| w.eq_ignore_ascii_case("note"));
    if word {
        let rest = t.get(4..).unwrap_or_default();
        let rest = rest.strip_prefix(['s', 'S']).unwrap_or(rest);
        return rest.is_empty() || rest.starts_with([':', ' ', '.', '\u{a0}']) || rest.starts_with(|c: char| c.is_ascii_digit());
    }
    let b = t.as_bytes();
    let mark = |s: &[u8]| {
        let digits = s.iter().take_while(|c| c.is_ascii_digit()).count();
        let len = if digits > 0 && digits <= 2 {
            digits
        } else if s.first().is_some_and(u8::is_ascii_lowercase) {
            1
        } else {
            0
        };
        (len > 0).then_some(len)
    };
    match b.first() {
        Some(b'(') => b.get(1..).and_then(mark).is_some_and(|n| b.get(n + 1) == Some(&b')')),
        Some(_) => mark(b).is_some_and(|n| matches!(b.get(n), Some(b'.' | b')')) && b.get(n + 1).is_none_or(|c| c.is_ascii_whitespace())),
        None => false,
    }
}

/// What a page's last table needs to be continued on the next page.
pub(super) struct Open<'a> {
    pub(super) page: usize,
    pub(super) table: &'a TableStructure,
    /// Its caption's number, when it has one.
    pub(super) number: Option<&'a str>,
    /// Whether nothing but running blocks and its notes lies under it on its page.
    pub(super) ends_page: bool,
}

/// Whether `t`, the first table of page `page`, continues `prev`, the last table of the page
/// before; and if so whether its first rows repeat that table's header. It does when their
/// column edges match and its caption continues `prev`'s, its first rows repeat `prev`'s header,
/// or, with no caption, `prev` ends its page and nothing but running blocks lies above `t`.
pub(super) fn continues(prev: &Open, page: usize, t: &TableStructure, captions: &[Caption], starts_page: bool) -> Option<bool> {
    let (a, b) = (&prev.table.table.cols, &t.table.cols);
    if prev.page.checked_add(1) != Some(page) || a.len() != b.len() || a.iter().zip(b).any(|(x, y)| (x - y).abs() > EDGE_TOL) {
        return None;
    }
    let header = prev.table.header_rows;
    let repeats = header > 0 && t.table.rows.len() > header && same_text(t.table.rows.get(..header)?, prev.table.table.rows.get(..header)?);
    let caption = t.caption.and_then(|c| captions.get(c));
    let continued = caption.is_some_and(|c| c.continued && Some(c.number.as_str()) == prev.number);
    (repeats || continued || (caption.is_none() && prev.ends_page && starts_page)).then_some(repeats)
}

/// Whether two runs of rows hold the same text, cell by cell.
fn same_text(a: &[Vec<Cell>], b: &[Vec<Cell>]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.len() == y.len() && x.iter().zip(y).all(|(c, d)| c.text == d.text))
}

/// A page's top table and whether nothing but running blocks lies above it, and its bottom table
/// and whether nothing but running blocks and its notes lies under it.
pub(super) fn page_ends(page: &Page, tables: &[TableStructure], running: &[bool], consumed: &[bool]) -> [Option<(usize, bool)>; 2] {
    let other = |bi: usize, notes: &[usize]| {
        !running.get(bi).copied().unwrap_or(false) && !consumed.get(bi).copied().unwrap_or(false) && !notes.contains(&bi)
    };
    let top = tables.iter().enumerate().max_by(|a, b| a.1.table.rect[3].total_cmp(&b.1.table.rect[3]));
    let bottom = tables.iter().enumerate().min_by(|a, b| a.1.table.rect[1].total_cmp(&b.1.table.rect[1]));
    let top = top.map(|(k, t)| (k, !page.blocks.iter().enumerate().any(|(bi, b)| other(bi, &[]) && b.rect[1] >= t.table.rect[3] - 2.0)));
    let bottom = bottom.map(|(k, t)| (k, !page.blocks.iter().enumerate().any(|(bi, b)| other(bi, &t.notes) && b.rect[3] <= t.table.rect[1] + 2.0)));
    [top, bottom]
}
