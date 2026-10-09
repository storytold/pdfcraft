//! pdfcraft-export — Export a PDF ▸ Word, HTML, RTF (L4).
//!
//! The engine reduces each page to [`Page`]: paragraphs (text, box, size, bold/italic) and
//! images (encoded bytes and box), in reading order. The writers turn that into a flowing
//! document: headings are the paragraphs set larger than the body text, and images sit where
//! they fall between paragraphs.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod zip;

pub use zip::Zip;

/// A paragraph of a page.
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub text: String,
    /// [x0, y0, x1, y1] in user space (y up).
    pub rect: [f64; 4],
    /// Font size in points.
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
}

/// An image of a page.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    /// "png" or "jpg".
    pub ext: &'static str,
    pub bytes: Vec<u8>,
    pub rect: [f64; 4],
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    pub width: f64,
    pub height: f64,
    pub blocks: Vec<Block>,
    pub images: Vec<Image>,
}

/// One cell of a detected table. `span` is how many grid columns the cell covers (a header
/// row that stretches over sub-columns).
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub text: String,
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
    pub span: usize,
}

/// A table detected from the page's text layout: rows of cells, and the left edge of each
/// grid column (user space, y up) so the writers can size the columns.
#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub rect: [f64; 4],
    pub cols: Vec<f64>,
    pub rows: Vec<Vec<Cell>>,
}

