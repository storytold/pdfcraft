use gridcraft_model::{Sparkline, SparklineKind};
use serde_json::json;

use super::*;

fn s() -> Session {
    let mut s = Session::new();
    s.new_workbook();
    s
}
fn pdf_of(s: &mut Session, p: Json) -> (Vec<u8>, u64) {
    let r = s.execute("file.exportPdf", p).unwrap();
    let bytes = crate::io::base64_decode(r["base64"].as_str().unwrap()).unwrap();
    (bytes, r["pages"].as_u64().unwrap())
}
fn contains(hay: &[u8], needle: &str) -> bool {
    hay.windows(needle.len()).any(|w| w == needle.as_bytes())
}
fn arabic_face_available() -> bool {
    let mut doc = gridcraft_pdf::PdfDoc::new();
    doc.add_page(100.0, 100.0).text(0.0, 50.0, 10.0, Font::Helvetica, [0, 0, 0], "مرحبا");
    !doc.missing_arabic_font()
}

#[test]
fn right_to_left_sheets_mirror_link_rectangles_and_targets() {
    fn numbers_after(pdf: &[u8], key: &str, n: usize) -> Vec<f32> {
        let text = String::from_utf8_lossy(pdf);
        let at = text.find(key).unwrap() + key.len();
        text[at..].split([' ', ']']).filter_map(|t| t.parse().ok()).take(n).collect()
    }
    let mut s = s();
    s.execute("insert.link", json!({"cell": "A1", "target": "https://example.org/report"})).unwrap();
    s.execute("insert.link", json!({"cell": "A2", "target": "Sheet1!B60"})).unwrap();
    s.execute("cell.set", json!({"cell": "B60", "input": "Destination"})).unwrap();
    let (ltr, _) = pdf_of(&mut s, json!({}));
    s.execute("view.rightToLeft", json!({"on": true})).unwrap();
    let (rtl, _) = pdf_of(&mut s, json!({}));
    let axis = numbers_after(&ltr, "/MediaBox [", 4)[2];
    let (l, r) = (numbers_after(&ltr, "/Rect [", 4), numbers_after(&rtl, "/Rect [", 4));
    assert!((r[0] - (axis - l[2])).abs() < 0.01 && (r[2] - (axis - l[0])).abs() < 0.01, "{l:?} mirrors to {r:?}");
    assert_eq!(r[1], l[1]);
    let b_width = s.doc().unwrap().wb.active().unwrap().col_width(1);
    let (lx, rx) = (numbers_after(&ltr, "/XYZ ", 1)[0], numbers_after(&rtl, "/XYZ ", 1)[0]);
    let k = (l[2] - l[0]) / s.doc().unwrap().wb.active().unwrap().col_width(0);
    assert!((rx - (axis - lx - b_width * k)).abs() < 0.01, "target {lx} mirrors to {rx}");
}

#[test]
fn chart_data_table_prints_formatted_values_and_follows_recalculation() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Quarter", "Sales"], ["Q1", 1234], ["Q2", 5678]]})).unwrap();
    s.execute("home.numberFormat", json!({"range": "B2:B3", "format": "#,##0.00"})).unwrap();
    s.execute("insert.chart", json!({"range": "A1:B3", "at": "D2", "type": "column"})).unwrap();
    let (off, pages) = pdf_of(&mut s, json!({"fitToPage": true}));
    assert_eq!(pages, 1);
    let count = |bytes: &[u8], text: &str| String::from_utf8_lossy(bytes).matches(text).count();
    s.execute("chart.set", json!({"dataTable": true})).unwrap();
    let (on, _) = pdf_of(&mut s, json!({"fitToPage": true}));
    assert_eq!(count(&on, "(1,234.00) Tj"), count(&off, "(1,234.00) Tj") + 1);
    assert_eq!(count(&on, "(5,678.00) Tj"), count(&off, "(5,678.00) Tj") + 1);
    s.execute("cell.set", json!({"cell": "B2", "input": "=1000+111"})).unwrap();
    let (updated, _) = pdf_of(&mut s, json!({"fitToPage": true}));
    assert!(!contains(&updated, "(1,234.00) Tj"));
    assert!(count(&updated, "(1,111.00) Tj") >= 2, "cell and chart data table both print the new value");
}

#[test]
fn pdf_export_keeps_safe_links_on_their_printed_pages() {
    let mut s = s();
    s.execute("insert.link", json!({"cell": "A1", "target": "https://example.org/report"})).unwrap();
    s.execute("insert.link", json!({"cell": "A2", "target": "Sheet1!A60"})).unwrap();
    s.execute("insert.link", json!({"cell": "A3", "target": "file:///private/report"})).unwrap();
    s.execute("cell.set", json!({"cell": "A60", "input": "Destination"})).unwrap();
    let (pdf, pages) = pdf_of(&mut s, json!({}));
    assert_eq!(pages, 2);
    let text = String::from_utf8_lossy(&pdf);
    assert_eq!(text.matches("/Subtype /Link").count(), 2);
    assert!(text.contains("/URI (https://example.org/report)"));
    assert!(text.contains("/Dest [") && text.contains("/XYZ "));
    assert!(!text.contains("/URI (file:"));

    // A link to a cell omitted by the selected print range must not point to a nonexistent
    // destination. The external link in the selected range remains clickable.
    let (cropped, _) = pdf_of(&mut s, json!({"range": "A1:A3"}));
    let cropped = String::from_utf8_lossy(&cropped);
    assert_eq!(cropped.matches("/Subtype /Link").count(), 1);
    assert!(!cropped.contains("/Dest ["));
}

#[test]
fn pdf_export_rotates_a_shape_outline_and_its_text() {
    let make_page = |rotation| {
        let mut s = s();
        let id = s
            .execute(
                "insert.shape",
                json!({"kind": "rectangle", "at": "A1", "width": 120, "height": 60, "text": "TURNED", "fill": "#FF0000", "line": "#0000FF"}),
            )
            .unwrap()["shape"]
            .as_u64()
            .unwrap();
        if rotation != 0 {
            s.execute("object.move", json!({"kind": "shape", "id": id, "at": "A1", "rotation": rotation})).unwrap();
        }
        pdf_pages(&mut s).remove(0)
    };
    let upright = make_page(0);
    let turned = make_page(90);
    let red_path = |page: &str| {
        let start = page.find("1 0 0 rg").expect("red rectangle fill");
        let tail = &page[start..];
        let end = tail.find(" B").expect("filled and stroked rectangle path") + 2;
        tail[..end].to_string()
    };
    assert_ne!(red_path(&upright), red_path(&turned), "the rectangle's printed outline is rotated");
    assert!(turned.contains("0 -1 1 0") && turned.contains("(TURNED) Tj"), "shape text uses the rotation matrix: {turned}");
}
#[test]
fn arabic_with_characters_its_font_lacks_still_exports() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "مرحبا 😀"})).unwrap();
    s.execute("cell.set", json!({"cell": "A2", "input": "مرحبا Привет"})).unwrap();
    let out = s.execute("file.exportPdf", json!({})).unwrap();
    assert_eq!(out["pages"], 1);
}

#[test]
fn wrapping_keeps_combining_marks_with_their_letters() {
    let word = "a\u{301}a\u{301}a\u{301}a\u{301}";
    let lines = wrap_lines(word, Font::Helvetica, 10.0, 7.0, TextDirection::Auto);
    assert_eq!(lines.iter().map(|line| line.text.as_str()).collect::<String>(), word);
    assert!(lines.iter().all(|line| line.text == "a\u{301}"));
}

#[test]
fn arabic_cells_headers_and_charts_export_searchable_shaped_text() {
    if !arabic_face_available() && std::env::var_os("CRAFT_FONTS_REQUIRED").is_none() {
        return;
    }
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["مَرْحَبًا 123 (ABC)", "القيمة"], ["تفاح", 3], ["كمثرى", 5], ["العَرَبِيَّة العَرَبِيَّة العَرَبِيَّة العَرَبِيَّة العَرَبِيَّة", null], ["تقرير ١٢٣ (ABC)", null], ["مرحبا", null]]})).unwrap();
    s.execute("home.columnWidth", json!({"cols": "A:B", "chars": 32})).unwrap();
    s.execute("home.rowHeight", json!({"rows": "1:6", "height": 50})).unwrap();
    s.execute("home.fontSize", json!({"range": "A1:B6", "size": 20})).unwrap();
    s.execute("home.rowHeight", json!({"rows": "4:4", "height": 90})).unwrap();
    s.execute("home.bold", json!({"range": "A2"})).unwrap();
    s.execute("home.italic", json!({"range": "A3"})).unwrap();
    s.execute("home.wrapText", json!({"range": "A4"})).unwrap();
    s.execute("home.orientation", json!({"range": "A6", "angle": 30})).unwrap();
    s.execute("pageLayout.pageSetup", json!({"header": "&Cتقرير المبيعات 2026", "gridlines": true})).unwrap();
    s.execute("insert.chart", json!({"range": "A1:B3", "type": "column"})).unwrap();
    {
        let d = s.doc_mut().unwrap();
        let wb = std::sync::Arc::make_mut(&mut d.wb);
        for chart in &mut wb.sheet_mut(0).unwrap().charts {
            chart.anchor.cell = CellRef::new(8, 0);
            chart.anchor.width = 500.0;
            chart.anchor.height = 280.0;
        }
    }
    let (pdf, pages) = pdf_of(&mut s, json!({"fitToPage": true}));
    assert_eq!(pages, 1);
    assert!(contains(&pdf, "/Subtype /Type3"));
    for text in ["مَرْحَبًا 123 (ABC)", "تقرير المبيعات 2026", "القيمة", "تفاح", "كمثرى"] {
        let utf16 = text.encode_utf16().map(|unit| format!("{unit:04X}")).collect::<String>();
        assert!(contains(&pdf, &format!("/ActualText <FEFF{utf16}>")), "missing text {text}");
    }
    let lines = wrap_lines("العَرَبِيَّةالعَرَبِيَّة", Font::Helvetica, 20.0, 40.0, TextDirection::Auto);
    assert_eq!(lines.iter().map(|line| line.text.as_str()).collect::<String>(), "العَرَبِيَّةالعَرَبِيَّة");
    assert!(lines.iter().all(|line| !line.text.starts_with(['َ', 'ِ', 'ُ', 'ّ', 'ْ'])));
    assert!(lines.iter().all(|line| gridcraft_pdf::text_width_with_direction(Font::Helvetica, 20.0, &line.text, line.direction) <= 40.0));
    let path = std::env::temp_dir().join(format!("gridcraft-arabic-export-{}.pdf", std::process::id()));
    std::fs::write(&path, &pdf).unwrap();
    eprintln!("Arabic PDF artifact: {}", path.display());
}

