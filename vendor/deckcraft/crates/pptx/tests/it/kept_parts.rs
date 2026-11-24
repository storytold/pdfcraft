//! Custom XML data (SharePoint metadata), the sensitivity label and custom document properties
//! survive opening and saving a deck.

use std::io::{Cursor, Read, Write};

use deckcraft_engine::Session;
use zip::write::SimpleFileOptions;

const ITEM: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><b:Sources xmlns:b="http://schemas.openxmlformats.org/officeDocument/2006/bibliography"/>"#;
const PROPS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?><ds:datastoreItem ds:itemID="{11111111-2222-3333-4444-555555555555}" xmlns:ds="http://schemas.openxmlformats.org/officeDocument/2006/customXml"><ds:schemaRefs/></ds:datastoreItem>"#;
const ITEM_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXmlProps" Target="itemProps1.xml"/></Relationships>"#;
const LABEL: &str = r#"<?xml version="1.0" encoding="utf-8" standalone="yes"?><clbl:labelList xmlns:clbl="http://schemas.microsoft.com/office/2020/mipLabelMetadata"><clbl:label id="{aaaaaaaa-0000-0000-0000-000000000001}" enabled="1" method="Standard" siteId="{bbbbbbbb-0000-0000-0000-000000000002}" removed="0"/></clbl:labelList>"#;
const CUSTOM: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/custom-properties" xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes"><property fmtid="{D5CDD505-2E9C-101B-9397-08002B2CF9AE}" pid="2" name="ContentTypeId"><vt:lpwstr>0x0101</vt:lpwstr></property></Properties>"#;

fn deck() -> Vec<u8> {
    let mut s = Session::new();
    deckcraft_engine::sample::open_sample(&mut s).expect("sample");
    let p = s.doc().expect("doc").doc.as_ref().clone();
    let bytes = deckcraft_pptx::export(&p).expect("export");
    let mut src = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for i in 0..src.len() {
        let mut f = src.by_index(i).expect("entry");
        let name = f.name().to_string();
        let mut data = Vec::new();
        f.read_to_end(&mut data).expect("read");
        let edit = |d: Vec<u8>, at: &str, add: &str| String::from_utf8(d).expect("utf8").replacen(at, &format!("{add}{at}"), 1).into_bytes();
        let data = match name.as_str() {
            "[Content_Types].xml" => edit(
                data,
                "</Types>",
                r#"<Override PartName="/customXml/itemProps1.xml" ContentType="application/vnd.openxmlformats-officedocument.customXmlProperties+xml"/><Override PartName="/docProps/custom.xml" ContentType="application/vnd.openxmlformats-officedocument.custom-properties+xml"/><Override PartName="/docMetadata/LabelInfo.xml" ContentType="application/vnd.ms-office.classificationlabels+xml"/>"#,
            ),
            "_rels/.rels" => edit(
                data,
                "</Relationships>",
                r#"<Relationship Id="rIdL" Type="http://schemas.microsoft.com/office/2020/02/relationships/classificationlabels" Target="docMetadata/LabelInfo.xml"/><Relationship Id="rIdC" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/custom-properties" Target="docProps/custom.xml"/>"#,
            ),
            "ppt/_rels/presentation.xml.rels" => edit(
                data,
                "</Relationships>",
                r#"<Relationship Id="rIdX" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXml" Target="../customXml/item1.xml"/>"#,
            ),
            _ => data,
        };
        out.start_file(name, SimpleFileOptions::default()).expect("start");
        out.write_all(&data).expect("write");
    }
    for (n, d) in [
        ("customXml/item1.xml", ITEM),
        ("customXml/itemProps1.xml", PROPS),
        ("customXml/_rels/item1.xml.rels", ITEM_RELS),
        ("docMetadata/LabelInfo.xml", LABEL),
        ("docProps/custom.xml", CUSTOM),
    ] {
        out.start_file(n, SimpleFileOptions::default()).expect("start");
        out.write_all(d.as_bytes()).expect("write");
    }
    out.finish().expect("finish").into_inner()
}

fn read(zip: &mut zip::ZipArchive<Cursor<Vec<u8>>>, name: &str) -> String {
    let mut s = String::new();
    zip.by_name(name).unwrap_or_else(|_| panic!("{name} missing")).read_to_string(&mut s).expect("read");
    s
}

#[test]
fn custom_xml_label_and_custom_properties_survive_save() {
    let p = deckcraft_pptx::import(&deck()).expect("import");
    assert_eq!(p.kept_parts.len(), 5);
    let saved = deckcraft_pptx::export(&p).expect("export");
    let mut zip = zip::ZipArchive::new(Cursor::new(saved.clone())).expect("zip");
    for (n, d) in [
        ("customXml/item1.xml", ITEM),
        ("customXml/itemProps1.xml", PROPS),
        ("customXml/_rels/item1.xml.rels", ITEM_RELS),
        ("docMetadata/LabelInfo.xml", LABEL),
        ("docProps/custom.xml", CUSTOM),
    ] {
        assert_eq!(read(&mut zip, n), d, "{n}");
    }
    let ct = read(&mut zip, "[Content_Types].xml");
    assert!(ct.contains("/docMetadata/LabelInfo.xml") && ct.contains("customXmlProperties+xml") && ct.contains("custom-properties+xml"));
    let root = read(&mut zip, "_rels/.rels");
    assert!(root.contains("classificationlabels") && root.contains(r#"Target="docProps/custom.xml""#));
    assert!(read(&mut zip, "ppt/_rels/presentation.xml.rels").contains(r#"Target="../customXml/item1.xml""#));
    // And again: a second save keeps them once.
    let again = deckcraft_pptx::import(&saved).expect("reimport");
    assert_eq!(again.kept_parts, p.kept_parts);
}