/// What goes into the output, in order.
enum Item<'a> {
    Para(&'a Block, u8),
    /// Owned: tables are detected per page inside [`items`], so they cannot borrow.
    Table(Table),
    Img(&'a Image),
    PageBreak,
}

/// The body text size: the size most of the document's characters are set in.
fn body_size(pages: &[Page]) -> f64 {
    let mut by: Vec<(i64, usize)> = Vec::new();
    for b in pages.iter().flat_map(|p| &p.blocks) {
        let k = (b.size * 2.0).round() as i64;
        match by.iter_mut().find(|e| e.0 == k) {
            Some(e) => e.1 += b.text.len(),
            None => by.push((k, b.text.len())),
        }
    }
    by.into_iter().max_by_key(|e| e.1).map_or(11.0, |e| e.0 as f64 / 2.0)
}

/// Heading level for a block: 1 for ≥ 1.6× the body size, 2 for ≥ 1.25×, 0 for body text.
fn level(b: &Block, body: f64) -> u8 {
    let short = b.text.len() < 200;
    if short && b.size >= body * 1.6 {
        1
    } else if short && (b.size >= body * 1.25 || (b.bold && b.size >= body && b.text.len() < 80 && !b.text.ends_with('.'))) {
        2
    } else {
        0
    }
}

/// How far two cell left edges may drift (points) and still be the same grid column.
const COL_TOL: f64 = 4.0;
/// More grid columns than this isn't a table. It is Word's limit: a .docx whose table has 64 or
/// more columns doesn't open. It also bounds a table's size: every row is materialized to all
/// its columns, so a hostile page laid out as a staircase of blocks would otherwise make
/// (blocks / 2)² cells.
const MAX_COLS: usize = 63;
/// A cell is short: taller blocks are body text (or a multi-column layout), not table cells.
fn is_cell_like(b: &Block) -> bool {
    (b.rect[3] - b.rect[1]) <= b.size * 5.0 && !b.text.trim().is_empty()
}

/// Detect tables in a page's blocks from their text layout: columns are left edges shared by
/// several short blocks, rows are blocks whose vertical spans overlap. Returns the tables and,
/// per block, whether a table consumed it (so `items` can drop it from the paragraph flow).
fn tables(blocks: &[Block]) -> (Vec<Table>, Vec<bool>) {
    let mut consumed = vec![false; blocks.len()];
    // Candidate columns: cluster the left edges of cell-like blocks.
    let mut xs: Vec<(f64, usize)> = blocks.iter().enumerate().filter(|(_, b)| is_cell_like(b)).map(|(i, b)| (b.rect[0], i)).collect();
    xs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut cols: Vec<(f64, Vec<usize>)> = Vec::new(); // (left edge, member block indices)
    for (x, i) in xs {
        match cols.last_mut() {
            Some((e, m)) if (x - *e).abs() <= COL_TOL => {
                *e = (*e * m.len() as f64 + x) / (m.len() + 1) as f64;
                m.push(i);
            }
            _ => cols.push((x, vec![i])),
        }
    }
    let cols: Vec<(f64, Vec<usize>)> = cols.into_iter().filter(|(_, m)| m.len() >= 2).collect();
    if cols.len() < 2 {
        return (Vec::new(), consumed);
    }
    // Each block's column (an index into `cols`), so lookups stay linear on pages with many blocks.
    let mut col_of: Vec<Option<usize>> = vec![None; blocks.len()];
    for (ci, (_, members)) in cols.iter().enumerate() {
        for &i in members {
            if let Some(c) = col_of.get_mut(i) {
                *c = Some(ci);
            }
        }
    }
    // The column whose edge is within tolerance of `x` (edges are in ascending order).
    let column_at = |x: f64| -> Option<usize> {
        let k = cols.partition_point(|(e, _)| *e < x - COL_TOL);
        cols.get(k).filter(|(e, _)| (*e - x).abs() <= COL_TOL).map(|_| k)
    };
    // Assign every cell-like block to its nearest column (within tolerance).
    let mut rows: Vec<Vec<(usize, usize)>> = Vec::new(); // (column index, block index), top to bottom
    let mut order: Vec<usize> = (0..blocks.len()).collect();
    order.sort_by(|a, b| blocks[*b].rect[3].total_cmp(&blocks[*a].rect[3]).then(blocks[*a].rect[0].total_cmp(&blocks[*b].rect[0])));
    for &bi in &order {
        let b = &blocks[bi];
        if !is_cell_like(b) {
            continue;
        }
        let Some(ci) = column_at(b.rect[0]) else { continue };
        let joins = rows
            .last()
            .and_then(|row| row.first())
            .map(|(_, fi)| {
                let rb = &blocks[*fi];
                let overlap = rb.rect[3].min(b.rect[3]) - rb.rect[1].max(b.rect[1]);
                overlap > 0.5 * (rb.rect[3] - rb.rect[1]).min(b.rect[3] - b.rect[1])
            })
            .unwrap_or(false);
        if joins {
            if let Some(row) = rows.last_mut() {
                row.push((ci, bi));
            }
        } else {
            rows.push(vec![(ci, bi)]);
        }
    }
    // Runs of consecutive rows that have two or more distinct columns are tables.
    let mut out = Vec::new();
    let flush = |run: &mut Vec<Vec<(usize, usize)>>, out: &mut Vec<Table>, consumed: &mut Vec<bool>| {
        if run.len() < 2 {
            run.clear();
            return;
        }
        let mut used: Vec<usize> = run.iter().flatten().map(|(_, i)| *i).collect();
        used.sort_unstable();
        used.dedup();
        let mut edges: Vec<f64> = used.iter().filter_map(|&i| col_of.get(i).copied().flatten()).filter_map(|ci| cols.get(ci)).map(|c| c.0).collect();
        edges.sort_by(|a, b| a.total_cmp(b));
        edges.dedup_by(|a, b| (*a - *b).abs() <= COL_TOL);
        let ncols = edges.len();
        let ok = (ncols >= 3 || (ncols == 2 && run.len() >= 3)) && ncols <= MAX_COLS;
        if !ok {
            run.clear();
            return;
        }
        let right = used.iter().map(|&i| blocks[i].rect[2]).fold(f64::MIN, f64::max) + COL_TOL;
        let mut rows_out = Vec::new();
        let mut slots: Vec<Option<Cell>> = vec![None; ncols];
        for row in run.drain(..) {
            for slot in slots.iter_mut() {
                *slot = None;
            }
            for (_ci, bi) in row {
                let b = &blocks[bi];
                let Some(slot) = edges.iter().position(|e| (*e - b.rect[0]).abs() <= COL_TOL) else { continue };
                let span = edges[slot + 1..].iter().filter(|e| **e > b.rect[0] + COL_TOL && **e < b.rect[2] - COL_TOL).count() + 1;
                let cell = Cell { text: b.text.trim().to_string(), size: b.size, bold: b.bold, italic: b.italic, span };
                match &mut slots[slot] {
                    Some(c) => {
                        c.text.push(' ');
                        c.text.push_str(&cell.text);
                    }
                    None => slots[slot] = Some(cell),
                }
                consumed[bi] = true;
            }
            let mut materialized = Vec::with_capacity(ncols);
            let mut column = 0;
            while column < ncols {
                match slots[column].take() {
                    Some(mut cell) => {
                        cell.span = cell.span.min(ncols - column).max(1);
                        column += cell.span;
                        materialized.push(cell);
                    }
                    None => {
                        column += 1;
                        materialized.push(Cell { text: String::new(), size: 11.0, bold: false, italic: false, span: 1 });
                    }
                }
            }
            while materialized.last().is_some_and(|cell| cell.text.is_empty()) && materialized.iter().any(|cell| cell.span > 1) {
                materialized.pop();
            }
            rows_out.push(materialized);
        }
        let rect = [
            edges[0],
            used.iter().map(|&i| blocks[i].rect[1]).fold(f64::MAX, f64::min),
            right,
            used.iter().map(|&i| blocks[i].rect[3]).fold(f64::MIN, f64::max),
        ];
        out.push(Table { rect, cols: edges, rows: rows_out });
    };
    // Runs of consecutive multi-column rows are tables. Rows on either side of a run that
    // hold a single cell wide enough to span two columns (a header band, a totals row) join
    // the table; other single-column rows (captions, following text) stay paragraphs.
    let spans_run = |edges: &[f64], bi: usize| -> bool {
        let b = &blocks[bi];
        edges.iter().any(|e| (*e - b.rect[0]).abs() <= COL_TOL) && edges.iter().any(|e| *e > b.rect[0] + COL_TOL && *e < b.rect[2] - COL_TOL)
    };
    let run_edges = |run: &[Vec<(usize, usize)>]| -> Vec<f64> {
        let mut es: Vec<f64> =
            run.iter().flatten().filter_map(|(_, i)| col_of.get(*i).copied().flatten()).filter_map(|ci| cols.get(ci)).map(|c| c.0).collect();
        es.sort_by(|a, b| a.total_cmp(b));
        es.dedup_by(|a, b| (*a - *b).abs() <= COL_TOL);
        es
    };
    let qual: Vec<bool> = rows
        .iter()
        .map(|row| {
            let mut cs: Vec<usize> = row.iter().map(|(c, _)| *c).collect();
            cs.sort_unstable();
            cs.dedup();
            cs.len() >= 2
        })
        .collect();
    let mut i = 0;
    while i < rows.len() {
        if !qual[i] {
            i += 1;
            continue;
        }
        let start = i;
        while i < rows.len() && qual[i] {
            i += 1;
        }
        let end = i; // run = rows[start..end]
        let mut pieces: Vec<Vec<(usize, usize)>> = Vec::new();
        if start > 0 && rows[start - 1].len() == 1 && spans_run(&run_edges(&rows[start..end]), rows[start - 1][0].1) {
            pieces.push(rows[start - 1].clone());
        }
        pieces.extend(rows[start..end].iter().cloned());
        if end < rows.len() && rows[end].len() == 1 && spans_run(&run_edges(&rows[start..end]), rows[end][0].1) {
            pieces.push(rows[end].clone());
        }
        flush(&mut pieces, &mut out, &mut consumed);
    }
    (out, consumed)
}

fn items(pages: &[Page]) -> Vec<Item<'_>> {
    let body = body_size(pages);
    let mut out = Vec::new();
    for (i, p) in pages.iter().enumerate() {
        if i > 0 {
            out.push(Item::PageBreak);
        }
        let (tables, consumed) = tables(&p.blocks);
        // Blocks and images by their top edge, top to bottom.
        let mut parts: Vec<(f64, f64, Item)> = p
            .blocks
            .iter()
            .enumerate()
            .filter(|(bi, _)| !consumed.get(*bi).copied().unwrap_or(false))
            .map(|(_, b)| (b.rect[3], b.rect[0], Item::Para(b, level(b, body))))
            .collect();
        parts.extend(tables.into_iter().map(|t| (t.rect[3], t.rect[0], Item::Table(t))));
        parts.extend(p.images.iter().map(|im| (im.rect[3], im.rect[0], Item::Img(im))));
        parts.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.total_cmp(&b.1)));
        out.extend(parts.into_iter().map(|x| x.2));
    }
    out
}

