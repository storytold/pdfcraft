//! Pins the exact `<sheetData>` the writer produces: row order with row settings between and
//! around cell rows, cells added outside the store (cached spill values), every value kind, number
//! spellings, escaping, cell references far out in the grid and formulas with and without a file
//! spelling. The expected text was produced by the writer before its per-cell path was made
//! allocation-free, so any change in the bytes shows up here.

use std::sync::Arc;

use gridcraft_core::{CellError, CellRef, RangeRef, Value};
use gridcraft_model::{Cell, Formula, LineInfo, NumFmt, StyleId, Workbook};

use crate::package::Package;
use crate::write_xlsx;

fn at(a: &str) -> CellRef {
    CellRef::parse(a).unwrap()
}

fn formula(text: &str, value: Value, style: StyleId) -> Cell {
    Cell { value, formula: Some(Arc::new(Formula::new(text))), style, py: None }
}

fn sheet_data(wb: &Workbook) -> String {
    let bytes = write_xlsx(wb).unwrap();
    let mut pkg = Package::open(&bytes).unwrap();
    let xml = String::from_utf8(pkg.read("xl/worksheets/sheet1.xml").unwrap().unwrap()).unwrap();
    let start = xml.find("<sheetData>").unwrap();
    let end = xml.find("</sheetData>").unwrap() + "</sheetData>".len();
    xml[start..end].to_string()
}

#[test]
fn sheet_data_is_written_byte_for_byte_as_pinned() {
    let mut wb = Workbook::new();
    let money = wb.styles.derive(StyleId::DEFAULT, |s| s.num_fmt = NumFmt::new("0.00"));
    let sh = wb.sheet_mut(0).unwrap();
    sh.rows.insert(1, LineInfo { hidden: true, ..LineInfo::default() });
    sh.rows.insert(4, LineInfo { size: Some(30.0), custom: true, ..LineInfo::default() });
    sh.rows.insert(7, LineInfo { outline: 2, style: Some(money), ..LineInfo::default() });
    let set =
        |sh: &mut gridcraft_model::Sheet, a: &str, value: Value, style: StyleId| sh.cells.set(at(a), Cell { value, formula: None, style, py: None });
    set(sh, "A3", Value::text("a & <b> \"q\""), StyleId::DEFAULT);
    set(sh, "B3", Value::Number(-5.0), StyleId::DEFAULT);
    set(sh, "C3", Value::Number(0.1), money);
    set(sh, "D3", Value::Bool(true), StyleId::DEFAULT);
    set(sh, "E3", Value::Error(CellError::NA), StyleId::DEFAULT);
    set(sh, "Z3", Value::Number(-0.0), StyleId::DEFAULT);
    set(sh, "AA3", Value::Number(-2.5), StyleId::DEFAULT);
    set(sh, "A5", Value::Number(1e20), money);
    set(sh, "B5", Value::Number(1.5e-7), StyleId::DEFAULT);
    set(sh, "C5", Value::Number(123_456_789_012_345.0), StyleId::DEFAULT);
    set(sh, "D5", Value::Number(-1e15), StyleId::DEFAULT);
    set(sh, "E5", Value::Number(f64::NAN), StyleId::DEFAULT);
    set(sh, "F5", Value::text("Grüße\r\n\u{1}_x0041_ 日本"), StyleId::DEFAULT);
    set(sh, "G5", Value::text("a & <b> \"q\""), StyleId::DEFAULT);
    set(sh, "H5", Value::Empty, money);
    sh.cells.set(at("B1"), formula("SEQUENCE(2)", Value::Number(1.0), StyleId::DEFAULT));
    sh.spill_ranges.insert(at("B1"), RangeRef::parse("B1:B2").unwrap());
    sh.spill.insert(at("B2"), Value::Number(2.0));
    sh.cells.set(at("AB10000"), formula("SUM($A$1:B2,Sheet2!C:C,3:$5,'My Sheet'!$XFD$1)*2", Value::Number(0.25), money));
    sh.cells.set(at("AC10000"), formula("A1&\"<x>\"", Value::text("t & <u>"), StyleId::DEFAULT));
    sh.cells.set(at("AD10000"), formula("LET(x,1,x+XLOOKUP(1,A:A,B:B))", Value::Bool(false), StyleId::DEFAULT));
    sh.cells.set(at("AE10000"), formula("1/0", Value::Error(CellError::Div0), StyleId::DEFAULT));
    sh.cells.set(at("AF10000"), formula("A1", Value::Empty, StyleId::DEFAULT));
    sh.cells.set(at("XFD1048576"), Cell::value(Value::Number(7.0)));
    assert_eq!(sheet_data(&wb), EXPECTED);
}

const EXPECTED: &str = r#"<sheetData><row r="1"><c r="B1" cm="1"><f t="array" ref="B1:B2">_xlfn.SEQUENCE(2)</f><v>1</v></c></row><row r="2" hidden="1"><c r="B2"><v>2</v></c></row><row r="3"><c r="A3" t="s"><v>0</v></c><c r="B3"><v>-5</v></c><c r="C3" s="1"><v>0.1</v></c><c r="D3" t="b"><v>1</v></c><c r="E3" t="e"><v>#N/A</v></c><c r="Z3"><v>0</v></c><c r="AA3"><v>-2.5</v></c></row><row r="5" ht="22.5" customHeight="1"><c r="A5" s="1"><v>1E20</v></c><c r="B5"><v>1.5E-7</v></c><c r="C5"><v>123456789012345</v></c><c r="D5"><v>-1000000000000000.0</v></c><c r="E5"><v>0</v></c><c r="F5" t="s"><v>1</v></c><c r="G5" t="s"><v>0</v></c><c r="H5" s="1"/></row><row r="8" s="1" customFormat="1" outlineLevel="2"/><row r="10000"><c r="AB10000" s="1"><f>SUM($A$1:B2,Sheet2!C:C,3:$5,'My Sheet'!$XFD$1)*2</f><v>0.25</v></c><c r="AC10000" t="str"><f>A1&amp;"&lt;x&gt;"</f><v>t &amp; &lt;u&gt;</v></c><c r="AD10000" t="b"><f>_xlfn.LET(_xlpm.x,1,_xlpm.x+_xlfn.XLOOKUP(1,A:A,B:B))</f><v>0</v></c><c r="AE10000" t="e"><f>1/0</f><v>#DIV/0!</v></c><c r="AF10000"><f>A1</f></c></row><row r="1048576"><c r="XFD1048576"><v>7</v></c></row></sheetData>"#;