#[test]
fn arabic_reading_order_export_artifact() {
    if !arabic_face_available() && std::env::var_os("CRAFT_FONTS_REQUIRED").is_none() {
        return;
    }
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Context", "LeftToRight", "RightToLeft"], ["مرحبا ABC !", "مرحبا ABC !", "مرحبا ABC !"], ["123 مرحبا ABC", "123 مرحبا ABC", "123 مرحبا ABC"], ["(مرحبا) ABC", "(مرحبا) ABC", "(مرحبا) ABC"], ["مرحبا ABC ! DEF ? مرحبا", "مرحبا ABC ! DEF ? مرحبا", "مرحبا ABC ! DEF ? مرحبا"]]})).unwrap();
    s.execute("home.columnWidth", json!({"cols": "A:C", "chars": 24})).unwrap();
    s.execute("home.rowHeight", json!({"rows": "1:5", "height": 60})).unwrap();
    s.execute("home.fontSize", json!({"range": "A2:C5", "size": 18})).unwrap();
    for (range, order) in [("A2:A5", "Context"), ("B2:B5", "LeftToRight"), ("C2:C5", "RightToLeft")] {
        s.execute("home.formatCells", json!({"range": range, "style": {"align": {"reading_order": order}}})).unwrap();
    }
    s.execute("home.wrapText", json!({"range": "A5:C5"})).unwrap();
    s.execute("pageLayout.pageSetup", json!({"gridlines": true, "header": "&CArabic cell reading order"})).unwrap();
    let (pdf, _) = pdf_of(&mut s, json!({}));
    for text in ["مرحبا ABC !", "123 مرحبا ABC", "(مرحبا) ABC"] {
        let utf16 = text.encode_utf16().map(|unit| format!("{unit:04X}")).collect::<String>();
        assert!(contains(&pdf, &format!("/ActualText <FEFF{utf16}>")));
    }
    let path = std::env::temp_dir().join(format!("gridcraft-arabic-reading-order-{}.pdf", std::process::id()));
    std::fs::write(&path, &pdf).unwrap();
    eprintln!("Arabic reading-order PDF artifact: {}", path.display());
}

#[test]
fn wrapped_lines_keep_the_paragraph_direction() {
    let lines = wrap_lines("مرحبا ABC ! DEF ?\nLatin مرحبا !", Font::Helvetica, 10.0, 30.0, TextDirection::Auto);
    let split = lines.iter().position(|l| l.text.starts_with("Latin")).unwrap();
    assert!(split > 0);
    assert!(lines[..split].iter().all(|l| l.direction == TextDirection::RightToLeft));
    assert!(lines[split..].iter().all(|l| l.direction == TextDirection::LeftToRight));
    let lines = wrap_lines("123 مرحبا ABC !", Font::Helvetica, 10.0, 30.0, TextDirection::Context.resolve("123 مرحبا ABC !"));
    assert!(lines.iter().all(|l| l.direction == TextDirection::LeftToRight));
}

fn exported_text_x(pdf: &[u8], text: &str) -> f32 {
    let pdf = String::from_utf8_lossy(pdf);
    let marker = format!("({text}) Tj");
    let before = &pdf[..pdf.find(&marker).unwrap()];
    let matrix = before.lines().rev().find(|line| line.ends_with(" Tm")).unwrap();
    matrix.split_whitespace().nth(4).unwrap().parse().unwrap()
}

#[test]
fn cell_reading_order_and_conditional_context_reach_pdf() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "ABC !"})).unwrap();
    s.execute("home.formatCells", json!({"range": "A1", "style": {"align": {"reading_order": "RightToLeft"}}})).unwrap();
    let (rtl, _) = pdf_of(&mut s, json!({}));
    assert!(contains(&rtl, "(! ABC) Tj"));
    assert!(!contains(&rtl, "/Subtype /Type3"));
    s.execute("home.conditionalFormat", json!({"range": "A1", "rule": {"type": "noBlanks", "style": {"align": {"reading_order": "Context"}}}}))
        .unwrap();
    let (context, _) = pdf_of(&mut s, json!({}));
    assert!(contains(&context, "(ABC !) Tj"));
    assert!(!contains(&context, "(! ABC) Tj"));
    assert!(exported_text_x(&rtl, "! ABC") > exported_text_x(&context, "ABC !"));
}

#[test]
fn rtl_sheet_sparkline_reflects_with_cell_geometry() {
    let mut wb = Workbook::new();
    let sh = wb.sheet_mut(0).unwrap();
    sh.set_value(CellRef::new(0, 0), Value::Number(1.0));
    sh.set_value(CellRef::new(1, 0), Value::Number(3.0));
    sh.sparklines.push(Sparkline {
        cell: CellRef::new(0, 1),
        source: "A1:A2".into(),
        kind: SparklineKind::Line,
        color: Color::rgb(255, 0, 0),
        markers: false,
    });
    let options = PrintOptions { range: RangeRef::parse("A1:B2"), ..PrintOptions::default() };
    let ltr = render_pdf(&wb, &layouts(&wb, &options), "", "").unwrap().bytes;
    wb.sheet_mut(0).unwrap().right_to_left = true;
    let rtl = render_pdf(&wb, &layouts(&wb, &options), "", "").unwrap().bytes;
    let points = |pdf: &[u8]| {
        let pdf = String::from_utf8_lossy(pdf);
        let after = pdf.split("1 0 0 RG\n").nth(1).unwrap();
        let line = after.lines().find(|line| line.ends_with(" l  S")).unwrap();
        let values: Vec<_> = line.split_whitespace().collect();
        (values[0].parse::<f32>().unwrap(), values[3].parse::<f32>().unwrap())
    };
    let (left0, left1) = points(&ltr);
    let (right0, right1) = points(&rtl);
    assert!(left0 < left1 && right0 > right1);
    assert!((left0 + right0 - 612.0).abs() < 0.001);
    assert!((left1 + right1 - 612.0).abs() < 0.001);
}

#[test]
fn explicit_left_and_right_keep_their_side_on_a_right_to_left_sheet() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["LEFT"], ["RIGHT"]]})).unwrap();
    s.execute("home.alignLeft", json!({"range": "A1"})).unwrap();
    s.execute("home.alignRight", json!({"range": "A2"})).unwrap();
    let (ltr, _) = pdf_of(&mut s, json!({}));
    s.execute("view.rightToLeft", json!({"on": true})).unwrap();
    let (rtl, _) = pdf_of(&mut s, json!({}));
    let width = |label| gridcraft_pdf::text_width(Font::Helvetica, 11.0, label);
    let (left, right) = (width("LEFT"), width("RIGHT"));
    let ltr_gap = exported_text_x(&ltr, "RIGHT") + right - exported_text_x(&ltr, "LEFT");
    let rtl_gap = exported_text_x(&rtl, "RIGHT") + right - exported_text_x(&rtl, "LEFT");
    assert!((ltr_gap - rtl_gap).abs() < 0.01, "Left stays left of Right in the cell: {ltr_gap}, {rtl_gap}");
    assert!(exported_text_x(&rtl, "LEFT") + left < exported_text_x(&rtl, "RIGHT") + right);
}

