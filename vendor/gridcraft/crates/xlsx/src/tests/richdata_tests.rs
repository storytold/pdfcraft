//! Original images and hand-written package metadata independent of the writer.

use std::sync::Arc;

use gridcraft_core::{CellError, CellRef, RangeRef, Value};
use gridcraft_model::{Cell, CellPicture, Formula, Sheet, Workbook};

use super::make_zip;
use crate::package::Package;
use crate::{IoError, read_xlsx, write_xlsx};

fn at(value: &str) -> CellRef {
    CellRef::parse(value).unwrap()
}

/// An original 1x1 green RGB PNG. Stored DEFLATE block, Adler-32, and PNG CRC-32.
fn png() -> Vec<u8> {
    fn chunk(out: &mut Vec<u8>, kind: &[u8], body: &[u8]) {
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        let mut crc = !0u32;
        for &byte in kind.iter().chain(body) {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb88320 & 0u32.wrapping_sub(crc & 1));
            }
        }
        out.extend_from_slice(&(!crc).to_be_bytes());
    }
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut png, b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0]);
    let pixel = [0u8, 28, 170, 72];
    let mut zlib = vec![0x78, 0x01, 1, 4, 0, 0xfb, 0xff];
    zlib.extend_from_slice(&pixel);
    let (mut a, mut b) = (1u32, 0u32);
    for n in pixel {
        a += u32::from(n);
        b += a;
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());
    chunk(&mut png, b"IDAT", &zlib);
    chunk(&mut png, b"IEND", &[]);
    png
}

/// Original one-pixel baseline gray JPEG: zero DCT coefficients, tiny Huffman tables.
fn jpeg() -> Vec<u8> {
    fn segment(data: &mut Vec<u8>, marker: u8, payload: &[u8]) {
        data.extend_from_slice(&[0xff, marker]);
        data.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
        data.extend_from_slice(payload);
    }
    let mut jpeg = vec![0xff, 0xd8];
    let mut quant = vec![0];
    quant.extend_from_slice(&[1; 64]);
    segment(&mut jpeg, 0xdb, &quant);
    segment(&mut jpeg, 0xc0, &[8, 0, 1, 0, 1, 1, 1, 0x11, 0]);
    let mut huffman = Vec::new();
    for table in [0, 0x10] {
        huffman.extend_from_slice(&[table, 1]);
        huffman.extend_from_slice(&[0; 16]);
    }
    segment(&mut jpeg, 0xc4, &huffman);
    segment(&mut jpeg, 0xda, &[1, 1, 0, 0, 63, 0]);
    jpeg.extend_from_slice(&[0x3f, 0xff, 0xd9]);
    jpeg
}

fn picture(data: Vec<u8>, mime: &str, alt: &str) -> CellPicture {
    CellPicture { data, mime: mime.into(), alt: alt.into() }
}

