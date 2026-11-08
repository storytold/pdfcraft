//! Excel spreadsheet (.xlsx, Office Open XML SpreadsheetML) export.
//!
//! Provides spatial grid and vector-line table reconstruction, mapping tabular data
//! (invoices, shipping labels, financial tables, forms) into clean, properly merged
//! worksheet cells with data-type awareness (numbers vs. text), column widths, and borders.

use crate::{Block, Cell, Page, Table, Zip, esc};

/// Format index for cell styles in `styles.xml`
const STYLE_DEFAULT: u32 = 0;
const STYLE_HEADER: u32 = 1;
const STYLE_TEXT_BORDER: u32 = 2;
const STYLE_NUMBER_BORDER: u32 = 3;
const STYLE_TITLE: u32 = 4;
const STYLE_TOTAL_BORDER: u32 = 5;

/// Convert 1-based column index to Excel column string: 1 -> "A", 26 -> "Z", 27 -> "AA", etc.
pub fn col_name(mut col: usize) -> String {
    if col == 0 {
        return "A".to_string();
    }
    let mut s = String::new();
    while col > 0 {
        let rem = (col - 1) % 26;
        s.push((b'A' + rem as u8) as char);
        col = (col - 1) / 26;
    }
    s.chars().rev().collect()
}

/// Checks if a string represents a numeric value (e.g. "123", "$45.67", "1,234.50").
/// Returns the parsed f64 if valid. Conservative: a value that would change is left as text.
pub fn parse_numeric(s: &str) -> Option<f64> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return None;
    }
    // A percentage would be written as 45.00, not 45 %: keep the source text.
    if trimmed.contains('%') {
        return None;
    }
    let unsigned = trimmed.trim_start_matches(['+', '-', '$', '€', '£', '¥']);
    // Reject non-leading currency symbols in the middle of a string (e.g. "5$3")
    if unsigned.contains(['$', '€', '£', '¥']) {
        return None;
    }
    // Identifiers (ZIP codes, part numbers) keep their leading zero, and f64 cannot hold
    // more than 15 exact digits: both stay text.
    if unsigned.chars().filter(char::is_ascii_digit).count() > 15 || (unsigned.len() > 1 && unsigned.starts_with('0') && !unsigned.starts_with("0."))
    {
        return None;
    }
    // A comma is a thousands separator only in groups of three ("1,234.50", not "1,5").
    if unsigned.split('.').nth(1).is_some_and(|frac| frac.contains(',')) {
        return None;
    }
    if let Some(int_part) = unsigned.split('.').next()
        && int_part.contains(',')
    {
        let first_len = int_part.split(',').next().map_or(0, |g| g.len());
        if !(1..=3).contains(&first_len) {
            return None;
        }
        if !int_part.split(',').skip(1).all(|g| g.len() == 3 && g.bytes().all(|b| b.is_ascii_digit())) {
            return None;
        }
    }
    // Clean thousands commas and preserve sign
    let is_negative = trimmed.starts_with('-')
        || trimmed.starts_with("$-")
        || trimmed.starts_with("€-")
        || trimmed.starts_with("£-")
        || trimmed.starts_with("¥-")
        || trimmed.starts_with("-$")
        || trimmed.starts_with("-€")
        || trimmed.starts_with("-£")
        || trimmed.starts_with("-¥");
    let mut cleaned = String::with_capacity(unsigned.len() + 1);
    if is_negative {
        cleaned.push('-');
    }
    let mut has_digit = false;
    for c in unsigned.chars() {
        if c.is_ascii_digit() || c == '.' {
            if c.is_ascii_digit() {
                has_digit = true;
            }
            cleaned.push(c);
        } else if c == ',' {
            continue;
        } else {
            return None;
        }
    }
    if !has_digit {
        return None;
    }
    cleaned.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// An Excel cell in a reconstructed worksheet row.