#[test]
fn rtl_sheet_pdf_mirrors_cells_merges_headings_and_object_anchors() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["FIRST", "SECOND"], [null, "MERGED"]]})).unwrap();
    s.execute("home.mergeCenter", json!({"range": "B2:C2"})).unwrap();
    s.execute("pageLayout.pageSetup", json!({"printArea": "A1:D5", "headings": true, "gridlines": true, "left": 0.8, "right": 0.4})).unwrap();
    s.execute("insert.shape", json!({"kind": "rectangle", "at": "B4", "width": 128, "height": 30, "text": "OBJECT"})).unwrap();
    let (ltr, _) = pdf_of(&mut s, json!({}));
    s.execute("view.rightToLeft", json!({"on": true})).unwrap();
    let (rtl, _) = pdf_of(&mut s, json!({}));
    for label in ["FIRST", "SECOND", "MERGED", "A", "B", "C", "D", "1", "2", "OBJECT"] {
        let left = exported_text_x(&ltr, label);
        let right = exported_text_x(&rtl, label);
        let font_size = if ["A", "B", "C", "D", "1", "2"].contains(&label) { 9.0 } else { 11.0 };
        let width = gridcraft_pdf::text_width(Font::Helvetica, font_size, label);
        assert!((left + right + width - 612.0).abs() < 0.01, "{label}: {left}, {right}, {width}");
    }
    assert!(exported_text_x(&ltr, "FIRST") < exported_text_x(&ltr, "SECOND"));
    assert!(exported_text_x(&rtl, "FIRST") > exported_text_x(&rtl, "SECOND"));
    for (name, pdf) in [("ltr", ltr), ("rtl", rtl)] {
        let path = std::env::temp_dir().join(format!("gridcraft-print-direction-{name}-{}.pdf", std::process::id()));
        std::fs::write(&path, pdf).unwrap();
        eprintln!("Print direction PDF artifact: {}", path.display());
    }
}

#[test]
fn header_sections_split_join_and_field_names() {
    let spec = "&L&\"Arial,Bold\"Sales &&Co&CPage &P of &N&R&D";
    let parts = split_header_footer(spec);
    assert_eq!(parts, ["&\"Arial,Bold\"Sales &&Co".to_string(), "Page &P of &N".into(), "&D".into()]);
    assert_eq!(join_header_footer(&parts), spec);
    // Text before any section code is the centre section, as when printing.
    assert_eq!(split_header_footer("Report"), [String::new(), "Report".into(), String::new()]);
    assert_eq!(codes_to_fields("Page &P of &N, &A (&F) &&"), "Page &[Page] of &[Pages], &[Tab] (&[File]) &&");
    assert_eq!(fields_to_codes("Page &[Page] of &[Pages], &[Tab] (&[File]) &&"), "Page &P of &N, &A (&F) &&");
    // A dangling ampersand stays literal instead of eating the next section's code.
    assert_eq!(fields_to_codes("R&"), "R&&");
    let joined = join_header_footer(&[fields_to_codes("R&"), "x".into(), String::new()]);
    assert_eq!(split_header_footer(&joined)[0], "R&&");
    let cx = HfContext { page: 2, pages: 5, sheet: "Data".into(), ..Default::default() };
    let out = expand_header_footer(&join_header_footer(&[String::new(), fields_to_codes("&[Page] / &[Pages]"), fields_to_codes("&[Tab]")]), &cx);
    assert_eq!((out[1].text.as_str(), out[2].text.as_str()), ("2 / 5", "Data"));
    // Hostile input never panics.
    for t in ["&", "&\"", "&\"unterminated", "&&&", "&[Pag", "\u{1F600}&[Page]"] {
        let _ = join_header_footer(&split_header_footer(t));
        let _ = fields_to_codes(&codes_to_fields(t));
    }
}

#[test]
fn header_footer_date_follows_the_workbook_date_system() {
    let mut wb = Workbook::new();
    let date_1900 = hf_date_time(&wb).0;
    wb.date_system = gridcraft_core::DateSystem::D1904;
    assert_eq!(hf_date_time(&wb).0, date_1900);
}

#[test]
fn header_footer_section_edits_are_undoable_and_views_are_per_sheet() {
    let mut s = s();
    s.execute("pageLayout.headerFooter", json!({"header": "&LLeft&RRight"})).unwrap();
    let r = s.execute("pageLayout.headerFooter", json!({"part": "header", "section": "center", "text": "Page &[Page] of &[Pages]"})).unwrap();
    assert_eq!(r["header"], "&LLeft&CPage &P of &N&RRight");
    s.execute("pageLayout.headerFooter", json!({"part": "footer", "section": "right", "text": "&[Date]"})).unwrap();
    assert_eq!(s.doc().unwrap().wb.sheet(0).unwrap().print.footer, "&R&D");
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.doc().unwrap().wb.sheet(0).unwrap().print.header, "&LLeft&RRight");
    assert!(s.execute("pageLayout.headerFooter", json!({"part": "middle", "text": "x"})).is_err());
    assert!(s.execute("pageLayout.headerFooter", json!({"section": "top"})).is_err());
    let long = "x".repeat(1000);
    s.execute("pageLayout.headerFooter", json!({"part": "header", "section": "left", "text": long})).unwrap();
    assert!(s.doc().unwrap().wb.sheet(0).unwrap().print.header.chars().count() <= 255);
    // The view is per sheet and doesn't dirty a clean workbook.
    let mut fresh = Session::new();
    fresh.new_workbook();
    fresh.execute("home.insertSheet", json!({})).unwrap();
    fresh.doc_mut().unwrap().saved = fresh.doc().unwrap().wb.clone();
    fresh.execute("view.pageLayout", json!({"sheet": 0})).unwrap();
    let wb = &fresh.doc().unwrap().wb;
    assert_eq!((wb.sheet(0).unwrap().view, wb.sheet(1).unwrap().view), (SheetViewMode::PageLayout, SheetViewMode::Normal));
    assert!(!fresh.doc().unwrap().is_dirty(), "switching views is not an edit");
    s.execute("view.pageLayout", json!({"sheet": 0})).unwrap();
    s.execute("view.normal", json!({"sheet": 0})).unwrap();
    assert_eq!(s.doc().unwrap().wb.sheet(0).unwrap().view, SheetViewMode::Normal);
    assert_eq!(s.execute("view.ruler", json!({"on": false})).unwrap()["on"], false);
    assert!(s.hide_ruler);
    assert_eq!(s.execute("view.ruler", json!({})).unwrap()["on"], true);
}

#[test]
fn page_count_300_rows_letter_portrait() {
    let mut s = s();
    let rows: Vec<Vec<Json>> = (1..=300).map(|i| vec![json!(i)]).collect();
    s.execute("range.setValues", json!({"range": "A1", "values": rows})).unwrap();
    // 684pt of printable height / 15pt rows = 45 rows per page → 7 pages.
    let (pdf, pages) = pdf_of(&mut s, json!({}));
    assert_eq!(pages, 7);
    assert!(pdf.starts_with(b"%PDF-1.7"));
    assert!(contains(&pdf, "/Count 7"));
    let pv = s.execute("file.printPreview", json!({})).unwrap();
    assert_eq!(pv["pages"], 7);
    assert_eq!(pv["pageList"][0]["rows"], "1:45");
    assert_eq!(pv["pageList"][1]["rows"], "46:90");
    assert_eq!(s.execute("file.printPreview", json!({"page": 7})).unwrap()["page"]["rows"], "271:300");
    assert!(s.execute("file.printPreview", json!({"page": 8})).is_err());
    // Landscape: 504pt / 15 = 33 rows per page → 10 pages.
    s.execute("pageLayout.pageSetup", json!({"orientation": "landscape"})).unwrap();
    assert_eq!(pdf_of(&mut s, json!({})).1, 10);
    // Fit to one page.
    assert_eq!(pdf_of(&mut s, json!({"fitToPage": true})).1, 1);
    s.execute("pageLayout.pageSetup", json!({"fitWidth": 1, "fitHeight": 2, "orientation": "portrait"})).unwrap();
    assert_eq!(pdf_of(&mut s, json!({})).1, 2);
    // Manual break and repeated title rows.
    s.execute("pageLayout.pageSetup", json!({"scale": 100, "rowBreaks": [11], "titleRows": "1:1"})).unwrap();
    let pv = s.execute("file.printPreview", json!({})).unwrap();
    assert_eq!(pv["pageList"][0]["rows"], "1:10");
    // Title row takes one line on later pages: 44 body rows.
    assert_eq!(pv["pageList"][1]["rows"], "11:54");
}

#[test]
fn wide_sheet_goes_down_then_over() {
    let mut s = s();
    let rows: Vec<Vec<Json>> = (0..60).map(|r| (0..15).map(|c| json!(r * 100 + c)).collect()).collect();
    s.execute("range.setValues", json!({"range": "A1", "values": rows})).unwrap();
    let pv = s.execute("file.printPreview", json!({})).unwrap();
    // 511.2pt / 48pt = 10 columns per page; 45 rows per page.
    assert_eq!(pv["pages"], 4);
    assert_eq!(pv["pageList"][0]["range"], "A1:J45");
    assert_eq!(pv["pageList"][1]["range"], "A46:J60");
    assert_eq!(pv["pageList"][2]["range"], "K1:O45");
}

/// The ranges of the printed pages, in print order.
fn page_ranges(s: &mut Session) -> Vec<String> {
    let pv = s.execute("file.printPreview", json!({})).unwrap();
    let list = pv["pageList"].as_array().unwrap();
    assert_eq!(pv["pages"], list.len());
    for (i, page) in list.iter().enumerate() {
        assert_eq!(page["page"], i + 1);
        assert_eq!(page["pageOrder"], s.doc().unwrap().wb.active().unwrap().print.page_order.id());
    }
    list.iter().map(|p| p["range"].as_str().unwrap().to_string()).collect()
}

/// The content streams of an exported PDF's pages, in page order (pages without pictures are the
/// file's only streams).
fn pdf_pages(s: &mut Session) -> Vec<String> {
    let (pdf, pages) = pdf_of(s, json!({}));
    let text = String::from_utf8_lossy(&pdf);
    let streams: Vec<String> = text.split("endstream").filter(|part| part.contains("stream\n")).map(str::to_string).collect();
    assert_eq!(streams.len() as u64, pages);
    streams
}

