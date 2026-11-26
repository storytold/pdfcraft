//! Original BIFF12 records and ZIP packages assembled in Rust from the public format schema.
use gridcraft_core::{CellError, CellRef, DateSystem, RangeRef, Value};
use gridcraft_model::Visibility;

use super::make_zip;
use crate::{Format, read_xlsb, read_xlsx, sniff, write_xlsx};

const ROOT: &str = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="book" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="records/book.bin"/></Relationships>"#;
const RELS: &str = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="one" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="../data/one.bin"/><Relationship Id="two" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="../data/two.bin"/><Relationship Id="strings" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="../text/strings.bin"/></Relationships>"#;
const TYPES: &str = r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/records/book.bin" ContentType="application/vnd.ms-excel.sheet.binary.macroEnabled.main"/><Override PartName="/data/one.bin" ContentType="application/vnd.ms-excel.worksheet"/><Override PartName="/data/two.bin" ContentType="application/vnd.ms-excel.worksheet"/><Override PartName="/text/strings.bin" ContentType="application/vnd.ms-excel.sharedStrings"/></Types>"#;

fn record(id: u16, bytes: &[u8]) -> Vec<u8> {
    let mut out = vec![];
    if id < 128 {
        out.push(id as u8);
    } else {
        out.extend([(id as u8 & 0x7f) | 0x80, (id >> 7) as u8]);
    }
    let mut len = bytes.len();
    loop {
        let byte = (len & 0x7f) as u8;
        len >>= 7;
        out.push(if len == 0 { byte } else { byte | 0x80 });
        if len == 0 {
            break;
        }
    }
    out.extend(bytes);
    out
}
fn wide(text: &str) -> Vec<u8> {
    let units: Vec<_> = text.encode_utf16().collect();
    let mut out = (units.len() as u32).to_le_bytes().to_vec();
    for unit in units {
        out.extend(unit.to_le_bytes());
    }
    out
}
fn rich(text: &str, extras: bool) -> Vec<u8> {
    let mut out = vec![if extras { 3 } else { 0 }];
    out.extend(wide(text));
    if extras {
        out.extend(1u32.to_le_bytes());
        out.extend([0, 0, 0, 0]); // One original formatting run.
        out.extend(wide("sound"));
        out.extend(1u32.to_le_bytes());
        out.extend([0, 0, 0, 0, 1, 0, 0, 0, 0, 0]); // One ten-byte phonetic run.
    }
    out
}
fn row(r: u32) -> Vec<u8> {
    let mut out = r.to_le_bytes().to_vec();
    out.extend([0; 13]);
    record(0, &out)
}
fn cell(id: u16, col: u32, value: &[u8]) -> Vec<u8> {
    let mut data = col.to_le_bytes().to_vec();
    data.extend([0; 4]);
    data.extend(value);
    if (8..=11).contains(&id) {
        data.extend([0; 2]); // Formula flags.
        data.extend(3u32.to_le_bytes());
        data.extend([0x1e, 1, 0]); // Original PtgInt(1); ignored because we import the cache.
        data.extend(0u32.to_le_bytes());
    }
    record(id, &data)
}
fn worksheet(data: &[u8], tail: &[u8]) -> Vec<u8> {
    let mut out = record(129, &[]);
    out.extend(record(148, &[0xff; 16])); // A bogus dimension must never allocate a dense grid.
    out.extend(record(145, &[]));
    out.extend(data);
    out.extend(record(146, &[]));
    out.extend(tail);
    out.extend(record(130, &[]));
    out
}
fn book(date1904: bool) -> Vec<u8> {
    let mut out = record(131, &[]);
    let mut props = u32::from(date1904).to_le_bytes().to_vec();
    props.extend(0u32.to_le_bytes());
    props.extend(wide(""));
    out.extend(record(153, &props));
    out.extend(record(143, &[]));
    for (id, rel, name, state) in [(1u32, "one", "Data", 0u32), (2, "two", "Hidden", 1)] {
        let mut data = state.to_le_bytes().to_vec();
        data.extend(id.to_le_bytes());
        data.extend(wide(rel));
        data.extend(wide(name));
        out.extend(record(156, &data));
    }
    out.extend(record(144, &[]));
    out.extend(record(132, &[]));
    out
}
fn sst() -> Vec<u8> {
    let mut out = record(159, &[1, 0, 0, 0, 1, 0, 0, 0]);
    out.extend(record(19, &rich("shared 🦀 café", true)));
    out.extend(record(160, &[]));
    out
}
fn package(book: &[u8], sheet: &[u8], strings: &[u8], roots: &str, rels: &str, types: &str) -> Vec<u8> {
    make_zip(&[
        ("[Content_Types].xml", types.as_bytes()),
        ("_rels/.rels", roots.as_bytes()),
        ("records/book.bin", book),
        ("records/_rels/book.bin.rels", rels.as_bytes()),
        ("data/one.bin", sheet),
        ("data/two.bin", &worksheet(&[], &[])),
        ("text/strings.bin", strings),
    ])
}
fn file(sheet: &[u8]) -> Vec<u8> {
    package(&book(false), sheet, &sst(), ROOT, RELS, TYPES)
}
fn text(s: &str) -> Value {
    Value::Text(s.into())
}