struct XlsxCell {
    col: usize, // 1-indexed
    text: String,
    num: Option<f64>,
    style: u32,
}

/// A reconstructed worksheet row.
struct XlsxRow {
    row_num: usize, // 1-indexed
    cells: Vec<XlsxCell>,
}

fn compute_col_edges(blocks: &[Block]) -> Vec<f64> {
    let mut col_xs: Vec<f64> = blocks.iter().map(|b| b.rect[0]).collect();
    col_xs.sort_by(|a, b| a.total_cmp(b));
    let mut col_edges: Vec<f64> = Vec::new();
    for x in col_xs {
        match col_edges.last_mut() {
            Some(e) if (x - *e).abs() <= 16.0 => {
                *e = (*e + x) * 0.5;
            }
            _ => col_edges.push(x),
        }
    }
    col_edges
}

fn process_header_blocks(
    p: &Page,
    consumed: &[bool],
    col_edges: &[f64],
    rows: &mut Vec<XlsxRow>,
    row_tops: &mut Vec<(f64, usize)>,
    col_max_len: &mut Vec<usize>,
    current_row: &mut usize,
) {
    let mut header_blocks: Vec<&Block> = p
        .blocks
        .iter()
        .enumerate()
        .filter(|(i, b)| !consumed.get(*i).copied().unwrap_or(false) && !b.text.trim().is_empty())
        .map(|(_, b)| b)
        .collect();
    header_blocks.sort_by(|a, b| b.rect[3].total_cmp(&a.rect[3]).then(a.rect[0].total_cmp(&b.rect[0])));

    let mut line_groups: Vec<Vec<&Block>> = Vec::new();
    for b in header_blocks {
        if let Some(group) = line_groups.last_mut()
            && let Some(first) = group.first()
            && (first.rect[3] - b.rect[3]).abs() <= 6.0
        {
            group.push(b);
            continue;
        }
        line_groups.push(vec![b]);
    }

    for mut group in line_groups {
        let group_top_pt = if p.height > 0.0 {
            let max_y = group.iter().map(|b| b.rect[3]).fold(0.0, f64::max);
            (p.height - max_y).max(0.0)
        } else {
            0.0
        };
        group.sort_by(|a, b| a.rect[0].total_cmp(&b.rect[0]));
        let mut cells = Vec::new();
        let mut last_col = 0usize;
        for b in group {
            let natural_col = col_edges.iter().position(|&e| (b.rect[0] - e).abs() <= 16.0).unwrap_or(0) + 1;
            let col = natural_col.max(last_col + 1);
            last_col = col;

            let text = b.text.trim().to_string();
            let num = parse_numeric(&text);
            let style = if b.size >= 14.0 || b.bold { STYLE_TITLE } else { STYLE_DEFAULT };
            let len = text.len();
            if col_max_len.len() < col {
                col_max_len.resize(col, 10);
            }
            if let Some(l) = col_max_len.get_mut(col - 1) {
                *l = (*l).max(len);
            }
            cells.push(XlsxCell { col, text, num, style });
        }
        if !cells.is_empty() {
            row_tops.push((group_top_pt, *current_row));
            rows.push(XlsxRow { row_num: *current_row, cells });
            *current_row += 1;
        }
    }
}

fn is_table_header_row(rows: &[Vec<Cell>], row_idx: usize) -> bool {
    if row_idx != 0 || rows.len() < 2 {
        return false;
    }
    let row0 = match rows.first() {
        Some(r) => r,
        None => return false,
    };
    let non_empty: Vec<&Cell> = row0.iter().filter(|c| !c.text.trim().is_empty()).collect();
    if non_empty.is_empty() {
        return false;
    }
    // If every cell in row 0 is numeric, it is almost certainly a data row, not a header
    if non_empty.iter().all(|c| parse_numeric(&c.text).is_some()) {
        return false;
    }
    let row0_bold = non_empty.iter().any(|c| c.bold);
    let row0_has_span = non_empty.iter().any(|c| c.span > 1);
    let next_row_has_numeric = rows.get(1).is_some_and(|r1| r1.iter().any(|c| parse_numeric(&c.text).is_some()));
    row0_bold || row0_has_span || next_row_has_numeric || non_empty.iter().all(|c| parse_numeric(&c.text).is_none())
}