/// The page order measured in Excel: print area A1:D4 with a row break before row 3 and a column
/// break before column C, so four pages two tall and two wide, each filled and labelled in its
/// own colour, with `Page &P of &N` in the footer.
fn two_by_two_pages() -> Session {
    let mut s = s();
    for (range, color, label) in
        [("A1:B2", "#FF0000", "red"), ("A3:B4", "#00FF00", "green"), ("C1:D2", "#0000FF", "blue"), ("C3:D4", "#FFFF00", "yellow")]
    {
        s.execute("home.fillColor", json!({"range": range, "color": color})).unwrap();
        s.execute("cell.set", json!({"cell": &range[..2], "input": label})).unwrap();
    }
    s.execute("pageLayout.pageSetup", json!({"printArea": "A1:D4", "rowBreaks": [3], "colBreaks": ["C"], "footer": "Page &P of &N"})).unwrap();
    s
}

/// The colour of each printed page, read from its fill and label; each page's footer must
/// number it in print order.
fn printed_colours(s: &mut Session) -> Vec<&'static str> {
    let pages = pdf_pages(s);
    let n = pages.len();
    pages
        .iter()
        .enumerate()
        .map(|(i, page)| {
            assert!(page.contains(&format!("(Page {} of {n}) Tj", i + 1)), "page {}: {page}", i + 1);
            let found: Vec<&str> = [("red", "1 0 0 rg"), ("green", "0 1 0 rg"), ("blue", "0 0 1 rg"), ("yellow", "1 1 0 rg")]
                .into_iter()
                .filter(|(label, fill)| page.contains(&format!("({label}) Tj")) && page.contains(&format!("\n{fill}\n")))
                .map(|(label, _)| label)
                .collect();
            assert_eq!(found.len(), 1, "page {}: {found:?}", i + 1);
            found[0]
        })
        .collect()
}

#[test]
fn page_order_sets_the_sequence_of_the_pages_as_measured_in_excel() {
    let mut s = two_by_two_pages();
    // Down, then over is the default: down the first column of pages, then the next.
    assert_eq!(s.doc().unwrap().wb.active().unwrap().print.page_order, PageOrder::DownThenOver);
    assert_eq!(s.execute("pageLayout.pageSetup", json!({})).unwrap()["page_order"], "downThenOver");
    let down = ["A1:B2", "A3:B4", "C1:D2", "C3:D4"];
    assert_eq!(page_ranges(&mut s), down);
    assert_eq!(printed_colours(&mut s), ["red", "green", "blue", "yellow"]);
    // Over, then down: across the first row of pages, then the next.
    let r = s.execute("pageLayout.pageSetup", json!({"pageOrder": "overThenDown"})).unwrap();
    assert_eq!(r["page_order"], "overThenDown");
    let over = ["A1:B2", "C1:D2", "A3:B4", "C3:D4"];
    assert_eq!(page_ranges(&mut s), over);
    assert_eq!(printed_colours(&mut s), ["red", "blue", "green", "yellow"]);
    assert_eq!(s.execute("file.printPreview", json!({"page": 2})).unwrap()["page"]["range"], "C1:D2");
    // One undo step.
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(page_ranges(&mut s), down);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(page_ranges(&mut s), over);
    // Anything but the two orders is refused and changes nothing, even next to a good value.
    for bad in [json!("sideways"), json!("OverThenDown"), json!(""), json!(2), json!(null), json!(true), json!(["downThenOver"])] {
        let e = s.execute("pageLayout.pageSetup", json!({"pageOrder": bad, "orientation": "landscape"})).unwrap_err();
        assert!(e.to_string().contains("pageOrder"), "{bad}: {e}");
    }
    let pr = &s.doc().unwrap().wb.active().unwrap().print;
    assert_eq!((pr.page_order, pr.orientation), (PageOrder::OverThenDown, Orientation::Portrait));
    assert!(s.take_ui_requests().is_empty(), "a programmatic call never opens a dialog");
    // First-page and odd/even headers go by the printed sequence too.
    s.execute(
        "pageLayout.headerFooter",
        json!({"header": "&COdd &P", "evenHeader": "&CEven &P", "firstHeader": "&CCover", "differentFirst": true, "differentOddEven": true}),
    )
    .unwrap();
    let pages = pdf_pages(&mut s);
    for (page, (header, label)) in pages.iter().zip([("Cover", "red"), ("Even 2", "blue"), ("Odd 3", "green"), ("Even 4", "yellow")]) {
        assert!(page.contains(&format!("({header}) Tj")) && page.contains(&format!("({label}) Tj")), "{header}: {page}");
    }
    s.execute("pageLayout.pageSetup", json!({"pageOrder": "downThenOver"})).unwrap();
    assert_eq!(page_ranges(&mut s), down);
    let pages = pdf_pages(&mut s);
    for (page, (header, label)) in pages.iter().zip([("Cover", "red"), ("Even 2", "green"), ("Odd 3", "blue"), ("Even 4", "yellow")]) {
        assert!(page.contains(&format!("({header}) Tj")) && page.contains(&format!("({label}) Tj")), "{header}: {page}");
    }
}

#[test]
fn page_order_without_breaks_and_with_print_titles() {
    let mut s = s();
    // Every cell shows its own address: 60 rows and 15 columns are two pages tall and two wide.
    let rows: Vec<Vec<Json>> = (1..=60).map(|r| (0..15).map(|c| json!(format!("{}{r}", gridcraft_core::col_to_letters(c)))).collect()).collect();
    s.execute("range.setValues", json!({"range": "A1", "values": rows})).unwrap();
    for titles in [false, true] {
        if titles {
            s.execute("pageLayout.pageSetup", json!({"titleRows": "1:1", "titleCols": "A:A"})).unwrap();
        }
        s.execute("pageLayout.pageSetup", json!({"pageOrder": "downThenOver"})).unwrap();
        let down = page_ranges(&mut s);
        if !titles {
            assert_eq!(down, ["A1:J45", "A46:J60", "K1:O45", "K46:O60"]);
        }
        s.execute("pageLayout.pageSetup", json!({"pageOrder": "overThenDown"})).unwrap();
        let over = page_ranges(&mut s);
        assert_eq!(over, [down[0].clone(), down[2].clone(), down[1].clone(), down[3].clone()], "titles: {titles}");
        // Each printed page starts with its own top-left cell; the title row and column (A1 is in
        // both) repeat on every page in either order.
        for (order, ranges) in [("overThenDown", &over), ("downThenOver", &down)] {
            s.execute("pageLayout.pageSetup", json!({"pageOrder": order})).unwrap();
            for (i, (page, range)) in pdf_pages(&mut s).iter().zip(ranges.iter()).enumerate() {
                let top_left = range.split(':').next().unwrap();
                assert!(page.contains(&format!("({top_left}) Tj")), "{order} page {}: no {top_left}", i + 1);
                assert_eq!(page.contains("(A1) Tj"), titles || i == 0, "{order} page {}, titles: {titles}", i + 1);
            }
        }
    }
}

#[test]
fn first_and_even_pages_print_their_own_headers() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["a"], ["b"], ["c"], ["d"], ["e"], ["f"]]})).unwrap();
    s.execute("pageLayout.pageSetup", json!({"rowBreaks": [2, 4]})).unwrap();
    let r = s
        .execute(
            "pageLayout.headerFooter",
            json!({"header": "&COdd &P", "evenHeader": "&CEven &P", "firstHeader": "&CCover", "differentFirst": true, "differentOddEven": true}),
        )
        .unwrap();
    assert_eq!(r["firstHeader"], "&CCover");
    let (pdf, pages) = pdf_of(&mut s, json!({}));
    assert_eq!(pages, 3);
    assert!(contains(&pdf, "(Cover) Tj") && contains(&pdf, "(Even 2) Tj") && contains(&pdf, "(Odd 3) Tj"));
    assert!(!contains(&pdf, "(Odd 1) Tj"), "the first page has its own header");
    // One section of the even-page footer, edited in place.
    s.execute("pageLayout.headerFooter", json!({"page": "even", "part": "footer", "section": "right", "text": "&[Tab]"})).unwrap();
    assert_eq!(s.doc().unwrap().wb.sheet(0).unwrap().print.even_footer, "&R&A");
    assert!(s.execute("pageLayout.headerFooter", json!({"page": "last", "part": "header", "text": "x"})).is_err());
    // Turned off, every page prints the odd header again.
    s.execute("pageLayout.headerFooter", json!({"differentFirst": false, "differentOddEven": false})).unwrap();
    let (pdf, _) = pdf_of(&mut s, json!({}));
    assert!(contains(&pdf, "(Odd 1) Tj") && contains(&pdf, "(Odd 2) Tj") && !contains(&pdf, "(Cover) Tj"));
}

/// The y position of the first `(text) Tj` in a PDF (PDF space: larger is higher on the page).
fn text_y(pdf: &[u8], text: &str) -> Option<f32> {
    let s = String::from_utf8_lossy(pdf);
    let before = &s[..s.find(&format!("({text}) Tj"))?];
    let tm = before.rfind(" Tm")?;
    before[..tm].rsplit([' ', '\n']).next()?.parse().ok()
}

