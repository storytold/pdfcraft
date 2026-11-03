//! Preview and Apply share the text selected for implicit paragraph reflow.

use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

/// Create source rows containing a soft wrap followed by a hard break.
fn page() -> Vec<u8> {
    let content = "BT /F1 12 Tf 14.4 TL 1 0 0 1 72 700 Tm (Alpha beta gamma delta epsilon) Tj T* (zeta eta theta) Tj T* (Tail) Tj ET";
    fixture(content)
}

/// Build a valid single-page Helvetica PDF around synthetic text operators with correct stream lengths and xref offsets.
fn fixture(content: &str) -> Vec<u8> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_owned(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_owned(),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    bytes
}

/// Require one JSON automation result for the synthetic paragraph assertions.
fn call(auto: &mut Automation, name: &str, args: Value) -> Value {
    let result = auto.call(name, &args).unwrap();
    let Content::Json(value) = result.into_iter().next().unwrap() else { panic!("JSON result") };
    value
}

#[test]
/// Verify implicit width/direction preview and Apply select the same reflow text while preserving the hard break.
fn implicit_direction_reflow_preview_matches_explicit_text_and_apply() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("wrapped.pdf"), page()).unwrap();
    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let doc = call(&mut auto, "doc_open", json!({"path":"wrapped.pdf"}))["doc"].clone();
    let paragraphs = call(&mut auto, "text_paragraphs", json!({"doc":doc,"page":1}));
    assert_eq!(paragraphs["paragraphs"].as_array().unwrap().len(), 1);
    let block = &paragraphs["paragraphs"][0];
    let reflow = auto.session().docs().first().unwrap().text_blocks(0).remove(0).reflow_text;
    assert_eq!(reflow.matches('\n').count(), 1, "retain a hard break after a soft wrap");
    assert_eq!(block["editor_text"].as_str().unwrap().matches('\n').count(), 2);
    assert_ne!(block["text"], reflow, "exercise explicit reflow rather than unchanged source text");
    // Forced direction across source rows requires a uniform complete font face; the
    // standard-font fixture isolates text selection without adding that dependency.
    let implicit = call(&mut auto, "text_edit_layout", json!({"doc":doc,"page":1,"paragraph":1,"width":400,"direction":"auto"}));
    let explicit = call(&mut auto, "text_edit_layout", json!({"doc":doc,"page":1,"paragraph":1,"width":400,"direction":"auto","text":reflow}));
    assert_eq!(implicit, explicit, "preview must reflow the same text as Apply");
    assert_eq!(call(&mut auto, "text_paragraphs", json!({"doc":doc,"page":1})), paragraphs, "preview must not edit the document");
    call(&mut auto, "text_edit", json!({"doc":doc,"page":1,"paragraph":1,"width":400,"direction":"auto"}));
    let applied = call(&mut auto, "text_paragraphs", json!({"doc":doc,"page":1}));
    assert_eq!(applied["paragraphs"][0]["editor_text"], reflow);
}

/// Exercise implicit or literal replacement when the paragraph contains only soft source wraps.
fn all_soft_reflow(explicit: bool) {
    let dir = tempfile::tempdir().unwrap();
    let bytes = String::from_utf8(page()).unwrap().replace(" T* (Tail) Tj", "");
    // Rebuild the fixture rather than changing stream lengths/xref positions.
    let content = bytes.split("stream\n").nth(1).unwrap().split("\nendstream").next().unwrap();
    let bytes = fixture(content);
    std::fs::write(dir.path().join("soft.pdf"), bytes).unwrap();
    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let doc = call(&mut auto, "doc_open", json!({"path":"soft.pdf"}))["doc"].clone();
    let source = auto.session().docs().last().unwrap().text_blocks(0).remove(0);
    assert_eq!(source.reflow_text, source.text);
    assert!(source.editor_text.contains('\n'));
    let mut args = json!({"doc":doc,"page":1,"paragraph":1,"width":400,"direction":"auto"});
    if explicit {
        args["text"] = json!(source.text);
    }
    call(&mut auto, "text_edit", args);
    let after = call(&mut auto, "text_paragraphs", json!({"doc":doc,"page":1}));
    assert_eq!(after["paragraphs"][0]["editor_text"], source.reflow_text, "an explicit reflow draft must not become source visual rows");
}

#[test]
/// Verify implicit width/direction editing rejoins a paragraph whose source breaks are all soft.
fn implicit_all_soft_width_and_direction_reflows() {
    all_soft_reflow(false);
}