#[test]
fn xlsb_scalar_caches_rk_unicode_visibility_and_epochs_survive_xlsx_save() {
    let mut data = row(2);
    let real_rk = (3.5f64.to_bits() >> 32) as u32;
    let encoded = [
        cell(1, 0, &[]),
        cell(2, 1, &((-123i32 << 2) | 2).to_le_bytes()),
        cell(2, 2, &((123i32 << 2) | 3).to_le_bytes()),
        cell(2, 3, &real_rk.to_le_bytes()),
        cell(2, 4, &(real_rk | 1).to_le_bytes()),
        cell(3, 5, &[0x17]),
        cell(4, 6, &[1]),
        cell(5, 7, &123.5f64.to_le_bytes()),
        cell(6, 8, &wide("inline λ")),
        cell(7, 9, &0u32.to_le_bytes()),
        cell(8, 10, &wide("cached text")),
        cell(9, 11, &2.0f64.to_le_bytes()),
        cell(10, 12, &[0]),
        cell(11, 13, &[7]),
        cell(62, 14, &rich("rich text", true)),
    ];
    for cell in encoded {
        data.extend(cell);
    }
    let mut merge = vec![];
    for n in [5u32, 6, 0, 1] {
        merge.extend(n.to_le_bytes());
    }
    let sheet = worksheet(&data, &record(176, &merge));
    let expected = [
        Value::Empty,
        Value::Number(-123.0),
        Value::Number(1.23),
        Value::Number(3.5),
        Value::Number(0.035),
        Value::Error(CellError::Ref),
        Value::Bool(true),
        Value::Number(123.5),
        text("inline λ"),
        text("shared 🦀 café"),
        text("cached text"),
        Value::Number(2.0),
        Value::Bool(false),
        Value::Error(CellError::Div0),
        text("rich text"),
    ];
    for epoch in [false, true] {
        let bytes = package(&book(epoch), &sheet, &sst(), ROOT, RELS, TYPES);
        assert_eq!(sniff(&bytes), Format::Xlsb);
        let (wb, report) = read_xlsb(&bytes).unwrap();
        assert_eq!(wb.date_system, if epoch { DateSystem::D1904 } else { DateSystem::D1900 });
        assert_eq!(wb.sheets.len(), 2);
        assert_eq!(wb.sheets[1].visibility, Visibility::Hidden);
        assert_eq!(wb.sheets[0].cells.len(), 14);
        assert_eq!(wb.sheets[0].merges, vec![RangeRef::new(CellRef::new(5, 0), CellRef::new(6, 1))]);
        assert!(report.warnings.iter().any(|w| w.contains("may display as numbers") && w.contains("XLSB saving is not supported")));
        for (col, expected) in expected.iter().enumerate() {
            assert_eq!(&wb.sheets[0].value(CellRef::new(2, col as u32)), expected);
        }
        for col in 10..=13 {
            assert_eq!(wb.sheets[0].cell(CellRef::new(2, col)).and_then(|c| c.formula.as_ref()).map(|f| f.text.as_str()), Some("1"));
        }
        let (roundtrip, _) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
        assert_eq!(roundtrip.date_system, wb.date_system);
        for (col, expected) in expected.iter().enumerate() {
            assert_eq!(&roundtrip.sheets[0].value(CellRef::new(2, col as u32)), expected);
        }
    }
}

#[test]
fn xlsb_extension_blocks_are_skipped_with_strict_boundaries_and_record_framing() {
    let mut data = row(0);
    data.extend(cell(5, 0, &1.0f64.to_le_bytes()));
    data.extend(record(35, &[0; 4]));
    data.extend(record(37, &[1, 0, 0, 0, 0, 0]));
    data.extend(cell(5, 1, &99.0f64.to_le_bytes()));
    data.extend(record(130, &[])); // Looks like EndSheet, but is inside an ignored extension.
    data.extend(record(38, &[]));
    data.extend(record(36, &[]));
    data.extend([0xe8, 0x87, 0x80, 0x80, 0x80, 0x80]); // ID1000: ignored high second-ID/fourth-length bits.
    data.extend(cell(5, 1, &2.0f64.to_le_bytes()));
    let (wb, report) = read_xlsb(&file(&worksheet(&data, &[]))).unwrap();
    assert_eq!(wb.sheets[0].cells.len(), 2);
    assert_eq!(wb.sheets[0].value(CellRef::new(0, 1)), Value::Number(2.0));
    assert!(report.warnings.iter().any(|w| w.contains("blocks were skipped")));
    for bad in [
        vec![0x81],
        vec![0x81, 0],
        vec![1, 0x80],
        vec![1, 10, 0],
        record(36, &[]),
        [record(35, &[0; 4]), record(38, &[])].concat(),
        record(35, &[0; 4]),
        record(37, &[0, 0]),
    ] {
        assert!(read_xlsb(&file(&worksheet(&bad, &[]))).is_err(), "{bad:?}");
    }
    let nested = record(35, &[0; 4]).repeat(65);
    assert!(read_xlsb(&file(&worksheet(&nested, &[]))).unwrap_err().to_string().contains("nested too deeply"));
    let mut missing_end = worksheet(&[], &[]);
    missing_end.truncate(missing_end.len() - record(130, &[]).len());
    assert!(read_xlsb(&file(&missing_end)).is_err());
}