/// The x position and font size of the first `(text) Tj` in a PDF.
fn text_at(pdf: &[u8], text: &str) -> Option<(f32, f32)> {
    let s = String::from_utf8_lossy(pdf);
    let before = &s[..s.find(&format!("({text}) Tj"))?];
    let tf = before.rfind(" Tf")?;
    let size = before[..tf].rsplit(' ').next()?.parse().ok()?;
    let tm = before.rfind(" Tm")?;
    let mut nums = before[..tm].rsplit([' ', '\n']);
    let _y = nums.next()?;
    let x = nums.next()?.parse().ok()?;
    Some((x, size))
}

#[test]
fn headers_scale_with_the_sheet_and_align_with_its_margins_unless_turned_off() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "x"})).unwrap();
    s.execute("pageLayout.pageSetup", json!({"scale": 50, "left": 1.5, "right": 1.5})).unwrap();
    s.execute("pageLayout.headerFooter", json!({"header": "&LLeft&C&14Middle"})).unwrap();
    // Both on (the default): the text prints at half size, from the 1.5-inch left margin.
    let (pdf, _) = pdf_of(&mut s, json!({}));
    assert_eq!(text_at(&pdf, "Left"), Some((108.0, 5.0)));
    assert_eq!(text_at(&pdf, "Middle").map(|t| t.1), Some(7.0));
    let r = s.execute("pageLayout.headerFooter", json!({"scaleWithDoc": false, "alignWithMargins": false})).unwrap();
    assert_eq!((&r["scaleWithDoc"], &r["alignWithMargins"]), (&json!(false), &json!(false)));
    // Both off: full size, 0.7 inch from the paper's left edge.
    let (pdf, _) = pdf_of(&mut s, json!({}));
    assert_eq!(text_at(&pdf, "Left"), Some((50.4, 10.0)));
    assert_eq!(text_at(&pdf, "Middle").map(|t| t.1), Some(14.0));
    s.execute("edit.undo", json!({})).unwrap();
    let pr = &s.doc().unwrap().wb.sheet(0).unwrap().print;
    assert!(pr.hf_scale_with_doc && pr.hf_align_with_margins);
}

#[test]
fn pdf_prints_cjk_text_instead_of_question_marks() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["签字清单", 2]]})).unwrap();
    let (pdf, _) = pdf_of(&mut s, json!({}));
    assert!(!contains(&pdf, "(????) Tj"), "CJK cell printed as (????) Tj");
    assert!(contains(&pdf, "<7B7E5B576E055355> Tj"));
    assert!(contains(&pdf, "(2) Tj"));
}

#[test]
fn pdf_contains_text_and_header_codes_expand() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Quarterly Report", null], ["Total", 1234.5]]})).unwrap();
    s.execute("home.bold", json!({"range": "A1"})).unwrap();
    s.execute("pageLayout.pageSetup", json!({"header": "&LLeft &A&CMiddle&RPage &P of &N", "footer": "&C&F", "gridlines": true, "headings": true}))
        .unwrap();
    let (pdf, pages) = pdf_of(&mut s, json!({}));
    assert_eq!(pages, 1);
    assert!(contains(&pdf, "(Quarterly Report) Tj"));
    assert!(contains(&pdf, "(1234.5) Tj"));
    assert!(contains(&pdf, "(Left Sheet1) Tj"));
    assert!(contains(&pdf, "(Page 1 of 1) Tj"));
    assert!(contains(&pdf, "(Book1) Tj"));
    assert!(contains(&pdf, "/Helvetica-Bold"));
    let cx = HfContext {
        page: 3,
        pages: 9,
        sheet: "Data".into(),
        file: "b.xlsx".into(),
        date: "1/2/2026".into(),
        time: "9:00 AM".into(),
        ..Default::default()
    };
    let [l, c, r] = expand_header_footer("&L&\"Arial,Bold\"&A &D&C&14&BTitle && more&RP&P+1/&N &T", &cx);
    assert_eq!(l.text, "Data 1/2/2026");
    assert!(l.bold);
    assert_eq!(c.text, "Title & more");
    assert!(c.bold);
    assert_eq!(c.size, Some(14.0));
    assert_eq!(r.text, "P4/9 9:00 AM");
    assert_eq!(expand_header_footer("plain", &cx)[1].text, "plain");
    assert_eq!(expand_header_footer("&", &cx)[1].text, "");
    // A megabyte sheet name repeated by 5,000 `&A` codes stops at the section's 64 KiB.
    let long = HfContext { sheet: "S".repeat(1024 * 1024), ..cx };
    let [_, c, _] = expand_header_footer(&"&A".repeat(5_000), &long);
    assert!(c.text.is_empty(), "the name alone is over the limit");
    let short = HfContext { sheet: "S".repeat(1000), ..Default::default() };
    assert_eq!(expand_header_footer(&"&A".repeat(5_000), &short)[1].text.len(), 65_000);
}

#[test]
fn conditional_number_format_prints() {
    // #1163: a rule's number format replaces the cell's own, whether that is General or not;
    // cells the rule doesn't match keep theirs.
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [[1], [3], [4]]})).unwrap();
    s.execute("home.numberFormat", json!({"range": "A3", "code": "0.0"})).unwrap();
    let rule = json!({"type": "cellIs", "operator": "greater", "value": "2", "style": {"num_fmt": "0.00"}});
    s.execute("home.conditionalFormat", json!({"range": "A1:A3", "rule": rule})).unwrap();
    let (pdf, _) = pdf_of(&mut s, json!({}));
    assert!(contains(&pdf, "(1) Tj"), "an unmatched cell keeps General");
    assert!(contains(&pdf, "(3.00) Tj") && !contains(&pdf, "(3) Tj"), "General cell under the rule");
    assert!(contains(&pdf, "(4.00) Tj") && !contains(&pdf, "(4.0) Tj"), "formatted cell under the rule");
}

#[test]
fn pdf_uses_indexed_number_format_palette_color() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "42"})).unwrap();
    s.execute("home.numberFormat", json!({"range": "A1", "format": "[Color4]0"})).unwrap();
    let (pdf, pages) = pdf_of(&mut s, json!({}));
    assert_eq!(pages, 1);
    assert!(contains(&pdf, "0 1 0 rg"), "expected indexed Color4 green in PDF text stream");
}

#[test]
fn export_writes_file() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "B2", "input": "hello"})).unwrap();
    let dir = std::env::temp_dir().join(format!("gridcraft-print-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("out.pdf");
    let r = s.execute("file.exportPdf", json!({"path": path.to_str().unwrap()})).unwrap();
    assert_eq!(r["pages"], 1);
    let bytes = std::fs::read(&path).unwrap();
    assert!(bytes.starts_with(b"%PDF-"));
    assert!(contains(&bytes, "(hello) Tj"));
    let _ = std::fs::remove_dir_all(&dir);
    let r = s.execute("file.print", json!({})).unwrap();
    assert!(r["base64"].as_str().unwrap().len() > 100);
}

#[test]
fn save_as_pdf_writes_a_pdf_and_keeps_the_workbook_file() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "B2", "input": "hello"})).unwrap();
    let dir = std::env::temp_dir().join(format!("gridcraft-save-pdf-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Out.PDF");
    let r = s.execute("file.saveAs", json!({"path": path.to_str().unwrap()})).unwrap();
    assert_eq!(r["pages"], 1);
    let bytes = std::fs::read(&path).unwrap();
    assert!(bytes.starts_with(b"%PDF-"));
    assert!(contains(&bytes, "(hello) Tj"));
    // The PDF is a print, not the workbook's file: Save still asks where to put the workbook.
    let d = s.doc().unwrap();
    assert!(d.path.is_none() && d.is_dirty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn rich_sheet_renders() {
    let mut s = Session::new();
    s.execute("file.new", json!({"sample": "sales"})).unwrap();
    s.execute("range.setValues", json!({"range": "H1", "values": [["A long piece of text that overflows into neighbours"]]})).unwrap();
    s.execute("cell.set", json!({"cell": "H3", "input": "Wrapped text across several lines in a narrow cell"})).unwrap();
    s.execute("home.wrapText", json!({"range": "H3"})).unwrap_or_default();
    s.execute("selection.set", json!({"range": "J5:K6"})).unwrap();
    s.execute("home.mergeCenter", json!({})).unwrap();
    s.execute("cell.set", json!({"cell": "J5", "input": "Merged"})).unwrap();
    s.execute("home.borders", json!({"range": "J5:K6", "preset": "outside", "style": "thick"})).unwrap_or_default();
    let (pdf, pages) = pdf_of(&mut s, json!({"sheets": "all"}));
    assert!(pages >= 1);
    assert!(contains(&pdf, "(Merged) Tj"));
    assert!(contains(&pdf, "(A long piece of text that overflows into neighbours) Tj"));
}

#[test]
fn charts_and_images_are_drawn() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Fruit", "Qty"], ["Apples", 3], ["Pears", 5]]})).unwrap();
    s.execute("insert.chart", json!({"range": "A1:B3", "type": "pie"})).unwrap_or_default();
    s.execute("insert.chart", json!({"range": "A1:B3"})).unwrap_or_default();
    // A 2×2 PNG with transparency.
    let mut png = Vec::new();
    let img = image::RgbaImage::from_raw(2, 2, vec![255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 255, 0, 0, 0, 0]).unwrap();
    image::DynamicImage::ImageRgba8(img).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    let d = s.doc_mut().unwrap();
    let wb = std::sync::Arc::make_mut(&mut d.wb);
    let id = wb.next_object_id();
    wb.sheet_mut(0).unwrap().images.push(Image {
        id,
        anchor: Anchor { cell: CellRef::new(5, 5), dx: 0.0, dy: 0.0, width: 40.0, height: 40.0, mode: gridcraft_model::AnchorMode::MoveAndSize },
        data: png.into(),
        mime: "image/png".into(),
        alt: String::new(),
    });
    let charts = s.doc().unwrap().wb.active().unwrap().charts.len();
    let (pdf, _) = pdf_of(&mut s, json!({}));
    assert!(contains(&pdf, "/Subtype /Image"));
    assert!(contains(&pdf, "/SMask"));
    if charts > 0 {
        assert!(contains(&pdf, "(Apples) Tj") || contains(&pdf, "(Qty) Tj"));
    }
}

#[test]
fn page_setup_edits_settings() {
    let mut s = s();
    let r = s
        .execute(
            "pageLayout.pageSetup",
            json!({"paper": "A4", "orientation": "landscape", "margins": {"left": 1, "top": 0.5}, "right": 0.25, "centerH": true, "printArea": "A1:D20", "titleRows": "1:2", "titleCols": "A:A", "colBreaks": ["C"]}),
        )
        .unwrap();
    assert_eq!(r["paper"], "A4");
    assert_eq!(r["orientation"], "Landscape");
    assert_eq!(r["margins"][0], 1.0);
    assert_eq!(r["margins"][1], 0.25);
    assert_eq!(r["margins"][2], 0.5);
    assert_eq!(r["center_h"], true);
    assert_eq!(r["title_rows"], json!([0, 1]));
    assert_eq!(r["col_breaks"], json!([2]));
    let sh = s.doc().unwrap().wb.active().unwrap().print.clone();
    assert_eq!(sh.print_area, RangeRef::parse("A1:D20"));
    s.execute("pageLayout.pageSetup", json!({"printArea": ""})).unwrap();
    assert_eq!(s.doc().unwrap().wb.active().unwrap().print.print_area, None);
    assert!(s.execute("pageLayout.pageSetup", json!({"paper": "Napkin"})).is_err());
    assert!(s.execute("pageLayout.pageSetup", json!({"orientation": "sideways"})).is_err());
    // Undoable as one step.
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.doc().unwrap().wb.active().unwrap().print.print_area.is_some());
}