fn process_tables(
    p: &Page,
    tables: &[Table],
    rows: &mut Vec<XlsxRow>,
    row_tops: &mut Vec<(f64, usize)>,
    merges: &mut Vec<String>,
    col_max_len: &mut Vec<usize>,
    current_row: &mut usize,
) {
    if !rows.is_empty() && !tables.is_empty() {
        *current_row += 1;
    }

    for t in tables {
        let t_height = (t.rect[3] - t.rect[1]).max(0.0);
        let row_h = if !t.rows.is_empty() { t_height / t.rows.len() as f64 } else { 20.0 };
        for (row_idx, row) in t.rows.iter().enumerate() {
            let row_top_y = t.rect[3] - (row_idx as f64) * row_h;
            let row_top_pt = if p.height > 0.0 { (p.height - row_top_y).max(0.0) } else { 0.0 };
            let is_header = is_table_header_row(&t.rows, row_idx);
            let is_last = row_idx + 1 == t.rows.len();
            let mut col_cursor = 1usize;
            let mut cells = Vec::new();

            for c in row {
                let text = c.text.trim().to_string();
                let num = parse_numeric(&text);
                let style = if is_header {
                    STYLE_HEADER
                } else if is_last && c.bold {
                    STYLE_TOTAL_BORDER
                } else if num.is_some() {
                    STYLE_NUMBER_BORDER
                } else {
                    STYLE_TEXT_BORDER
                };

                let span = c.span.max(1);
                if span > 1 {
                    let c1 = col_name(col_cursor);
                    let c2 = col_name(col_cursor + span - 1);
                    merges.push(format!("{c1}{current_row}:{c2}{current_row}"));
                }

                let len = text.len();
                for sc in 0..span {
                    let c_idx = col_cursor + sc;
                    if col_max_len.len() < c_idx {
                        col_max_len.resize(c_idx, 10);
                    }
                    if let Some(l) = col_max_len.get_mut(c_idx - 1) {
                        *l = (*l).max(len / span);
                    }
                }

                cells.push(XlsxCell { col: col_cursor, text, num, style });
                col_cursor += span;
            }

            row_tops.push((row_top_pt, *current_row));
            rows.push(XlsxRow { row_num: *current_row, cells });
            *current_row += 1;
        }
        *current_row += 1; // blank row after table
    }
}

fn compute_image_anchor(top_pt: f64, row_tops: &[(f64, usize)], current_row: usize) -> (usize, i64) {
    if row_tops.is_empty() {
        let r = (top_pt / 20.0).floor().clamp(0.0, 500.0) as usize;
        let off = ((top_pt % 20.0) * 12700.0).round() as i64;
        (r, off)
    } else if top_pt <= row_tops[0].0 {
        (0, (top_pt * 12700.0).round() as i64)
    } else if top_pt >= row_tops.last().map(|(pt, _)| *pt).unwrap_or(0.0) {
        let last_r = row_tops.last().map(|(_, r)| *r).unwrap_or(1);
        let target_row = last_r.max(current_row.saturating_sub(1));
        (target_row.saturating_sub(1), 0i64)
    } else {
        let mut chosen_row = 0usize;
        let mut chosen_off = 0i64;
        for i in 0..row_tops.len().saturating_sub(1) {
            let (pt_a, r_a) = row_tops[i];
            let (pt_b, r_b) = row_tops[i + 1];
            if top_pt >= pt_a && top_pt <= pt_b {
                let frac = if (pt_b - pt_a) > 0.1 { (top_pt - pt_a) / (pt_b - pt_a) } else { 0.0 };
                let row_span = r_b.saturating_sub(r_a) as f64;
                let target_r = (r_a as f64) + frac * row_span;
                let mapped_row = target_r.floor() as usize;
                let mapped_frac = target_r - (mapped_row as f64);
                chosen_row = mapped_row.saturating_sub(1);
                chosen_off = (mapped_frac * 20.0 * 12700.0).round() as i64;
                break;
            }
        }
        (chosen_row, chosen_off)
    }
}