/// A1 vm2 -> block1/type2/v0 -> future0 -> rv1 -> structure1 -> relation1.
/// The deliberately different indices catch accidentally using vm directly as an image ID.
fn independent_parts(chain: bool) -> Vec<(String, Vec<u8>)> {
    let mut parts = Vec::new();
    let mut add = |name: &str, xml: &str| parts.push((name.to_string(), xml.as_bytes().to_vec()));
    add("[Content_Types].xml", super::CT);
    add(
        "_rels/.rels",
        r#"<Relationships><Relationship Id="book" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="pkg/book.xml"/></Relationships>"#,
    );
    add(
        "pkg/book.xml",
        r#"<workbook xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Pictures" sheetId="1" r:id="sheet"/></sheets></workbook>"#,
    );
    let rich =
        r#"<Relationship Id="value" Type="http://schemas.microsoft.com/office/2017/06/relationships/rdRichValue" Target="../rich/values.xml"/>"#;
    let rest = r#"<Relationship Id="structure" Type="http://schemas.microsoft.com/office/2017/06/relationships/rdRichValueStructure" Target="../rich/structures.xml"/><Relationship Id="images" Type="http://schemas.microsoft.com/office/2022/10/relationships/richValueRel" Target="../rich/images.xml"/>"#;
    add(
        "pkg/_rels/book.xml.rels",
        &format!(
            r#"<Relationships><Relationship Id="sheet" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="../sheets/data.xml"/><Relationship Id="metadata" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sheetMetadata" Target="../meta/state.xml"/>{}</Relationships>"#,
            if chain { String::new() } else { format!("{rich}{rest}") }
        ),
    );
    if chain {
        add("meta/_rels/state.xml.rels", &format!("<Relationships>{rich}</Relationships>"));
        add("rich/_rels/values.xml.rels", &format!("<Relationships>{rest}</Relationships>"));
    }
    add(
        "sheets/data.xml",
        r#"<worksheet><sheetData><row r="1"><c r="A1" t="e" vm="2"><v>#VALUE!</v></c><c r="B1" cm="1"><f t="array" ref="B1:B2">_xlfn.SEQUENCE(2)</f><v>1</v></c></row><row r="2"><c r="B2"><v>2</v></c></row></sheetData></worksheet>"#,
    );
    add(
        "meta/state.xml",
        r#"<metadata xmlns:rv="http://schemas.microsoft.com/office/spreadsheetml/2017/richdata"><metadataTypes count="2"><metadataType name="XLDAPR" cellMeta="1"/><metadataType name="XLRICHVALUE"/></metadataTypes><futureMetadata name="XLDAPR" count="1"><bk/></futureMetadata><futureMetadata name="XLRICHVALUE" count="2"><bk><extLst><ext uri="{3E2802C4-A4D2-4D8B-9148-E3BE6C30E623}"><rv:rvb i="1"/></ext></extLst></bk><bk><extLst><ext uri="{3E2802C4-A4D2-4D8B-9148-E3BE6C30E623}"><rv:rvb i="0"/></ext></extLst></bk></futureMetadata><cellMetadata count="1"><bk><rc t="1" v="0"/></bk></cellMetadata><valueMetadata count="2"><bk><rc t="1" v="0"/></bk><bk><rc t="1" v="0"/><rc t="2" v="0"/></bk></valueMetadata></metadata>"#,
    );
    add(
        "rich/values.xml",
        r#"<rvData count="2"><rv s="0"><v>unused</v></rv><rv s="1"><v>green &amp; &lt;image&gt;</v><v>1</v><v>5</v></rv></rvData>"#,
    );
    add(
        "rich/structures.xml",
        r#"<rvStructures count="2"><s t="unsupported"><k n="value" t="s"/></s><s t="_localImage"><k n="Text" t="s"/><k n="_rvRel:LocalImageIdentifier" t="i"/><k n="CalcOrigin" t="i"/></s></rvStructures>"#,
    );
    add(
        "rich/images.xml",
        r#"<richValueRels xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><rel r:id="unused"/><rel r:id="opaqueImageId"/></richValueRels>"#,
    );
    add(
        "rich/_rels/images.xml.rels",
        r#"<Relationships><Relationship Id="opaqueImageId" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/original.bin"/></Relationships>"#,
    );
    parts.push(("media/original.bin".into(), png()));
    parts
}

fn zip(parts: &[(String, Vec<u8>)]) -> Vec<u8> {
    make_zip(&parts.iter().map(|(name, bytes)| (name.as_str(), bytes.as_slice())).collect::<Vec<_>>())
}

fn replace(parts: &mut [(String, Vec<u8>)], name: &str, from: &str, to: &str) {
    let (_, bytes) = parts.iter_mut().find(|(n, _)| n == name).unwrap();
    let text = std::str::from_utf8(bytes).unwrap().replace(from, to);
    *bytes = text.into_bytes();
}

#[test]
fn reads_independent_indices_reordered_keys_and_both_relationship_layouts() {
    for chain in [false, true] {
        let (wb, _) = read_xlsx(&zip(&independent_parts(chain))).unwrap();
        let sheet = wb.sheet(0).unwrap();
        let cell = sheet.cells.get(at("A1")).unwrap();
        assert_eq!(sheet.cell_pictures.get(&at("A1")).map(Arc::as_ref), Some(&picture(png(), "image/png", "green & <image>")));
        assert_eq!(cell.value, Value::Error(CellError::Value));
        assert!(cell.formula.is_none());
        assert!(sheet.cells.get(at("B1")).unwrap().formula.as_ref().unwrap().array.is_none());
        assert!(sheet.cells.get(at("B2")).is_none());
    }
}