/// The Japanese paper sizes are the JIS ones: `B4`/`B5`/`B6` changed meaning from ISO, and the
/// ISO B series stays reachable by name. The postcards and envelopes are the JIS sizes too.
#[test]
fn japanese_paper_sizes_are_jis() {
    for (name, w, h) in [
        ("A4", 595.28, 841.89), // unchanged
        ("B4", 728.50, 1031.81),
        ("B5", 515.91, 728.50),
        ("B6", 362.83, 515.91),
        ("B4 (JIS)", 728.50, 1031.81),
        ("b5-jis", 515.91, 728.50),
        ("B4 (ISO)", 708.66, 1000.63),
        ("B5 (ISO)", 498.90, 708.66),
        ("B6 (ISO)", 354.33, 498.90),
        ("A6", 297.64, 419.53),
        ("Japanese Postcard", 283.46, 419.53),
        ("japanese double postcard", 419.53, 566.93),
        ("Japanese Envelope #3", 340.16, 666.14),
        ("chou #4", 255.12, 581.10),
        ("Japanese Envelope Kaku #2", 680.31, 941.10),
        ("kaku #3", 612.28, 785.20),
    ] {
        let got = paper_size(name);
        assert!((got.0 - w).abs() < 0.01 && (got.1 - h).abs() < 0.01, "{name}: {got:?}");
    }
    // 長形4号 is 90×205 mm (90×225 mm is 長形40号), so its height is 205 mm = 581.10 pt — not the
    // 225 mm it once had.
    let chou4 = paper_size("Japanese Envelope #4");
    assert_eq!((chou4.0, chou4.1), (255.12, 581.10));
    assert_eq!(chou4, paper_size("chou #4"));
    // Unknown names keep the old Letter fallback.
    assert_eq!(paper_size("Napkin"), (612.0, 792.0));
}

/// Page Setup's paper reaches the PDF page: the MediaBox is the paper's, A4 is unchanged, B5 is
/// JIS B5, and the Japanese postcard prints at 283.46×419.53 pt. Landscape swaps the sides and
/// fit-to-page keeps the paper size.
#[test]
fn page_setup_paper_sizes_reach_the_pdf_page() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["x"]]})).unwrap();
    let close = |v: &Json, want: f64| (v.as_f64().unwrap() - want).abs() < 0.01;
    for (paper, w, h) in [
        ("A4", 595.28f64, 841.89f64),
        ("B5", 515.91, 728.50),
        ("Japanese Postcard", 283.46, 419.53),
        ("Japanese Double Postcard", 419.53, 566.93),
        ("Japanese Envelope Kaku #2", 680.31, 941.10),
    ] {
        s.execute("pageLayout.pageSetup", json!({"paper": paper, "orientation": "portrait"})).unwrap();
        let pv = s.execute("file.printPreview", json!({})).unwrap();
        let page = &pv["pageList"][0];
        assert_eq!(page["paper"], paper);
        assert!(close(&page["width"], w) && close(&page["height"], h), "{paper}: {page}");
        let (pdf, pages) = pdf_of(&mut s, json!({}));
        assert_eq!(pages, 1);
        let want = format!("/MediaBox [0 0 {w} {h}]");
        assert!(contains(&pdf, &want), "{paper}: wanted {want}");
    }
    // Landscape swaps the sides, and fit-to-page does not change the paper.
    s.execute("pageLayout.pageSetup", json!({"paper": "B5", "orientation": "landscape"})).unwrap();
    let (pdf, pages) = pdf_of(&mut s, json!({}));
    assert_eq!(pages, 1);
    assert!(contains(&pdf, "/MediaBox [0 0 728.5 515.91]"));
    s.execute("pageLayout.pageSetup", json!({"paper": "B5", "orientation": "portrait"})).unwrap();
    let (pdf, _) = pdf_of(&mut s, json!({"fitToPage": true}));
    assert!(contains(&pdf, "/MediaBox [0 0 515.91 728.5]"));
    // `pageLayout.size` still takes any name, and an unknown one prints on Letter as before.
    s.execute("pageLayout.size", json!({"paper": "Napkin"})).unwrap();
    let pv = s.execute("file.printPreview", json!({})).unwrap();
    assert!(close(&pv["pageList"][0]["width"], 612.0) && close(&pv["pageList"][0]["height"], 792.0));
    // Page Setup refuses an unknown name.
    assert!(s.execute("pageLayout.pageSetup", json!({"paper": "Napkin"})).is_err());
}

#[test]
fn hostile_settings_never_panic_and_cap_pages() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "XFD1048576", "input": "far"})).unwrap();
    s.execute("cell.set", json!({"cell": "A1", "input": "near"})).unwrap();
    s.execute("pageLayout.pageSetup", json!({"scale": 400, "margins": {"left": 20, "right": 20, "top": 20, "bottom": 20}})).unwrap();
    let pv = s.execute("file.printPreview", json!({})).unwrap();
    assert!(pv["pages"].as_u64().unwrap() <= MAX_PAGES as u64);
    assert_eq!(pv["pages"].as_u64().unwrap(), MAX_PAGES as u64);
    let r = s.execute("file.exportPdf", json!({})).unwrap();
    assert_eq!(r["pages"], MAX_PAGES);
    // Over, then down stops at the cap as well, along the first row of pages.
    s.execute("pageLayout.pageSetup", json!({"pageOrder": "overThenDown"})).unwrap();
    let pv = s.execute("file.printPreview", json!({})).unwrap();
    assert_eq!(pv["pages"].as_u64().unwrap(), MAX_PAGES as u64);
    assert_eq!(pv["pageList"][1]["rows"], pv["pageList"][0]["rows"], "the second page is beside the first");
    s.execute("pageLayout.pageSetup", json!({"fitWidth": 1, "fitHeight": 1, "titleRows": "1:1048576", "titleCols": "A:XFD", "headings": true}))
        .unwrap();
    let pv = s.execute("file.printPreview", json!({})).unwrap();
    assert!(pv["pages"].as_u64().unwrap() <= MAX_PAGES as u64);
    assert!(s.execute("file.exportPdf", json!({"range": "A1:C3"})).is_ok());
    assert!(s.execute("file.exportPdf", json!({"range": "nope"})).is_err());
    assert!(s.execute("file.exportPdf", json!({"sheets": "some"})).is_err());
    // Empty workbook still prints one blank page.
    let mut e = Session::new();
    e.new_workbook();
    let r = e.execute("file.exportPdf", json!({})).unwrap();
    assert_eq!(r["pages"], 1);
    let r = e.execute("file.exportPdf", json!({"sheets": "all"})).unwrap();
    assert_eq!(r["pages"], 1);
    // Weird row sizes and huge fonts.
    s.execute("pageLayout.pageSetup", json!({"fitToPage": false, "scale": 10, "printArea": "A1:B3", "titleRows": "", "titleCols": ""})).unwrap();
    s.execute("home.fontSize", json!({"range": "A1", "size": 409})).unwrap_or_default();
    assert!(s.execute("file.exportPdf", json!({})).is_ok());
}

