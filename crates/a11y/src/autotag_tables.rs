//! Borderless tables among auto-tagged text objects, found from where their text sits.
//!
//! Single-line text objects on one baseline form a row; within a row, objects closer than one font
//! size join one chunk, and a wider gap separates two. Three or more consecutive rows of at least
//! two chunks, each at most [`ROW_GAP_EM`] font sizes below the one before, are a candidate. Its
//! chunks' horizontal spans, merged where they overlap or, from different rows, nearly touch, are
//! its columns. A candidate is a table when it has at least two columns, no row has two chunks in
//! one column, every column holds text in at least two rows, in each column at least two cells and
//! at least half of them share a left edge, a right edge or a centre to within [`ALIGN_EM`] font
//! sizes, each column's cells average at most [`MAX_MEAN_CELL_CHARS`] characters, and no other text
//! lies among its rows (a wrapped line, or text whose place is unknown, drawn between its cells);
//! otherwise it stays paragraphs and is reported for review. A column of prose lines set on the
//! rows' baselines beside a table therefore keeps the whole block paragraphs.
//! The first row is a header row when it is bold and the others are not, or when it is set apart
//! by a gap at least [`APART_RATIO`] times every other row gap. Ruling lines are not read, and
//! cells that span rows or columns are not recognised.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use crate::autotag_roles::Evidence;

const MIN_ROWS: usize = 3;
/// A gap between two text objects on one line this many font sizes wide separates two cells.
const GUTTER_EM: f64 = 1.0;
/// Rows further apart than this many font sizes end a table.
pub(crate) const ROW_GAP_EM: f64 = 2.5;
/// Cell edges this many font sizes apart or less are aligned.
pub(crate) const ALIGN_EM: f64 = 0.25;
/// A column whose cells average more characters than this is prose.
pub(crate) const MAX_MEAN_CELL_CHARS: usize = 30;
const MAX_COLUMNS: usize = 32;
const MAX_ROWS: usize = 500;
/// A first row whose gap to the second is this many times every other gap is set apart.
pub(crate) const APART_RATIO: f64 = 1.5;

/// One detected table: rows top to bottom, each row's cells left to right, each cell the indices
/// of its text objects in the page's order (possibly none).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Grid {
    pub rows: Vec<Vec<Vec<usize>>>,
    pub header: bool,
}

/// A block of aligned rows: a table, or the reason it stays paragraphs.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Candidate {
    /// `[x0, y0, x1, y1]` in user space around the rows' text.
    pub rect: [f64; 4],
    pub rows: usize,
    pub outcome: Result<Grid, &'static str>,
}

#[derive(Clone, Copy)]
struct Item {
    index: usize,
    x0: f64,
    x1: f64,
    y: f64,
    size: f64,
    chars: usize,
    bold: bool,
}

struct Chunk {
    x0: f64,
    x1: f64,
    items: Vec<Item>,
}

struct Row {
    y: f64,
    size: f64,
    chunks: Vec<Chunk>,
}

impl Row {
    fn span(&self) -> (f64, f64) {
        self.chunks.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), c| (lo.min(c.x0), hi.max(c.x1)))
    }
}

/// The table candidates among one page's text objects, top to bottom.
pub(crate) fn find(objects: &[Evidence]) -> Vec<Candidate> {
    let (mut items, mut others) = (Vec::new(), Vec::new());
    for (index, e) in objects.iter().enumerate().filter(|(_, e)| e.chars > 0 || e.unmapped) {
        match e.place.filter(|_| e.lines == 1 && e.size > 0.0) {
            Some([x0, y, x1, _]) => items.push(Item { index, x0, x1, y, size: e.size, chars: e.chars, bold: e.bold }),
            None => others.push((index, e.place)),
        }
    }
    let rows = rows(items);
    let mut out = Vec::new();
    let mut block: Vec<&Row> = Vec::new();
    for row in &rows {
        let tabular = row.chunks.len() >= 2;
        let joins = tabular && block.last().is_none_or(|above| above.y - row.y <= ROW_GAP_EM * above.size.max(row.size));
        if !joins {
            flush(&mut block, &others, &mut out);
        }
        if tabular {
            block.push(row);
        }
    }
    flush(&mut block, &others, &mut out);
    out
}