fn build_page_drawings(
    p: &Page,
    col_edges: &[f64],
    row_tops: &[(f64, usize)],
    current_row: usize,
    global_image_counter: &mut usize,
    media_files: &mut Vec<(String, Vec<u8>)>,
) -> Option<(String, String)> {
    if p.images.is_empty() {
        return None;
    }
    let mut drw_xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<xdr:wsDr xmlns:xdr=\"http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\">",
    );
    let mut drw_rels = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
    );

    for (local_idx, im) in p.images.iter().enumerate() {
        *global_image_counter += 1;
        let ext = if im.ext.is_empty() { "png" } else { im.ext };
        let media_name = format!("image{global_image_counter}.{ext}");
        media_files.push((media_name.clone(), im.bytes.to_vec()));

        let rotated = im.rotation.abs() > f64::EPSILON;
        let (w, h) = if rotated {
            (im.size[0].max(10.0), im.size[1].max(10.0))
        } else {
            ((im.rect[2] - im.rect[0]).max(10.0), (im.rect[3] - im.rect[1]).max(10.0))
        };
        let cx = (w * 12700.0).round() as i64;
        let cy = (h * 12700.0).round() as i64;
        let rot_attr = if rotated { format!(" rot=\"{}\"", ((-im.rotation).rem_euclid(360.0) * 60000.0).round() as i64) } else { String::new() };

        let natural_col = col_edges.iter().position(|&e| (im.rect[0] - e).abs() <= 16.0);
        let col = natural_col.unwrap_or_else(|| (im.rect[0] / 64.0).floor().clamp(0.0, 50.0) as usize);
        let top_pt = if p.height > 0.0 { (p.height - im.rect[3]).max(0.0) } else { 0.0 };
        let (row, row_off) = compute_image_anchor(top_pt, row_tops, current_row);
        let col_off = ((im.rect[0] % 64.0) * 12700.0).round() as i64;

        let r_id = local_idx + 1;
        drw_xml.push_str(&format!(
            "<xdr:oneCellAnchor><xdr:from><xdr:col>{col}</xdr:col><xdr:colOff>{col_off}</xdr:colOff><xdr:row>{row}</xdr:row><xdr:rowOff>{row_off}</xdr:rowOff></xdr:from><xdr:ext cx=\"{cx}\" cy=\"{cy}\"/><xdr:pic><xdr:nvPicPr><xdr:cNvPr id=\"{global_image_counter}\" name=\"Picture {global_image_counter}\"/><xdr:cNvPicPr><a:picLocks noChangeAspect=\"1\"/></xdr:cNvPicPr></xdr:nvPicPr><xdr:blipFill><a:blip xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" r:embed=\"rIdImg{r_id}\"/><a:stretch><a:fillRect/></a:stretch></xdr:blipFill><xdr:spPr><a:xfrm{rot_attr}><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></xdr:spPr></xdr:pic><xdr:clientData/></xdr:oneCellAnchor>"
        ));
        drw_rels.push_str(&format!(
            "<Relationship Id=\"rIdImg{r_id}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"../media/{media_name}\"/>"
        ));
    }

    drw_xml.push_str("</xdr:wsDr>");
    drw_rels.push_str("</Relationships>");
    Some((drw_xml, drw_rels))
}