#[test]
fn export_pdf_without_a_path_returns_bytes_and_never_opens_a_dialog() {
    let mut s = s();
    for params in [json!({}), json!({"path": ""}), json!({"path": null})] {
        let r = s.execute("file.exportPdf", params).unwrap();
        assert!(r.get("path").is_none(), "no file is written without a path");
        let bytes = crate::io::base64_decode(r["base64"].as_str().unwrap()).unwrap();
        assert!(bytes.starts_with(b"%PDF-"));
    }
    assert!(s.take_ui_requests().is_empty(), "a programmatic call never queues a dialog");
    // Headless callers (CLI, MCP) get the same bytes, not an error.
    assert!(s.execute_headless("file.exportPdf", json!({})).is_ok());
    // With no workbook open it is a clear error, not a panic.
    let mut none = Session::new();
    assert!(none.execute("file.exportPdf", json!({})).is_err());
}

#[test]
fn bands_unit() {
    let b = bands(0, 9, |_| 10.0, |_| 35.0, &[], 100);
    assert_eq!(b, vec![(0, 2), (3, 5), (6, 8), (9, 9)]);
    let b = bands(0, 9, |_| 10.0, |_| 35.0, &[5], 100);
    assert_eq!(b, vec![(0, 2), (3, 4), (5, 7), (8, 9)]);
    // A line larger than the page gets its own band.
    let b = bands(0, 2, |r| if r == 1 { 100.0 } else { 10.0 }, |_| 35.0, &[], 100);
    assert_eq!(b, vec![(0, 0), (1, 1), (2, 2)]);
    assert_eq!(bands(0, 1_000_000, |_| 10.0, |_| 5.0, &[], 3).len(), 3);
    let w = wrap_lines("the quick brown fox jumps", Font::Helvetica, 10.0, 50.0, TextDirection::Auto);
    assert!(w.len() >= 2);
    assert!(w.iter().all(|l| gridcraft_pdf::text_width_with_direction(Font::Helvetica, 10.0, &l.text, l.direction) <= 50.0 || !l.text.contains(' ')));
    let w = wrap_lines("Supercalifragilistic", Font::Helvetica, 10.0, 20.0, TextDirection::Auto);
    assert!(w.len() > 2);
    let long = "abcdefghij".repeat(300);
    let w = wrap_lines(&long, Font::Helvetica, 10.0, 50.0, TextDirection::Auto);
    assert_eq!(w.iter().map(|l| l.text.as_str()).collect::<String>(), long);
    assert!(w.iter().all(|l| gridcraft_pdf::text_width_with_direction(Font::Helvetica, 10.0, &l.text, l.direction) <= 50.0));
    let mut at = 0;
    for l in &w[..w.len() - 1] {
        let one_more = &long[at..at + l.text.len() + 1];
        assert!(gridcraft_pdf::text_width_with_direction(Font::Helvetica, 10.0, one_more, l.direction) > 50.0, "each split is as long as fits");
        at += l.text.len();
    }
}

fn wrapped_text(text: &str, font: Font, size: f32, width: f32) -> Vec<String> {
    wrap_lines(text, font, size, width, TextDirection::Auto).into_iter().map(|line| line.text).collect()
}

#[test]
fn wrapping_ignores_the_width_of_undrawn_bidi_marks() {
    let word = "i\u{200e}".repeat(20);
    assert!(gridcraft_pdf::text_width(Font::Helvetica, 10.0, &word) <= 50.0);
    assert_eq!(wrapped_text(&word, Font::Helvetica, 10.0, 50.0), vec![word.clone()]);
}

/// The wrapper `wrap_lines` replaced, re-measuring strings, as the reference for its lines.
fn wrap_lines_remeasuring(text: &str, font: Font, size: f32, width: f32) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split(' ') {
            let cand = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if line.is_empty() || gridcraft_pdf::text_width(font, size, &cand) <= width {
                line = cand;
            } else {
                out.push(std::mem::take(&mut line));
                line = word.to_string();
            }
            while gridcraft_pdf::text_width(font, size, &line) > width && line.chars().count() > 1 {
                let chars: Vec<char> = line.chars().collect();
                let (mut lo, mut hi) = (1usize, chars.len());
                while lo < hi {
                    let mid = lo + (hi - lo).div_ceil(2);
                    let cut: String = chars[..mid].iter().collect();
                    if gridcraft_pdf::text_width(font, size, &cut) <= width {
                        lo = mid;
                    } else {
                        hi = mid - 1;
                    }
                }
                if lo == chars.len() {
                    break;
                }
                out.push(chars[..lo].iter().collect());
                line = chars[lo..].iter().collect();
            }
            if out.len() > 2000 {
                return out;
            }
        }
        out.push(line);
    }
    out
}

#[test]
fn wrap_lines_matches_the_remeasuring_wrapper() {
    let texts = [
        "the quick brown fox jumps",
        "Supercalifragilistic",
        "a  b   c\n\nverylongwordthatneedsbreaking short\nend ",
        "Wide WWWWWWWWWW words, iiiiiiii narrow ones; mixed MMMMiiiiMMMM",
        "€ ü ß — unicode Ünïcödé text",
        "",
        " ",
    ];
    for text in texts {
        for font in [Font::Helvetica, Font::HelveticaBold, Font::Courier] {
            for (size, width) in [(10.0, 50.0), (10.0, 20.0), (11.0, 1.0), (8.0, 300.0), (12.0, 0.0)] {
                assert_eq!(
                    wrapped_text(text, font, size, width),
                    wrap_lines_remeasuring(text, font, size, width),
                    "{text:?} {font:?} {size} {width}"
                );
            }
        }
    }
}

#[test]
fn wrapping_a_long_word_in_a_narrow_box_stops_at_the_line_limit() {
    // One character per line: the old wrapper re-measured and copied the rest of the word for
    // each of the 100,000 lines before checking the limit.
    let word = "w".repeat(100_000);
    let lines = wrapped_text(&word, Font::Helvetica, 10.0, 1.0);
    assert!(lines.len() <= 2002, "{}", lines.len());
    assert!(lines.iter().all(|l| l == "w"));
    // A long paragraph in a very wide box is one line.
    let para = vec!["word"; 50_000].join(" ");
    assert_eq!(wrapped_text(&para, Font::Helvetica, 10.0, 1.0e9), vec![para.clone()]);
}

#[test]
fn wrapping_a_long_arabic_word_measures_pieces_not_the_rest_of_the_word() {
    let word = "مرحبا".repeat(10_000);
    if !arabic_face_available() {
        return;
    }
    let lines = wrapped_text(&word, Font::Helvetica, 10.0, 20.0);
    assert!(lines.len() > 2000 && lines.len() <= 2002, "{}", lines.len());
    assert!(lines.iter().take(2000).all(|line| gridcraft_pdf::text_width(Font::Helvetica, 10.0, line) <= 20.0));
    assert!(word.starts_with(&lines.concat()));
}

#[test]
fn center_across_spans_following_empty_center_across_cells() {
    let mut styles = StyleTable::default();
    let across = styles.intern(Style { align: Alignment { h: HAlign::CenterAcross, ..Alignment::default() }, ..Style::default() });
    let mut sh = Sheet::new("S");
    for col in 0..3 {
        sh.set_style(CellRef::new(0, col), across);
    }
    sh.set_value(CellRef::new(0, 0), Value::Text("Title".into()));
    let a1 = CellRef::new(0, 0);
    assert_eq!(center_across_width(&styles, &sh, a1, &[]), sh.col_width(1) + sh.col_width(2));
    sh.set_value(CellRef::new(0, 2), Value::Text("x".into()));
    assert_eq!(center_across_width(&styles, &sh, a1, &[]), sh.col_width(1));
    let merged = [RangeRef::new(CellRef::new(0, 1), CellRef::new(1, 1))];
    assert_eq!(center_across_width(&styles, &sh, a1, &merged), 0.0);
}

#[test]
fn rotated_wrapped_text_prints_rotated() {
    // Upright headers over narrow columns (mark sheets) combine rotation with Wrap Text.
    let mut s = s();
    let header = "Term 1 : Drama : Continuous Assessment";
    s.execute("range.setValues", json!({"range": "A1", "values": [[header, header]]})).unwrap();
    s.execute("home.wrapText", json!({"range": "A1:B1", "on": true})).unwrap();
    s.execute("home.orientation", json!({"range": "A1", "angle": "up"})).unwrap();
    s.execute("home.orientation", json!({"range": "B1", "angle": "down"})).unwrap();
    s.execute("home.columnWidth", json!({"cols": "A:B", "chars": 6})).unwrap();
    s.execute("home.rowHeight", json!({"rows": "1:1", "height": 267})).unwrap();
    let (pdf, _) = pdf_of(&mut s, json!({}));
    let pdf = String::from_utf8_lossy(&pdf);
    // The page holds only these two cells, so every text block on it is a piece of a header.
    let blocks: Vec<&str> = pdf.split("BT\n").skip(1).filter(|b| b.contains(" Tj")).collect();
    let up = blocks.iter().filter(|b| b.contains("\n0 1 -1 0 ")).count();
    let down = blocks.iter().filter(|b| b.contains("\n0 -1 1 0 ")).count();
    // Every piece of both headers is rotated, each wrapped along the row height into a few long lines.
    assert_eq!(up + down, blocks.len(), "a header line printed flat: {blocks:?}");
    assert!((1..=3).contains(&up) && (1..=3).contains(&down), "up {up}, down {down}: {blocks:?}");
}

