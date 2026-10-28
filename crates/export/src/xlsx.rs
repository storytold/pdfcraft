//! Excel spreadsheet (.xlsx, Office Open XML SpreadsheetML): sheets per page,
//! detected tables mapped to grid cells, remaining text blocks placed in rows.

use crate::{Block, Page, Zip, esc, tables};

/// Convert a 0-based column index to an Excel column name ("A", "B", ..., "Z", "AA", etc.).
pub fn col_name(mut col_idx: usize) -> String {
    let mut name = String::new();
    loop {
        let rem = col_idx % 26;
        if let Some(ch) = char::from_u32(b'A' as u32 + rem as u32) {
            name.push(ch);
        }
        if col_idx < 26 {
            break;
        }
        col_idx = (col_idx / 26).saturating_sub(1);
    }
    name.chars().rev().collect()
}

/// A cell on a worksheet row.
struct SheetCell {
    col: usize,
    text: String,
    bold: bool,
}

/// A row on a worksheet.
struct SheetRow {
    row_num: usize,
    cells: Vec<SheetCell>,
}

/// Format a worksheet as XML.
fn worksheet_xml(rows: &[SheetRow], merges: &[String], max_cols: usize) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
        <worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" \
        xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\n",
    );

    if max_cols > 0 {
        let bounded_cols = max_cols.min(16384);
        xml.push_str(&format!(
            "<cols><col min=\"1\" max=\"{bounded_cols}\" width=\"16\" customWidth=\"1\"/></cols>\n"
        ));
    }

    xml.push_str("<sheetData>\n");
    for row in rows {
        let r_num = row.row_num;
        xml.push_str(&format!("<row r=\"{r_num}\">\n"));
        for cell in &row.cells {
            let col_str = col_name(cell.col);
            let cell_ref = format!("{col_str}{r_num}");
            let style_attr = if cell.bold { " s=\"1\"" } else { "" };
            let trimmed = cell.text.trim();

            let is_numeric = !trimmed.is_empty()
                && trimmed.parse::<f64>().is_ok()
                && (trimmed == "0" || !trimmed.starts_with('0') || trimmed.starts_with("0."));

            if is_numeric {
                xml.push_str(&format!("<c r=\"{cell_ref}\"{style_attr}><v>{}</v></c>\n", esc(trimmed)));
            } else {
                xml.push_str(&format!(
                    "<c r=\"{cell_ref}\"{style_attr} t=\"inlineStr\"><is><t>{}</t></is></c>\n",
                    esc(&cell.text)
                ));
            }
        }
        xml.push_str("</row>\n");
    }
    xml.push_str("</sheetData>\n");

    if !merges.is_empty() {
        xml.push_str(&format!("<mergeCells count=\"{}\">\n", merges.len()));
        for m in merges {
            xml.push_str(&format!("<mergeCell ref=\"{m}\"/>\n"));
        }
        xml.push_str("</mergeCells>\n");
    }

    xml.push_str("</worksheet>");
    xml
}

