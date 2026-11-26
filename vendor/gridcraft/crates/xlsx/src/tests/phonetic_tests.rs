//! Japanese phonetic runs (`rPh`, furigana) of shared strings survive an open + save.
//!
//! Behaviour reference: ISO/IEC 29500-1 §18.4.6 `rPh` (`sb`/`eb` are zero-based offsets into the
//! base text, `sb < eb`, child `<t>`) and §18.4.7 `phoneticPr` (`fontId`, `type`, `alignment`).

use gridcraft_core::{CellRef, Value};
use gridcraft_model::{Cell, PhoneticProps, PhoneticRun, PhoneticString, StyleId, Workbook};

use super::minimal;
use crate::package::Package;
use crate::{read_xlsx, write_xlsx};

/// A string item with two `<rPh>` runs and a `<phoneticPr>`, and one with no phonetic data.
const SST: &str = r#"<?xml version="1.0"?><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="2" uniqueCount="2"><si><t>東京都</t><rPh sb="0" eb="2"><t>トウキョウ</t></rPh><rPh sb="2" eb="3"><t>ト &amp; &lt;x&gt;</t></rPh><phoneticPr fontId="1" type="noConversion" alignment="center"/></si><si><t>大阪</t></si></sst>"#;

const SHEET_BODY: &str = r#"<sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c></row></sheetData>"#;

const SST_RELS: &str = r#"<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/>"#;

fn at(a: &str) -> CellRef {
    CellRef::parse(a).unwrap()
}

/// A one-sheet package whose shared string table is `sst`.
fn package(sst: &str) -> Vec<u8> {
    minimal(SHEET_BODY, &[("xl/sharedStrings.xml", sst)], SST_RELS, "")
}

/// A part of a written package, as text.
fn part(bytes: &[u8], name: &str) -> String {
    let mut pkg = Package::open(bytes).unwrap();
    String::from_utf8(pkg.read(name).unwrap().unwrap()).unwrap()
}

fn text_cell(sheet: &mut gridcraft_model::Sheet, a: &str, text: &str) {
    sheet.cells.set(at(a), Cell { value: Value::text(text), formula: None, style: StyleId::DEFAULT, py: None });
}

#[test]
fn phonetic_runs_and_props_survive_open_and_save() {
    let (wb, report) = read_xlsx(&package(SST)).unwrap();
    assert!(!report.warnings.iter().any(|w| w.contains("phonetic")), "{:?}", report.warnings);
    let sheet = wb.sheet(0).unwrap();
    // A reading is a hint, not part of the text.
    assert_eq!(sheet.value(at("A1")), Value::text("東京都"));
    assert_eq!(sheet.value(at("B1")), Value::text("大阪"));
    assert_eq!(
        wb.phonetics,
        vec![PhoneticString {
            text: "東京都".into(),
            runs: vec![PhoneticRun { start: 0, end: 2, text: "トウキョウ".into() }, PhoneticRun { start: 2, end: 3, text: "ト & <x>".into() },],
            props: Some(PhoneticProps { font_id: 1, kind: Some("noConversion".into()), alignment: Some("center".into()) }),
        }]
    );

    let bytes = write_xlsx(&wb).unwrap();
    let sst = part(&bytes, "xl/sharedStrings.xml");
    assert!(
        sst.contains(
            r#"<si><t>東京都</t><rPh sb="0" eb="2"><t>トウキョウ</t></rPh><rPh sb="2" eb="3"><t>ト &amp; &lt;x&gt;</t></rPh><phoneticPr fontId="1" type="noConversion" alignment="center"/></si>"#
        ),
        "{sst}"
    );
    assert!(sst.contains("<si><t>大阪</t></si>"), "a string without phonetic data stays plain: {sst}");

    // A second open + save keeps the runs and reproduces the same part.
    let (again, report) = read_xlsx(&bytes).unwrap();
    assert_eq!(again.phonetics, wb.phonetics);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(part(&write_xlsx(&again).unwrap(), "xl/sharedStrings.xml"), sst);
}