fn build_sheet_xml(page_index: usize, rows: &[XlsxRow], merges: &[String], col_max_len: &[usize], has_drawing: bool) -> String {
    let mut cols_xml = String::new();
    if !col_max_len.is_empty() {
        cols_xml.push_str("<cols>");
        for (ci, &max_len) in col_max_len.iter().enumerate() {
            let col = ci + 1;
            let w = ((max_len as f64) * 1.2 + 4.0).clamp(10.0, 50.0);
            cols_xml.push_str(&format!("<col min=\"{col}\" max=\"{col}\" width=\"{w:.1}\" customWidth=\"1\"/>"));
        }
        cols_xml.push_str("</cols>");
    }

    let mut sheet_data = String::new();
    for r in rows {
        sheet_data.push_str(&format!("<row r=\"{}\">", r.row_num));
        for cell in &r.cells {
            let r_ref = format!("{}{}", col_name(cell.col), r.row_num);
            if let Some(val) = cell.num {
                sheet_data.push_str(&format!("<c r=\"{r_ref}\" s=\"{}\"><v>{val}</v></c>", cell.style));
            } else if !cell.text.is_empty() {
                sheet_data.push_str(&format!("<c r=\"{r_ref}\" t=\"inlineStr\" s=\"{}\"><is><t>{}</t></is></c>", cell.style, esc(&cell.text)));
            } else {
                sheet_data.push_str(&format!("<c r=\"{r_ref}\" s=\"{}\"/>", cell.style));
            }
        }
        sheet_data.push_str("</row>");
    }

    let mut merge_xml = String::new();
    if !merges.is_empty() {
        merge_xml.push_str(&format!("<mergeCells count=\"{}\">", merges.len()));
        for m in merges {
            merge_xml.push_str(&format!("<mergeCell ref=\"{m}\"/>"));
        }
        merge_xml.push_str("</mergeCells>");
    }

    let drawing_tag = if has_drawing { "<drawing r:id=\"rIdDrw1\"/>" } else { "" };
    let selected = if page_index == 0 { " tabSelected=\"1\"" } else { "" };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" \
xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
<sheetViews><sheetView{selected} workbookViewId=\"0\"/></sheetViews>\
<sheetFormatPr defaultRowHeight=\"20\"/>\
{cols_xml}\
<sheetData>{sheet_data}</sheetData>\
{merge_xml}\
<pageMargins left=\"0.7\" right=\"0.7\" top=\"0.75\" bottom=\"0.75\" header=\"0.3\" footer=\"0.3\"/>\
{drawing_tag}\
</worksheet>"
    )
}

