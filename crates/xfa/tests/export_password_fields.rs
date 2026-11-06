use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, Object, PdfString, SaveOptions, Stream, write_full};
use pdfcraft_xfa::{export_data, render_into};

fn xfa_shell(xdp: &str) -> Vec<u8> {
    let mut doc = Document::new_empty();
    let xfa_stream = doc.add(Object::Stream(Stream::from_raw(Dict::new(), xdp.as_bytes().to_vec())));
    let pages_root = doc.add(Object::Null);
    let page = doc.add(Object::Dict(
        [
            (b"Type".to_vec(), Object::name("Page")),
            (b"Parent".to_vec(), Object::Ref(pages_root)),
            (b"MediaBox".to_vec(), Object::Array([0.0, 0.0, 612.0, 792.0].into_iter().map(Object::Real).collect())),
        ]
        .into_iter()
        .collect(),
    ));
    doc.set(
        pages_root,
        Object::Dict(
            [
                (b"Type".to_vec(), Object::name("Pages")),
                (b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)])),
                (b"Count".to_vec(), Object::Int(1)),
            ]
            .into_iter()
            .collect(),
        ),
    );
    let acro: Dict = [(b"Fields".to_vec(), Object::Array(Vec::new())), (b"XFA".to_vec(), Object::Ref(xfa_stream))].into_iter().collect();
    let cat = doc.add(Object::Dict(
        [(b"Type".to_vec(), Object::name("Catalog")), (b"Pages".to_vec(), Object::Ref(pages_root)), (b"AcroForm".to_vec(), Object::Dict(acro))]
            .into_iter()
            .collect(),
    ));
    doc.trailer_mut().set(b"Root".to_vec(), Object::Ref(cat));
    write_full(&doc, &SaveOptions::default()).unwrap()
}

/// #3945: XFA XML and XDP data exports do not write typed `<passwordEdit/>` values in plain text;
/// fields with a template default export that default and fields without one export empty.
#[test]
fn xfa_export_data_omits_typed_password_edit_values() {
    let xdp = r#"<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb">
 <pageSet><pageArea name="front"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>
 <subform name="page1" layout="tb">
  <field name="user" w="2in" h="0.3in"><ui><textEdit/></ui></field>
  <field name="pin" w="2in" h="0.3in"><ui><passwordEdit/></ui></field>
  <field name="preset" w="2in" h="0.3in"><ui><passwordEdit/></ui><value><text>0000</text></value></field>
 </subform>
</subform>
</template>
<xfa:datasets xmlns:xfa="http://www.xfa.org/schema/xfa-data/1.0/">
<xfa:data><form><page1><user>Ada</user><pin>hunter2</pin><preset>9999</preset></page1></form></xfa:data>
</xfa:datasets>
</xdp:xdp>"#;
    let mut doc = Document::open(Arc::new(xfa_shell(xdp))).unwrap();
    render_into(&mut doc).unwrap();

    for is_xdp in [false, true] {
        let exported = export_data(&doc, is_xdp, "form.pdf").unwrap();
        assert!(exported.contains("<user>Ada</user>"), "is_xdp={is_xdp}: {exported}");
        assert!(exported.contains("<preset>0000</preset>"), "is_xdp={is_xdp}: {exported}");
        assert!(!exported.contains("hunter2") && !exported.contains("9999"), "is_xdp={is_xdp} leaked password: {exported}");
    }
}

fn password_export_document(pin: &str, preset: &str) -> String {
    format!(
        r#"<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
 <subform name="form"><field name="pin"><ui><passwordEdit/></ui></field>
 <field name="preset"><ui><passwordEdit/></ui><value><text>0000</text></value></field>
 <field name="user"><ui><textEdit/></ui></field></subform>
</template>
<xfa:datasets xmlns:xfa="http://www.xfa.org/schema/xfa-data/1.0/">
 <xfa:data><form><pin>{pin}</pin><preset>{preset}</preset><user>Ada</user></form></xfa:data>
</xfa:datasets></xdp:xdp>"#
    )
}