#[test]
fn dynamic_array_metadata_is_spelled_the_same_with_and_without_pictures() {
    // #2339: with an in-cell picture the XLDAPR extension URI was written in upper case, and
    // Excel refused the workbook; the dynamic-array-only metadata, which Excel opens, is lower case.
    let xldapr_uri = |pictures: bool| {
        let mut wb = Workbook::new();
        let sheet = wb.sheet_mut(0).unwrap();
        if pictures {
            sheet.set_picture(at("B2"), Arc::new(picture(png(), "image/png", "red")));
        }
        let mut f = Cell::formula(Formula::new("SEQUENCE(3)"));
        f.value = Value::Number(1.0);
        sheet.cells.set(at("D1"), f);
        sheet.spill_ranges.insert(at("D1"), RangeRef::parse("D1:D3").unwrap());
        let bytes = write_xlsx(&wb).unwrap();
        let mut package = Package::open(&bytes).unwrap();
        let metadata = package.read_xml("xl/metadata.xml").unwrap().unwrap();
        let future = metadata.kids("futureMetadata").find(|m| m.attr("name") == Some("XLDAPR")).unwrap();
        future.child("bk").unwrap().child("extLst").unwrap().child("ext").unwrap().attr("uri").unwrap().to_string()
    };
    assert_eq!(xldapr_uri(false), "{bdbb8cdc-fa1e-496e-a857-3c3f30c029c3}");
    assert_eq!(xldapr_uri(true), xldapr_uri(false));
}

#[test]
fn pictures_roundtrip_across_sheets_with_and_without_dynamic_arrays() {
    for dynamic in [false, true] {
        let mut wb = Workbook::new();
        let first = Arc::new(picture(png(), "image/png", "green & <a> Ω\nline"));
        wb.sheet_mut(0).unwrap().set_picture(at("B2"), first.clone());
        wb.sheet_mut(0).unwrap().set_picture(at("A4"), Arc::new(picture(jpeg(), "image/jpeg", "gray")));
        let mut second = Sheet::new("Second");
        second.set_picture(at("C7"), first.clone());
        wb.sheets.push(Arc::new(second));
        if dynamic {
            let sheet = wb.sheet_mut(0).unwrap();
            let mut f = Cell::formula(Formula::new("SEQUENCE(2)"));
            f.value = Value::Number(1.0);
            sheet.cells.set(at("E1"), f);
            sheet.spill_ranges.insert(at("E1"), RangeRef::parse("E1:E2").unwrap());
            sheet.spill.insert(at("E2"), Value::Number(2.0));
        }
        let bytes = write_xlsx(&wb).unwrap();
        let mut package = Package::open(&bytes).unwrap();
        let metadata = package.read_xml("xl/metadata.xml").unwrap().unwrap();
        assert_eq!(metadata.child("metadataTypes").unwrap().attr_u32("count"), Some(if dynamic { 2 } else { 1 }));
        assert_eq!(metadata.child("valueMetadata").unwrap().attr_u32("count"), Some(2));
        let (loaded, report) = read_xlsx(&bytes).unwrap();
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert_eq!(loaded.sheet(0).unwrap().cell_pictures.get(&at("B2")), Some(&first));
        assert_eq!(loaded.sheet(1).unwrap().cell_pictures.get(&at("C7")), Some(&first));
        assert_eq!(loaded.sheet(0).unwrap().cell_pictures.get(&at("A4")).unwrap().data, jpeg());
        if dynamic {
            let f = loaded.sheet(0).unwrap().cells.get(at("E1")).unwrap().formula.as_ref().unwrap();
            assert_eq!(f.text, "SEQUENCE(2)");
            assert!(f.array.is_none());
        }
        let rels = package.rels("xl/workbook.xml").unwrap();
        assert!(rels.iter().any(|r| r.kind == "rdRichValue"));
        assert!(rels.iter().any(|r| r.kind == "richValueRel"));
        let sheet = package.read_xml("xl/worksheets/sheet1.xml").unwrap().unwrap();
        let picture_cell = sheet.child("sheetData").unwrap().kids("row").flat_map(|row| row.kids("c")).find(|c| c.attr("r") == Some("B2")).unwrap();
        assert_eq!(picture_cell.attr("vm"), Some("1"));
        assert_eq!(picture_cell.attr("t"), Some("e"));
        assert_eq!(picture_cell.child("v").unwrap().text, "#VALUE!");
    }
}