/// An Excel document (.xlsx, Office Open XML SpreadsheetML).
pub fn xlsx(pages: &[Page], title: &str) -> Vec<u8> {
    let dummy = [Page::default()];
    let pages = if pages.is_empty() { &dummy[..] } else { pages };

    let mut sheets: Vec<String> = Vec::new();
    let mut sheet_drawings: Vec<Option<(String, String)>> = Vec::new(); // (drawing_xml, drawing_rels)
    let mut media_files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut global_image_counter = 0usize;

    for (page_index, p) in pages.iter().enumerate() {
        let (tables, consumed) = crate::tables(&p.blocks);
        let mut rows: Vec<XlsxRow> = Vec::new();
        let mut row_tops: Vec<(f64, usize)> = Vec::new();
        let mut merges: Vec<String> = Vec::new();
        let mut col_max_len: Vec<usize> = Vec::new();
        let mut current_row = 1usize;

        let col_edges = compute_col_edges(&p.blocks);
        process_header_blocks(p, &consumed, &col_edges, &mut rows, &mut row_tops, &mut col_max_len, &mut current_row);
        process_tables(p, &tables, &mut rows, &mut row_tops, &mut merges, &mut col_max_len, &mut current_row);

        let drawings = build_page_drawings(p, &col_edges, &row_tops, current_row, &mut global_image_counter, &mut media_files);
        let has_drawing = drawings.is_some();
        sheet_drawings.push(drawings);

        sheets.push(build_sheet_xml(page_index, &rows, &merges, &col_max_len, has_drawing));
    }

    // Styles XML
    let styles_xml = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">\
<fonts count=\"4\">\
<font><sz val=\"11\"/><name val=\"Calibri\"/><color theme=\"1\"/></font>\
<font><b/><sz val=\"11\"/><name val=\"Calibri\"/><color theme=\"1\"/></font>\
<font><b/><sz val=\"14\"/><name val=\"Calibri\"/><color theme=\"1\"/></font>\
<font><i/><sz val=\"10\"/><name val=\"Calibri\"/><color theme=\"1\"/></font>\
</fonts>\
<fills count=\"3\">\
<fill><patternFill patternType=\"none\"/></fill>\
<fill><patternFill patternType=\"gray125\"/></fill>\
<fill><patternFill patternType=\"solid\"><fgColor rgb=\"FFF1F5F9\"/></patternFill></fill>\
</fills>\
<borders count=\"3\">\
<border><left/><right/><top/><bottom/></border>\
<border><left style=\"thin\"><color rgb=\"FFD3D3D3\"/></left><right style=\"thin\"><color rgb=\"FFD3D3D3\"/></right><top style=\"thin\"><color rgb=\"FFD3D3D3\"/></top><bottom style=\"thin\"><color rgb=\"FFD3D3D3\"/></bottom></border>\
<border><left style=\"thin\"><color rgb=\"FFD3D3D3\"/></left><right style=\"thin\"><color rgb=\"FFD3D3D3\"/></right><top style=\"thin\"><color rgb=\"FFD3D3D3\"/></top><bottom style=\"double\"><color rgb=\"FF64748B\"/></bottom></border>\
</borders>\
<cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\"/></cellStyleXfs>\
<cellXfs count=\"6\">\
<xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/>\
<xf numFmtId=\"0\" fontId=\"1\" fillId=\"2\" borderId=\"1\" xfId=\"0\" applyFont=\"1\" applyFill=\"1\" applyBorder=\"1\"><alignment horizontal=\"center\" vertical=\"center\" wrapText=\"1\"/></xf>\
<xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"1\" xfId=\"0\" applyBorder=\"1\"><alignment vertical=\"center\"/></xf>\
<xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"1\" xfId=\"0\" applyBorder=\"1\"><alignment horizontal=\"right\" vertical=\"center\"/></xf>\
<xf numFmtId=\"0\" fontId=\"2\" fillId=\"0\" borderId=\"0\" xfId=\"0\" applyFont=\"1\"><alignment vertical=\"center\"/></xf>\
<xf numFmtId=\"0\" fontId=\"1\" fillId=\"0\" borderId=\"2\" xfId=\"0\" applyFont=\"1\" applyBorder=\"1\"><alignment vertical=\"center\"/></xf>\
</cellXfs>\
</styleSheet>";

    // Build package
    let mut z = Zip::default();

    // [Content_Types].xml
    let mut types = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
<Default Extension=\"xml\" ContentType=\"application/xml\"/>\
<Default Extension=\"png\" ContentType=\"image/png\"/>\
<Default Extension=\"jpg\" ContentType=\"image/jpeg\"/>\
<Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.openxmlformats-package.core-properties+xml\"/>\
<Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>\
<Override PartName=\"/xl/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml\"/>",
    );
    for i in 1..=sheets.len() {
        types.push_str(&format!(
            "<Override PartName=\"/xl/worksheets/sheet{i}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>"
        ));
        if sheet_drawings.get(i - 1).and_then(|d| d.as_ref()).is_some() {
            types.push_str(&format!(
                "<Override PartName=\"/xl/drawings/drawing{i}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.drawing+xml\"/>"
            ));
        }
    }
    types.push_str("</Types>");
    z.add("[Content_Types].xml", types.as_bytes(), true);

    // _rels/.rels
    let root_rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/>\
