//! Excel spreadsheet (.xlsx, Office Open XML SpreadsheetML) export.
//!
//! Provides spatial grid and vector-line table reconstruction, mapping tabular data
//! (invoices, shipping labels, financial tables, forms) into clean, properly merged
//! worksheet cells with data-type awareness (numbers vs. text), column widths, and borders.

use crate::{Block, Page, Zip, esc};

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
    // Identifiers (ZIP codes, part numbers) keep their leading zero, and f64 cannot hold
    // more than 15 exact digits: both stay text.
    if unsigned.chars().filter(char::is_ascii_digit).count() > 15
        || (unsigned.len() > 1 && unsigned.starts_with('0') && !unsigned.starts_with("0."))
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
    // Clean currency and percentage signs
    let mut cleaned = String::with_capacity(trimmed.len());
    let mut has_digit = false;
    for c in trimmed.chars() {
        if c.is_ascii_digit() || c == '.' || c == '-' || c == '+' {
            if c.is_ascii_digit() {
                has_digit = true;
            }
            cleaned.push(c);
        } else if c == ',' || c == '$' || c == '€' || c == '£' || c == '¥' || c == '%' {
            // Ignored formatting characters
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

/// An Excel document (.xlsx, Office Open XML SpreadsheetML).
pub fn xlsx(pages: &[Page], title: &str) -> Vec<u8> {
    let dummy = [Page::default()];
    let pages = if pages.is_empty() { &dummy[..] } else { pages };

    let mut sheets: Vec<String> = Vec::new();

    for (page_index, p) in pages.iter().enumerate() {
        let (tables, consumed) = crate::tables(&p.blocks);
        let mut rows: Vec<XlsxRow> = Vec::new();
        let mut merges: Vec<String> = Vec::new();
        let mut col_max_len: Vec<usize> = Vec::new();

        let mut current_row = 1usize;

        // Process non-table blocks (e.g. Title, header info)
        let mut header_blocks: Vec<&Block> = p
            .blocks
            .iter()
            .enumerate()
            .filter(|(i, b)| !consumed.get(*i).copied().unwrap_or(false) && !b.text.trim().is_empty())
            .map(|(_, b)| b)
            .collect();
        header_blocks.sort_by(|a, b| b.rect[3].total_cmp(&a.rect[3]).then(a.rect[0].total_cmp(&b.rect[0])));

        // Cluster blocks into rows if there are standalone blocks
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

        // Emit header blocks
        for mut group in line_groups {
            group.sort_by(|a, b| a.rect[0].total_cmp(&b.rect[0]));
            let mut cells = Vec::new();
            for (ci, b) in group.iter().enumerate() {
                let col = ci + 1;
                let text = b.text.trim().to_string();
                let num = parse_numeric(&text);
                let style = if b.size >= 14.0 || b.bold {
                    STYLE_TITLE
                } else {
                    STYLE_DEFAULT
                };
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
                rows.push(XlsxRow { row_num: current_row, cells });
                current_row += 1;
            }
        }

        if !rows.is_empty() && !tables.is_empty() {
            // Add a blank row between headers and tables
            current_row += 1;
        }

        // Process detected tables
        for t in tables {
            for (row_idx, row) in t.rows.iter().enumerate() {
                let is_header = row_idx == 0;
                let is_last = row_idx + 1 == t.rows.len();
                let mut col_cursor = 1usize;
                let mut cells = Vec::new();

                for c in row {
                    let text = c.text.trim().to_string();
                    let num = if is_header { None } else { parse_numeric(&text) };
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

                rows.push(XlsxRow { row_num: current_row, cells });
                current_row += 1;
            }
            current_row += 1; // blank row after table
        }

        // Generate sheet XML
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
        for r in &rows {
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
            for m in &merges {
                merge_xml.push_str(&format!("<mergeCell ref=\"{m}\"/>"));
            }
            merge_xml.push_str("</mergeCells>");
        }

        // Only the first sheet is selected; marking every sheet selected opens Excel in
        // group mode, where an edit applies to every page sheet at once.
        let selected = if page_index == 0 { " tabSelected=\"1\"" } else { "" };
        let sheet_xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" \
xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
<sheetViews><sheetView{selected} workbookViewId=\"0\"/></sheetViews>\
<sheetFormatPr defaultRowHeight=\"20\"/>\
{cols_xml}\
<sheetData>{sheet_data}</sheetData>\
{merge_xml}\
<pageMargins left=\"0.7\" right=\"0.7\" top=\"0.75\" bottom=\"0.75\" header=\"0.3\" footer=\"0.3\"/>\
</worksheet>"
        );

        sheets.push(sheet_xml);
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
<Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.openxmlformats-package.core-properties+xml\"/>\
<Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>\
<Override PartName=\"/xl/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml\"/>",
    );
    for i in 1..=sheets.len() {
        types.push_str(&format!(
            "<Override PartName=\"/xl/worksheets/sheet{i}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>"
        ));
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
                },
                Block {
                    text: "Item".into(),
                    rect: [72.0, 650.0, 150.0, 662.0],
                    size: 11.0,
                    bold: true,
                    italic: false,
                    color: [0.0; 3],
                    rotation: 0.0,
                },
                Block {
                    text: "Cost".into(),
                    rect: [200.0, 650.0, 250.0, 662.0],
                    size: 11.0,
                    bold: true,
                    italic: false,
                    color: [0.0; 3],
                    rotation: 0.0,
                },
                Block {
                    text: "Widget".into(),
                    rect: [72.0, 630.0, 150.0, 642.0],
                    size: 11.0,
                    bold: false,
                    italic: false,
                    color: [0.0; 3],
                    rotation: 0.0,
                },
                Block {
                    text: "$19.99".into(),
                    rect: [200.0, 630.0, 250.0, 642.0],
                    size: 11.0,
                    bold: false,
                    italic: false,
                    color: [0.0; 3],
                    rotation: 0.0,
                },
            ],
            images: Vec::new(),
            background: None,
        };

        let bytes = xlsx(&[p], "Test Invoice");
        assert!(bytes.starts_with(b"PK\x03\x04"));
        for part in ["[Content_Types].xml", "_rels/.rels", "docProps/core.xml", "xl/workbook.xml", "xl/styles.xml", "xl/worksheets/sheet1.xml"] {
            assert!(bytes.windows(part.len()).any(|w| w == part.as_bytes()), "missing part {part}");
        }
    }
}