#[test]
fn broken_picture_references_keep_the_scalar_fallback_with_warnings() {
    for (part, from, to) in [
        ("sheets/data.xml", "vm=\"2\"", "vm=\"999\""),
        ("meta/state.xml", "<rc t=\"2\" v=\"0\"/>", "<rc t=\"2\" v=\"99\"/>"),
        ("meta/state.xml", "<rv:rvb i=\"1\"/>", "<rv:rvb i=\"99\"/>"),
        ("rich/values.xml", "<rv s=\"1\">", "<rv s=\"99\">"),
        ("rich/values.xml", "<v>1</v>", "<v>99</v>"),
        ("rich/_rels/images.xml.rels", "Target=\"../media/original.bin\"", "Target=\"https://example.invalid/image.png\" TargetMode=\"External\""),
        ("rich/_rels/images.xml.rels", "original.bin", "missing.bin"),
        ("rich/structures.xml", "t=\"_localImage\"", "t=\"unsupportedImage\""),
        ("rich/images.xml", "opaqueImageId", "missingImageId"),
    ] {
        let mut parts = independent_parts(false);
        replace(&mut parts, part, from, to);
        let (wb, report) = read_xlsx(&zip(&parts)).unwrap();
        let cell = wb.sheet(0).unwrap().cells.get(at("A1")).unwrap();
        assert!(!wb.sheet(0).unwrap().cell_pictures.contains_key(&at("A1")), "{part}: {to}");
        assert_eq!(cell.value, Value::Error(CellError::Value));
        assert!(report.warnings.iter().any(|w| w.contains("picture")), "{:?}", report.warnings);
    }
}

#[test]
fn hostile_image_bytes_and_dimensions_are_bounded() {
    let mut parts = independent_parts(false);
    let (_, image) = parts.iter_mut().find(|(n, _)| n == "media/original.bin").unwrap();
    image[16..20].copy_from_slice(&u32::MAX.to_be_bytes());
    let (wb, report) = read_xlsx(&zip(&parts)).unwrap();
    assert!(!wb.sheet(0).unwrap().cell_pictures.contains_key(&at("A1")));
    assert!(report.warnings.iter().any(|w| w.contains("pixels")));
    let (_, image) = parts.iter_mut().find(|(n, _)| n == "media/original.bin").unwrap();
    image.resize(16 * 1024 * 1024 + 1, 0);
    let (wb, report) = read_xlsx(&zip(&parts)).unwrap();
    assert!(!wb.sheet(0).unwrap().cell_pictures.contains_key(&at("A1")));
    assert!(report.warnings.iter().any(|w| w.contains("oversized")));
    let mut wb = Workbook::new();
    wb.sheet_mut(0).unwrap().set_picture(at("A1"), Arc::new(picture(vec![1, 2, 3], "image/png", "invalid")));
    assert!(matches!(write_xlsx(&wb), Err(IoError::Format(_))));
}

/// Advertise a large decompressed size without allocating a large fixture. Package reads
/// must reject the header before touching the (deliberately inconsistent) compressed data.
fn oversized_entry(mut bytes: Vec<u8>, path: &str, size: u32) -> Vec<u8> {
    let mut i = 0;
    while i + 46 <= bytes.len() {
        if bytes[i..].starts_with(b"PK\x01\x02") {
            let len = usize::from(u16::from_le_bytes([bytes[i + 28], bytes[i + 29]]));
            if bytes.get(i + 46..i + 46 + len) == Some(path.as_bytes()) {
                bytes[i + 24..i + 28].copy_from_slice(&size.to_le_bytes());
                return bytes;
            }
        }
        i += 1;
    }
    panic!("fixture entry not found");
}