#[test]
fn xlsb_bad_values_strings_and_coordinates_fail_without_silent_data_loss() {
    let malformed = [
        cell(4, 0, &[2]),
        cell(3, 0, &[0xff]),
        cell(5, 0, &f64::INFINITY.to_le_bytes()),
        cell(2, 0, &0x7ff00000u32.to_le_bytes()),
        cell(7, 0, &1u32.to_le_bytes()),
        cell(6, 0, &[1, 0, 0, 0, 0, 0xd8]),
        cell(6, 0, &u32::MAX.to_le_bytes()),
        cell(5, 16384, &1.0f64.to_le_bytes()),
        cell(5, 0, &[0; 3]),
        cell(62, 0, &[1, 0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]),
    ];
    for bad in malformed {
        assert!(read_xlsb(&file(&worksheet(&[row(0), bad].concat(), &[]))).is_err());
    }
    assert!(read_xlsb(&file(&worksheet(&cell(5, 0, &1.0f64.to_le_bytes()), &[]))).is_err());
    assert!(read_xlsb(&file(&worksheet(&row(1048576), &[]))).is_err());
    let repeated = [row(0), cell(5, 0, &1.0f64.to_le_bytes()), cell(5, 0, &2.0f64.to_le_bytes())].concat();
    assert!(read_xlsb(&file(&worksheet(&repeated, &[]))).is_err());
    let mut formula_payload = [0u8; 18].to_vec(); // Cell header + f64 cache + flags, but no parsed-formula structure.
    assert!(read_xlsb(&file(&worksheet(&[row(0), record(9, &formula_payload)].concat(), &[]))).is_err());
    formula_payload.extend(0u32.to_le_bytes()); // Invalid zero token count.
    assert!(read_xlsb(&file(&worksheet(&[row(0), record(9, &formula_payload)].concat(), &[]))).is_err());
    let bad_sst = [record(159, &[1, 0, 0, 0, 2, 0, 0, 0]), record(160, &[])].concat();
    assert!(read_xlsb(&package(&book(false), &worksheet(&[], &[]), &bad_sst, ROOT, RELS, TYPES)).is_err());
}

#[test]
fn xlsb_requires_internal_typed_relationships_and_sniffs_only_root_workbook() {
    let sheet = worksheet(&[], &[]);
    for (roots, rels, types) in [
        (ROOT.replace("officeDocument\"", "wrong\""), RELS.to_owned(), TYPES.to_owned()),
        (ROOT.replace("Target=", "TargetMode=\"External\" Target="), RELS.to_owned(), TYPES.to_owned()),
        (ROOT.to_owned(), RELS.replace("Target=\"../data/one.bin\"", "Target=\"missing.bin\""), TYPES.to_owned()),
        (
            ROOT.to_owned(),
            RELS.replace("Target=\"../data/one.bin\"", "TargetMode=\"External\" Target=\"https://example.com/data.bin\""),
            TYPES.to_owned(),
        ),
        (
            ROOT.to_owned(),
            RELS.replace("Target=\"../text/strings.bin\"", "TargetMode=\"External\" Target=\"https://example.com/strings.bin\""),
            TYPES.to_owned(),
        ),
        (ROOT.to_owned(), RELS.to_owned(), TYPES.replace("application/vnd.ms-excel.worksheet", "application/xml")),
        (ROOT.to_owned(), RELS.trim_end_matches("</Relationships>").to_owned(), TYPES.to_owned()),
        (ROOT.to_owned(), RELS.to_owned(), TYPES.replace("http://schemas.openxmlformats.org/package/2006/content-types", "wrong")),
    ] {
        assert!(read_xlsb(&package(&book(false), &sheet, &sst(), &roots, &rels, &types)).is_err());
    }
    let chartsheet = RELS.replace(
        "Id=\"two\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet",
        "Id=\"two\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/chartsheet",
    );
    let (wb, report) = read_xlsb(&package(&book(false), &sheet, &sst(), ROOT, &chartsheet, TYPES)).unwrap();
    assert_eq!(wb.sheets.len(), 1);
    assert!(report.warnings.iter().any(|w| w.contains("non-worksheet")));
    // An unrelated binary part must not change the format of an XML workbook package.
    let xml_book = TYPES.replace(
        "/records/book.bin\" ContentType=\"application/vnd.ms-excel.sheet.binary.macroEnabled.main",
        "/elsewhere.bin\" ContentType=\"application/vnd.ms-excel.sheet.binary.macroEnabled.main",
    );
    assert_eq!(sniff(&package(&book(false), &sheet, &sst(), ROOT, RELS, &xml_book)), Format::Xlsx);
    let spec_type = TYPES.replace("application/vnd.ms-excel.sheet.binary.macroEnabled.main", "application/vnd.ms-excel.main");
    assert_eq!(sniff(&package(&book(false), &sheet, &sst(), ROOT, RELS, &spec_type)), Format::Xlsb);
    assert!(read_xlsb(&package(&book(false), &sheet, &sst(), ROOT, RELS, &spec_type)).is_ok());
}