#[test]
/// Verify a literal flat draft is still reflowed when it equals the source's flattened text.
fn explicit_flat_text_equal_to_source_text_reflows() {
    all_soft_reflow(true);
}

#[test]
/// Verify direction-only editing keeps visual rows and saved break kinds so later widening rejoins only soft wraps.
fn direction_only_preserves_soft_and_hard_breaks_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("wrapped.pdf"), page()).unwrap();
    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let doc = call(&mut auto, "doc_open", json!({"path":"wrapped.pdf"}))["doc"].clone();
    let source = auto.session().docs().last().unwrap().text_blocks(0).remove(0);
    assert_eq!(source.reflow_text.matches('\n').count(), 1);
    call(&mut auto, "text_edit", json!({"doc":doc,"page":1,"paragraph":1,"direction":"auto"}));
    let after = auto.session().docs().last().unwrap().text_blocks(0).remove(0);
    assert_eq!(after.editor_text, source.editor_text, "direction alone keeps visual rows");
    assert_eq!(after.reflow_text, source.reflow_text, "direction alone must retain source break types");
    call(&mut auto, "doc_save", json!({"doc":doc,"path":"saved.pdf"}));
    let reopened = call(&mut auto, "doc_open", json!({"path":"saved.pdf"}))["doc"].clone();
    assert_eq!(auto.session().docs().last().unwrap().text_blocks(0).remove(0).reflow_text, source.reflow_text);
    call(&mut auto, "text_edit", json!({"doc":reopened,"page":1,"paragraph":1,"width":400,"direction":"auto"}));
    let after = call(&mut auto, "text_paragraphs", json!({"doc":reopened,"page":1}));
    assert_eq!(after["paragraphs"][0]["editor_text"], source.reflow_text, "widening rejoins only soft wraps after reopen");
}

#[test]
/// Verify Latin range styling under stored LTR direction while retaining the RTL mixed-style refusal.
fn explicit_ltr_latin_allows_range_styles_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("latin.pdf"), fixture("BT /F1 12 Tf 1 0 0 1 72 700 Tm (Alpha beta) Tj ET")).unwrap();
    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let doc = call(&mut auto, "doc_open", json!({"path":"latin.pdf"}))["doc"].clone();
    let styled = json!({"doc":doc,"page":1,"paragraph":1,"direction":"ltr","ranges":[{"range":[0,5],"color":"#cc0000"}]});
    call(&mut auto, "text_edit_layout", styled.clone());
    call(&mut auto, "text_edit", styled);
    call(&mut auto, "doc_save", json!({"doc":doc,"path":"ltr.pdf"}));
    let reopened = call(&mut auto, "doc_open", json!({"path":"ltr.pdf"}))["doc"].clone();
    // Direction is now stored in the PDF. A later style-only edit must also work.
    call(&mut auto, "text_edit", json!({"doc":reopened,"page":1,"paragraph":1,"ranges":[{"range":[6,10],"size":16}]}));
    let lines = call(&mut auto, "text_lines", json!({"doc":reopened,"page":1}));
    assert!(
        lines["lines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|line| line["text"].as_str().is_some_and(|text| text.trim() == "beta") && line["size"].as_f64() == Some(16.0)),
        "{lines}"
    );
    assert!(
        auto.call("text_edit", &json!({"doc":reopened,"page":1,"paragraph":1,"direction":"rtl","ranges":[{"range":[0,5],"size":18}]})).is_err(),
        "RTL-base mixed styles still need a uniform full-face row"
    );
}

/// Empty or inherited row-alignment hints do not turn soft wraps into hard breaks
/// during implicit width reflow; Apply retains only the actual hard break.
#[test]
fn empty_or_inherited_aligns_reflow_soft_wraps() {
    for aligns in [json!([]), json!([null]), json!([null, null]), json!(["inherit"]), json!(["inherit", null])] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("wrapped.pdf"), page()).unwrap();
        let mut auto = Automation::new().with_root(dir.path()).unwrap();
        let doc = call(&mut auto, "doc_open", json!({"path":"wrapped.pdf"}))["doc"].clone();
        let source = auto.session().docs().last().unwrap().text_blocks(0).remove(0);
        assert_ne!(source.editor_text, source.reflow_text);
        let implicit = json!({"doc":doc,"page":1,"paragraph":1,"width":400,"direction":"auto","aligns":aligns});
        call(&mut auto, "text_edit", implicit);
        assert_eq!(
            call(&mut auto, "text_paragraphs", json!({"doc":doc,"page":1}))["paragraphs"][0]["editor_text"],
            source.reflow_text,
            "Apply must rejoin only soft wraps with {aligns}"
        );
    }
}