/// Text lines top to bottom, each split into chunks at gaps of a gutter's width.
fn rows(mut items: Vec<Item>) -> Vec<Row> {
    items.sort_by(|a, b| b.y.total_cmp(&a.y).then(a.x0.total_cmp(&b.x0)));
    let mut lines: Vec<Vec<Item>> = Vec::new();
    for item in items {
        match lines.last_mut() {
            Some(line) if line.first().is_some_and(|f| (f.y - item.y).abs() <= 0.5 * f.size) => line.push(item),
            _ => lines.push(vec![item]),
        }
    }
    lines
        .into_iter()
        .map(|mut line| {
            line.sort_by(|a, b| a.x0.total_cmp(&b.x0));
            let size = line.iter().map(|i| i.size).fold(0.0, f64::max);
            let y = line.first().map_or(0.0, |i| i.y);
            let mut chunks: Vec<Chunk> = Vec::new();
            for item in line {
                match chunks.last_mut() {
                    Some(c) if item.x0 - c.x1 < GUTTER_EM * item.size => {
                        c.x1 = c.x1.max(item.x1);
                        c.items.push(item);
                    }
                    _ => chunks.push(Chunk { x0: item.x0, x1: item.x1, items: vec![item] }),
                }
            }
            Row { y, size, chunks }
        })
        .collect()
}

/// Records `block` as a candidate when it has enough rows. `others` are the page's text objects
/// that are not one placed line, by index and place if known.
fn flush(block: &mut Vec<&Row>, others: &[(usize, Option<[f64; 4]>)], out: &mut Vec<Candidate>) {
    if block.len() >= MIN_ROWS {
        let rect = block.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |[x0, y0, x1, y1], r| {
            let (lo, hi) = r.span();
            [x0.min(lo), y0.min(r.y), x1.max(hi), y1.max(r.y + r.size)]
        });
        let outcome = if block.len() > MAX_ROWS {
            Err("it has more than 500 rows")
        } else if intruded(block, rect, others) {
            Err("other text lies among its rows")
        } else {
            grid(block)
        };
        out.push(Candidate { rect, rows: block.len(), outcome });
    }
    block.clear();
}

/// Whether text that is not a cell would be read out of place: a wrapped text object inside the
/// rows' bounds, or one whose place is unknown drawn between the first and last cell.
fn intruded(block: &[&Row], [x0, y0, x1, y1]: [f64; 4], others: &[(usize, Option<[f64; 4]>)]) -> bool {
    let indices = || block.iter().flat_map(|r| r.chunks.iter().flat_map(|c| c.items.iter().map(|i| i.index)));
    let (Some(first), Some(last)) = (indices().min(), indices().max()) else { return false };
    others.iter().any(|&(index, place)| match place {
        Some([a0, b0, a1, b1]) => a1 > x0 && a0 < x1 && b1 >= y0 && b0 <= y1,
        None => index > first && index < last,
    })
}

/// The block's columns and cells, or why it is not a table.
fn grid(rows: &[&Row]) -> Result<Grid, &'static str> {
    let mut sizes: Vec<f64> = rows.iter().map(|r| r.size).collect();
    sizes.sort_by(f64::total_cmp);
    let median = sizes.get(sizes.len() / 2).copied().unwrap_or(0.0);
    let gutter = GUTTER_EM * median;
    let mut spans: Vec<(f64, f64, usize)> = rows.iter().enumerate().flat_map(|(at, r)| r.chunks.iter().map(move |c| (c.x0, c.x1, at))).collect();
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    // A span joins the column it overlaps, or one it nearly touches where its row has no cell yet:
    // two cells of one row are two columns.
    let mut columns: Vec<(f64, f64, HashSet<usize>)> = Vec::new();
    for (a, b, row) in spans {
        match columns.last_mut() {
            Some(c) if a < c.1 || (a < c.1 + gutter && !c.2.contains(&row)) => {
                c.1 = c.1.max(b);
                c.2.insert(row);
            }
            _ => columns.push((a, b, HashSet::from([row]))),
        }
    }
    if columns.len() < 2 {
        return Err("its columns do not line up from row to row");
    }
    if columns.len() > MAX_COLUMNS {
        return Err("it has more than 32 columns");
    }
    let mut cells = vec![vec![Vec::new(); columns.len()]; rows.len()];
    // Per column: each filled cell's left and right edge, and the characters of its cells.
    let (mut edges, mut chars) = (vec![Vec::new(); columns.len()], vec![0usize; columns.len()]);
    for (row, out) in rows.iter().zip(cells.iter_mut()) {
        let mut spans: Vec<Option<(f64, f64)>> = vec![None; columns.len()];
        for chunk in &row.chunks {
            let column = columns.iter().position(|c| chunk.x0 >= c.0 && chunk.x1 <= c.1).ok_or("a cell crosses two columns")?;
            let (Some(cell), Some(span), Some(count)) = (out.get_mut(column), spans.get_mut(column), chars.get_mut(column)) else {
                return Err("a cell crosses two columns");
            };
            if span.is_some() {
                // A wider cell in another row spans both: cells that span columns are not read.
                return Err("a row has two cells in one column");
            }
            *span = Some((chunk.x0, chunk.x1));
            cell.extend(chunk.items.iter().map(|i| i.index));
            *count = count.saturating_add(chunk.items.iter().fold(0, |n, i| n.saturating_add(i.chars)));
        }
        for (column, span) in edges.iter_mut().zip(spans) {
            column.extend(span);
        }
    }
    if edges.iter().any(|e| e.len() < 2) {
        return Err("a column has text in only one row");
    }
    if !edges.iter().all(|e| aligned(e, ALIGN_EM * median)) {
        return Err("its columns do not line up from row to row");
    }
    if chars.iter().zip(&edges).any(|(&n, e)| n > MAX_MEAN_CELL_CHARS.saturating_mul(e.len())) {
        return Err("its cells hold running text");
    }
    Ok(Grid { rows: cells, header: header(rows) })
}