/// Whether a character may appear in the output. XML 1.0 forbids the C0 controls other than tab,
/// line feed and carriage return, and U+FFFE/U+FFFF. Text extracted from PDFs often holds them
/// (fonts without a usable `/ToUnicode`), and Word refuses a whole document over one (#72).
fn allowed(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r') || !(c.is_ascii_control() || c == '\u{FFFE}' || c == '\u{FFFF}')
}

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars().filter(|c| allowed(*c)) {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            c => o.push(c),
        }
    }
    o
}

fn base64(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        s.push(A[(n >> 18) as usize & 63] as char);
        s.push(A[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { A[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { A[n as usize & 63] as char } else { '=' });
    }
    s
}

/// One HTML file: headings and paragraphs, tables as real `<table>` elements, images inline, a
/// rule between pages.
pub fn html(pages: &[Page], title: &str) -> String {
    let mut s = format!(
        "<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n<title>{}</title>\n<style>body{{max-width:46em;margin:2em auto;font-family:sans-serif;line-height:1.45}}\
img{{max-width:100%}}hr{{border:0;border-top:1px solid #ccc;margin:2em 0}}\
table{{border-collapse:collapse;margin:1em 0}}td,th{{border:1px solid #999;padding:2px 6px;vertical-align:top}}</style>\n</head>\n<body>\n",
        esc(title)
    );
    for it in items(pages) {
        match it {
            Item::Para(b, lvl) => {
                let mut t = esc(&b.text);
                if b.italic {
                    t = format!("<em>{t}</em>");
                }
                if b.bold && lvl == 0 {
                    t = format!("<strong>{t}</strong>");
                }
                match lvl {
                    0 => s.push_str(&format!("<p>{t}</p>\n")),
                    l => s.push_str(&format!("<h{l}>{t}</h{l}>\n")),
                }
            }
            Item::Table(t) => {
                s.push_str("<table>\n");
                for row in &t.rows {
                    s.push_str("<tr>");
                    for c in row {
                        let mut body = esc(&c.text);
                        if c.italic {
                            body = format!("<em>{body}</em>");
                        }
                        if c.bold {
                            body = format!("<strong>{body}</strong>");
                        }
                        let span = if c.span > 1 { format!(" colspan=\"{}\"", c.span) } else { String::new() };
                        s.push_str(&format!("<td{span}>{body}</td>"));
                    }
                    s.push_str("</tr>\n");
                }
                s.push_str("</table>\n");
            }
            Item::Img(im) => {
                let mime = if im.ext == "jpg" { "image/jpeg" } else { "image/png" };
                s.push_str(&format!("<p><img alt=\"\" src=\"data:{mime};base64,{}\"></p>\n", base64(&im.bytes)));
            }
            Item::PageBreak => s.push_str("<hr>\n"),
        }
    }
    s.push_str("</body>\n</html>\n");
    s
}

/// A formatted text run (shared by paragraphs and table cells).
fn run_xml(text: &str, size: f64, bold: bool, italic: bool) -> String {
    let mut rpr = String::new();
    if bold {
        rpr.push_str("<w:b/>");
    }
    if italic {
        rpr.push_str("<w:i/>");
    }
    let half_points = if size.is_finite() { (size * 2.0).round().clamp(2.0, 3276.0) as i64 } else { 24 };
    rpr.push_str(&format!("<w:sz w:val=\"{half_points}\"/>"));
    format!("<w:r><w:rPr>{rpr}</w:rPr><w:t xml:space=\"preserve\">{}</w:t></w:r>", esc(text))
}

/// One Word table: explicit single borders (so it renders without a table style), a grid sized
/// from the detected column edges, `gridSpan` for spanning cells, empty cells for holes.
fn docx_table(t: &Table) -> String {
    let mut edges = t.cols.clone();
    edges.push(t.rect[2]);
    let widths: Vec<i64> = edges.windows(2).map(|w| ((w[1] - w[0]).max(20.0) * 20.0).round().clamp(60.0, 31680.0) as i64).collect();
    let border = "<w:top w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"999999\"/>";
    let mut s = String::from("<w:tbl><w:tblPr><w:tblW w:w=\"0\" w:type=\"auto\"/><w:tblBorders>");
    for b in [
        border,
        "<w:left w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"999999\"/>",
        "<w:bottom w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"999999\"/>",
        "<w:right w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"999999\"/>",
        "<w:insideH w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"999999\"/>",
        "<w:insideV w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"999999\"/>",
    ] {
        s.push_str(b);
    }
    s.push_str("</w:tblBorders></w:tblPr><w:tblGrid>");
    for w in &widths {
        s.push_str(&format!("<w:gridCol w:w=\"{w}\"/>"));
    }
    s.push_str("</w:tblGrid>");
    for row in &t.rows {
        s.push_str("<w:tr>");
        for c in row {
            let span = if c.span > 1 { format!("<w:gridSpan w:val=\"{}\"/>", c.span) } else { String::new() };
            let w: i64 = widths.iter().take(c.span.min(widths.len())).sum();
            s.push_str(&format!(
                "<w:tc><w:tcPr><w:tcW w:w=\"{w}\" w:type=\"dxa\"/>{span}</w:tcPr><w:p>{}</w:p></w:tc>",
                run_xml(&c.text, c.size, c.bold, c.italic)
            ));
        }
        // Pad the grid so every row covers all columns (Word rejects short rows).
        let used: usize = row.iter().map(|c| c.span).sum();
        for _ in used..t.cols.len() {
            s.push_str("<w:tc><w:tcPr><w:tcW w:w=\"0\" w:type=\"auto\"/></w:tcPr><w:p/></w:tc>");
        }
        s.push_str("</w:tr>");
    }
    s.push_str("</w:tbl>");
    s
}

/// A Word document (.docx, Office Open XML): Heading 1/2 and Normal paragraphs, images inline at
/// their size on the page, page breaks between pages.
pub fn docx(pages: &[Page], title: &str) -> Vec<u8> {
    // The first page's size, within what Word accepts (0.1 to 22 in; a long receipt is taller),
    // in twips. Margins are 1 in, less on small pages so text keeps room.
    let twips = |pt: f64| if pt.is_finite() { (pt * 20.0).round().clamp(144.0, 31680.0) as i64 } else { 12240 };
    let (pw, ph) = pages.first().map_or((12240, 15840), |p| (twips(p.width), twips(p.height)));
    let (mx, my) = ((pw / 8).min(1440), (ph / 8).min(1440));
    // Images are at most the text width, in points.
    let text_w = (pw - 2 * mx) as f64 / 20.0;
    let mut body = String::new();
    let mut media: Vec<(String, &Image)> = Vec::new();
    // Word needs a paragraph between adjacent tables and after the last one in the body.
    let mut after_table = false;
    let run = |b: &Block| run_xml(&b.text, b.size, b.bold, b.italic);
    for it in items(pages) {
        match &it {
            Item::Para(b, lvl) => {
                let style = match lvl {
                    1 => "<w:pPr><w:pStyle w:val=\"Heading1\"/></w:pPr>",
                    2 => "<w:pPr><w:pStyle w:val=\"Heading2\"/></w:pPr>",
                    _ => "",
                };
                body.push_str(&format!("<w:p>{style}{}</w:p>", run(b)));
            }
            Item::Table(t) => {
                if after_table {
                    body.push_str("<w:p/>");
                }
                body.push_str(&docx_table(t));
            }
            Item::Img(im) => {
                let n = media.len() + 1;
                let name = format!("image{n}.{}", im.ext);
                // Size on the page, in EMU (12700 per point), at most the text width.
                let (w, h) = ((im.rect[2] - im.rect[0]).max(1.0), (im.rect[3] - im.rect[1]).max(1.0));
                let k = (text_w / w).min(1.0);
                let (cx, cy) = ((w * k * 12700.0) as i64, (h * k * 12700.0) as i64);
                body.push_str(&format!(
                    "<w:p><w:r><w:drawing><wp:inline><wp:extent cx=\"{cx}\" cy=\"{cy}\"/><wp:docPr id=\"{n}\" name=\"Picture {n}\"/>\
<a:graphic xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
<pic:pic xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\"><pic:nvPicPr><pic:cNvPr id=\"{n}\" name=\"{name}\"/><pic:cNvPicPr/></pic:nvPicPr>\
<pic:blipFill><a:blip r:embed=\"rIdImg{n}\"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>\
<pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr></pic:pic>\
</a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"
                ));
                media.push((name, im));
            }
            Item::PageBreak => body.push_str("<w:p><w:r><w:br w:type=\"page\"/></w:r></w:p>"),
        }
        after_table = matches!(it, Item::Table(_));
    }
    if after_table {
        body.push_str("<w:p/>");
    }
    let doc = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\"><w:body>{body}\
<w:sectPr><w:pgSz w:w=\"{pw}\" w:h=\"{ph}\"/><w:pgMar w:top=\"{my}\" w:right=\"{mx}\" w:bottom=\"{my}\" w:left=\"{mx}\" w:header=\"{}\" w:footer=\"{}\" w:gutter=\"0\"/></w:sectPr></w:body></w:document>",
        my / 2,
        my / 2
    );
    let mut types = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/>\
<Default Extension=\"png\" ContentType=\"image/png\"/><Default Extension=\"jpg\" ContentType=\"image/jpeg\"/>\
<Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>\
<Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/>\
<Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.openxmlformats-package.core-properties+xml\"/>",
    );
    types.push_str("</Types>");
    let rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/>\
<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties\" Target=\"docProps/core.xml\"/></Relationships>";
    let mut doc_rels = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rIdStyles\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>",
    );
    for (i, (name, _)) in media.iter().enumerate() {
        doc_rels.push_str(&format!(
            "<Relationship Id=\"rIdImg{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"media/{name}\"/>",
            i + 1
        ));
    }
    doc_rels.push_str("</Relationships>");
    let styles = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
