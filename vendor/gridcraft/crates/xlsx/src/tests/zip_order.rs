//! libmagic (PHP `finfo`, `file`) detects an OOXML package from the first zip entries (#1090).

use gridcraft_model::Workbook;

use crate::write_xlsx;

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

#[test]
fn package_starts_with_content_types_rels_and_workbook_before_any_large_part() {
    let mut wb = Workbook::new();
    // A long, hard-to-compress description makes docProps/core.xml big.
    wb.props.description =
        (0..20_000).map(|i| char::from(b'a' + (i * 7 % 26) as u8)).chain((0..20_000).map(|i| char::from(b'0' + (i * i % 10) as u8))).collect();
    let bytes = write_xlsx(&wb).unwrap();

    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
    let names: Vec<String> = (0..zip.len()).filter_map(|i| zip.by_index(i).ok().map(|f| f.name().to_string())).collect();
    assert_eq!(names.first().map(String::as_str), Some("[Content_Types].xml"), "{names:?}");
    assert_eq!(names.get(1).map(String::as_str), Some("_rels/.rels"), "{names:?}");
    assert_eq!(names.get(2).map(String::as_str), Some("xl/_rels/workbook.xml.rels"), "{names:?}");
    assert_eq!(names.get(3).map(String::as_str), Some("xl/workbook.xml"), "{names:?}");

    // No data descriptor (general-purpose bit 3) on the first local header, entry name at offset 30.
    assert_eq!(bytes.get(..4), Some(&b"PK\x03\x04"[..]));
    assert_eq!(bytes.get(6).map(|b| b & 0x08), Some(0));
    assert_eq!(bytes.get(30..49), Some(&b"[Content_Types].xml"[..]));
    // libmagic looks for an `xl/` entry name within the first few KB: keep it well inside that.
    let at = find(&bytes, b"xl/_rels/workbook.xml.rels");
    assert!(at.is_some_and(|p| p < 2048), "xl/ entry at {at:?}");
}
