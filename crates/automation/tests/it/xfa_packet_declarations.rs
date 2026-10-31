//! Invented packet contents only: the packet-array read path must reach application layout.

use std::sync::Arc;

use pdfcraft_automation::{Automation, Content};
use pdfcraft_cos::{Dict, Document, Object, PdfString, SaveOptions, Stream, write_incremental};
use serde_json::{Value, json};

fn call(a: &mut Automation, tool: &str, args: Value) -> Value {
    let mut result = a.call(tool, &args).unwrap();
    let Content::Json(value) = result.remove(0) else { panic!("expected JSON from {tool}") };
    value
}

#[test]
fn packet_leading_declarations_open_with_dynamic_layout_through_tools() {
    const DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>";
    const OPEN: &str = "<xdp:xdp xmlns:xdp=\"http://ns.adobe.com/xdp/\" xmlns:xfa=\"http://www.xfa.org/schema/xfa-data/1.0/\">";
    const TEMPLATE: &str = r#"<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb">
 <pageSet><pageArea name="master"><contentArea x="36pt" y="36pt" w="540pt" h="720pt"/>
  <medium short="612pt" long="792pt"/></pageArea></pageSet>
 <field name="entry" w="200pt" h="20pt"><ui><textEdit/></ui><value><text>default</text></value></field>
 <field name="fallback" w="200pt" h="20pt"><ui><textEdit/></ui><value><text>kept</text></value></field>
</subform></template>"#;
    const DATA: &str = "<xfa:datasets><xfa:data><form><entry>Synthetic &amp; Co</entry></form></xfa:data></xfa:datasets>";
    let mut cos = Document::open(Arc::new(pdfcraft_xfa::fixtures::shell(""))).unwrap();
    let mut packets = Vec::new();
    for (name, text) in [
        ("preamble", format!("{DECL}{OPEN}")),
        ("template", format!("{DECL}{TEMPLATE}")),
        ("datasets", format!("{DECL}{DATA}")),
        ("postamble", "</xdp:xdp>".into()),
    ] {
        let stream = cos.add(Object::Stream(Stream::flate(Dict::new(), text.as_bytes())));
        packets.extend([Object::String(PdfString::literal(name.as_bytes().to_vec())), Object::Ref(stream)]);
    }
    let mut acro = Dict::new();
    acro.set(b"Fields".to_vec(), Object::Array(Vec::new()));
    acro.set(b"XFA".to_vec(), Object::Array(packets));
    cos.update_dict(cos.root().unwrap(), |catalog| catalog.set(b"AcroForm".to_vec(), Object::Dict(acro))).unwrap();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("packets.pdf"), write_incremental(&cos, &SaveOptions::default()).unwrap()).unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    call(&mut a, "js_enabled", json!({ "enabled": false }));
    let opened = call(&mut a, "doc_open", json!({ "path": "packets.pdf" }));
    assert_eq!(opened["pages"], 1);
    let info = call(&mut a, "doc_info", json!({ "doc": opened["doc"] }));
    assert_eq!(info["xfa"], "dynamic");
    assert!(info["xfa_layout"].is_object(), "layout must succeed, not just open the placeholder");
    assert_eq!(info["xfa_layout"]["pages"], 1);
    assert_eq!(info["xfa_layout"]["fields"], 2);
    assert_eq!(info["fields"].as_array().unwrap().len(), 2);
}
