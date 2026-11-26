//! doc_optimize's flatten_layers: visible layers become plain content, hidden ones go, and the
//! open document keeps its layers.

use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

/// A page with text in no layer, in the visible layer "Notes" (5) and in the hidden layer
/// "Draft" (6), with `extra` objects from 7 on and `extra_props` in its `/Properties`.
fn fixture(extra_props: &str, extra: &[&str]) -> Vec<u8> {
    let content = "BT /F1 12 Tf 10 150 Td (PLAIN) Tj ET /OC /L1 BDC BT /F1 12 Tf 10 100 Td (VISIBLEONE) Tj ET EMC \
                   /OC /L2 BDC BT /F1 12 Tf 10 50 Td (HIDDENONE) Tj ET EMC";
    let page = format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> /Properties << /L1 5 0 R /L2 6 0 R {extra_props} >> >> >>"
    );
    let stream = format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len());
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [5 0 R 6 0 R] /D << /OFF [6 0 R] >> >> >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        page,
        stream,
        "<< /Type /OCG /Name (Notes) >>".to_string(),
        "<< /Type /OCG /Name (Draft) >>".to_string(),
    ];
    objs.extend(extra.iter().map(|o| o.to_string()));
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn call(a: &mut Automation, tool: &str, args: Value) -> Value {
    let result = a.call(tool, &args).unwrap();
    let Content::Json(value) = &result[0] else { panic!("expected JSON from {tool}") };
    value.clone()
}

fn layers(a: &mut Automation, doc: &Value) -> usize {
    call(a, "doc_info", json!({ "doc": doc }))["layers"].as_array().map_or(0, Vec::len)
}

fn words(a: &mut Automation, doc: &Value) -> Vec<String> {
    let text = call(a, "text_extract", json!({ "doc": doc }));
    text["pages"][0]["text"].as_str().unwrap().split_whitespace().map(str::to_string).collect()
}

#[test]
fn optimizing_flattens_visible_layers_and_drops_hidden_ones() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("layers.pdf"), fixture("", &[])).unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let doc = call(&mut a, "doc_open", json!({ "path": "layers.pdf" }))["doc"].clone();

    // Without the option, the layers stay.
    let kept = call(&mut a, "doc_optimize", json!({ "doc": doc, "path": "kept.pdf" }));
    assert_eq!(kept["layers_flattened"], 0, "{kept}");
    let reopened = call(&mut a, "doc_open", json!({ "path": "kept.pdf" }))["doc"].clone();
    assert_eq!(layers(&mut a, &reopened), 2);

    let flat = call(&mut a, "doc_optimize", json!({ "doc": doc, "path": "flat.pdf", "flatten_layers": true }));
    assert_eq!((flat["layers_flattened"].as_u64(), flat["hidden_layers_removed"].as_u64()), (Some(1), Some(1)), "{flat}");
    let reopened = call(&mut a, "doc_open", json!({ "path": "flat.pdf" }))["doc"].clone();
    assert_eq!(layers(&mut a, &reopened), 0, "no layer is left");
    let text = words(&mut a, &reopened);
    assert!(text.contains(&"PLAIN".to_string()) && text.contains(&"VISIBLEONE".to_string()), "{text:?}");
    assert!(!text.contains(&"HIDDENONE".to_string()), "{text:?}");
    let bytes = std::fs::read(dir.path().join("flat.pdf")).unwrap();
    assert!(!bytes.windows(13).any(|w| w == b"/OCProperties"));
    // The open document keeps its layers.
    assert_eq!(layers(&mut a, &doc), 2);
}

#[test]
fn optimizing_refuses_to_flatten_layers_combined_by_a_membership() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("member.pdf"), fixture("/M 7 0 R", &["<< /Type /OCMD /OCGs [5 0 R] >>"])).unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let doc = call(&mut a, "doc_open", json!({ "path": "member.pdf" }))["doc"].clone();
    let refused = a.call("doc_optimize", &json!({ "doc": doc, "path": "flat.pdf", "flatten_layers": true }));
    assert!(refused.is_err(), "a membership dictionary must refuse the run");
    assert!(!dir.path().join("flat.pdf").exists(), "nothing is written");
}
