//! Page Setup › Sheet › Page order: `<pageSetup pageOrder>` is read and written back. As in
//! Excel's files, only "over, then down" is written; "down, then over" is the schema default.

use gridcraft_model::{PageOrder, Workbook};

use super::minimal;
use crate::{read_xlsx, write_xlsx};

/// The first sheet's XML as written.
fn sheet_xml(bytes: &[u8]) -> String {
    String::from_utf8(crate::package::Package::open(bytes).unwrap().read("xl/worksheets/sheet1.xml").unwrap().unwrap()).unwrap()
}

/// The `<pageSetup …/>` element of a sheet's XML.
fn page_setup(xml: &str) -> &str {
    let start = xml.find("<pageSetup").unwrap_or_else(|| panic!("no pageSetup: {xml}"));
    let len = xml[start..].find("/>").unwrap() + 2;
    &xml[start..start + len]
}

#[test]
fn over_then_down_is_read_and_written_back() {
    // The attributes Excel writes for a sheet set to Over, then down.
    let body = r#"<sheetData/><pageMargins left="0.7" right="0.7" top="0.75" bottom="0.75" header="0.3" footer="0.3"/><pageSetup paperSize="9" pageOrder="overThenDown" orientation="portrait"/>"#;
    let (wb, _) = read_xlsx(&minimal(body, &[], "", "")).unwrap();
    assert_eq!(wb.sheet(0).unwrap().print.page_order, PageOrder::OverThenDown);
    let bytes = write_xlsx(&wb).unwrap();
    let xml = sheet_xml(&bytes);
    let ps = page_setup(&xml);
    assert_eq!(ps, r#"<pageSetup paperSize="9" pageOrder="overThenDown" orientation="portrait"/>"#);
    let (back, _) = read_xlsx(&bytes).unwrap();
    assert_eq!(back.sheet(0).unwrap().print.page_order, PageOrder::OverThenDown);
    // CT_PageSetup's attribute order: Fit To before the page order, the page order before the
    // orientation.
    let mut wb = back;
    wb.sheet_mut(0).unwrap().print.fit_to = Some((1, 2));
    let xml = sheet_xml(&write_xlsx(&wb).unwrap());
    assert!(page_setup(&xml).contains(r#"fitToHeight="2" pageOrder="overThenDown" orientation="portrait""#), "{xml}");
}

#[test]
fn down_then_over_writes_no_page_order() {
    // Written out, absent, unknown (read as the default) and a sheet without page setup at all.
    for body in [
        r#"<sheetData/><pageSetup paperSize="9" pageOrder="downThenOver" orientation="portrait"/>"#,
        r#"<sheetData/><pageSetup paperSize="9" orientation="portrait"/>"#,
        r#"<sheetData/><pageSetup pageOrder="sideways"/>"#,
        "<sheetData/>",
    ] {
        let (wb, _) = read_xlsx(&minimal(body, &[], "", "")).unwrap();
        assert_eq!(wb.sheet(0).unwrap().print.page_order, PageOrder::DownThenOver, "{body}");
        let xml = sheet_xml(&write_xlsx(&wb).unwrap());
        assert!(!xml.contains("pageOrder"), "{body}: {xml}");
        assert!(xml.matches("<pageSetup").count() <= 1, "{xml}");
    }
    // A new workbook, and a sheet set back to Down, then over.
    let mut wb = Workbook::new();
    assert!(!sheet_xml(&write_xlsx(&wb).unwrap()).contains("pageOrder"));
    wb.sheet_mut(0).unwrap().print.page_order = PageOrder::OverThenDown;
    assert!(sheet_xml(&write_xlsx(&wb).unwrap()).contains(r#"pageOrder="overThenDown""#));
    wb.sheet_mut(0).unwrap().print.page_order = PageOrder::DownThenOver;
    assert!(!sheet_xml(&write_xlsx(&wb).unwrap()).contains("pageOrder"));
}

#[test]
fn page_margins_are_written_as_their_decimals() {
    // #1609 item 7: the f32 margins were widened to f64, so 0.3 came out as 0.30000001192092896.
    let body = r#"<sheetData/><pageMargins left="0.3" right="0.25" top="1.1" bottom="0.75" header="0.3" footer="0.3"/>"#;
    let (wb, _) = read_xlsx(&minimal(body, &[], "", "")).unwrap();
    let xml = sheet_xml(&write_xlsx(&wb).unwrap());
    assert!(xml.contains(r#"<pageMargins left="0.3" right="0.25" top="1.1" bottom="0.75" header="0.3" footer="0.3"/>"#), "{xml}");
    // The default sheet writes Excel's default margins exactly.
    let xml = sheet_xml(&write_xlsx(&Workbook::new()).unwrap());
    assert!(xml.contains(r#"<pageMargins left="0.7" right="0.7" top="0.75" bottom="0.75" header="0.3" footer="0.3"/>"#), "{xml}");
}

#[test]
fn first_page_number_is_read_and_written_back() {
    // #4293: Excel writes Page Setup's First page number 7 as these two attributes; both were lost.
    let body = r#"<sheetData/><pageMargins left="0.7" right="0.7" top="0.75" bottom="0.75" header="0.3" footer="0.3"/><pageSetup paperSize="9" firstPageNumber="7" orientation="portrait" useFirstPageNumber="1"/>"#;
    let (wb, _) = read_xlsx(&minimal(body, &[], "", "")).unwrap();
    assert_eq!(wb.sheet(0).unwrap().print.first_page_number, Some(7));
    let bytes = write_xlsx(&wb).unwrap();
    let xml = sheet_xml(&bytes);
    assert_eq!(page_setup(&xml), r#"<pageSetup paperSize="9" firstPageNumber="7" orientation="portrait" useFirstPageNumber="1"/>"#);
    let (back, _) = read_xlsx(&bytes).unwrap();
    assert_eq!(back.sheet(0).unwrap().print.first_page_number, Some(7));
    // In the schema's order with the other attributes: after the scale, before Fit To.
    let mut wb = back;
    wb.sheet_mut(0).unwrap().print.scale = 80;
    wb.sheet_mut(0).unwrap().print.fit_to = Some((1, 2));
    let xml = sheet_xml(&write_xlsx(&wb).unwrap());
    assert!(page_setup(&xml).contains(r#"scale="80" firstPageNumber="7" fitToWidth="1""#), "{xml}");
    // Automatic: no number, a number Excel keeps without using it, and `useFirstPageNumber="0"`.
    for body in [
        r#"<sheetData/><pageSetup paperSize="9" orientation="portrait"/>"#,
        r#"<sheetData/><pageSetup paperSize="9" firstPageNumber="5" orientation="portrait"/>"#,
        r#"<sheetData/><pageSetup paperSize="9" firstPageNumber="5" orientation="portrait" useFirstPageNumber="0"/>"#,
        "<sheetData/>",
    ] {
        let (wb, _) = read_xlsx(&minimal(body, &[], "", "")).unwrap();
        assert_eq!(wb.sheet(0).unwrap().print.first_page_number, None, "{body}");
        let xml = sheet_xml(&write_xlsx(&wb).unwrap());
        assert!(!xml.contains("irstPageNumber"), "{body}: {xml}");
    }
    // 1 is a number of its own (Excel's field shows 1, not Automatic) and is kept too.
    let mut wb = Workbook::new();
    wb.sheet_mut(0).unwrap().print.first_page_number = Some(1);
    let xml = sheet_xml(&write_xlsx(&wb).unwrap());
    assert!(xml.contains(r#"firstPageNumber="1""#) && xml.contains(r#"useFirstPageNumber="1""#), "{xml}");
}