#[test]
fn xfa_export_data_scans_the_same_wrapped_packet_it_exports() {
    let text = format!("<config/>{}", password_export_document("leaf-secret", "other-secret"));
    assert!(pdfcraft_xfa::data::parse_datasets(&text).is_none(), "the unwrapped scan cannot read this packet");
    let doc = Document::open(Arc::new(xfa_shell(&text))).unwrap();
    for xdp in [false, true] {
        let exported = export_data(&doc, xdp, "form.pdf").unwrap();
        assert!(!exported.contains("leaf-secret") && !exported.contains("other-secret"), "{exported}");
        assert!(exported.contains("<user>Ada</user>") && exported.contains("<preset>0000</preset>"), "{exported}");
        assert!(roxmltree::Document::parse(&exported).is_ok());
    }
    assert!(pdfcraft_xfa::read_packets(&doc).unwrap().unwrap().xdp.contains("leaf-secret"), "export does not mutate the open form");

    // Wrapping itself is not an error, including when no password field needs clearing.
    let ordinary = text.replace("passwordEdit", "textEdit");
    let doc = Document::open(Arc::new(xfa_shell(&ordinary))).unwrap();
    for xdp in [false, true] {
        assert!(export_data(&doc, xdp, "form.pdf").unwrap().contains("<pin>leaf-secret</pin>"));
    }
}

#[test]
fn xfa_export_data_rejects_partial_password_redaction() {
    for structured in ["<value>structured-secret</value>", "<other>structured-secret</other>", "<value>0000</value>"] {
        let text = password_export_document("leaf-secret", structured);
        let doc = Document::open(Arc::new(xfa_shell(&text))).unwrap();
        for xdp in [false, true] {
            assert!(export_data(&doc, xdp, "form.pdf").is_err(), "structured password must not survive a partial export");
        }
        let original = pdfcraft_xfa::read_packets(&doc).unwrap().unwrap().xdp;
        assert!(original.contains("leaf-secret") && original.contains(structured), "a refused export leaves the document unchanged");
    }
}

#[test]
fn xfa_export_data_rejects_unreadable_password_data() {
    let text = password_export_document("leaf-secret", "other-secret").replace("</xfa:data>", "</broken>");
    let doc = Document::open(Arc::new(xfa_shell(&text))).unwrap();
    for xdp in [false, true] {
        assert!(export_data(&doc, xdp, "form.pdf").is_err());
    }
}

#[test]
fn xfa_export_data_rejects_an_unreadable_password_template() {
    let text = password_export_document("leaf-secret", "other-secret")
        .replace("<subform name=\"form\">", "<area name=\"form\">")
        .replace("</subform>", "</area>");
    let doc = Document::open(Arc::new(xfa_shell(&text))).unwrap();
    for xdp in [false, true] {
        assert!(export_data(&doc, xdp, "form.pdf").is_err());
    }
}

#[test]
fn xfa_export_data_scans_wrapped_array_packets() {
    let text = password_export_document("leaf-secret", "other-secret");
    let parsed = roxmltree::Document::parse(&text).unwrap();
    let template = parsed.descendants().find(|n| n.has_tag_name(("http://www.xfa.org/schema/xfa-template/3.3/", "template"))).unwrap();
    let datasets = parsed.descendants().find(|n| n.tag_name().name() == "datasets").unwrap();
    let mut doc = Document::open(Arc::new(xfa_shell(&text))).unwrap();
    let template = doc.add(Object::Stream(Stream::from_raw(Dict::new(), text[template.range()].as_bytes().to_vec())));
    let data = format!("<config/>{}", &text[datasets.range()]);
    let data = doc.add(Object::Stream(Stream::from_raw(Dict::new(), data.into_bytes())));
    let root = doc.root().unwrap();
    let mut catalog = doc.get(root).as_dict().unwrap().clone();
    let mut acro = catalog.get(b"AcroForm").unwrap().as_dict().unwrap().clone();
    acro.set(
        b"XFA".to_vec(),
        Object::Array(vec![
            Object::String(PdfString::text("template")),
            Object::Ref(template),
            Object::String(PdfString::text("datasets")),
            Object::Ref(data),
        ]),
    );
    catalog.set(b"AcroForm".to_vec(), Object::Dict(acro));
    doc.set(root, Object::Dict(catalog));
    for xdp in [false, true] {
        let exported = export_data(&doc, xdp, "form.pdf").unwrap();
        assert!(!exported.contains("leaf-secret") && !exported.contains("other-secret"), "{exported}");
        assert!(exported.contains("<user>Ada</user>") && exported.contains("<preset>0000</preset>"), "{exported}");
    }
}

