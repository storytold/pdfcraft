//! Integration-style tests: round trips, hand-written fixtures, hostile input and CSV.

mod array_export;
mod cellmetadata;
mod chart_book_names;
mod chart_bool_default;
mod chart_combo_kinds;
mod chart_data_table;
mod chart_ref_offsets;
mod controls_tests;
mod csv_tests;
mod encrypted_tests;
mod equation_tests;
mod fixtures;
mod google_sheets_dummy;
mod ink_roundtrip;
mod literal_category_roundtrip;
mod locale_numfmt;
mod malformed;
mod multilevel_category_roundtrip;
mod note_box;
mod ods_tests;
mod page_order;
mod passthrough_tests;
mod phonetic_tests;
mod pivot_tests;
mod richdata_tests;
mod roundtrip;
mod shape_text_rtl;
mod shared_media;
mod sheet_data_pin;
mod sheet_names;
mod smooth_roundtrip;
mod trendline_roundtrip;
mod xls_dv_tests;
mod xls_style_tests;
mod xls_tests;
mod xlsb_tests;
mod zip_order;

use std::io::Write;

/// Builds a zip package from (name, content) pairs.
pub fn make_zip(parts: &[(&str, &[u8])]) -> Vec<u8> {
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, data) in parts {
        z.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
        z.write_all(data).unwrap();
    }
    z.finish().unwrap().into_inner()
}

pub const CT: &str = r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/></Types>"#;
pub const ROOT_RELS: &str = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#;

/// A minimal one-sheet package with the given sheet XML body (inside `<worksheet>`), plus
/// optional extra parts and workbook relationships.
pub fn minimal(sheet_body: &str, extra_parts: &[(&str, &str)], wb_extra_rels: &str, wb_extra: &str) -> Vec<u8> {
    let wb = format!(
        r#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">{wb_extra}<sheets><sheet name="Data" sheetId="1" r:id="rId1"/></sheets></workbook>"#
    );
    let rels = format!(
        r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>{wb_extra_rels}</Relationships>"#
    );
    let sheet = format!(
        r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">{sheet_body}</worksheet>"#
    );
    let mut parts: Vec<(&str, &[u8])> = vec![
        ("[Content_Types].xml", CT.as_bytes()),
        ("_rels/.rels", ROOT_RELS.as_bytes()),
        ("xl/workbook.xml", wb.as_bytes()),
        ("xl/_rels/workbook.xml.rels", rels.as_bytes()),
        ("xl/worksheets/sheet1.xml", sheet.as_bytes()),
    ];
    for (n, c) in extra_parts {
        parts.push((n, c.as_bytes()));
    }
    make_zip(&parts)
}