<w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/><w:pPr><w:spacing w:after=\"120\"/></w:pPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading1\"><w:name w:val=\"heading 1\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:pPr><w:keepNext/><w:spacing w:before=\"240\"/><w:outlineLvl w:val=\"0\"/></w:pPr><w:rPr><w:b/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading2\"><w:name w:val=\"heading 2\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:pPr><w:keepNext/><w:spacing w:before=\"200\"/><w:outlineLvl w:val=\"1\"/></w:pPr><w:rPr><w:b/></w:rPr></w:style>\
</w:styles>";
    let core = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><dc:title>{}</dc:title></cp:coreProperties>",
        esc(title)
    );
    let mut z = Zip::default();
    z.add("[Content_Types].xml", types.as_bytes(), true);
    z.add("_rels/.rels", rels.as_bytes(), true);
    z.add("docProps/core.xml", core.as_bytes(), true);
    z.add("word/document.xml", doc.as_bytes(), true);
    z.add("word/styles.xml", styles.as_bytes(), true);
    z.add("word/_rels/document.xml.rels", doc_rels.as_bytes(), true);
    for (name, im) in &media {
        z.add(&format!("word/media/{name}"), &im.bytes, false);
    }
    z.finish()
}

fn rtf_text(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars().filter(|c| allowed(*c)) {
        match c {
            '\\' | '{' | '}' => {
                o.push('\\');
                o.push(c);
            }
            c if (c as u32) < 128 => o.push(c),
            c => {
                // \uN with a ? fallback; UTF-16 units as signed 16-bit numbers.
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    o.push_str(&format!("\\u{}?", *u as i16));
                }
            }
        }
    }
    o
}