fn static_password_document(data: &str, som: &str, depth: usize) -> Document {
    let text = format!("<xfa:datasets xmlns:xfa=\"http://www.xfa.org/schema/xfa-data/1.0/\"><xfa:data>{data}</xfa:data></xfa:datasets>");
    let mut doc = Document::open(Arc::new(xfa_shell(&text))).unwrap();
    let mut field = doc.add(Object::Dict(
        [(b"FT".to_vec(), Object::name("Tx")), (b"Ff".to_vec(), Object::Int(1 << 13)), (b"T".to_vec(), Object::String(PdfString::text(som)))]
            .into_iter()
            .collect(),
    ));
    for _ in 0..depth {
        field = doc.add(Object::Dict([(b"Kids".to_vec(), Object::Array(vec![Object::Ref(field)]))].into_iter().collect()));
    }
    let root = doc.root().unwrap();
    let mut catalog = doc.get(root).as_dict().unwrap().clone();
    let mut acro = catalog.get(b"AcroForm").unwrap().as_dict().unwrap().clone();
    acro.set(b"Fields".to_vec(), Object::Array(vec![Object::Ref(field)]));
    catalog.set(b"AcroForm".to_vec(), Object::Dict(acro));
    doc.set(root, Object::Dict(catalog));
    doc
}

fn assert_password_scan_boundary(doc: &Document, complete: bool) {
    let before = write_full(doc, &SaveOptions::default()).unwrap();
    for xdp in [false, true] {
        let result = export_data(doc, xdp, "form.pdf");
        if complete {
            let exported = result.unwrap();
            assert!(!exported.contains("boundary-secret"), "{exported}");
        } else {
            assert!(matches!(result, Err(pdfcraft_xfa::XfaError::Malformed(_))), "a truncated scan must refuse export");
        }
    }
    assert_eq!(write_full(doc, &SaveOptions::default()).unwrap(), before, "export never changes the original packets");
}

#[test]
fn xfa_export_data_rejects_dataset_depth_truncation() {
    for depth in [63, 64] {
        let data = format!("{}<pin>boundary-secret</pin>{}", "<n>".repeat(depth), "</n>".repeat(depth));
        let som = format!("{}pin[0]", "n[0].".repeat(depth));
        let doc = static_password_document(&data, &som, 0);
        assert_password_scan_boundary(&doc, depth == 63);
    }
}

#[test]
fn xfa_export_data_rejects_template_depth_truncation() {
    for depth in [63, 64] {
        let fields = format!("{}<field name=\"pin\"><ui><passwordEdit/></ui></field>{}", "<subform>".repeat(depth), "</subform>".repeat(depth));
        let text = format!(
            "<xdp:xdp xmlns:xdp=\"http://ns.adobe.com/xdp/\"><template><subform name=\"form\">{fields}</subform></template><datasets><data><form><pin>boundary-secret</pin></form></data></datasets></xdp:xdp>"
        );
        let doc = Document::open(Arc::new(xfa_shell(&text))).unwrap();
        assert_password_scan_boundary(&doc, depth == 63);
    }
}