#[test]
fn xlsb_invalid_shared_string_counts_are_rejected() {
    let bad_sst = record(159, &[0xff; 8]);
    assert!(read_xlsb(&package(&book(false), &worksheet(&[], &[]), &bad_sst, ROOT, RELS, TYPES)).is_err());
    let mut count_mismatch = record(159, &[1, 0, 0, 0, 1, 0, 0, 0]);
    count_mismatch.extend(record(160, &[]));
    assert!(read_xlsb(&package(&book(false), &worksheet(&[], &[]), &count_mismatch, ROOT, RELS, TYPES)).is_err());
}

#[test]
fn xlsb_import_budget_preserves_shared_limits_and_the_browser_text_cap() {
    let limits = crate::xlsb::import_limits();
    assert_eq!(limits.binary_cells, 4_000_000);
    assert_eq!(limits.binary_records, 40_000_000);
    assert_eq!(limits.binary_text, if cfg!(target_arch = "wasm32") { 64 << 20 } else { 256 << 20 });
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn xlsb_native_import_accepts_more_than_64_mib_of_decoded_text() {
    // Three UTF-8 bytes per character cross the old cap with ~43 MiB of BIFF12 input.
    let value = "界".repeat(32767);
    let encoded = wide(&value);
    let bytes = {
        let mut data = row(0);
        for col in 0..684 {
            data.extend(cell(6, col, &encoded));
        }
        file(&worksheet(&data, &[]))
    };
    let (workbook, _) = read_xlsb(&bytes).unwrap();
    assert_eq!(workbook.sheets[0].value(CellRef::new(0, 683)), text(&value));
}

#[test]
fn xlsb_phonetic_runs_use_documented_size_and_tolerate_padding() {
    // [MS-XLSB] 2.5.103: a PhRun is 10 bytes. Phonetic data is not imported, so
    // padding after the documented runs (e.g. 12-byte runs) is ignored, while
    // a run shorter than documented is still rejected.
    let phonetic = |runs: u32, bytes: usize| {
        let mut out = vec![2];
        out.extend(wide("ruby"));
        out.extend(wide("ルビ"));
        out.extend(runs.to_le_bytes());
        out.extend(vec![0; bytes]);
        out
    };
    for padded in [phonetic(1, 10), phonetic(1, 12), phonetic(2, 24)] {
        let (wb, _) = read_xlsb(&file(&worksheet(&[row(0), cell(62, 0, &padded)].concat(), &[]))).unwrap();
        assert_eq!(wb.sheets[0].value(CellRef::new(0, 0)), text("ruby"));
    }
    assert!(read_xlsb(&file(&worksheet(&[row(0), cell(62, 0, &phonetic(2, 12))].concat(), &[]))).is_err());
}

#[test]
fn xlsb_formulas_defined_names_and_external_links_delegate_to_craft_xlsx() {
    let mut wb_records = record(131, &[]);
    let mut props = 0u32.to_le_bytes().to_vec();
    props.extend(0u32.to_le_bytes());
    props.extend(wide(""));
    wb_records.extend(record(153, &props));
    wb_records.extend(record(143, &[]));
    for (id, rel, name, state) in [(1u32, "one", "Data", 0u32), (2, "two", "Hidden", 1)] {
        let mut data = state.to_le_bytes().to_vec();
        data.extend(id.to_le_bytes());
        data.extend(wide(rel));
        data.extend(wide(name));
        wb_records.extend(record(156, &data));
    }
    wb_records.extend(record(144, &[]));
    // Supporting books: book 0 = BrtSupSelf (357), book 1 = BrtSupBookSrc (355).
    wb_records.extend(record(357, &[]));
    wb_records.extend(record(355, &[]));
    // BrtExternSheet (362): 2 XTIs: ixti 0 -> (book=0, sheet=1..1), ixti 1 -> (book=1, sheet=0..0).
    let mut xti = 2u32.to_le_bytes().to_vec();
    for (bk, first, last) in [(0u32, 1i32, 1i32), (1, 0, 0)] {
        xti.extend(bk.to_le_bytes());
        xti.extend(first.to_le_bytes());
        xti.extend(last.to_le_bytes());
    }
    wb_records.extend(record(362, &xti));
    // BrtName (39): "Rate" = Hidden!$A$1 (PtgRef3d ixti=0, r=0, c=0).
    let mut ptg_ref3d_hidden = vec![0x3A, 0, 0];
    ptg_ref3d_hidden.extend(0u32.to_le_bytes());
    ptg_ref3d_hidden.extend(0u16.to_le_bytes());
    let mut brt_name = 0u32.to_le_bytes().to_vec();
    brt_name.push(0);
    brt_name.extend(u32::MAX.to_le_bytes());
    brt_name.extend(wide("Rate"));
    brt_name.extend((ptg_ref3d_hidden.len() as u32).to_le_bytes());
    brt_name.extend(&ptg_ref3d_hidden);
    brt_name.extend(0u32.to_le_bytes());
    brt_name.extend(u32::MAX.to_le_bytes()); // null comment
    wb_records.extend(record(39, &brt_name));
    wb_records.extend(record(132, &[]));

    let fmla_num = |col: u32, cached: f64, rgce: &[u8]| {
        let mut data = col.to_le_bytes().to_vec();
        data.extend([0; 4]);
        data.extend(cached.to_le_bytes());
        data.extend([0; 2]);
        data.extend((rgce.len() as u32).to_le_bytes());
        data.extend(rgce);
        data.extend(0u32.to_le_bytes());
        record(9, &data)
    };
    let ptg_name = [0x23, 1, 0, 0, 0];
    let ptg_ext = {
        let mut v = vec![0x3A, 1, 0];
        v.extend(0u32.to_le_bytes());
        v.extend(0u16.to_le_bytes());
        v
    };
    let mut data = row(0);
    data.extend(fmla_num(0, 42.0, &ptg_ref3d_hidden));
    data.extend(fmla_num(1, 7.0, &ptg_name));
    data.extend(fmla_num(2, 99.0, &ptg_ext));

    let bytes = package(&wb_records, &worksheet(&data, &[]), &sst(), ROOT, RELS, TYPES);
    let (wb, report) = read_xlsb(&bytes).unwrap();
    assert_eq!(wb.names.len(), 1);
    assert_eq!((wb.names[0].name.as_str(), wb.names[0].formula.as_str(), wb.names[0].scope), ("Rate", "Hidden!$A$1", None));
    assert_eq!(wb.sheets[0].cell(CellRef::new(0, 0)).and_then(|c| c.formula.as_ref()).map(|f| f.text.as_str()), Some("Hidden!$A$1"));
    assert_eq!(wb.sheets[0].cell(CellRef::new(0, 1)).and_then(|c| c.formula.as_ref()).map(|f| f.text.as_str()), Some("Rate"));
    assert!(wb.sheets[0].cell(CellRef::new(0, 2)).is_some_and(|c| c.formula.is_none() && c.value == Value::Number(99.0)));
    assert!(report.warnings.iter().any(|w| w.contains("external link")), "{:?}", report.warnings);

    let (rt, _) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
    assert_eq!(rt.names.len(), 1);
    assert_eq!(rt.sheets[0].cell(CellRef::new(0, 0)).and_then(|c| c.formula.as_ref()).map(|f| f.text.as_str()), Some("Hidden!$A$1"));
    assert_eq!(rt.sheets[0].cell(CellRef::new(0, 1)).and_then(|c| c.formula.as_ref()).map(|f| f.text.as_str()), Some("Rate"));
    assert_eq!(rt.sheets[0].value(CellRef::new(0, 2)), Value::Number(99.0));
}

#[test]
fn xlsb_external_book_formulas_and_names_decode_and_roundtrip_through_xlsx() {
    let mut wb_records = record(131, &[]);
    let mut props = 0u32.to_le_bytes().to_vec();
    props.extend(0u32.to_le_bytes());
    props.extend(wide(""));
    wb_records.extend(record(153, &props));
    wb_records.extend(record(143, &[]));
    for (id, rel, name, state) in [(1u32, "one", "Data", 0u32), (2, "two", "Hidden", 1)] {
        let mut data = state.to_le_bytes().to_vec();
        data.extend(id.to_le_bytes());
        data.extend(wide(rel));
        data.extend(wide(name));
        wb_records.extend(record(156, &data));
    }
    wb_records.extend(record(144, &[]));
    // Supporting books: book 0 = BrtSupSelf (357), book 1 = BrtSupBookSrc (355, rel "ext1").
    wb_records.extend(record(357, &[]));
    wb_records.extend(record(355, &wide("ext1")));
    // BrtExternSheet (362): ixti 0 -> (book=1, sheet=0..0), ixti 1 -> (book=1, sheet=-2..-2).
    let mut xti = 2u32.to_le_bytes().to_vec();
    for (bk, first, last) in [(1u32, 0i32, 0i32), (1, -2, -2)] {
        xti.extend(bk.to_le_bytes());
        xti.extend(first.to_le_bytes());
        xti.extend(last.to_le_bytes());
    }
    wb_records.extend(record(362, &xti));
    wb_records.extend(record(132, &[]));

    // External link part (links/externalLink1.bin): BrtBeginSupBook (360), BrtSupTabs (359),
    // BrtSupNameStart/Bits/End (577/586/587), BrtEndSupBook (588).
    let mut ext_bin = Vec::new();
    let mut begin_sup = 0u16.to_le_bytes().to_vec();
    begin_sup.extend(wide("rId1"));
    begin_sup.extend(u32::MAX.to_le_bytes());
    ext_bin.extend(record(360, &begin_sup));
    let mut tabs = 2u32.to_le_bytes().to_vec();
    tabs.extend(wide("Data"));
    tabs.extend(wide("Other Sheet"));
    ext_bin.extend(record(359, &tabs));
    ext_bin.extend(record(577, &wide("ExtName")));
    // BrtSupNameFmla (585): ExtPtgRef3D (0x3A, sheet 0..0, r=0, c=0)
    let mut fmla_payload = 9u32.to_le_bytes().to_vec();
    fmla_payload.push(0x3A);
    fmla_payload.extend(0u16.to_le_bytes());
    fmla_payload.extend(0u16.to_le_bytes());
    fmla_payload.extend(0u16.to_le_bytes());
    fmla_payload.extend(0u16.to_le_bytes());
    ext_bin.extend(record(585, &fmla_payload));
    let mut bits = vec![0u8; 7];
    bits[2..6].copy_from_slice(&0u32.to_le_bytes());
    ext_bin.extend(record(586, &bits));
    ext_bin.extend(record(587, &[]));
    // Sheet-scoped external name ("LocalExt" on sheet 1 -> sheetId="0")
    ext_bin.extend(record(577, &wide("LocalExt")));
    let mut local_bits = vec![0u8; 7];
    local_bits[2..6].copy_from_slice(&1u32.to_le_bytes());
    ext_bin.extend(record(586, &local_bits));
    ext_bin.extend(record(587, &[]));
    // BrtExternTableStart (363) with refreshError=1, row 0, and all 5 cell types.
    let mut t_start = 0u32.to_le_bytes().to_vec();
    t_start.push(1);
    ext_bin.extend(record(363, &t_start));
    ext_bin.extend(record(366, &0u32.to_le_bytes()));
    ext_bin.extend(record(367, &0u32.to_le_bytes()));
    ext_bin.extend(record(368, &[&1u32.to_le_bytes()[..], &4.5f64.to_le_bytes()[..]].concat()));
    ext_bin.extend(record(369, &[&2u32.to_le_bytes()[..], &[1u8]].concat()));
    ext_bin.extend(record(370, &[&3u32.to_le_bytes()[..], &[0x07u8]].concat()));
    ext_bin.extend(record(371, &[&4u32.to_le_bytes()[..], &wide("hi")[..]].concat()));
    ext_bin.extend(record(364, &[]));
    ext_bin.extend(record(588, &[]));

    let fmla_num = |col: u32, cached: f64, rgce: &[u8]| {
        let mut data = col.to_le_bytes().to_vec();
        data.extend([0; 4]);
        data.extend(cached.to_le_bytes());
        data.extend([0; 2]);
        data.extend((rgce.len() as u32).to_le_bytes());
        data.extend(rgce);
        data.extend(0u32.to_le_bytes());
        record(9, &data)
    };
    let ptg_ext_ref = {
        let mut v = vec![0x3A, 0, 0];
        v.extend(0u32.to_le_bytes());
        v.extend(0u16.to_le_bytes());
        v
    };
    let ptg_ext_name = [0x39, 1, 0, 1, 0, 0, 0];
    let mut data = row(0);
    data.extend(fmla_num(0, 99.0, &ptg_ext_ref));
    data.extend(fmla_num(1, 12.5, &ptg_ext_name));

    let rels = RELS.replace(
        "</Relationships>",
        r#"<Relationship Id="ext1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/externalLink" Target="../links/externalLink1.bin"/></Relationships>"#,
    );
    let types =
        TYPES.replace("</Types>", r#"<Override PartName="/links/externalLink1.bin" ContentType="application/vnd.ms-excel.externalLink"/></Types>"#);
    let link_rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/externalLinkPath" Target="Book2.xlsx" TargetMode="External"/></Relationships>"#;
    let bytes = make_zip(&[
        ("[Content_Types].xml", types.as_bytes()),
        ("_rels/.rels", ROOT.as_bytes()),
        ("records/book.bin", &wb_records),
        ("records/_rels/book.bin.rels", rels.as_bytes()),
        ("data/one.bin", &worksheet(&data, &[])),
        ("data/two.bin", &worksheet(&[], &[])),
        ("text/strings.bin", &sst()),
        ("links/externalLink1.bin", &ext_bin),
        ("links/_rels/externalLink1.bin.rels", link_rels.as_bytes()),
    ]);

    let (wb, _) = read_xlsb(&bytes).unwrap();
    let c0 = wb.sheets[0].cell(CellRef::new(0, 0)).unwrap();
    assert_eq!(c0.value, Value::Number(99.0));
    assert_eq!(c0.formula.as_ref().map(|f| f.text.as_str()), Some("[1]Data!$A$1"));
    let c1 = wb.sheets[0].cell(CellRef::new(0, 1)).unwrap();
    assert_eq!(c1.value, Value::Number(12.5));
    assert_eq!(c1.formula.as_ref().map(|f| f.text.as_str()), Some("[1]!ExtName"));

    let saved = write_xlsx(&wb).unwrap();
    let (rt, _) = read_xlsx(&saved).unwrap();
    assert_eq!(rt.sheets[0].cell(CellRef::new(0, 0)).and_then(|c| c.formula.as_ref()).map(|f| f.text.as_str()), Some("[1]Data!$A$1"));
    assert_eq!(rt.sheets[0].value(CellRef::new(0, 0)), Value::Number(99.0));
    assert_eq!(rt.sheets[0].cell(CellRef::new(0, 1)).and_then(|c| c.formula.as_ref()).map(|f| f.text.as_str()), Some("[1]!ExtName"));
    assert_eq!(rt.sheets[0].value(CellRef::new(0, 1)), Value::Number(12.5));

    // Verify the external workbook target relationship survives XLSB -> XLSX -> read_xlsx -> XLSX.
    let saved2 = write_xlsx(&rt).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(saved2)).unwrap();
    let read_zip = |z: &mut zip::ZipArchive<std::io::Cursor<Vec<u8>>>, name: &str| {
        let mut s = String::new();
        std::io::Read::read_to_string(&mut z.by_name(name).unwrap(), &mut s).unwrap();
        s
    };
    let wb_xml = read_zip(&mut zip, "xl/workbook.xml");
    assert!(wb_xml.contains("<externalReferences"), "{wb_xml}");
    let wb_rels = read_zip(&mut zip, "xl/_rels/workbook.xml.rels");
    let link_target = wb_rels
        .split("<Relationship ")
        .find(|r| r.contains("externalLink\""))
        .and_then(|r| r.split("Target=\"").nth(1))
        .and_then(|s| s.split('"').next())
        .expect("workbook.xml.rels should have an externalLink relationship")
        .trim_start_matches('/')
        .to_string();
    let link_part = if link_target.starts_with("xl/") { link_target } else { format!("xl/{link_target}") };
    let link_xml = read_zip(&mut zip, &link_part);
    assert!(link_xml.contains("<sheetName val=\"Data\"/>") && link_xml.contains("name=\"ExtName\""), "{link_xml}");
    let (dir, file) = link_part.rsplit_once('/').unwrap();
    let link_rels_xml = read_zip(&mut zip, &format!("{dir}/_rels/{file}.rels"));
    assert!(
        link_rels_xml.contains("externalLinkPath")
            && link_rels_xml.contains("Target=\"Book2.xlsx\"")
            && link_rels_xml.contains("TargetMode=\"External\""),
        "{link_rels_xml}"
    );
}

#[test]
fn xlsb_external_link_ordinals_preserve_missing_slots_on_xlsx_save() {
    let mut wb_records = record(131, &[]);
    let mut props = 0u32.to_le_bytes().to_vec();
    props.extend(0u32.to_le_bytes());
    props.extend(wide(""));
    wb_records.extend(record(153, &props));
    wb_records.extend(record(143, &[]));
    let mut s1 = 0u32.to_le_bytes().to_vec();
    s1.extend(1u32.to_le_bytes());
    s1.extend(wide("one"));
    s1.extend(wide("Data"));
    wb_records.extend(record(156, &s1));
    wb_records.extend(record(144, &[]));
    // Book 0 = self, Book 1 = non-workbook DDE ("ext1"), Book 2 = valid workbook ("ext2").
    wb_records.extend(record(357, &[]));
    wb_records.extend(record(355, &wide("ext1")));
    wb_records.extend(record(355, &wide("ext2")));
    let mut xti = 2u32.to_le_bytes().to_vec();
    for (bk, first, last) in [(1u32, -2i32, -2i32), (2, 0, 0)] {
        xti.extend(bk.to_le_bytes());
        xti.extend(first.to_le_bytes());
        xti.extend(last.to_le_bytes());
    }
    wb_records.extend(record(362, &xti));
    wb_records.extend(record(132, &[]));

    // DDE link part (links/dde1.bin): sbt=1 ("Excel", "topic") with item "R1C1".
    let mut dde_bin = Vec::new();
    let mut dde_begin = 1u16.to_le_bytes().to_vec();
    dde_begin.extend(wide("Excel"));
    dde_begin.extend(wide("topic"));
    dde_bin.extend(record(360, &dde_begin));
    dde_bin.extend(record(577, &wide("R1C1")));
    dde_bin.extend(record(586, &[0u8; 7]));
    dde_bin.extend(record(587, &[]));
    dde_bin.extend(record(588, &[]));

    let mut ext_bin = Vec::new();
    let mut begin_sup = 0u16.to_le_bytes().to_vec();
    begin_sup.extend(wide("rId1"));
    begin_sup.extend(u32::MAX.to_le_bytes());
    ext_bin.extend(record(360, &begin_sup));
    let mut tabs = 1u32.to_le_bytes().to_vec();
    tabs.extend(wide("SheetA"));
    ext_bin.extend(record(359, &tabs));
    ext_bin.extend(record(588, &[]));

    let fmla_num = |col: u32, cached: f64, rgce: &[u8]| {
        let mut data = col.to_le_bytes().to_vec();
        data.extend([0; 4]);
        data.extend(cached.to_le_bytes());
        data.extend([0; 2]);
        data.extend((rgce.len() as u32).to_le_bytes());
        data.extend(rgce);
        data.extend(0u32.to_le_bytes());
        record(9, &data)
    };
    let ptg_1 = [0x39, 0, 0, 1, 0, 0, 0];
    let ptg_2 = {
        let mut v = vec![0x3A, 1, 0];
        v.extend(0u32.to_le_bytes());
        v.extend(0u16.to_le_bytes());
        v
    };
    let mut data = row(0);
    data.extend(fmla_num(0, 11.0, &ptg_1));
    data.extend(fmla_num(1, 22.0, &ptg_2));

    let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="one" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="../data/one.bin"/><Relationship Id="strings" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="../text/strings.bin"/><Relationship Id="ext1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/externalLink" Target="../links/dde1.bin"/><Relationship Id="ext2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/externalLink" Target="../links/externalLink2.bin"/></Relationships>"#;
    let types = r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/records/book.bin" ContentType="application/vnd.ms-excel.sheet.binary.macroEnabled.main"/><Override PartName="/data/one.bin" ContentType="application/vnd.ms-excel.worksheet"/><Override PartName="/text/strings.bin" ContentType="application/vnd.ms-excel.sharedStrings"/><Override PartName="/links/dde1.bin" ContentType="application/vnd.ms-excel.externalLink"/><Override PartName="/links/externalLink2.bin" ContentType="application/vnd.ms-excel.externalLink"/></Types>"#;
    let link_rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/externalLinkPath" Target="SecondBook.xlsx" TargetMode="External"/></Relationships>"#;
    let bytes = make_zip(&[
        ("[Content_Types].xml", types.as_bytes()),
        ("_rels/.rels", ROOT.as_bytes()),
        ("records/book.bin", &wb_records),
        ("records/_rels/book.bin.rels", rels.as_bytes()),
        ("data/one.bin", &worksheet(&data, &[])),
        ("text/strings.bin", &sst()),
        ("links/dde1.bin", &dde_bin),
        ("links/externalLink2.bin", &ext_bin),
        ("links/_rels/externalLink2.bin.rels", link_rels.as_bytes()),
    ]);

    let (wb, report) = read_xlsb(&bytes).unwrap();
    assert!(report.warnings.iter().any(|w| w.contains("unresolved or non-workbook external links")), "{:?}", report.warnings);
    assert!(report.warnings.iter().any(|w| w.contains("replaced with their cached values")), "{:?}", report.warnings);
    assert!(wb.sheets[0].cell(CellRef::new(0, 0)).is_some_and(|c| c.formula.is_none() && c.value == Value::Number(11.0)));
    assert_eq!(wb.sheets[0].cell(CellRef::new(0, 1)).and_then(|c| c.formula.as_ref()).map(|f| f.text.as_str()), Some("[1]SheetA!$A$1"));

    // Also verify numeric structured-table headers like `SUM(Sales[2024])` are not mistaken for
    // external ordinals when an omitted DDE link triggers ordinal remapping.
    assert_eq!(crate::xlsb::remap_external_formula("SUM(Sales[2024])", &[None, None, Some(1)]), Ok(None));
    assert_eq!(crate::xlsb::remap_external_formula("Sales[[#Headers],[2024]]", &[None, None, Some(1)]), Ok(None));
    assert_eq!(
        crate::xlsb::remap_external_formula("'[2]Other Sheet'!A1+SUM(Sales[2024])", &[None, None, Some(1)]),
        Ok(Some("'[1]Other Sheet'!A1+SUM(Sales[2024])".into()))
    );
    assert_eq!(crate::xlsb::remap_external_formula("[1]!'R1C1'+SUM(Sales[2024])", &[None, None, Some(1)]), Err(()));

    let saved = write_xlsx(&wb).unwrap();
    let (reopened, rep2) = read_xlsx(&saved).unwrap();
    assert!(!rep2.warnings.iter().any(|w| w.contains("dropped")), "{:?}", rep2.warnings);
    assert!(reopened.sheets[0].cell(CellRef::new(0, 0)).is_some_and(|c| c.formula.is_none() && c.value == Value::Number(11.0)));
    assert_eq!(reopened.sheets[0].cell(CellRef::new(0, 1)).and_then(|c| c.formula.as_ref()).map(|f| f.text.as_str()), Some("[1]SheetA!$A$1"));

    let saved2 = write_xlsx(&reopened).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(saved2)).unwrap();
    let read_zip = |z: &mut zip::ZipArchive<std::io::Cursor<Vec<u8>>>, name: &str| {
        let mut s = String::new();
        std::io::Read::read_to_string(&mut z.by_name(name).unwrap(), &mut s).unwrap();
        s
    };
    let wb_xml = read_zip(&mut zip, "xl/workbook.xml");
    let ref_ids: Vec<&str> =
        wb_xml.split("<externalReference ").skip(1).filter_map(|s| s.split("r:id=\"").nth(1).and_then(|t| t.split('"').next())).collect();
    assert_eq!(ref_ids.len(), 1, "only the surviving external link should be emitted: {wb_xml}");
    let wb_rels = read_zip(&mut zip, "xl/_rels/workbook.xml.rels");
    let link_target = wb_rels
        .split("<Relationship ")
        .find(|r| r.contains(&format!("Id=\"{}\"", ref_ids[0])))
        .and_then(|r| r.split("Target=\"").nth(1))
        .and_then(|s| s.split('"').next())
        .expect("externalReference r:id should resolve in workbook.xml.rels")
        .trim_start_matches('/');
    let link_part = if link_target.starts_with("xl/") { link_target.to_string() } else { format!("xl/{link_target}") };
    let link_xml = read_zip(&mut zip, &link_part);
    assert!(link_xml.contains("<externalBook ") && link_xml.contains("<sheetName val=\"SheetA\"/>"), "{link_xml}");
    let (dir, file) = link_part.rsplit_once('/').unwrap();
    let link_rels = read_zip(&mut zip, &format!("{dir}/_rels/{file}.rels"));
    assert!(
        link_rels.contains("externalLinkPath") && link_rels.contains("Target=\"SecondBook.xlsx\"") && link_rels.contains("TargetMode=\"External\""),
        "{link_rels}"
    );
}