/// Rich Text Format: paragraphs with their sizes and bold/italic, real table rows, page breaks
/// between pages (images are left out).
pub fn rtf(pages: &[Page]) -> String {
    let mut s = String::from("{\\rtf1\\ansi\\deff0{\\fonttbl{\\f0 Helvetica;}}\n");
    for it in items(pages) {
        match it {
            Item::Para(b, _) => {
                let mut fmt = format!("\\fs{}", (b.size * 2.0).round() as i64);
                if b.bold {
                    fmt.push_str("\\b");
                }
                if b.italic {
                    fmt.push_str("\\i");
                }
                s.push_str(&format!("{{\\pard{fmt} {}\\par}}\n", rtf_text(&b.text)));
            }
            Item::Table(t) => {
                let mut edges = t.cols.clone();
                edges.push(t.rect[2]);
                let cellx: Vec<i64> = edges.iter().skip(1).map(|e| (e * 20.0).round() as i64).collect();
                for row in &t.rows {
                    s.push_str("\\trowd\\trgaph108");
                    // The row's cell boundaries come first: each cell ends at the right edge of
                    // the last grid column it spans.
                    let mut column = 0;
                    for c in row {
                        column += c.span.max(1);
                        if let Some(x) = cellx.get(column.min(cellx.len()).saturating_sub(1)) {
                            s.push_str(&format!("\\cellx{x}"));
                        }
                    }
                    for c in row {
                        let mut fmt = format!("\\intbl\\fs{}", (c.size * 2.0).round() as i64);
                        if c.bold {
                            fmt.push_str("\\b");
                        }
                        if c.italic {
                            fmt.push_str("\\i");
                        }
                        s.push_str(&format!("{{{fmt} {}}}\\cell", rtf_text(&c.text)));
                    }
                    s.push_str("\\row\n");
                }
            }
            Item::Img(_) => {}
            Item::PageBreak => s.push_str("\\page\n"),
        }
    }
    s.push('}');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> Page {
        let b = |t: &str, y: f64, size: f64, bold: bool| Block { text: t.into(), rect: [72.0, y, 500.0, y + size], size, bold, italic: false };
        Page {
            width: 612.0,
            height: 792.0,
            blocks: vec![
                b("Body text after the picture, which is long enough to be the main text size of the page.", 400.0, 11.0, false),
                b("Annual Report", 700.0, 24.0, true),
                b("Overview", 650.0, 14.0, true),
                b("Some <body> text & more of it so that eleven points is the body size here.", 620.0, 11.0, false),
            ],
            images: vec![Image {
                ext: "png",
                bytes: b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x02\0\0\0\x03".to_vec(),
                rect: [72.0, 450.0, 272.0, 600.0],
            }],
        }
    }

    #[test]
    fn html_has_headings_paragraphs_and_images_in_order() {
        let h = html(&[page(), page()], "Report");
        let order: Vec<usize> = ["<h1>Annual Report</h1>", "<h2>Overview</h2>", "<p>Some &lt;body&gt; text &amp;", "<img", "<p>Body text after"]
            .iter()
            .map(|x| h.find(x).unwrap_or_else(|| panic!("{x} missing in {h}")))
            .collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}");
        assert_eq!(h.matches("<hr>").count(), 1);
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
    }

    #[test]
    fn docx_is_a_word_package() {
        let d = docx(&[page()], "Report");
        assert!(d.starts_with(b"PK"));
        let names: Vec<&str> =
            ["[Content_Types].xml", "word/document.xml", "word/styles.xml", "word/_rels/document.xml.rels", "word/media/image1.png", "_rels/.rels"]
                .to_vec();
        for n in names {
            assert!(d.windows(n.len()).any(|w| w == n.as_bytes()), "{n}");
        }
    }

    /// One part of a package written by [`Zip`], inflated.
    fn part(zip: &[u8], name: &str) -> String {
        let at = |i: usize, n: usize| zip[i..i + n].iter().rev().fold(0usize, |v, b| v << 8 | *b as usize);
        let mut i = 0;
        while zip[i..].starts_with(b"PK\x03\x04") {
            let (size, name_len) = (at(i + 18, 4), at(i + 26, 2));
            let data = i + 30 + name_len;
            if &zip[i + 30..data] == name.as_bytes() {
                let mut s = String::new();
                std::io::Read::read_to_string(&mut flate2::read::DeflateDecoder::new(&zip[data..data + size]), &mut s).unwrap();
                return s;
            }
            i = data + size;
        }
        panic!("{name} missing")
    }

    #[test]
    fn word_opens_text_with_control_characters_and_long_pages() {
        // #72: text extracted without a usable /ToUnicode holds C0 controls, which XML forbids;
        // Word then refused the whole file. A till receipt is also taller than Word's 22 in.
        let mut p = page();
        p.height = 2400.0;
        p.blocks.push(Block {
            text: "Total\u{0}\u{3}\u{c} 4.50\u{FFFF}\tGBP".into(),
            rect: [72.0, 100.0, 300.0, 110.0],
            size: f64::NAN,
            bold: false,
            italic: false,
        });
        let xml = part(&docx(&[p.clone()], "Receipt\u{1}"), "word/document.xml");
        assert!(xml.contains(">Total 4.50\tGBP</w:t>"), "{xml}");
        assert!(!xml.chars().any(|c| !allowed(c)));
        assert!(xml.contains("<w:pgSz w:w=\"12240\" w:h=\"31680\"/>"), "clamped to 22 in: {xml}");
        assert!(xml.contains("<w:sz w:val=\"24\"/>"), "a non-finite size falls back to 12 pt");
        assert!(rtf(&[p]).contains("Total 4.50\tGBP"));
        // A narrow page keeps room for text: margins shrink.
        let narrow = Page { width: 100.0, height: 200.0, ..page() };
        let xml = part(&docx(&[narrow], "Narrow"), "word/document.xml");
        assert!(xml.contains("<w:pgMar w:top=\"500\" w:right=\"250\" w:bottom=\"500\" w:left=\"250\""), "{xml}");
    }

    #[test]
    fn rtf_escapes_and_sizes() {
        let mut p = page();
        p.blocks.push(Block { text: "Café {x}".into(), rect: [72.0, 100.0, 200.0, 110.0], size: 10.0, bold: false, italic: true });
        let r = rtf(&[p]);
        assert!(r.starts_with("{\\rtf1") && r.ends_with('}'));
        assert!(r.contains("\\fs48\\b Annual Report"));
        assert!(r.contains("\\fs20\\i Caf\\u233? \\{x\\}"), "{r}");
    }

    /// A 3-column table with a spanning header row and a hole in the last row.
    fn table_page() -> Page {
        let cell = |t: &str, x: f64, y: f64, w: f64| Block { text: t.into(), rect: [x, y, x + w, y + 12.0], size: 11.0, bold: false, italic: false };
        Page {
            width: 612.0,
            height: 792.0,
            blocks: vec![
                cell("Meter", 72.0, 620.0, 120.0),
                cell("Unit", 232.0, 620.0, 80.0),
                cell("Reading", 392.0, 620.0, 100.0),
                cell("A1", 72.0, 590.0, 120.0),
                cell("kWh", 232.0, 590.0, 80.0),
                cell("37414.00", 392.0, 590.0, 100.0),
                cell("A2", 72.0, 560.0, 120.0),
                cell("kW", 232.0, 560.0, 80.0),
                // A2's row is missing its third cell (an empty cell in the grid).
                cell("Totals", 72.0, 530.0, 240.0),
                Block {
                    text: "Notes follow the table and are long enough to be the body text size of the page here.".into(),
                    rect: [72.0, 460.0, 500.0, 471.0],
                    size: 11.0,
                    bold: false,
                    italic: false,
                },
            ],
            images: Vec::new(),
        }
    }

    #[test]
    fn grid_text_becomes_a_real_table_in_every_format() {
        let p = table_page();
        let h = html(std::slice::from_ref(&p), "Bill");
        assert!(h.contains("<table>"), "{h}");
        assert!(h.contains("<td colspan=\"2\">Totals</td>"), "spanning cell: {h}");
        assert_eq!(h.matches("<tr>").count(), 4, "{h}");
        assert!(h.contains("<td></td>"), "the hole is an empty cell: {h}");
        assert!(h.contains("<p>Notes follow"), "the paragraph after the table stays: {h}");

        let d = docx(std::slice::from_ref(&p), "Bill");
        assert!(d.starts_with(b"PK"));
        assert!(d.windows(b"word/document.xml".len()).any(|w| w == b"word/document.xml"));
        let (tables, _) = tables(&p.blocks);
        assert_eq!(tables.len(), 1);
        let table_xml = docx_table(&tables[0]);
        assert!(table_xml.contains("<w:tblGrid>"));
        assert!(table_xml.contains("<w:gridSpan w:val=\"2\"/>"), "{table_xml}");
        assert_eq!(table_xml.matches("<w:tr>").count(), 4);

        let r = rtf(&[p]);
        assert!(r.contains("\\trowd"), "{r}");
        assert_eq!(r.matches("\\row").count(), 4);
        // Boundaries precede the cells; "Totals" spans two columns, so it ends at the third edge.
        assert!(r.contains("\\trowd\\trgaph108\\cellx4640\\cellx7840\\cellx9920{"), "{r}");
        assert!(r.contains("\\trowd\\trgaph108\\cellx7840{\\intbl\\fs22 Totals}\\cell\\row"), "{r}");
    }

    #[test]
    fn a_staircase_of_blocks_is_not_a_huge_table() {
        // Row k holds blocks in columns k and k + 1: every column has two members and every row
        // two columns, so without a cap this would be one table of (n / 2)² cells.
        let n = 4000;
        let blocks: Vec<Block> = (0..n)
            .map(|i| {
                let (row, col) = (i / 2, i / 2 + i % 2);
                let (x, y) = (10.0 + col as f64 * 20.0, 10_000.0 - row as f64 * 14.0);
                Block { text: "x".into(), rect: [x, y, x + 8.0, y + 12.0], size: 11.0, bold: false, italic: false }
            })
            .collect();
        let started = std::time::Instant::now();
        let (tables, _) = tables(&blocks);
        assert!(tables.iter().all(|t| t.cols.len() <= MAX_COLS), "{} columns", tables.iter().map(|t| t.cols.len()).max().unwrap_or(0));
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "took {:?}", started.elapsed());
    }

    #[test]
    fn word_opens_tables_up_to_its_column_limit() {
        // Rows of short numbers on a regular grid, n columns wide.
        let grid = |n: usize| Page {
            width: 1500.0,
            height: 792.0,
            blocks: (0..3)
                .flat_map(|row| {
                    (0..n).map(move |col| {
                        let (x, y) = (20.0 + col as f64 * 22.0, 700.0 - row as f64 * 14.0);
                        Block { text: (col + 1).to_string(), rect: [x, y, x + 8.0, y + 8.0], size: 8.0, bold: false, italic: false }
                    })
                })
                .collect(),
            images: Vec::new(),
        };
        let grid_cols = |p: &Page| part(&docx(std::slice::from_ref(p), "Grid"), "word/document.xml").matches("<w:gridCol ").count();
        assert_eq!(grid_cols(&grid(63)), 63, "63 columns is still a table");
        // Word refuses to open a document with a 64-column table, so that grid stays text.
        assert_eq!(grid_cols(&grid(64)), 0);
    }

    #[test]
    fn two_column_body_text_is_not_mistaken_for_a_table() {
        let para = |t: &str, x: f64, y: f64| Block {
            text: t.into(),
            rect: [x, y, x + 200.0, y + 90.0], // nine lines tall: body text, not a cell
            size: 11.0,
            bold: false,
            italic: false,
        };
        let p = Page {
            width: 612.0,
            height: 792.0,
            blocks: vec![
                para("Left column text of the page, long enough to be a flowing paragraph.", 72.0, 500.0),
                para("Right column text of the page, long enough to be a flowing paragraph.", 320.0, 500.0),
                para("Second left block, still a tall flowing paragraph rather than a cell.", 72.0, 400.0),
                para("Second right block, still a tall flowing paragraph rather than a cell.", 320.0, 400.0),
            ],
            images: Vec::new(),
        };
        let h = html(&[p], "Paper");
        assert!(!h.contains("<table>"), "tall two-column text must stay paragraphs: {h}");
    }
}
