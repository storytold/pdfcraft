//! Excel workbooks (.xlsx): [`xlsx`].

use crate::{Item, Page, Zip, allowed, cells, esc, hex, items};

/// Excel's column name for a 0-based index: A, B, …, Z, AA, ….
fn col_name(mut i: usize) -> String {
    let mut s = Vec::new();
    loop {
        s.push(b'A' + (i % 26) as u8);
        if i < 26 {
            break;
        }
        i = i / 26 - 1;
    }
    s.reverse();
    String::from_utf8(s).unwrap_or_default()
}

/// How an Excel export lays the document out on sheets (the whole document by default).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum XlsxSheets {
    /// One sheet for the whole document: the pages follow one another down the sheet, so a
    /// table that runs across pages stays together.
    #[default]
    WholeDocument,
    /// One sheet per page, named "Page N".
    PerPage,
}

/// Excel's own limits: rows and columns of a sheet, characters of a cell and of a sheet name.
const XLSX_MAX_ROWS: usize = 1_048_576;
const XLSX_MAX_COLS: usize = 16_384;
const XLSX_CELL_CHARS: usize = 32_767;
const XLSX_SHEET_NAME_CHARS: usize = 31;

/// A sheet name Excel accepts: none of `[ ] : * ? / \`, no characters XML can't hold, no
/// apostrophe at either end, at most 31 characters, not empty, not "History" (Excel reserves it),
/// and not one already in `taken` (compared without regard to case, as Excel does).
fn sheet_name(wanted: &str, taken: &[String]) -> String {
    let cleaned: String = wanted.chars().filter(|c| allowed(*c) && !c.is_control() && !"[]:*?/\\".contains(*c)).collect();
    let mut base = cleaned.trim().trim_matches('\'').trim().to_string();
    if base.is_empty() {
        base = "Sheet".into();
    }
    if base.eq_ignore_ascii_case("History") {
        base = "History sheet".into();
    }
    let fit = |s: &str, room: usize| -> String { s.chars().take(room).collect::<String>().trim_end().trim_end_matches('\'').to_string() };
    let mut name = fit(&base, XLSX_SHEET_NAME_CHARS);
    let mut n = 2;
    while taken.iter().any(|t| t.to_lowercase() == name.to_lowercase()) {
        let suffix = format!(" ({n})");
        name = format!("{}{suffix}", fit(&base, XLSX_SHEET_NAME_CHARS - suffix.chars().count()));
        n += 1;
    }
    name
}

/// The workbook's fonts, number formats and cell styles (a style is a font with a number format).
#[derive(Default)]
struct XlsxStyles {
    /// (size in half points, bold, italic, colour).
    fonts: Vec<(i64, bool, bool, String)>,
    /// Custom number formats, numbered from 164 as Excel's own are.
    formats: Vec<String>,
    /// (font index, number format id); index + 1 is the cell's `s`.
    xfs: Vec<(usize, usize)>,
}

impl XlsxStyles {
    fn font(&mut self, size: f64, bold: bool, italic: bool, color: [f64; 3]) -> usize {
        let size = if size.is_finite() { (size.clamp(1.0, 409.0) * 2.0).round() as i64 } else { 22 };
        let key = (size, bold, italic, hex(color).unwrap_or_default());
        match self.fonts.iter().position(|f| *f == key) {
            Some(i) => i + 1,
            None => {
                self.fonts.push(key);
                self.fonts.len()
            }
        }
    }

    /// The cell style for `font` (from [`Self::font`]) and `format` ("" = General).
    fn style(&mut self, font: usize, format: &str) -> usize {
        let id = if format.is_empty() {
            0
        } else {
            163 + match self.formats.iter().position(|f| f == format) {
                Some(i) => i + 1,
                None => {
                    self.formats.push(format.to_string());
                    self.formats.len()
                }
            }
        };
        match self.xfs.iter().position(|x| *x == (font, id)) {
            Some(i) => i + 1,
            None => {
                self.xfs.push((font, id));
                self.xfs.len()
            }
        }
    }
}

/// An Excel workbook (.xlsx, Office Open XML): the whole document on one sheet named after it, or
/// a sheet per page ("Page N"). A detected table becomes rows of cells (a cell that spans columns
/// is merged across them); a paragraph becomes a row with its text in the first column; in
/// reading order. Size, bold, italic, colour and family are kept; images are left out (a
/// spreadsheet is for the figures). Each cell's text is read by [`cells::classify`]: numbers,
/// amounts, percentages and unambiguous dates become values Excel computes with, shown exactly as
/// on the page; everything else stays text. Excel's limits hold: a sheet that would pass its last
/// row continues on a new one, columns past the last are left out, a cell keeps its first 32,767
/// characters, and characters XML can't hold are dropped.
pub fn xlsx(pages: &[Page], title: &str, layout: XlsxSheets) -> Vec<u8> {
    xlsx_bounded(pages, title, layout, XLSX_MAX_ROWS, XLSX_MAX_COLS)
}