#[test]
fn a_plain_japanese_string_writes_the_plain_si() {
    let mut wb = Workbook::new();
    text_cell(wb.sheet_mut(0).unwrap(), "A1", "日本語");
    let bytes = write_xlsx(&wb).unwrap();
    let sst = part(&bytes, "xl/sharedStrings.xml");
    assert!(sst.contains("<si><t>日本語</t></si>"), "{sst}");
    assert!(!sst.contains("<rPh") && !sst.contains("<phoneticPr"), "{sst}");
}

#[test]
fn property_only_shared_string_survives_open_and_save() {
    // `phoneticPr` without any `rPh` is valid (a shared string item permits it on its own), so
    // open + save must not delete it.
    let sst = r#"<?xml version="1.0"?><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="1" uniqueCount="1"><si><t>東京</t><phoneticPr fontId="2" type="Hiragana" alignment="left"/></si></sst>"#;
    let body = r#"<sheetData><row r="1"><c r="A1" t="s"><v>0</v></c></row></sheetData>"#;
    let bytes = minimal(body, &[("xl/sharedStrings.xml", sst)], SST_RELS, "");
    let (wb, report) = read_xlsx(&bytes).unwrap();
    assert!(!report.warnings.iter().any(|w| w.contains("phonetic")), "{:?}", report.warnings);
    assert_eq!(
        wb.phonetics,
        vec![PhoneticString {
            text: "東京".into(),
            runs: vec![],
            props: Some(PhoneticProps { font_id: 2, kind: Some("Hiragana".into()), alignment: Some("left".into()) }),
        }]
    );
    let written = write_xlsx(&wb).unwrap();
    let out = part(&written, "xl/sharedStrings.xml");
    assert!(out.contains(r#"<si><t>東京</t><phoneticPr fontId="2" type="Hiragana" alignment="left"/></si>"#), "{out}");
    // A second open + save reproduces the same part.
    let (again, report) = read_xlsx(&written).unwrap();
    assert_eq!(again.phonetics, wb.phonetics);
    assert!(!report.warnings.iter().any(|w| w.contains("phonetic")), "{:?}", report.warnings);
    assert_eq!(part(&write_xlsx(&again).unwrap(), "xl/sharedStrings.xml"), out);
}

#[test]
fn hostile_phonetic_runs_lose_only_the_runs() {
    // Missing `eb`, missing `sb`, `sb >= eb`, offsets past the text, junk offsets, no offsets at
    // all, no `<t>`, an empty `<t>`, and junk `phoneticPr` attributes.
    let sst = r#"<?xml version="1.0"?><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="1" uniqueCount="1"><si junk="1"><t>課きく</t><rPh sb="1"><t>no eb</t></rPh><rPh eb="2"><t>no sb</t></rPh><rPh sb="3" eb="1"><t>reversed</t></rPh><rPh sb="0" eb="99"><t>clamped</t></rPh><rPh sb="junk" eb="1"><t>junk</t></rPh><rPh><t>no offsets</t></rPh><rPh sb="0" eb="1" junk="2"/><rPh sb="0" eb="1"><t/></rPh><phoneticPr fontId="junk" type="junk" alignment="junk"/></si></sst>"#;
    let (wb, report) = read_xlsx(&package(sst)).unwrap();
    // The base text is intact and only the one run that can be clamped is kept.
    assert_eq!(wb.sheet(0).unwrap().value(at("A1")), Value::text("課きく"));
    assert_eq!(
        wb.phonetics,
        vec![PhoneticString {
            text: "課きく".into(),
            runs: vec![PhoneticRun { start: 0, end: 3, text: "clamped".into() }],
            props: Some(PhoneticProps::default()),
        }]
    );
    assert!(report.warnings.iter().any(|w| w.contains("phonetic runs were dropped")), "{:?}", report.warnings);

    // What is written back is valid: offsets within the text, a fontId on every `<phoneticPr>`.
    let bytes = write_xlsx(&wb).unwrap();
    let out = part(&bytes, "xl/sharedStrings.xml");
    assert!(out.contains(r#"<si><t>課きく</t><rPh sb="0" eb="3"><t>clamped</t></rPh><phoneticPr fontId="0"/></si>"#), "{out}");
    assert_eq!(read_xlsx(&bytes).unwrap().0.phonetics, wb.phonetics);
}

#[test]
fn identical_text_with_different_readings_keeps_the_first_and_warns() {
    let sst = r#"<?xml version="1.0"?><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="2" uniqueCount="2"><si><t>東</t><rPh sb="0" eb="1"><t>ヒガシ</t></rPh></si><si><t>東</t><rPh sb="0" eb="1"><t>アズマ</t></rPh></si></sst>"#;
    let (wb, report) = read_xlsx(&package(sst)).unwrap();
    // The saved table holds each text once, so only one reading can be written: the first.
    assert_eq!(wb.phonetics.len(), 1);
    assert_eq!(wb.phonetics[0].runs.first().map(|r| r.text.as_str()), Some("ヒガシ"));
    assert!(report.warnings.iter().any(|w| w.contains("phonetic readings")), "{:?}", report.warnings);

    // The same text twice with the same reading is not a conflict.
    let same = sst.replace("アズマ", "ヒガシ");
    let (wb, report) = read_xlsx(&package(&same)).unwrap();
    assert_eq!(wb.phonetics.len(), 1);
    assert!(!report.warnings.iter().any(|w| w.contains("phonetic readings")), "{:?}", report.warnings);
}

#[test]
fn phonetic_data_never_used_by_a_cell_is_not_written() {
    let mut wb = Workbook::new();
    text_cell(wb.sheet_mut(0).unwrap(), "A1", "日本語");
    wb.phonetics =
        vec![PhoneticString { text: "別の文字列".into(), runs: vec![PhoneticRun { start: 0, end: 2, text: "ベツ".into() }], props: None }];
    let bytes = write_xlsx(&wb).unwrap();
    let sst = part(&bytes, "xl/sharedStrings.xml");
    assert!(sst.contains("<si><t>日本語</t></si>"), "{sst}");
    assert!(!sst.contains("<rPh"), "{sst}");
    let (back, report) = read_xlsx(&bytes).unwrap();
    assert!(back.phonetics.is_empty());
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
}

#[test]
fn comments_and_inline_strings_still_ignore_phonetic_runs() {
    let body = r#"<sheetData><row r="1"><c r="A1" t="inlineStr"><is><r><t>inline</t></r><rPh sb="0" eb="6"><t>インライン</t></rPh><phoneticPr fontId="1"/></is></c></row></sheetData>"#;
    let sheet_rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments" Target="../comments1.xml"/></Relationships>"#;
    let comments = r#"<comments xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><authors><author>Ann</author></authors><commentList><comment ref="A1" authorId="0"><text><r><rPr><b/></rPr><t>Ann:</t></r><r><t>メモ</t></r><rPh sb="0" eb="2"><t>メモ</t></rPh><phoneticPr fontId="1"/></text></comment></commentList></comments>"#;
    let bytes = minimal(body, &[("xl/worksheets/_rels/sheet1.xml.rels", sheet_rels), ("xl/comments1.xml", comments)], "", "");
    let (wb, _) = read_xlsx(&bytes).unwrap();
    let sheet = wb.sheet(0).unwrap();
    assert_eq!(sheet.value(at("A1")), Value::text("inline"));
    assert_eq!(sheet.comments.get(&at("A1")).map(|c| c.text.as_str()), Some("メモ"));
    // Neither path has a shared string table entry to key phonetic data by.
    assert!(wb.phonetics.is_empty());
}