/// Whether at least two of a column's cells, and at least half of them, share a left edge, a
/// right edge or a centre to within `tolerance`.
fn aligned(cells: &[(f64, f64)], tolerance: f64) -> bool {
    let edges: [fn((f64, f64)) -> f64; 3] = [|(a, _)| a, |(_, b)| b, |(a, b)| (a + b) / 2.0];
    let most = edges
        .into_iter()
        .map(|edge| {
            let mut at: Vec<f64> = cells.iter().map(|&c| edge(c)).collect();
            at.sort_by(f64::total_cmp);
            let (mut start, mut most) = (0usize, 0usize);
            for (end, &x) in at.iter().enumerate() {
                while at.get(start).is_some_and(|&first| x - first > tolerance) {
                    start = start.saturating_add(1);
                }
                most = most.max(end.saturating_add(1).saturating_sub(start));
            }
            most
        })
        .max()
        .unwrap_or(0);
    most >= 2 && most.saturating_mul(2) >= cells.len()
}

/// Whether the first row heads the table: bold over text that is not, or set apart by its gap.
fn header(rows: &[&Row]) -> bool {
    let bold_share = |rows: &[&Row]| {
        let (mut all, mut bold) = (0usize, 0usize);
        for item in rows.iter().flat_map(|r| r.chunks.iter().flat_map(|c| &c.items)) {
            all = all.saturating_add(item.chars);
            if item.bold {
                bold = bold.saturating_add(item.chars);
            }
        }
        (all > 0).then(|| bold as f64 / all as f64)
    };
    let (first, rest) = (rows.get(..1).unwrap_or_default(), rows.get(1..).unwrap_or_default());
    if let (Some(f), Some(r)) = (bold_share(first), bold_share(rest))
        && f >= 0.6
        && r < 0.3
    {
        return true;
    }
    let gaps: Vec<f64> = rows.windows(2).filter_map(|w| Some(w.first()?.y - w.get(1)?.y)).collect();
    let Some((&under, others)) = gaps.split_first() else { return false };
    !others.is_empty() && others.iter().all(|&g| under >= APART_RATIO * g)
}

/// The text objects, by index on the page, that the page's tables take as cells.
pub(crate) fn tabled(candidates: &[Candidate]) -> HashSet<usize> {
    candidates.iter().filter_map(|c| c.outcome.as_ref().ok()).flat_map(|g| g.rows.iter().flatten().flatten().copied()).collect()
}

/// Each table by its text object that comes first on the page, where its structure is written.
pub(crate) fn first_cells<'a>(grids: &[&'a Grid]) -> HashMap<usize, &'a Grid> {
    grids.iter().filter_map(|&g| g.rows.iter().flatten().flatten().min().map(|&first| (first, g))).collect()
}

/// The structure elements [`write`] made for one table.
pub(crate) struct Written {
    pub table: ObjRef,
    /// Each text object of the table, by index into the page's, with the cell that holds it and
    /// that cell's type.
    pub cells: Vec<(usize, ObjRef, &'static str)>,
}

fn element(kind: &str, parent: ObjRef) -> Dict {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("StructElem"));
    d.set(b"S".to_vec(), Object::name(kind));
    d.set(b"P".to_vec(), Object::Ref(parent));
    d
}

/// Writes `Table` › `TR` › `TH`/`TD` under `parent` for `grid` on `page`. A cell's `/K` is the
/// marked-content ID of its text object, which is the object's index on the page, or an array of
/// them; an empty cell has none and is a `TD`, in the header row too. Header cells carry
/// `/Scope /Column`.
pub(crate) fn write(next: &mut Document, parent: ObjRef, page: ObjRef, grid: &Grid) -> Result<Written, crate::AutoTagError> {
    let mcid = |i: usize| i64::try_from(i).map(Object::Int).map_err(|_| crate::AutoTagError::TooManyPages);
    let table = next.add(Object::Null);
    let (mut rows, mut cells) = (Vec::with_capacity(grid.rows.len()), Vec::new());
    for (at, row) in grid.rows.iter().enumerate() {
        let tr = next.add(Object::Null);
        let mut kids = Vec::with_capacity(row.len());
        for objects in row {
            // A missing cell is an empty `TD`, in the header row too.
            let kind = if grid.header && at == 0 && !objects.is_empty() { "TH" } else { "TD" };
            let mut cell = element(kind, tr);
            cell.set(b"Pg".to_vec(), Object::Ref(page));
            match objects.as_slice() {
                [] => {}
                [one] => cell.set(b"K".to_vec(), mcid(*one)?),
                many => cell.set(b"K".to_vec(), Object::Array(many.iter().map(|&i| mcid(i)).collect::<Result<_, _>>()?)),
            }
            if kind == "TH" {
                let mut attributes = Dict::new();
                attributes.set(b"O".to_vec(), Object::name("Table"));
                attributes.set(b"Scope".to_vec(), Object::name("Column"));
                cell.set(b"A".to_vec(), Object::Dict(attributes));
            }
            let cell = next.add(cell);
            kids.push(Object::Ref(cell));
            cells.extend(objects.iter().map(|&i| (i, cell, kind)));
        }
        let mut tr_element = element("TR", table);
        tr_element.set(b"K".to_vec(), Object::Array(kids));
        next.set(tr, tr_element);
        rows.push(Object::Ref(tr));
    }
    let mut table_element = element("Table", parent);
    table_element.set(b"K".to_vec(), Object::Array(rows));
    next.set(table, table_element);
    Ok(Written { table, cells })
}