#[test]
fn xfa_export_data_rejects_truncated_page_sets() {
    let text = format!(
        "<xdp:xdp xmlns:xdp=\"http://ns.adobe.com/xdp/\"><template><subform name=\"form\">{}<pageArea name=\"master\"><field name=\"pin\"><ui><passwordEdit/></ui></field></pageArea>{}</subform></template><datasets><data><master><pin>boundary-secret</pin></master></data></datasets></xdp:xdp>",
        "<pageSet>".repeat(65),
        "</pageSet>".repeat(65)
    );
    let doc = Document::open(Arc::new(xfa_shell(&text))).unwrap();
    assert_password_scan_boundary(&doc, false);
}

#[test]
fn xfa_export_data_rejects_password_sibling_budget_truncation() {
    for draws in [pdfcraft_xfa::MAX_FORM_NODES - 2, pdfcraft_xfa::MAX_FORM_NODES - 1] {
        let text = format!(
            "<xdp:xdp xmlns:xdp=\"http://ns.adobe.com/xdp/\"><template><subform name=\"form\">{}<field name=\"pin\"><ui><passwordEdit/></ui></field></subform></template><datasets><data><form><pin>boundary-secret</pin></form></data></datasets></xdp:xdp>",
            "<draw/>".repeat(draws)
        );
        let doc = Document::open(Arc::new(xfa_shell(&text))).unwrap();
        assert_password_scan_boundary(&doc, draws == pdfcraft_xfa::MAX_FORM_NODES - 2);
    }
}

#[test]
fn xfa_export_data_rejects_password_child_budget_truncation() {
    let text = format!(
        "<xdp:xdp xmlns:xdp=\"http://ns.adobe.com/xdp/\"><template><subform name=\"form\">{}<subform><field name=\"pin\"><ui><passwordEdit/></ui></field></subform></subform></template><datasets><data><form><pin>boundary-secret</pin></form></data></datasets></xdp:xdp>",
        "<draw/>".repeat(pdfcraft_xfa::MAX_FORM_NODES - 2)
    );
    let doc = Document::open(Arc::new(xfa_shell(&text))).unwrap();
    assert_password_scan_boundary(&doc, false);
}

#[test]
fn xfa_export_data_rejects_password_instance_budget_truncation() {
    // Layout caps each subform at 50 instances. Four levels exceed the 200,000-node
    // scan budget using empty instances alone, with no later field/child to flag it.
    let text = r#"<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/"><template><subform name="form">
<field name="pin"><ui><passwordEdit/></ui></field>
<subform name="row"><occur initial="50" max="-1"/><subform name="cell"><occur initial="50" max="-1"/>
<subform name="unit"><occur initial="50" max="-1"/><subform name="atom"><occur initial="50" max="-1"/></subform></subform>
</subform></subform>
</subform></template>
<datasets><data><form><pin>boundary-secret</pin></form></data></datasets></xdp:xdp>"#;
    let doc = Document::open(Arc::new(xfa_shell(text))).unwrap();
    assert_password_scan_boundary(&doc, false);
}

#[test]
fn xfa_export_data_rejects_static_password_depth_truncation() {
    for depth in [32, 33] {
        let doc = static_password_document("<form><pin>boundary-secret</pin></form>", "form[0].pin[0]", depth);
        assert_password_scan_boundary(&doc, depth == 32);
    }
}

#[test]
fn xfa_export_data_rejects_clamped_password_indices() {
    for index in [100_000, 100_001] {
        let data = format!("<form>{}<pin>boundary-secret</pin></form>", "<pin/>".repeat(index));
        let doc = static_password_document(&data, &format!("form[0].pin[{index}]"), 0);
        assert_password_scan_boundary(&doc, index == 100_000);
        // Same-named ordinary draws still advance the final password field's SOM index.
        let text = format!(
            "<xdp:xdp xmlns:xdp=\"http://ns.adobe.com/xdp/\"><template><subform name=\"form\">{}<field name=\"pin\"><ui><passwordEdit/></ui></field></subform></template><datasets><data>{data}</data></datasets></xdp:xdp>",
            "<draw name=\"pin\"/>".repeat(index)
        );
        let doc = Document::open(Arc::new(xfa_shell(&text))).unwrap();
        assert_password_scan_boundary(&doc, index == 100_000);
    }
}