#[test]
fn picture_metadata_limits_and_relationship_read_errors_do_not_block_workbooks() {
    let mut parts = independent_parts(false);
    replace(&mut parts, "sheets/data.xml", "</sheetData>", "<row r=\"3\"><c r=\"C3\"><v>42</v></c></row></sheetData>");
    let oversized_xml = oversized_entry(zip(&parts), "rich/values.xml", 16 * 1024 * 1024 + 1);
    let broken_rels = oversized_entry(zip(&parts), "rich/_rels/images.xml.rels", 512 * 1024 * 1024 + 1);
    let (_, values) = parts.iter_mut().find(|(path, _)| path == "rich/values.xml").unwrap();
    *values = format!("<rvData count=\"100001\">{}</rvData>", "<rv/>".repeat(100_001)).into_bytes();
    for bytes in [oversized_xml, broken_rels, zip(&parts)] {
        let (wb, report) = read_xlsx(&bytes).expect("optional picture data must not block opening");
        let sh = wb.sheet(0).unwrap();
        assert!(sh.cell_pictures.is_empty());
        assert_eq!(sh.value(at("A1")), Value::Error(CellError::Value));
        assert_eq!(sh.value(at("C3")), Value::Number(42.0));
        assert!(report.warnings.iter().any(|w| w.contains("cell-picture data could not be read")), "{:?}", report.warnings);
    }
}

/// Many values over one wide structure, and a repeated relationship id: the structure's layout
/// is worked out once and relationships are indexed, so the import is linear in the records;
/// the repeated id is invalid and keeps the scalar fallback.
#[test]
fn wide_structures_and_repeated_relationship_ids_are_resolved_through_indexes() {
    let mut parts = independent_parts(false);
    // 200,000 keys in one structure and 50,000 short values that name it: rebuilding the key
    // list per value would be ten billion steps.
    let padding = r#"<k n="pad" t="s"/>"#.repeat(200_000);
    replace(&mut parts, "rich/structures.xml", r#"<k n="CalcOrigin" t="i"/>"#, &format!(r#"<k n="CalcOrigin" t="i"/>{padding}"#));
    // The count is right, and the one valid picture record comes after the repeated values (cell
    // A1 is pointed at it), so an early read failure can't pass for the malformed-value warning.
    let valid = r#"<rv s="1"><v>green &amp; &lt;image&gt;</v><v>1</v><v>5</v></rv>"#;
    let values = r#"<rv s="1"><v/></rv>"#.repeat(50_000);
    // The valid record supplies a value for each of the structure's padding keys.
    let wide_valid = valid.replace("</rv>", &format!("{}</rv>", "<v>0</v>".repeat(200_000)));
    replace(&mut parts, "rich/values.xml", valid, &format!("{values}{wide_valid}"));
    replace(&mut parts, "rich/values.xml", r#"<rvData count="2">"#, r#"<rvData count="50002">"#);
    replace(&mut parts, "meta/state.xml", r#"<rv:rvb i="1"/>"#, r#"<rv:rvb i="50001"/>"#);
    let (wb, report) = read_xlsx(&zip(&parts)).unwrap();
    assert!(wb.sheet(0).unwrap().cell_pictures.contains_key(&at("A1")), "{:?}", report.warnings);
    assert!(report.warnings.iter().all(|w| !w.contains("count")), "{:?}", report.warnings);
    // The same id twice makes every reference to it invalid.
    let mut parts = independent_parts(false);
    let dup = r#"<Relationship Id="opaqueImageId" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/original.bin"/>"#;
    replace(&mut parts, "rich/_rels/images.xml.rels", dup, &dup.repeat(2));
    let (wb, report) = read_xlsx(&zip(&parts)).unwrap();
    assert!(!wb.sheet(0).unwrap().cell_pictures.contains_key(&at("A1")));
    assert!(report.warnings.iter().any(|w| w.contains("invalid image relationship")), "{:?}", report.warnings);
}