/// [`xlsx`] within `max_rows` and `max_cols` (Excel's own limits; smaller ones in tests).
fn xlsx_bounded(pages: &[Page], title: &str, layout: XlsxSheets, max_rows: usize, max_cols: usize) -> Vec<u8> {
    struct Sheet {
        name: String,
        rows: Vec<String>,
        merges: Vec<String>,
        widths: Vec<f64>,
    }
    let mut styles = XlsxStyles::default();
    let mut names: Vec<String> = Vec::new();
    let new_sheet = |wanted: &str, names: &mut Vec<String>| -> Sheet {
        let name = sheet_name(wanted, names);
        names.push(name.clone());
        Sheet { name, rows: Vec::new(), merges: Vec::new(), widths: Vec::new() }
    };
    let base = match layout {
        XlsxSheets::WholeDocument => title.to_string(),
        XlsxSheets::PerPage => "Page 1".to_string(),
    };
    let mut page_no = 1;
    let mut sheets: Vec<Sheet> = Vec::new();
    let mut sheet = new_sheet(&base, &mut names);
    let cell = |styles: &mut XlsxStyles, col: usize, row: usize, font: usize, text: &str| -> String {
        let r = format!("{}{row}", col_name(col));
        match cells::classify(text) {
            cells::Value::Number { value, format } => format!("<c r=\"{r}\" s=\"{}\"><v>{value}</v></c>", styles.style(font, &format)),
            cells::Value::Date { serial, format } => format!("<c r=\"{r}\" s=\"{}\"><v>{serial}</v></c>", styles.style(font, &format)),
            cells::Value::Text => {
                let text: String = text.chars().take(XLSX_CELL_CHARS).collect();
                format!("<c r=\"{r}\" s=\"{}\" t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>", styles.style(font, ""), esc(&text))
            }
        }
    };
    for it in items(pages) {
        // Rows this item needs; past Excel's last row the content goes on on a new sheet.
        let needed = match &it {
            Item::Para(..) => 1,
            Item::Table(t) => t.rows.len().min(max_rows),
            _ => 0,
        };
        if needed > 0 && sheet.rows.len() + needed > max_rows {
            let next = new_sheet(&sheet.name, &mut names);
            sheets.push(std::mem::replace(&mut sheet, next));
        }
        let row = sheet.rows.len() + 1;
        match it {
            Item::Para(b, _) => {
                let font = styles.font(b.size, b.bold, b.italic, b.color);
                let xml = cell(&mut styles, 0, row, font, &b.text);
                sheet.rows.push(format!("<row r=\"{row}\">{xml}</row>"));
            }
            Item::Table(t) => {
                let mut edges = t.cols.clone();
                edges.push(t.rect[2]);
                for (k, w) in edges.windows(2).enumerate().take(max_cols) {
                    let width = (w[1] - w[0]).max(20.0);
                    if sheet.widths.len() <= k {
                        sheet.widths.resize(k + 1, 0.0);
                    }
                    sheet.widths[k] = sheet.widths[k].max(width);
                }
                for (i, cells_of_row) in t.rows.iter().take(max_rows).enumerate() {
                    let row = row + i;
                    let mut xml = String::new();
                    let mut col = 0;
                    for c in cells_of_row {
                        if col >= max_cols {
                            break;
                        }
                        let font = styles.font(c.size, c.bold, c.italic, c.color);
                        xml.push_str(&cell(&mut styles, col, row, font, &c.text));
                        let last = (col + c.span.max(1) - 1).min(max_cols - 1);
                        if last > col {
                            sheet.merges.push(format!("{}{row}:{}{row}", col_name(col), col_name(last)));
                        }
                        col += c.span.max(1);
                    }
                    sheet.rows.push(format!("<row r=\"{row}\">{xml}</row>"));
                }
            }
            Item::Img(_) => {}
            Item::PageBreak => {
                if layout == XlsxSheets::PerPage {
                    page_no += 1;
                    let next = new_sheet(&format!("Page {page_no}"), &mut names);
                    sheets.push(std::mem::replace(&mut sheet, next));
                }
            }
        }
    }
    sheets.push(sheet);
    let head = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";
    let mut types = format!(
        "{head}<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
<Default Extension=\"xml\" ContentType=\"application/xml\"/>\
<Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>\
<Override PartName=\"/xl/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml\"/>\
<Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.openxmlformats-package.core-properties+xml\"/>"
    );
    let mut workbook = format!(
        "{head}<workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheets>"
    );
    let mut wb_rels = format!("{head}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">");
    let mut z = Zip::default();
    for (i, sheet) in sheets.iter().enumerate() {
        let n = i + 1;
        types.push_str(&format!("<Override PartName=\"/xl/worksheets/sheet{n}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>"));
        workbook.push_str(&format!("<sheet name=\"{}\" sheetId=\"{n}\" r:id=\"rId{n}\"/>", esc(&sheet.name)));
        wb_rels.push_str(&format!("<Relationship Id=\"rId{n}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet{n}.xml\"/>"));
        let mut xml = format!(
            "{head}<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">"
        );
        if !sheet.widths.is_empty() {
            xml.push_str("<cols>");
            for (k, w) in sheet.widths.iter().enumerate() {
                // Points to Excel's character units (Calibri 11 is about 5.5 pt a character).
                let chars = (w / 5.5).clamp(4.0, 120.0);
                xml.push_str(&format!("<col min=\"{}\" max=\"{}\" width=\"{chars:.1}\" customWidth=\"1\"/>", k + 1, k + 1));
            }
            xml.push_str("</cols>");
        }
        if sheet.rows.is_empty() {
            xml.push_str("<sheetData/>");
        } else {
            xml.push_str("<sheetData>");
            xml.extend(sheet.rows.iter().map(String::as_str));
            xml.push_str("</sheetData>");
        }
        if !sheet.merges.is_empty() {
            xml.push_str(&format!("<mergeCells count=\"{}\">", sheet.merges.len()));
            for m in &sheet.merges {
                xml.push_str(&format!("<mergeCell ref=\"{m}\"/>"));
            }
            xml.push_str("</mergeCells>");
        }
        xml.push_str("</worksheet>");
        z.add(&format!("xl/worksheets/sheet{n}.xml"), xml.as_bytes(), true);
    }
    types.push_str("</Types>");
    workbook.push_str("</sheets></workbook>");
    wb_rels.push_str(&format!(
        "<Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/></Relationships>",
        sheets.len() + 1
    ));
    let mut out = String::from(head);
    out.push_str("<styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">");
    if !styles.formats.is_empty() {
        out.push_str(&format!("<numFmts count=\"{}\">", styles.formats.len()));
        for (i, f) in styles.formats.iter().enumerate() {
            out.push_str(&format!("<numFmt numFmtId=\"{}\" formatCode=\"{}\"/>", 164 + i, esc(f)));
        }
        out.push_str("</numFmts>");
    }
    out.push_str(&format!("<fonts count=\"{}\"><font><sz val=\"11\"/><name val=\"Calibri\"/></font>", styles.fonts.len() + 1));
    for (size, bold, italic, color) in &styles.fonts {
        out.push_str("<font>");
        if *bold {
            out.push_str("<b/>");
        }
        if *italic {
            out.push_str("<i/>");
        }
        out.push_str(&format!("<sz val=\"{}\"/>", *size as f64 / 2.0));
        if !color.is_empty() {
            out.push_str(&format!("<color rgb=\"FF{color}\"/>"));
        }
        out.push_str("<name val=\"Calibri\"/></font>");
    }
    out.push_str(
        "</fonts><fills count=\"2\"><fill><patternFill patternType=\"none\"/></fill><fill><patternFill patternType=\"gray125\"/></fill></fills>\
<borders count=\"1\"><border><left/><right/><top/><bottom/><diagonal/></border></borders>\
<cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\"/></cellStyleXfs>",
    );
    out.push_str(&format!("<cellXfs count=\"{}\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/>", styles.xfs.len() + 1));
    for (font, fmt) in &styles.xfs {
        let apply = if *fmt == 0 { "" } else { " applyNumberFormat=\"1\"" };
        out.push_str(&format!("<xf numFmtId=\"{fmt}\" fontId=\"{font}\" fillId=\"0\" borderId=\"0\" xfId=\"0\" applyFont=\"1\"{apply}/>"));
    }
    out.push_str("</cellXfs><cellStyles count=\"1\"><cellStyle name=\"Normal\" xfId=\"0\" builtinId=\"0\"/></cellStyles></styleSheet>");
    let rels = format!(
        "{head}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/>\
<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties\" Target=\"docProps/core.xml\"/></Relationships>"
    );
    let core = format!(
        "{head}<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><dc:title>{}</dc:title></cp:coreProperties>",
        esc(title)
    );
    z.add("[Content_Types].xml", types.as_bytes(), true);
    z.add("_rels/.rels", rels.as_bytes(), true);
    z.add("docProps/core.xml", core.as_bytes(), true);
    z.add("xl/workbook.xml", workbook.as_bytes(), true);
    z.add("xl/_rels/workbook.xml.rels", wb_rels.as_bytes(), true);
    z.add("xl/styles.xml", out.as_bytes(), true);
    z.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Block;
    use crate::tests::{page, part, table_page};

    /// A page of one-line paragraphs, top to bottom: each becomes a row of column A.
    fn lines_page(lines: &[&str]) -> Page {
        Page {
            width: 612.0,
            height: 792.0,
            blocks: lines
                .iter()
                .enumerate()
                .map(|(i, t)| Block {
                    text: (*t).into(),
                    rect: [72.0, 700.0 - 30.0 * i as f64, 300.0, 711.0 - 30.0 * i as f64],
                    size: 11.0,
                    bold: false,
                    italic: false,
                    color: [0.0; 3],
                    rotation: 0.0,
                })
                .collect(),
            images: Vec::new(),
        }
    }

    #[test]
    fn xlsx_is_a_workbook_with_real_tables_and_numbers() {
        let x = xlsx(&[table_page(), page()], "Report", XlsxSheets::PerPage);
        assert!(x.starts_with(b"PK"));
        let wb = part(&x, "xl/workbook.xml");
        assert!(wb.contains("name=\"Page 1\"") && wb.contains("name=\"Page 2\""), "{wb}");
        let s1 = part(&x, "xl/worksheets/sheet1.xml");
        // The table's header row, a reading that sums, Totals merged over two columns, then the notes.
        assert!(s1.contains("<c r=\"A1\" s=\"1\" t=\"inlineStr\"><is><t xml:space=\"preserve\">Meter</t></is></c>"), "{s1}");
        // The reading is a number, with a style whose format shows it as the page did.
        assert!(s1.contains("<c r=\"C1\"") && s1.contains("<c r=\"C2\" s=\"2\"><v>37414</v></c>"), "{s1}");
        assert!(s1.contains("<mergeCell ref=\"A4:B4\"/>"), "{s1}");
        assert!(s1.contains("<row r=\"5\"><c r=\"A5\"") && s1.contains("Notes follow the table"), "{s1}");
        assert!(s1.contains("<col min=\"1\" max=\"1\""), "{s1}");
        // One font per size/bold/italic/colour, a cell style per font and number format, the
        // custom formats numbered from 164.
        let styles = part(&x, "xl/styles.xml");
        assert!(styles.contains("<font><b/><sz val=\"24\"/>"), "{styles}");
        assert!(styles.contains("<numFmt numFmtId=\"164\"") && styles.contains("<xf numFmtId=\"164\" fontId=\"1\""), "{styles}");
        part(&x, "xl/worksheets/sheet2.xml");
        assert!(part(&x, "[Content_Types].xml").contains("sheet2.xml"));
        // Cell values: cells.rs's tests; here, that a code stays text.
        for t in ["PO-4471", "12,34", "1.2.3", "", "5.", ",5"] {
            assert_eq!(cells::classify(t), cells::Value::Text, "{t}");
        }
        assert_eq!((col_name(0), col_name(25), col_name(26), col_name(701)), ("A".into(), "Z".into(), "AA".into(), "ZZ".into()));
    }

    #[test]
    fn xlsx_one_sheet_for_the_whole_document_or_one_per_page() {
        let pages = [table_page(), table_page()];
        let whole = xlsx(&pages, "Meter readings", XlsxSheets::WholeDocument);
        let wb = part(&whole, "xl/workbook.xml");
        assert!(wb.contains("<sheet name=\"Meter readings\" sheetId=\"1\"") && !wb.contains("sheetId=\"2\""), "{wb}");
        let s1 = part(&whole, "xl/worksheets/sheet1.xml");
        // The second page's table follows the first's on the same sheet: rows 6 to 10.
        assert!(s1.contains("<row r=\"6\"><c r=\"A6\"") && s1.contains("<row r=\"10\">"), "{s1}");
        let per_page = xlsx(&pages, "Meter readings", XlsxSheets::PerPage);
        let wb = part(&per_page, "xl/workbook.xml");
        assert!(wb.contains("name=\"Page 1\"") && wb.contains("name=\"Page 2\""), "{wb}");
        assert_eq!(XlsxSheets::default(), XlsxSheets::WholeDocument);
    }

    #[test]
    fn sheet_names_follow_excels_rules() {
        assert_eq!(sheet_name("Q3: costs [draft]/v2?", &[]), "Q3 costs draftv2");
        assert_eq!(sheet_name("'Quoted'", &[]), "Quoted");
        assert_eq!(sheet_name("", &[]), "Sheet");
        assert_eq!(sheet_name("Tab\u{1}name", &[]), "Tabname");
        assert_eq!(sheet_name("history", &[]), "History sheet", "Excel reserves History");
        let long = sheet_name("An annual report title that is far too long for a tab", &[]);
        assert!(long.chars().count() <= 31 && !long.ends_with(' '), "cut to 31, no trailing space: {long:?}");
        assert_eq!(sheet_name("Page 1", &["page 1".into()]), "Page 1 (2)", "unique, whatever the case");
        let taken = vec![long.clone()];
        let second = sheet_name("An annual report title that is far too long for a tab", &taken);
        assert!(second.ends_with(" (2)") && second.chars().count() <= 31, "{second}");
    }

    #[test]
    fn xlsx_stays_within_excels_limits() {
        // Three rows and two columns a sheet (Excel's own are 1,048,576 and 16,384): the table's
        // third column is left out, and what doesn't fit goes on on a second sheet.
        let x = xlsx_bounded(&[table_page()], "Report", XlsxSheets::WholeDocument, 3, 2);
        let wb = part(&x, "xl/workbook.xml");
        assert!(wb.contains("name=\"Report\"") && wb.contains("name=\"Report (2)\""), "{wb}");
        let s1 = part(&x, "xl/worksheets/sheet1.xml");
        assert!(!s1.contains("<c r=\"C"), "no third column: {s1}");
        // A table taller than a whole sheet keeps its first rows, and nothing panics.
        let _ = xlsx_bounded(&[table_page(), table_page()], "Report", XlsxSheets::PerPage, 1, 1);
    }

    #[test]
    fn xlsx_drops_characters_xml_cant_hold() {
        let x = xlsx(&[lines_page(&["Total\u{1}\u{FFFF} due"])], "Bad\u{2}title", XlsxSheets::WholeDocument);
        let s1 = part(&x, "xl/worksheets/sheet1.xml");
        assert!(s1.contains(">Total due<"), "{s1}");
        assert!(part(&x, "xl/workbook.xml").contains("name=\"Badtitle\""));
        assert!(part(&x, "docProps/core.xml").contains("<dc:title>Badtitle</dc:title>"));
    }

    /// The values read back by another program's reader (calamine, a test-only dependency): what
    /// Excel would see, independently of this file's own writer.
    #[test]
    fn xlsx_values_as_another_reader_sees_them() {
        use calamine::{Data, Reader, Xlsx, open_workbook_from_rs};
        let lines = ["00123", "4111111111111111", "$1,234.50", "(400)", "12.5%", "2026-10-14", "03/04/2026", "14 Oct 2026", "1,000.00-", "PO-4471"];
        let x = xlsx(&[lines_page(&lines)], "Values", XlsxSheets::WholeDocument);
        let mut wb: Xlsx<_> = open_workbook_from_rs(std::io::Cursor::new(x)).expect("calamine opens it");
        let range = wb.worksheet_range("Values").expect("the sheet");
        let at = |row: u32| range.get_value((row, 0)).cloned().unwrap_or(Data::Empty);
        assert_eq!(at(0), Data::String("00123".into()), "leading zeros stay text");
        assert_eq!(at(1), Data::String("4111111111111111".into()), "16 digits stay text");
        assert_eq!(at(2), Data::Float(1234.5));
        assert_eq!(at(3), Data::Float(-400.0));
        assert_eq!(at(4), Data::Float(0.125));
        match at(5) {
            Data::DateTime(d) => assert_eq!(d.as_f64(), 46309.0),
            other => panic!("2026-10-14 should be a date: {other:?}"),
        }
        assert_eq!(at(6), Data::String("03/04/2026".into()), "ambiguous: text");
        assert!(matches!(at(7), Data::DateTime(_)), "{:?}", at(7));
        assert_eq!(at(8), Data::Float(-1000.0));
        assert_eq!(at(9), Data::String("PO-4471".into()));
    }
}