<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties\" Target=\"docProps/core.xml\"/>\
</Relationships>";
    z.add("_rels/.rels", root_rels.as_bytes(), true);

    // docProps/core.xml
    let core = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" \
xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\
<dc:title>{}</dc:title>\
<dc:creator>PDF Craft</dc:creator>\
</cp:coreProperties>",
        esc(title)
    );
    z.add("docProps/core.xml", core.as_bytes(), true);

    // xl/workbook.xml & xl/_rels/workbook.xml.rels
    let mut sheet_tags = String::new();
    let mut wb_rels = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rIdStyles\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>",
    );
    for i in 1..=sheets.len() {
        let name = if sheets.len() == 1 { "Sheet1".to_string() } else { format!("Page {i}") };
        sheet_tags.push_str(&format!("<sheet name=\"{name}\" sheetId=\"{i}\" r:id=\"rId{i}\"/>"));
        wb_rels.push_str(&format!(
            "<Relationship Id=\"rId{i}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet{i}.xml\"/>"
        ));
    }
    wb_rels.push_str("</Relationships>");

    let workbook_xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" \
xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
<sheets>{sheet_tags}</sheets>\
</workbook>"
    );

    z.add("xl/workbook.xml", workbook_xml.as_bytes(), true);
    z.add("xl/_rels/workbook.xml.rels", wb_rels.as_bytes(), true);
    z.add("xl/styles.xml", styles_xml.as_bytes(), true);

    for (i, sheet_xml) in sheets.iter().enumerate() {
        z.add(&format!("xl/worksheets/sheet{}.xml", i + 1), sheet_xml.as_bytes(), true);
        if let Some((drw_xml, drw_rels)) = sheet_drawings.get(i).and_then(|d| d.as_ref()) {
            let sheet_rels = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rIdDrw1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing\" Target=\"../drawings/drawing{}.xml\"/></Relationships>",
                i + 1
            );
            z.add(&format!("xl/worksheets/_rels/sheet{}.xml.rels", i + 1), sheet_rels.as_bytes(), true);
            z.add(&format!("xl/drawings/drawing{}.xml", i + 1), drw_xml.as_bytes(), true);
            z.add(&format!("xl/drawings/_rels/drawing{}.xml.rels", i + 1), drw_rels.as_bytes(), true);
        }
    }

    for (name, bytes) in &media_files {
        z.add(&format!("xl/media/{name}"), bytes, false);
    }

    z.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_col_name_mapping() {
        assert_eq!(col_name(1), "A");
        assert_eq!(col_name(26), "Z");
        assert_eq!(col_name(27), "AA");
        assert_eq!(col_name(28), "AB");
        assert_eq!(col_name(52), "AZ");
        assert_eq!(col_name(53), "BA");
    }

    #[test]
    fn test_numeric_parsing() {
        assert_eq!(parse_numeric("123"), Some(123.0));
        assert_eq!(parse_numeric("  $1,234.56  "), Some(1234.56));
        assert_eq!(parse_numeric("€99.95"), Some(99.95));
        assert_eq!(parse_numeric("£-12"), Some(-12.0));
        assert_eq!(parse_numeric("¥-12"), Some(-12.0));
        assert_eq!(parse_numeric("-£12.50"), Some(-12.50));
        assert_eq!(parse_numeric("-¥1200"), Some(-1200.0));
        assert_eq!(parse_numeric("5$3"), None);
        assert_eq!(parse_numeric("45%"), None);
        assert_eq!(parse_numeric("02134"), None);
        assert_eq!(parse_numeric("1,5"), None);
        assert_eq!(parse_numeric("555,1234"), None);
        assert_eq!(parse_numeric("12.34,56"), None);
        assert_eq!(parse_numeric("1234,567"), None);
        assert_eq!(parse_numeric(",123"), None);
        assert_eq!(parse_numeric("NotANumber"), None);
        assert_eq!(parse_numeric(""), None);
    }

    #[test]
    fn test_table_header_detection() {
        let dummy_cell = |text: &str, bold: bool| Cell { text: text.to_string(), size: 10.0, bold, italic: false, color: [0.0; 3], span: 1 };
        // Table with string header and numeric data
        let rows_with_header =
            vec![vec![dummy_cell("Item", true), dummy_cell("Price", true)], vec![dummy_cell("Widget", false), dummy_cell("19.99", false)]];
        assert!(is_table_header_row(&rows_with_header, 0));
        assert!(!is_table_header_row(&rows_with_header, 1));

        // Table with pure numeric data in row 0
        let rows_all_numeric =
            vec![vec![dummy_cell("100", false), dummy_cell("200", false)], vec![dummy_cell("300", false), dummy_cell("400", false)]];
        assert!(!is_table_header_row(&rows_all_numeric, 0));
    }

    #[test]
    fn test_xlsx_package_generation() {
        let p = Page {
            width: 612.0,
            height: 792.0,
            blocks: vec![
                Block {
                    text: "Invoice #1001".into(),
                    rect: [72.0, 700.0, 200.0, 715.0],
                    size: 16.0,
                    bold: true,
                    italic: false,
                    color: [0.0; 3],
                    rotation: 0.0,
                    space_around: [false, false],
                },
                Block {
                    text: "Item".into(),
                    rect: [72.0, 650.0, 150.0, 662.0],
                    size: 11.0,
                    bold: true,
                    italic: false,
                    color: [0.0; 3],
                    rotation: 0.0,
                    space_around: [false, false],
                },
                Block {
                    text: "Cost".into(),
                    rect: [200.0, 650.0, 250.0, 662.0],
                    size: 11.0,
                    bold: true,
                    italic: false,
                    color: [0.0; 3],
                    rotation: 0.0,
                    space_around: [false, false],
                },
                Block {
                    text: "Widget".into(),
                    rect: [72.0, 630.0, 150.0, 642.0],
                    size: 11.0,
                    bold: false,
                    italic: false,
                    color: [0.0; 3],
                    rotation: 0.0,
                    space_around: [false, false],
                },
                Block {
                    text: "$19.99".into(),
                    rect: [200.0, 630.0, 250.0, 642.0],
                    size: 11.0,
                    bold: false,
                    italic: false,
                    color: [0.0; 3],
                    rotation: 0.0,
                    space_around: [false, false],
                },
            ],
            images: vec![crate::Image {
                ext: "png",
                bytes: vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A].into(),
                rect: [50.0, 50.0, 100.0, 100.0],
                size: [50.0, 50.0],
                rotation: 0.0,
            }],
            background: None,
        };

        let bytes = xlsx(&[p], "Test Invoice");
        assert!(bytes.starts_with(b"PK\x03\x04"));
        for part in [
            "[Content_Types].xml",
            "_rels/.rels",
            "docProps/core.xml",
            "xl/workbook.xml",
            "xl/styles.xml",
            "xl/worksheets/sheet1.xml",
            "xl/drawings/drawing1.xml",
            "xl/media/image1.png",
        ] {
            assert!(bytes.windows(part.len()).any(|w| w == part.as_bytes()), "missing part {part}");
        }
    }

    #[test]
    fn test_sheet_xml_content() {
        let rows = vec![XlsxRow {
            row_num: 1,
            cells: vec![
                XlsxCell { col: 1, text: "Widget".into(), num: None, style: STYLE_DEFAULT },
                XlsxCell { col: 2, text: "$19.99".into(), num: Some(19.99), style: STYLE_NUMBER_BORDER },
            ],
        }];
        let xml = build_sheet_xml(0, &rows, &["A1:B1".to_string()], &[10, 10], true);
        assert!(xml.contains("<v>19.99</v>"));
        assert!(xml.contains("<mergeCell ref=\"A1:B1\"/>"));
        assert!(xml.contains("<drawing r:id=\"rIdDrw1\"/>"));
    }
}
