//! Dynamic-array cell metadata (`cm`) read from a file is written back, so Excel keeps
//! evaluating those formulas as arrays rather than with implicit intersection.

use gridcraft_core::CellRef;

use crate::package::Package;
use crate::xml::El;
use crate::{read_xlsx, write_xlsx};

const METADATA: &str = r#"<?xml version="1.0"?><metadata xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:xda="http://schemas.microsoft.com/office/spreadsheetml/2017/dynamicarray"><metadataTypes count="1"><metadataType name="XLDAPR" minSupportedVersion="120000" cellMeta="1"/></metadataTypes><futureMetadata name="XLDAPR" count="1"><bk><extLst><ext uri="{bdbb8cdc-fa1e-496e-a857-3c3f30c029c3}"><xda:dynamicArrayProperties fDynamic="1" fCollapsed="0"/></ext></extLst></bk></futureMetadata><cellMetadata count="1"><bk><rc t="1" v="0"/></bk></cellMetadata></metadata>"#;
const METADATA_REL: &str =
    r#"<Relationship Id="rId9" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sheetMetadata" Target="metadata.xml"/>"#;

/// Formulas Excel saves with `cm` although they return one value, and which GridCraft's own
/// shape analysis would save without it.
const DYNAMIC: [(&str, &str); 5] = [
    ("D1", "SUMPRODUCT(B1:B3*C1:C3)"),
    ("D2", "SUMPRODUCT(IFERROR(B1:B3/C1:C3,0))"),
    ("D3", "IF(AND(B1:B3>0),\"all\",\"some\")"),
    ("D4", "MID(CELL(\"filename\",A1),FIND(\"]\",CELL(\"filename\",A1))+1,31)"),
    ("D5", "SUMPRODUCT(--(YEAR(A1:A3)=2024))"),
];

fn source() -> Vec<u8> {
    let mut rows = String::new();
    for r in 1..=3 {
        rows.push_str(&format!(r#"<row r="{r}"><c r="A{r}"><v>{}</v></c><c r="B{r}"><v>{r}</v></c><c r="C{r}"><v>{}</v></c>"#, 45290 + r, r + 1));
        let (address, text) = DYNAMIC[r as usize - 1];
        rows.push_str(&format!(r#"<c r="{address}" cm="1"><f t="array" ref="{address}">{}</f><v>0</v></c></row>"#, crate::xml::esc(text)));
    }
    for (address, text) in &DYNAMIC[3..] {
        let r = &address[1..];
        rows.push_str(&format!(
            r#"<row r="{r}"><c r="{address}" cm="1"><f t="array" ref="{address}">{}</f><v>0</v></c></row>"#,
            crate::xml::esc(text)
        ));
    }
    // The same SUMPRODUCT saved by legacy Excel: no `cm`.
    rows.push_str(r#"<row r="6"><c r="D6"><f>SUMPRODUCT(B1:B3*C1:C3)</f><v>20</v></c></row>"#);
    super::minimal(&format!("<sheetData>{rows}</sheetData>"), &[("xl/metadata.xml", METADATA)], METADATA_REL, "")
}

fn cell<'a>(sheet: &'a El, address: &str) -> &'a El {
    sheet.child("sheetData").unwrap().kids("row").flat_map(|r| r.kids("c")).find(|c| c.attr("r") == Some(address)).unwrap()
}

fn at(s: &str) -> CellRef {
    CellRef::parse(s).unwrap()
}

fn assert_written_dynamic(bytes: &[u8]) {
    let mut pkg = Package::open(bytes).unwrap();
    let sheet = pkg.read_xml("xl/worksheets/sheet1.xml").unwrap().unwrap();
    for (address, text) in DYNAMIC {
        let c = cell(&sheet, address);
        assert_eq!(c.attr("cm"), Some("1"), "{address} lost its dynamic-array metadata");
        let f = c.child("f").unwrap();
        assert_eq!((f.attr("t"), f.attr("ref")), (Some("array"), Some(address)));
        assert_eq!(f.text, text);
    }
    let legacy = cell(&sheet, "D6");
    assert_eq!(legacy.attr("cm"), None, "a legacy formula must not become dynamic");
    assert_eq!(legacy.child("f").unwrap().attr("t"), None);

    let metadata = pkg.read_xml("xl/metadata.xml").unwrap().expect("metadata part written");
    assert_eq!(metadata.path(&["metadataTypes", "metadataType"]).unwrap().attr("name"), Some("XLDAPR"));
    let record = metadata.path(&["cellMetadata", "bk", "rc"]).unwrap();
    assert_eq!((record.attr("t"), record.attr("v")), (Some("1"), Some("0")));
    assert!(pkg.rels("xl/workbook.xml").unwrap().iter().any(|r| r.kind == "sheetMetadata" && r.target == "xl/metadata.xml"));
    let types = pkg.read_xml("[Content_Types].xml").unwrap().unwrap();
    assert!(types.kids("Override").any(|e| e.attr("PartName") == Some("/xl/metadata.xml")
        && e.attr("ContentType") == Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheetMetadata+xml")));
}

#[test]
fn dynamic_array_metadata_is_read_onto_formulas() {
    let (wb, report) = read_xlsx(&source()).unwrap();
    assert!(!report.warnings.iter().any(|w| w.contains("metadata")), "{:?}", report.warnings);
    let sh = wb.sheet(0).unwrap();
    for (address, _) in DYNAMIC {
        let f = sh.cell(at(address)).unwrap().formula.as_ref().unwrap();
        assert!(f.dynamic, "{address}");
        assert_eq!(f.array, None, "{address} is not a legacy CSE array");
    }
    assert!(!sh.cell(at("D6")).unwrap().formula.as_ref().unwrap().dynamic);
}

#[test]
fn dynamic_array_metadata_survives_saving_scalar_results() {
    let (wb, _) = read_xlsx(&source()).unwrap();
    let bytes = write_xlsx(&wb).unwrap();
    assert_written_dynamic(&bytes);
    // And again: the mark read from GridCraft's own file is kept too.
    let (back, report) = read_xlsx(&bytes).unwrap();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_written_dynamic(&write_xlsx(&back).unwrap());
}