#[cfg(test)]
mod tests {
    use pdfcraft_cos::{Document, ObjRef, Object};

    use crate::tests::{pdf, stream};
    use crate::{AutoTagResult, Options, Rule, Status, autotag_text, check, table_candidates, tag_content, tags};

    /// One text object at `(x, y)` in 10 pt Helvetica (`F1`) or Helvetica-Bold (`F2`).
    fn cell(font: &str, x: f64, y: f64, text: &str) -> String {
        format!("BT /{font} 10 Tf {x} {y} Td ({text}) Tj ET ")
    }

    /// A row of cells at `y` with left edges at `xs`; an empty string leaves the cell out.
    fn row(font: &str, y: f64, xs: &[f64], texts: &[&str]) -> String {
        xs.iter().zip(texts).filter(|(_, t)| !t.is_empty()).map(|(&x, t)| cell(font, x, y, t)).collect()
    }

    /// A one-page document drawing `content`, with `extra` added to the page dictionary, in
    /// Helvetica (`F1`), Helvetica-Bold (`F2`) and a two-byte Type 0 font with no Unicode mapping
    /// (`F3`).
    fn page(content: &str, extra: &str) -> Document {
        pdf(
            &[
                "<< /Type /Catalog /Pages 2 0 R >>".into(),
                "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
                format!(
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R {extra} /Resources << /Font << /F1 5 0 R /F2 6 0 R /F3 7 0 R >> >> >>"
                ),
                stream(content),
                "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
                "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold >>".into(),
                "<< /Type /Font /Subtype /Type0 /BaseFont /House /Encoding /Identity-H /DescendantFonts [8 0 R] >>".into(),
                "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /House /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /DW 1000 >>".into(),
            ],
            "",
        )
    }

    const XS: [f64; 3] = [72.0, 200.0, 330.0];

    fn kinds(doc: &Document) -> Vec<String> {
        tags(doc).iter().map(|t| t.kind.clone()).collect()
    }

    /// The tagged-content and table rules that do not pass.
    fn not_passed(doc: &Document) -> Vec<(Rule, Status)> {
        let rules = [Rule::TaggedContent, Rule::TableRows, Rule::TableCells, Rule::TableHeaders, Rule::TableRegularity];
        let report = check(doc, &Options { rules: rules.into(), pages: None });
        rules.iter().filter_map(|&r| report.result(r).map(|x| (r, x.status))).filter(|(_, s)| *s != Status::Passed).collect()
    }

    /// The tag kinds of a document holding one table of three columns with these cells.
    fn table(cells: &[&str]) -> Vec<String> {
        let mut out = vec!["Document".to_string(), "Table".to_string()];
        for row in cells.chunks(3) {
            out.push("TR".into());
            out.extend(row.iter().map(|s| (*s).to_string()));
        }
        out
    }

    fn marked_content(doc: &Document) -> String {
        let page = pdfcraft_model::pages(doc).into_iter().next().unwrap();
        let contents = doc.resolve(page.dict.get(b"Contents").unwrap());
        let Object::Stream(s) = &*contents else { panic!("page content") };
        String::from_utf8(s.decoded_strict().unwrap()).unwrap()
    }

