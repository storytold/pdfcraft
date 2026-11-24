//! #2871: Google Sheets' XLSX export writes a formula that uses a function it thinks Excel lacks
//! as `IFERROR(__xludf.DUMMYFUNCTION("<formula, quotes doubled>"),<cached value>)`. GridCraft
//! reads the formula inside when GridCraft calculates every function it calls, keeps the file's
//! cached value, and saves the real formula.

use gridcraft_core::{CellRef, Value};
use gridcraft_model::{Formula, Workbook};

use crate::package::Package;
use crate::{read_xlsx, read_xlsx_with, write_xlsx};

fn at(s: &str) -> CellRef {
    CellRef::parse(s).unwrap()
}

/// The issue's formula as Google writes it, with real newlines in the string literal.
const HEBREW: &str = "IFERROR(__xludf.DUMMYFUNCTION(\"MAX(\n IFERROR(MAX(FILTER(ROW(C:C), C:C=\"\"שולם\"\")), 0),\n IFERROR(MAX(FILTER(ROW(D:D), D:D=\"\"שולם\"\")), 0)\n)\"),40)";
const HEBREW_MODEL: &str = "MAX(IFERROR(MAX(FILTER(ROW(C:C),C:C=\"שולם\")),0),IFERROR(MAX(FILTER(ROW(D:D),D:D=\"שולם\")),0))";

/// A sheet as Google's export writes it: plain, `t="array"` and shared formulas, a wrapper inside
/// a larger formula, one whose inner text doesn't parse and one with a Google-only function.
fn google_book() -> Vec<u8> {
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let body = format!(
        concat!(
            "<sheetData>",
            r#"<row r="1"><c r="A1"><f>{hebrew}</f><v>40</v></c><c r="B1"><v>1</v></c>"#,
            r#"<c r="E1"><f t="array" ref="E1:E2">_xlfn.IFERROR(__xludf.DUMMYFUNCTION("FILTER(B1:B3,B1:B3>1)"),2)</f><v>2</v></c>"#,
            r#"<c r="F1"><f t="shared" ref="F1:F3" si="0">IFERROR(__xludf.DUMMYFUNCTION("B1*2"),2)</f><v>2</v></c>"#,
            r#"<c r="G1"><f>1+IFERROR(__xludf.DUMMYFUNCTION("B1+B2"),5)</f><v>6</v></c>"#,
            r#"<c r="H1"><f>IFERROR(__xludf.DUMMYFUNCTION("SUM("),5)</f><v>5</v></c>"#,
            r#"<c r="I1" t="str"><f>IFERROR(__xludf.dummyfunction("""a""&amp;B1&amp;CHAR(10)&amp;&#10;B2"),"a1")</f><v>a1</v></c>"#,
            r#"<c r="J1" t="str"><f>IFERROR(__xludf.DUMMYFUNCTION("GOOGLETRANSLATE(""hello"",""en"",""fr"")"),"bonjour")</f><v>bonjour</v></c></row>"#,
            r#"<row r="2"><c r="B2"><v>4</v></c><c r="E2"><v>3</v></c><c r="F2"><f t="shared" si="0"/><v>8</v></c></row>"#,
            r#"<row r="3"><c r="B3"><v>3</v></c><c r="F3"><f t="shared" si="0"/><v>6</v></c></row>"#,
            "</sheetData>"
        ),
        hebrew = esc(HEBREW),
    );
    super::minimal(&body, &[], "", "")
}

fn formula(wb: &Workbook, cell: &str) -> Formula {
    let c = wb.sheets[0].cell(at(cell)).unwrap();
    Formula::clone(c.formula.as_ref().unwrap_or_else(|| panic!("{cell} has no formula")))
}

fn value(wb: &Workbook, cell: &str) -> Value {
    wb.sheets[0].cell(at(cell)).unwrap().value.clone()
}

fn assert_google_book(wb: &Workbook) {
    assert_eq!(formula(wb, "A1").text, HEBREW_MODEL);
    let e1 = formula(wb, "E1");
    assert_eq!(e1.text, "FILTER(B1:B3,B1:B3>1)");
    assert_eq!(e1.array, Some(gridcraft_core::RangeRef::parse("E1:E2").unwrap()));
    assert_eq!(formula(wb, "F1").text, "B1*2");
    assert_eq!(formula(wb, "F2").text, "B2*2");
    assert_eq!(formula(wb, "F3").text, "B3*2");
    assert_eq!(formula(wb, "G1").text, "1+(B1+B2)");
    assert_eq!(formula(wb, "H1").text, "IFERROR(__XLUDF.DUMMYFUNCTION(\"SUM(\"),5)");
    assert_eq!(formula(wb, "I1").text, "\"a\"&B1&CHAR(10)&B2");
    // GridCraft has no GOOGLETRANSLATE: the wrapper stays, so the cell shows the cached text.
    assert_eq!(formula(wb, "J1").text, "IFERROR(__XLUDF.DUMMYFUNCTION(\"GOOGLETRANSLATE(\"\"hello\"\",\"\"en\"\",\"\"fr\"\")\"),\"bonjour\")");
    // The cached values are the file's (inside an array range the reader keeps only the anchor's).
    for (cell, v) in [("A1", 40.0), ("E1", 2.0), ("F1", 2.0), ("F2", 8.0), ("F3", 6.0), ("G1", 6.0), ("H1", 5.0)] {
        assert_eq!(value(wb, cell), Value::Number(v), "{cell}");
    }
    assert_eq!(value(wb, "I1"), Value::text("a1"));
    assert_eq!(value(wb, "J1"), Value::text("bonjour"));
    // Every unwrapped formula parses.
    for cell in ["A1", "E1", "F1", "F2", "F3", "G1", "I1"] {
        assert!(formula(wb, cell).expr().is_some(), "{cell}");
    }
}

/// Reads a file as the engine opens one.
fn open(bytes: &[u8]) -> Workbook {
    read_xlsx_with(bytes, &crate::fmla::test_known_function).unwrap().0
}

#[test]
fn google_sheets_wrapper_reads_as_the_original_formula() {
    assert_google_book(&open(&google_book()));
    // Without a function list the reader keeps the file's formula.
    let (wb, _) = read_xlsx(&google_book()).unwrap();
    assert!(formula(&wb, "A1").text.contains("DUMMYFUNCTION"));
}

/// Saving writes the real formula, with the file prefixes Excel expects, as for any formula
/// GridCraft reads; reopening gives the same workbook.
#[test]
fn google_sheets_wrapper_saves_the_real_formula() {
    let wb = open(&google_book());
    let saved = write_xlsx(&wb).unwrap();
    let mut pkg = Package::open(&saved).unwrap();
    let xml = String::from_utf8(pkg.read("xl/worksheets/sheet1.xml").unwrap().unwrap()).unwrap();
    assert!(xml.contains("MAX(IFERROR(MAX(_xlfn._xlws.FILTER(ROW(C:C),C:C=\"שולם\")),0)"), "{xml}");
    assert!(xml.contains("_xlfn._xlws.FILTER(B1:B3,B1:B3&gt;1)"), "{xml}");
    // Only the formula whose inner text doesn't parse and the Google-only one keep their wrapper.
    assert_eq!(xml.matches("DUMMYFUNCTION").count(), 2, "{xml}");
    assert_google_book(&open(&saved));
}