#[test]
fn checkbox_cells_print_as_boxes_not_true_false() {
    // #2972: a checkbox cell printed its value as TRUE or FALSE.
    let mut s = s();
    s.execute("insert.checkbox", json!({"range": "A1:A2"})).unwrap();
    s.execute("cell.toggleCheckbox", json!({"cell": "A2"})).unwrap();
    s.execute("cell.set", json!({"cell": "C1", "input": "TRUE"})).unwrap();
    let (pdf, _) = pdf_of(&mut s, json!({}));
    assert!(!contains(&pdf, "(FALSE) Tj"), "the unchecked box printed as FALSE");
    // Only the plain TRUE in C1 prints as text; the checked box is a filled box.
    let text = String::from_utf8_lossy(&pdf);
    assert_eq!(text.matches("(TRUE) Tj").count(), 1, "the checked box printed as TRUE");
    assert!(contains(&pdf, "0.063 0.486 0.255 rg"), "the checked box is not filled");
    assert!(contains(&pdf, "0.431 0.431 0.431 RG"), "the unchecked box is not outlined");
}

#[test]
fn an_equation_without_a_layout_prints_its_source_one_line_per_line() {
    use super::objects::{MAX_FALLBACK_LINES, fallback_lines};
    let mut s = s();
    s.execute("insert.equation", json!({"latex": "x", "cell": "B2"})).unwrap();
    // A file can carry a source the layout engine refuses: two lines here, in a frame 100 px
    // (75 pt) tall, which holds six 12 pt lines.
    let with_source = |text: &str| {
        let mut wb = (*s.doc().unwrap().wb).clone();
        if let Some(sh) = wb.sheet_mut(0) {
            for x in sh.shapes.iter_mut() {
                x.text = text.into();
                x.anchor.height = 100.0;
            }
        }
        let mut t = Session::new();
        t.add_document(crate::DocState::new(wb, None, "Book".into(), crate::Trust::FromFile));
        pdf_of(&mut t, json!({})).0
    };
    let pdf = with_source("frac{\nsecond line");
    assert!(contains(&pdf, "(frac{) Tj"), "the first line is its own text run");
    assert!(contains(&pdf, "(second line) Tj"), "and so is the second");
    assert!(!contains(&pdf, "(frac{?second line) Tj"), "the newline is not printed as ?");
    let (y1, y2) = (text_y(&pdf, "frac{").unwrap(), text_y(&pdf, "second line").unwrap());
    assert!(y1 > y2 && y1 - y2 < 13.0, "the second line sits one line below the first: {y1} {y2}");
    // Lines the frame cannot hold are not printed.
    let pdf = with_source(&"l\n".repeat(10_000));
    assert_eq!(String::from_utf8_lossy(&pdf).matches("(l) Tj").count(), 6);
    // Whatever the frame, the count is capped; a frame too low for one line still shows the first.
    let tall = "l\n".repeat(10_000);
    assert_eq!(fallback_lines(&tall, f32::INFINITY, 12.0).count(), MAX_FALLBACK_LINES);
    assert_eq!(fallback_lines(&tall, 1.0e9, 12.0).count(), MAX_FALLBACK_LINES);
    assert_eq!(fallback_lines("a\nb\nc", 30.0, 12.0).count(), 2);
    assert_eq!(fallback_lines("a\nb", 0.0, 12.0).count(), 1);
    assert_eq!(fallback_lines("a\nb", f32::NAN, 12.0).count(), 1);
}

#[test]
fn the_fallback_text_is_clipped_to_the_equation_frame() {
    // A long line of a source that does not lay out, in a narrow frame (40 by 100 px, 30 by
    // 75 pt): the lines are drawn inside a clip of the frame, nested in the block's, so they
    // cannot run over neighbours, and the clip is lifted again before the next object (a text
    // box below the equation in the same block) is drawn.
    let mut s = s();
    s.execute("insert.equation", json!({"latex": "x", "cell": "B2"})).unwrap();
    s.execute("insert.textBox", json!({"at": "B12", "text": "later box"})).unwrap();
    let mut wb = (*s.doc().unwrap().wb).clone();
    if let Some(sh) = wb.sheet_mut(0) {
        for x in sh.shapes.iter_mut().filter(|x| x.kind == gridcraft_model::ShapeKind::Equation) {
            x.text = format!("frac{{{}", "l".repeat(200));
            x.anchor.width = 40.0;
            x.anchor.height = 100.0;
        }
    }
    let mut t = Session::new();
    t.add_document(crate::DocState::new(wb, None, "Book".into(), crate::Trust::FromFile));
    let (pdf, _) = pdf_of(&mut t, json!({}));
    let text = String::from_utf8_lossy(&pdf);
    let clip = text.find("30 75 re W n").expect("a clip the size of the frame");
    let run = text.find("(frac{llll").expect("the source line");
    let later = text.find("(later box) Tj").expect("the text box after the equation");
    assert!(clip < run && run < later, "clip, then the text, then the later object");
    // The net number of saved graphics states (`q` less `Q`) before an operator.
    let depth = |at: usize| text[..at].lines().filter(|l| *l == "q").count() as i64 - text[..at].lines().filter(|l| *l == "Q").count() as i64;
    assert_eq!(depth(run), depth(clip), "the text is drawn under the frame's clip");
    assert_eq!(depth(later), depth(clip) - 1, "the later object is drawn with the frame's state restored");
}

#[test]
#[allow(clippy::disallowed_methods)] // tests run only off wasm, where the clock exists
fn an_equation_source_of_a_megabyte_prints_a_bounded_excerpt() {
    // A file can carry a source far past `MAX_SOURCE_BYTES` (the drawing part may be 512 MiB). It
    // never lays out, so the fallback prints the first 4,096 bytes and does no work on the rest:
    // the PDF holds one run of that length, not the megabyte, and export takes bounded time.
    let mut s = s();
    s.execute("insert.equation", json!({"latex": "x", "cell": "B2"})).unwrap();
    let mut wb = (*s.doc().unwrap().wb).clone();
    if let Some(sh) = wb.sheet_mut(0) {
        for x in sh.shapes.iter_mut() {
            x.text = "l".repeat(1 << 20);
        }
    }
    let mut t = Session::new();
    t.add_document(crate::DocState::new(wb, None, "Book".into(), crate::Trust::FromFile));
    let start = std::time::Instant::now();
    let (pdf, _) = pdf_of(&mut t, json!({}));
    let took = start.elapsed();
    let text = String::from_utf8_lossy(&pdf);
    let longest = text.split(|c| c != 'l').map(str::len).max().unwrap_or(0);
    assert_eq!(longest, gridcraft_math::MAX_SOURCE_BYTES, "the run is the excerpt and no more");
    assert!(pdf.len() < 64 * 1024, "the PDF holds the excerpt, not the megabyte: {} bytes", pdf.len());
    assert!(took < std::time::Duration::from_secs(5), "{took:?}");
}

#[test]
fn equations_print_with_the_symbol_font() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "x"})).unwrap();
    s.execute("insert.equation", json!({"latex": "\\sum_{i=1}^{n} \\alpha_i \\le \\infty \\pm \\sqrt{x} \\times \\partial y", "cell": "B2"}))
        .unwrap();
    s.execute("insert.equation", json!({"latex": "\\mathbb{R} \\oplus \\text{\u{4E2D}}", "cell": "B8"})).unwrap();
    let (pdf, _) = pdf_of(&mut s, json!({}));
    assert!(contains(&pdf, "/BaseFont /Symbol >>"), "maths symbols use the Symbol font");
    assert!(contains(&pdf, "/BaseFont /Times-Italic"), "variables are italic");
    // The summation (0xE5) and alpha (0x61) codes in Symbol.
    assert!(contains(&pdf, "(\\345) Tj") && contains(&pdf, "(a) Tj"));
    // A glyph no standard font has is a [?] box.
    assert!(contains(&pdf, "(?) Tj"));
}

#[test]
fn chart_budget_counts_path_points_and_drops_charts_that_do_not_fit() {
    use gridcraft_chart::Prim;
    let line = |n: usize| Prim::Line { pts: vec![[0.0, 0.0]; n], color: [0, 0, 0, 255], width: 1.0, dash: false };
    // A smoothed line is its points, not one unit.
    assert_eq!(chart_cost(&[line(4000), Prim::Circle { cx: 0.0, cy: 0.0, r: 1.0, fill: None, stroke: None }]), 4002);
    // Room for exactly two copies of a 1,000-point chart.
    let mut pics = PicCache { chart_prims: MAX_CHART_PRIMS - 2002, ..Default::default() };
    let mut renders = 0;
    for expect in [true, true, false, false] {
        let got = pics.chart((0, 0), || {
            renders += 1;
            vec![line(1000)]
        });
        assert_eq!(got.is_some(), expect);
    }
    assert_eq!(renders, 1, "a rejected chart is not rendered again");
    assert!(pics.charts.is_empty(), "and its primitives are not kept");
    // A chart too big for what is left is dropped without being cached at all.
    let mut pics = PicCache { chart_prims: MAX_CHART_PRIMS - 10, ..Default::default() };
    assert!(pics.chart((0, 1), || vec![line(1000)]).is_none());
    assert!(pics.charts.is_empty() && pics.rejected_charts.contains(&(0, 1)));
}