    #[test]
    fn three_rows_in_three_aligned_columns_are_a_table() {
        let content = [
            row("F1", 700.0, &XS, &["Apples", "12", "Red"]),
            row("F1", 686.0, &XS, &["Pears", "7", "Green"]),
            row("F1", 672.0, &XS, &["Plums", "30", "Purple"]),
        ]
        .concat();
        let found = table_candidates(&page(&content, "")).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].rows, found[0].columns, found[0].header, found[0].not_a_table), (3, 3, false, None));
        let mut doc = page(&content, "");
        assert_eq!(autotag_text(&mut doc).unwrap(), AutoTagResult { pages: 1, paragraphs: 0, headings: 0 });
        assert_eq!(kinds(&doc), table(&["TD"; 9]));
        // Without a header row the table fails only the header rule.
        assert_eq!(not_passed(&doc), [(Rule::TableHeaders, Status::Failed)]);
    }

    #[test]
    fn a_bold_first_row_becomes_column_headers_and_the_table_rules_pass() {
        let content = [
            row("F2", 700.0, &XS, &["Fruit", "Count", "Colour"]),
            row("F1", 686.0, &XS, &["Apples", "12", "Red"]),
            row("F1", 672.0, &XS, &["Pears", "7", "Green"]),
            row("F1", 658.0, &XS, &["Plums", "30", "Purple"]),
        ]
        .concat();
        let mut doc = page(&content, "");
        autotag_text(&mut doc).unwrap();
        let mut expected = vec!["TH"; 3];
        expected.extend(["TD"; 9]);
        assert_eq!(kinds(&doc), table(&expected));
        let scopes: Vec<_> = tags(&doc).into_iter().filter(|t| t.kind == "TH").map(|t| t.scope).collect();
        assert_eq!(scopes, vec![Some("Column".to_string()); 3]);
        assert_eq!(not_passed(&doc), []);
        let text = marked_content(&doc);
        assert!(text.contains("/TH <</MCID 0>> BDC") && text.contains("/TD <</MCID 3>> BDC"), "{text}");
    }

    #[test]
    fn a_first_row_set_apart_by_a_wider_gap_is_a_header_row() {
        let content = [
            row("F1", 700.0, &XS, &["Fruit", "Count", "Colour"]),
            row("F1", 680.0, &XS, &["Apples", "12", "Red"]),
            row("F1", 668.0, &XS, &["Pears", "7", "Green"]),
            row("F1", 656.0, &XS, &["Plums", "30", "Purple"]),
        ]
        .concat();
        let mut doc = page(&content, "");
        autotag_text(&mut doc).unwrap();
        assert_eq!(kinds(&doc)[2..6], ["TR", "TH", "TH", "TH"]);
        assert_eq!(not_passed(&doc), []);
    }

    #[test]
    fn an_empty_cell_keeps_every_row_the_same_width() {
        let content = [
            row("F2", 700.0, &XS, &["Fruit", "Count", "Colour"]),
            row("F1", 686.0, &XS, &["Apples", "", "Red"]),
            row("F1", 672.0, &XS, &["Pears", "7", "Green"]),
        ]
        .concat();
        let mut doc = page(&content, "");
        autotag_text(&mut doc).unwrap();
        assert_eq!(kinds(&doc), table(&["TH", "TH", "TH", "TD", "TD", "TD", "TD", "TD", "TD"]));
        assert_eq!(not_passed(&doc), []);
    }

    #[test]
    fn a_ragged_label_and_value_list_stays_paragraphs_and_is_flagged() {
        // Each value follows its label at a gutter's width, so the values start where the labels
        // end, at a different place in each row.
        let content = [
            cell("F1", 72.0, 700.0, "Name:"),
            cell("F1", 112.0, 700.0, "Ada Lovelace"),
            cell("F1", 72.0, 686.0, "Address:"),
            cell("F1", 126.0, 686.0, "12 Hill Street"),
            cell("F1", 72.0, 672.0, "Telephone number:"),
            cell("F1", 172.0, 672.0, "555 0100"),
        ]
        .concat();
        let found = table_candidates(&page(&content, "")).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].not_a_table, Some("its columns do not line up from row to row"));
        let mut doc = page(&content, "");
        assert_eq!(autotag_text(&mut doc).unwrap().paragraphs, 6);
        assert!(!kinds(&doc).iter().any(|k| k == "Table"));
    }

    #[test]
    fn paragraphs_above_and_below_a_table_stay_paragraphs() {
        let prose = "BT /F1 10 Tf 12 TL 72 740 Td (A paragraph of running text that goes on) Tj T* (to a second line above the table.) Tj ET ";
        let content = [
            prose.to_string(),
            row("F2", 700.0, &XS, &["Fruit", "Count", "Colour"]),
            row("F1", 686.0, &XS, &["Apples", "12", "Red"]),
            row("F1", 672.0, &XS, &["Pears", "7", "Green"]),
            cell("F1", 72.0, 660.0, "A note set just under the table."),
        ]
        .concat();
        let mut doc = page(&content, "");
        assert_eq!(autotag_text(&mut doc).unwrap().paragraphs, 2);
        let mut expected = vec!["Document", "P", "Table"];
        for kind in ["TH", "TD", "TD"] {
            expected.extend(["TR", kind, kind, kind]);
        }
        expected.push("P");
        assert_eq!(kinds(&doc), expected);
        assert_eq!(not_passed(&doc), []);
    }

    #[test]
    fn a_table_in_smaller_type_than_the_prose_leaves_the_prose_as_paragraphs() {
        // The table's cells hold more characters than the 12 pt line; were they counted, 10 pt
        // would be the body size and the line a heading.
        let content = [
            "BT /F1 12 Tf 72 740 Td (A line of body text above the table.) Tj ET ".to_string(),
            row("F1", 700.0, &XS, &["Apples and pears", "12 crates", "Red and green"]),
            row("F1", 686.0, &XS, &["Pears", "7 crates", "Green and yellow"]),
            row("F1", 672.0, &XS, &["Plums", "30 crates", "Purple or red"]),
        ]
        .concat();
        let mut doc = page(&content, "");
        assert_eq!(autotag_text(&mut doc).unwrap(), AutoTagResult { pages: 1, paragraphs: 1, headings: 0 });
        assert_eq!(kinds(&doc)[..3], ["Document", "P", "Table"]);
    }

    #[test]
    fn a_paragraph_whose_words_line_up_is_not_a_table() {
        // Each word is its own text object and the lines are set alike, so the words line up in
        // columns, but they are a word space apart, not a gutter.
        let words = ["The", "quick", "brown", "fox"];
        let xs = [72.0, 92.0, 121.0, 154.0];
        let content: String = [700.0, 688.0, 676.0, 664.0].iter().map(|&y| row("F1", y, &xs, &words)).collect();
        assert_eq!(table_candidates(&page(&content, "")).unwrap(), []);
        let mut doc = page(&content, "");
        autotag_text(&mut doc).unwrap();
        assert!(kinds(&doc).iter().skip(1).all(|k| k == "P"));
    }

    #[test]
    fn prose_in_two_columns_is_not_a_table() {
        let left = "This column holds a sentence of prose text";
        let right = "and so does this one, set beside it here.";
        let content: String = [700.0, 688.0, 676.0].iter().map(|&y| row("F1", y, &[72.0, 320.0], &[left, right])).collect();
        let found = table_candidates(&page(&content, "")).unwrap();
        assert_eq!(found.first().and_then(|f| f.not_a_table), Some("its cells hold running text"));
    }

    #[test]
    fn prose_beside_a_table_on_its_baselines_is_not_a_table() {
        let prose = "This column holds a sentence of prose text";
        let rows = [(700.0, "12", "Red"), (686.0, "7", "Green"), (672.0, "30", "Purple")];
        let content: String = rows.iter().map(|&(y, n, c)| row("F1", y, &[72.0, 330.0, 440.0], &[prose, n, c])).collect();
        let found = table_candidates(&page(&content, "")).unwrap();
        assert_eq!(found.first().and_then(|f| f.not_a_table), Some("its cells hold running text"));
        let mut doc = page(&content, "");
        autotag_text(&mut doc).unwrap();
        assert!(!kinds(&doc).iter().any(|k| k == "Table"));
    }

    #[test]
    fn values_that_drift_from_row_to_row_are_not_a_column() {
        // Each value starts 20 units right of the one above: the spans overlap into one column,
        // but no two share an edge.
        let rows = [(700.0, "Name", 200.0), (686.0, "Size", 220.0), (672.0, "Kind", 240.0)];
        let content: String = rows.iter().map(|&(y, label, x)| [cell("F1", 72.0, y, label), cell("F1", x, y, "Value")].concat()).collect();
        let found = table_candidates(&page(&content, "")).unwrap();
        assert_eq!(found.first().and_then(|f| f.not_a_table), Some("its columns do not line up from row to row"));
    }

    #[test]
    fn right_aligned_and_centred_columns_line_up() {
        // Digits are 5.56 units wide at 10 pt in Helvetica, so the numbers end at x 250. Red,
        // Green and Purple are 18.34, 27.79 and 28.9 wide, centred on x 400.
        let rows = [(700.0, "Apples", "7", "Red", 18.34), (686.0, "Pears", "12", "Green", 27.79), (672.0, "Plums", "130", "Purple", 28.9)];
        let content: String = rows
            .iter()
            .map(|&(y, fruit, n, colour, width)| {
                let digits = n.len() as f64;
                [cell("F1", 72.0, y, fruit), cell("F1", 250.0 - 5.56 * digits, y, n), cell("F1", 400.0 - width / 2.0, y, colour)].concat()
            })
            .collect();
        let found = table_candidates(&page(&content, "")).unwrap();
        assert_eq!(found.first().map(|f| (f.columns, f.not_a_table)), Some((3, None)));
    }

    #[test]
    fn a_column_with_text_in_one_row_is_not_a_table() {
        let content = [
            row("F1", 700.0, &XS, &["Apples", "12", "an aside"]),
            row("F1", 686.0, &XS, &["Pears", "7", ""]),
            row("F1", 672.0, &XS, &["Plums", "30", ""]),
        ]
        .concat();
        let found = table_candidates(&page(&content, "")).unwrap();
        assert_eq!(found.first().and_then(|f| f.not_a_table), Some("a column has text in only one row"));
    }

    #[test]
    fn rows_too_far_apart_or_on_a_rotated_page_are_not_read_as_tables() {
        let spread: String = [700.0, 650.0, 600.0].iter().map(|&y| row("F1", y, &XS, &["Apples", "12", "Red"])).collect();
        assert_eq!(table_candidates(&page(&spread, "")).unwrap(), []);
        let close: String = [700.0, 686.0, 672.0].iter().map(|&y| row("F1", y, &XS, &["Apples", "12", "Red"])).collect();
        assert_eq!(table_candidates(&page(&close, "")).unwrap().len(), 1);
        assert_eq!(table_candidates(&page(&close, "/Rotate 90")).unwrap(), []);
    }

    /// The structure element the parent tree names for a marked-content ID on the first page.
    fn parent_of(doc: &Document, mcid: usize) -> Option<ObjRef> {
        let tree = crate::alt::tree_root(doc)?.get(b"ParentTree").and_then(Object::as_ref)?;
        let nums = doc.get(tree).as_dict()?.get(b"Nums")?.as_array()?.clone();
        let parents = doc.resolve(nums.get(1)?).as_array()?.clone();
        parents.get(mcid)?.as_ref()
    }

    #[test]
    fn a_cell_of_two_text_objects_lists_both_and_survives_save_and_reopen() {
        // "Apples" and "and pears" are two text objects a word space apart: one cell.
        let content = [
            row("F2", 700.0, &XS, &["Fruit", "Count", "Colour"]),
            cell("F1", 72.0, 686.0, "Apples"),
            cell("F1", 106.0, 686.0, "and pears"),
            row("F1", 686.0, &XS[1..], &["12", "Red"]),
            row("F1", 672.0, &XS, &["Plums", "30", "Purple"]),
        ]
        .concat();
        let mut doc = page(&content, "");
        autotag_text(&mut doc).unwrap();
        let bytes = pdfcraft_cos::write_incremental(&doc, &pdfcraft_cos::SaveOptions::default()).unwrap();
        let doc = Document::open(std::sync::Arc::new(bytes)).unwrap();
        assert_eq!(kinds(&doc), table(&["TH", "TH", "TH", "TD", "TD", "TD", "TD", "TD", "TD"]));
        let found = tags(&doc);
        assert!(found.iter().filter(|t| t.kind == "TH" || t.kind == "TD").all(|t| t.page == Some(0)), "{found:?}");
        // Objects 3 and 4 are the two halves of the first data cell.
        let joined = found.iter().find(|t| t.kind == "TD").unwrap().object;
        let mcids: Vec<i64> = tag_content(&doc, joined).unwrap().iter().map(|c| c.mcid).collect();
        assert_eq!(mcids, [3, 4]);
        assert_eq!((parent_of(&doc, 3), parent_of(&doc, 4)), (Some(joined), Some(joined)));
        let header = doc.get(found.iter().find(|t| t.kind == "TH").unwrap().object);
        let attributes = header.as_dict().unwrap().get(b"A").unwrap().as_dict().unwrap().clone();
        let names = (attributes.get(b"O").and_then(Object::as_name), attributes.get(b"Scope").and_then(Object::as_name));
        assert_eq!(names, (Some(&b"Table"[..]), Some(&b"Column"[..])));
        assert_eq!(not_passed(&doc), []);
    }

    #[test]
    fn a_wrapped_or_turned_text_object_among_the_rows_keeps_them_paragraphs() {
        let rows = [
            row("F1", 700.0, &XS, &["Apples", "12", "Red"]),
            row("F1", 686.0, &XS, &["Pears", "7", "Green"]),
            row("F1", 672.0, &XS, &["Plums", "30", "Purple"]),
        ];
        // A cell wrapped onto two lines in one text object, in the middle column of the last rows.
        let wrapped = "BT /F1 10 Tf 6 TL 200 680 Td (wrapped) Tj T* (cell) Tj ET ";
        let content = [rows[0].clone(), wrapped.to_string(), rows[1].clone(), rows[2].clone()].concat();
        let found = table_candidates(&page(&content, "")).unwrap();
        assert_eq!(found.first().and_then(|f| f.not_a_table), Some("other text lies among its rows"));
        // Turned text has no place; drawn between the cells, it is taken to be among them.
        let turned = "BT /F1 10 Tf 0 1 -1 0 460 672 Tm (note) Tj ET ";
        let content = [rows[0].clone(), turned.to_string(), rows[1].clone(), rows[2].clone()].concat();
        let found = table_candidates(&page(&content, "")).unwrap();
        assert_eq!(found.first().and_then(|f| f.not_a_table), Some("other text lies among its rows"));
        let mut doc = page(&content, "");
        autotag_text(&mut doc).unwrap();
        assert!(!kinds(&doc).iter().any(|k| k == "Table"));
        // After the last cell it is not.
        let content = [rows.concat(), turned.to_string()].concat();
        assert_eq!(table_candidates(&page(&content, "")).unwrap().first().map(|f| f.not_a_table), Some(None));
    }

    #[test]
    fn too_many_columns_or_rows_are_not_a_table() {
        let xs: Vec<f64> = (0..33).map(|i| 10.0 + f64::from(i) * 18.0).collect();
        let wide: String = [700.0, 686.0, 672.0].iter().map(|&y| row("F1", y, &xs, &["a"; 33])).collect();
        let found = table_candidates(&page(&wide, "")).unwrap();
        assert_eq!(found.first().and_then(|f| f.not_a_table), Some("it has more than 32 columns"));
        let tall: String =
            (0..501).map(|i| format!("BT /F1 1 Tf 72 {y} Td (a) Tj ET BT /F1 1 Tf 90 {y} Td (b) Tj ET ", y = 760.0 - f64::from(i) * 1.5)).collect();
        let found = table_candidates(&page(&tall, "")).unwrap();
        assert_eq!(found.first().and_then(|f| f.not_a_table), Some("it has more than 500 rows"));
    }

    #[test]
    fn text_without_a_unicode_mapping_among_the_rows_keeps_them_paragraphs() {
        let rows = [
            row("F1", 700.0, &XS, &["Apples", "12", "Red"]),
            row("F1", 686.0, &XS, &["Pears", "7", "Green"]),
            row("F1", 672.0, &XS, &["Plums", "30", "Purple"]),
        ];
        let unmapped = "BT /F3 10 Tf 460 693 Td <00410042> Tj ET ";
        let content = [rows[0].clone(), unmapped.to_string(), rows[1].clone(), rows[2].clone()].concat();
        let found = table_candidates(&page(&content, "")).unwrap();
        assert_eq!(found.first().and_then(|f| f.not_a_table), Some("other text lies among its rows"));
        // In an invisible render mode it is not text the table would hide.
        let hidden = "BT /F3 10 Tf 3 Tr 460 693 Td <00410042> Tj 0 Tr ET ";
        let content = [rows[0].clone(), hidden.to_string(), rows[1].clone(), rows[2].clone()].concat();
        assert_eq!(table_candidates(&page(&content, "")).unwrap().first().map(|f| f.not_a_table), Some(None));
    }

    #[test]
    fn a_missing_cell_beside_a_wider_one_keeps_its_column() {
        // "a" is 5.56 units wide, "aaaaaa" 33.36: the middle row's first cell ends 6.64 units
        // before the middle column starts, within a gutter, but the other rows have cells in both.
        let xs = [72.0, 112.0, 172.0];
        let content =
            [row("F1", 700.0, &xs, &["a", "b", "c"]), row("F1", 686.0, &xs, &["aaaaaa", "", "c"]), row("F1", 672.0, &xs, &["a", "b", "c"])].concat();
        let found = table_candidates(&page(&content, "")).unwrap();
        assert_eq!(found.first().map(|f| (f.columns, f.not_a_table)), Some((3, None)));
        let mut doc = page(&content, "");
        autotag_text(&mut doc).unwrap();
        assert_eq!(kinds(&doc), table(&["TD"; 9]));
    }

    #[test]
    fn a_cell_spanning_two_columns_is_not_read_as_a_table() {
        // The first row's cell covers both cells below it; merging them would write two as one.
        let content = [
            row("F1", 700.0, &[72.0, 250.0], &["aaaaaaaaaaaaaaaaaaaa", "c"]),
            row("F1", 686.0, &[72.0, 112.0, 250.0], &["a", "b", "c"]),
            row("F1", 672.0, &[72.0, 112.0, 250.0], &["a", "b", "c"]),
        ]
        .concat();
        let found = table_candidates(&page(&content, "")).unwrap();
        assert_eq!(found.first().and_then(|f| f.not_a_table), Some("a row has two cells in one column"));
    }

    #[test]
    fn a_missing_header_cell_is_an_empty_data_cell() {
        let content = [
            row("F2", 700.0, &XS, &["Fruit", "", "Colour"]),
            row("F1", 686.0, &XS, &["Apples", "12", "Red"]),
            row("F1", 672.0, &XS, &["Pears", "7", "Green"]),
            row("F1", 658.0, &XS, &["Plums", "30", "Purple"]),
        ]
        .concat();
        let mut doc = page(&content, "");
        autotag_text(&mut doc).unwrap();
        let mut expected = vec!["TH", "TD", "TH"];
        expected.extend(["TD"; 9]);
        assert_eq!(kinds(&doc), table(&expected));
        let scopes: Vec<_> = tags(&doc).into_iter().filter(|t| t.kind == "TH").map(|t| t.scope).collect();
        assert_eq!(scopes, vec![Some("Column".to_string()); 2]);
    }
}