/// Convert page content into worksheet rows and merge specifications.
fn page_to_sheet(page: &Page) -> (Vec<SheetRow>, Vec<String>, usize) {
    let (detected_tables, consumed) = tables(&page.blocks);
    let mut rows_out: Vec<SheetRow> = Vec::new();
    let mut merges: Vec<String> = Vec::new();
    let mut max_cols = 1usize;
    let mut current_row_num = 1usize;

    // Group unconsumed blocks into horizontal rows (within 5 pt y-delta)
    let mut unconsumed_blocks: Vec<&Block> = page
        .blocks
        .iter()
        .enumerate()
        .filter(|(i, _)| !consumed.get(*i).copied().unwrap_or(false))
        .map(|(_, b)| b)
        .collect();
    // Sort top-to-bottom (y descending)
    unconsumed_blocks.sort_by(|a, b| b.rect[3].total_cmp(&a.rect[3]));

    // Interleave tables and unconsumed block rows by top-to-bottom position
    enum PageItem<'a> {
        TableItem(&'a crate::Table),
        BlockRow(Vec<&'a Block>),
    }

    let mut items: Vec<(f64, PageItem<'_>)> = Vec::new();
    for t in &detected_tables {
        items.push((t.rect[3], PageItem::TableItem(t)));
    }

    // Cluster unconsumed blocks into rows
    let mut current_block_row: Vec<&Block> = Vec::new();
    for b in unconsumed_blocks {
        if let Some(first) = current_block_row.first() {
            if (first.rect[1] - b.rect[1]).abs() <= 6.0 {
                current_block_row.push(b);
                continue;
            } else {
                let y = current_block_row.first().map_or(0.0, |b| b.rect[3]);
                let mut row_blocks = std::mem::take(&mut current_block_row);
                row_blocks.sort_by(|a, b| a.rect[0].total_cmp(&b.rect[0]));
                items.push((y, PageItem::BlockRow(row_blocks)));
                current_block_row.push(b);
            }
        } else {
            current_block_row.push(b);
        }
    }
    if !current_block_row.is_empty() {
        let y = current_block_row.first().map_or(0.0, |b| b.rect[3]);
        current_block_row.sort_by(|a, b| a.rect[0].total_cmp(&b.rect[0]));
        items.push((y, PageItem::BlockRow(current_block_row)));
    }

    // Sort all page items top-to-bottom
    items.sort_by(|a, b| b.0.total_cmp(&a.0));

    for (_, item) in items {
        match item {
            PageItem::TableItem(t) => {
                for r in &t.rows {
                    let mut cells = Vec::new();
                    let mut col_idx = 0usize;
                    for c in r {
                        let span = c.span.max(1);
                        if span > 1 {
                            let start_col = col_name(col_idx);
                            let end_col = col_name(col_idx + span - 1);
                            merges.push(format!("{start_col}{current_row_num}:{end_col}{current_row_num}"));
                        }
                        cells.push(SheetCell {
                            col: col_idx,
                            text: c.text.clone(),
                            bold: c.bold,
                        });
                        col_idx = col_idx.saturating_add(span);
                    }
                    max_cols = max_cols.max(col_idx);
                    rows_out.push(SheetRow {
                        row_num: current_row_num,
                        cells,
                    });
                    current_row_num = current_row_num.saturating_add(1);
                }
                // Leave an empty gap row after table
                current_row_num = current_row_num.saturating_add(1);
            }
            PageItem::BlockRow(blocks) => {
                let mut cells = Vec::new();
                for (ci, b) in blocks.into_iter().enumerate() {
                    cells.push(SheetCell {
                        col: ci,
                        text: b.text.clone(),
                        bold: b.bold,
                    });
                }
                max_cols = max_cols.max(cells.len());
                rows_out.push(SheetRow {
                    row_num: current_row_num,
                    cells,
                });
                current_row_num = current_row_num.saturating_add(1);
            }
        }
    }

    (rows_out, merges, max_cols)
}

const ROOT_RELS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\n\
  <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/>\n\
</Relationships>";

const STYLES_XML: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">\n\
  <fonts count=\"2\">\n\
    <font><sz val=\"11\"/><name val=\"Calibri\"/></font>\n\
    <font><b/><sz val=\"11\"/><name val=\"Calibri\"/></font>\n\
  </fonts>\n\
  <fills count=\"2\">\n\
    <fill><patternFill patternType=\"none\"/></fill>\n\
    <fill><patternFill patternType=\"gray125\"/></fill>\n\
  </fills>\n\
  <borders count=\"1\">\n\
    <border><left/><right/><top/><bottom/><diagonal/></border>\n\
  </borders>\n\
  <cellStyleXfs count=\"1\">\n\
    <xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\"/>\n\
  </cellStyleXfs>\n\
  <cellXfs count=\"2\">\n\
    <xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/>\n\
    <xf numFmtId=\"0\" fontId=\"1\" fillId=\"0\" borderId=\"0\" xfId=\"0\" applyFont=\"1\"/>\n\
  </cellXfs>\n\
</styleSheet>";

/// An Excel spreadsheet (.xlsx, Office Open XML SpreadsheetML).
pub fn xlsx(pages: &[Page], _title: &str) -> Vec<u8> {
    let mut z = Zip::default();
    let sheet_count = pages.len().max(1);

    // [Content_Types].xml
    let mut content_types = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
        <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\n\
          <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\n\
          <Default Extension=\"xml\" ContentType=\"application/xml\"/>\n\
          <Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>\n\
          <Override PartName=\"/xl/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml\"/>\n",
    );
    for i in 1..=sheet_count {
        content_types.push_str(&format!(
            "  <Override PartName=\"/xl/worksheets/sheet{i}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>\n"
        ));
    }
    content_types.push_str("</Types>");

    // xl/_rels/workbook.xml.rels
    let mut wb_rels = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
        <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\n\
          <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>\n",
    );
    for i in 1..=sheet_count {
        wb_rels.push_str(&format!(
            "  <Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet{i}.xml\"/>\n",
            i + 1
        ));
    }
    wb_rels.push_str("</Relationships>");

    // xl/workbook.xml
    let mut wb_xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
        <workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" \
        xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\n\
          <sheets>\n",
    );
    for i in 1..=sheet_count {
        let name = if sheet_count == 1 { "Sheet1".to_string() } else { format!("Sheet {i}") };
        wb_xml.push_str(&format!("    <sheet name=\"{name}\" sheetId=\"{i}\" r:id=\"rId{}\"/>\n", i + 1));
    }
    wb_xml.push_str("  </sheets>\n</workbook>");

    z.add("[Content_Types].xml", content_types.as_bytes(), true);
    z.add("_rels/.rels", ROOT_RELS.as_bytes(), true);
    z.add("xl/_rels/workbook.xml.rels", wb_rels.as_bytes(), true);
    z.add("xl/workbook.xml", wb_xml.as_bytes(), true);
    z.add("xl/styles.xml", STYLES_XML.as_bytes(), true);

    if pages.is_empty() {
        let ws = worksheet_xml(&[], &[], 1);
        z.add("xl/worksheets/sheet1.xml", ws.as_bytes(), true);
    } else {
        for (i, p) in pages.iter().enumerate() {
            let (rows, merges, max_cols) = page_to_sheet(p);
            let ws = worksheet_xml(&rows, &merges, max_cols);
            z.add(&format!("xl/worksheets/sheet{}.xml", i + 1), ws.as_bytes(), true);
        }
    }

    z.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Block;

    fn block(text: &str, x0: f64, y0: f64, x1: f64, y1: f64, bold: bool) -> Block {
        Block {
            text: text.to_string(),
            rect: [x0, y0, x1, y1],
            size: 11.0,
            bold,
            italic: false,
            color: [0.0, 0.0, 0.0],
            rotation: 0.0,
        }
    }

    #[test]
    fn col_name_formatting() {
        assert_eq!(col_name(0), "A");
        assert_eq!(col_name(1), "B");
        assert_eq!(col_name(25), "Z");
        assert_eq!(col_name(26), "AA");
        assert_eq!(col_name(27), "AB");
        assert_eq!(col_name(51), "AZ");
        assert_eq!(col_name(52), "BA");
    }

    #[test]
    fn xlsx_is_a_spreadsheetml_package() {
        let page = Page {
            width: 612.0,
            height: 792.0,
            blocks: vec![
                block("Item", 50.0, 700.0, 150.0, 715.0, true),
                block("Qty", 160.0, 700.0, 200.0, 715.0, true),
                block("Price", 210.0, 700.0, 260.0, 715.0, true),
                block("Apples", 50.0, 680.0, 150.0, 695.0, false),
                block("5", 160.0, 680.0, 200.0, 695.0, false),
                block("12.50", 210.0, 680.0, 260.0, 695.0, false),
            ],
            images: Vec::new(),
        };

        let data = xlsx(&[page], "Invoice");
        assert!(data.starts_with(b"PK"), "xlsx is a ZIP file");

        let names = [
            "[Content_Types].xml",
            "xl/workbook.xml",
            "xl/styles.xml",
            "xl/_rels/workbook.xml.rels",
            "xl/worksheets/sheet1.xml",
            "_rels/.rels",
        ];
        for n in names {
            assert!(data.windows(n.len()).any(|w| w == n.as_bytes()), "contains {n}");
        }
    }

    #[test]
    fn xlsx_empty_pages_produce_valid_sheet() {
        let data = xlsx(&[], "Empty");
        assert!(data.starts_with(b"PK"));
        assert!(data.windows(b"sheet1.xml".len()).any(|w| w == b"sheet1.xml"));
    }

    #[test]
    fn xlsx_multi_page_creates_multiple_sheets() {
        let p1 = Page {
            width: 612.0,
            height: 792.0,
            blocks: vec![block("Page 1 text", 50.0, 700.0, 200.0, 715.0, false)],
            images: Vec::new(),
        };
        let p2 = Page {
            width: 612.0,
            height: 792.0,
            blocks: vec![block("Page 2 text", 50.0, 700.0, 200.0, 715.0, false)],
            images: Vec::new(),
        };

        let data = xlsx(&[p1, p2], "Multi");
        assert!(data.windows(b"sheet1.xml".len()).any(|w| w == b"sheet1.xml"));
        assert!(data.windows(b"sheet2.xml".len()).any(|w| w == b"sheet2.xml"));
    }
}
