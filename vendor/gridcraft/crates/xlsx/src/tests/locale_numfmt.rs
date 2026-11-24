//! Number formats whose meaning depends on the reader's locale are written with their code: the
//! currency formats 5–8 and the accounting formats with a currency, 42 and 44. Written as a bare
//! built-in id, LibreOffice and Excel in other locales show their own version (a 44 cell holding
//! -0.14 reads `$ (0.14)` in US Excel but `- 0.14` in LibreOffice).

use std::io::Read;
use std::sync::Arc;

use gridcraft_core::{CellRef, Value};
use gridcraft_model::*;

use crate::tables::builtin_format;
use crate::{read_xlsx, write_xlsx};

fn styles_xml(bytes: &[u8]) -> String {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut s = String::new();
    zip.by_name("xl/styles.xml").unwrap().read_to_string(&mut s).unwrap();
    s
}

/// Writes one cell formatted with `code`; returns the `numFmtId` its cell format points at, the
/// `styles.xml` and the code read back.
fn round_trip(code: &str) -> (u32, String, String) {
    let mut wb = Workbook::new();
    let style = wb.styles.derive(StyleId::DEFAULT, |s| s.num_fmt = NumFmt::new(code));
    let mut sheet = Sheet::new("Sheet1");
    sheet.cells.set(CellRef::new(0, 0), Cell { value: Value::Number(-0.14), formula: None, style, py: None });
    wb.sheets = vec![Arc::new(sheet)];
    let bytes = write_xlsx(&wb).unwrap();
    let xml = styles_xml(&bytes);
    let xfs = &xml[xml.find("<cellXfs").unwrap()..];
    let id = xfs
        .match_indices("numFmtId=\"")
        .map(|(i, m)| xfs[i + m.len()..].split('"').next().unwrap().parse::<u32>().unwrap())
        .find(|&id| id != 0)
        .unwrap();
    let (back, _) = read_xlsx(&bytes).unwrap();
    let st = back.styles.get(back.sheet(0).unwrap().cell(CellRef::new(0, 0)).unwrap().style);
    (id, xml, st.num_fmt.as_str().to_string())
}

#[test]
fn currency_and_accounting_formats_are_written_with_their_code() {
    for builtin in [5, 6, 7, 8, 42, 44] {
        let code = builtin_format(builtin).unwrap();
        let (id, xml, back) = round_trip(code);
        assert!(id >= 164, "format {builtin} written as built-in id {id}");
        assert!(xml.contains(&format!("<numFmt numFmtId=\"{id}\"")), "format {builtin}: no <numFmt> for id {id}");
        assert_eq!(back, code, "format {builtin}");
    }
}

#[test]
fn locale_independent_built_ins_keep_their_id() {
    for builtin in [3, 4, 10, 14, 41, 43, 49] {
        let (id, _, back) = round_trip(builtin_format(builtin).unwrap());
        assert_eq!(id, builtin);
        assert_eq!(back, builtin_format(builtin).unwrap());
    }
}
