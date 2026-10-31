//! Form-bound extraction through the public automation paths.
use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

fn call(a: &mut Automation, name: &str, args: Value) -> Value {
    match a.call(name, &args).unwrap().remove(0) {
        Content::Json(value) => value,
        _ => panic!("expected JSON"),
    }
}

fn fixture(form: &str, page: &str, bbox: &str, matrix: &str) -> Vec<u8> {
    fixture_with_font(form, page, bbox, matrix, "<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>")
}

fn fixture_with_font(form: &str, page: &str, bbox: &str, matrix: &str, font: &str) -> Vec<u8> {
    let bodies = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /XObject << /Fm 6 0 R >> >> >>"
            .into(),
        format!("<< /Length {} >>\nstream\n{page}\nendstream", page.len()),
        font.into(),
        format!(
            "<< /Type /XObject /Subtype /Form /BBox [{bbox}] /Matrix [{matrix}] /Resources << /Font << /F1 5 0 R >> >> /Length {} >>\nstream\n{form}\nendstream",
            form.len()
        ),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = vec![];
    for (i, b) in bodies.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{b}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(b"xref\n0 7\n0000000000 65535 f \n");
    for n in offsets {
        out.extend_from_slice(format!("{n:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer << /Size 7 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    out
}

#[test]
fn form_bounds_survive_extraction_find_export_and_save() {
    let dir = tempfile::tempdir().unwrap();
    let inside = "BT /F1 6 Tf 10 10 Td (INSIDE) Tj ET";
    let both = format!("{inside} BT /F1 6 Tf 100 100 Td (OUTSIDE) Tj ET");
    let original = fixture(&both, "/Fm Do", "0 0 50 50", "1 0 0 1 0 0");
    std::fs::write(dir.path().join("source.pdf"), &original).unwrap();
    std::fs::write(dir.path().join("control.pdf"), fixture(inside, "/Fm Do", "0 0 50 50", "1 0 0 1 0 0")).unwrap();
    let mut api = Automation::new().with_root(dir.path()).unwrap();
    let doc = call(&mut api, "doc_open", json!({"path":"source.pdf"}))["doc"].as_u64().unwrap();
    let control = call(&mut api, "doc_open", json!({"path":"control.pdf"}))["doc"].as_u64().unwrap();
    let expected = api.render_pixels(control, 1, 1.0).unwrap();
    assert!(expected.2.as_chunks::<4>().0.iter().any(|p| p[0] < 255 || p[1] < 255 || p[2] < 255));
    for stage in ["before", "reopened"] {
        let id = if stage == "before" {
            doc
        } else {
            call(&mut api, "doc_save", json!({"doc":doc,"path":"saved.pdf"}));
            call(&mut api, "doc_open", json!({"path":"saved.pdf"}))["doc"].as_u64().unwrap()
        };
        let rendered = api.render_pixels(id, 1, 1.0).unwrap();
        assert_eq!(rendered.2.as_ref(), expected.2.as_ref(), "{stage}");
        assert_eq!(call(&mut api, "text_extract", json!({"doc":id}))["pages"], json!([{"page":1,"text":"INSIDE"}]));
        for (query, count) in [("INSIDE", 1), ("OUTSIDE", 0)] {
            let found = call(&mut api, "text_find", json!({"doc":id,"query":query}));
            assert_eq!(found["count"], count, "{stage} {query}");
            assert_eq!(found["complete"], true);
            assert_eq!(found["warning_count"], 0);
        }
        let output = format!("{stage}.txt");
        call(&mut api, "doc_export_text", json!({"doc":id,"path":output}));
        assert_eq!(std::fs::read(dir.path().join(output)).unwrap(), b"INSIDE\n");
        assert_eq!(call(&mut api, "doc_info", json!({"doc":id}))["document"]["dirty"], false);
    }
    assert_eq!(std::fs::read(dir.path().join("source.pdf")).unwrap(), original);
}

#[test]
fn mixed_actual_text_never_exports_the_hidden_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let ops = "/Span <</ActualText (SECRET VISIBLE)>> BDC BT /F1 6 Tf 100 10 Td (SECRET) Tj ET BT /F1 6 Tf 10 10 Td (VISIBLE) Tj ET EMC";
    std::fs::write(dir.path().join("mixed.pdf"), fixture(ops, "/Fm Do", "0 0 50 50", "1 0 0 1 0 0")).unwrap();
    let mut api = Automation::new().with_root(dir.path()).unwrap();
    let doc = call(&mut api, "doc_open", json!({"path":"mixed.pdf"}))["doc"].clone();
    assert_eq!(call(&mut api, "text_extract", json!({"doc":doc}))["pages"][0]["text"], "VISIBLE");
    assert_eq!(call(&mut api, "text_find", json!({"doc":doc,"query":"SECRET"}))["count"], 0);
    call(&mut api, "doc_export_text", json!({"doc":doc,"path":"mixed.txt"}));
    assert_eq!(std::fs::read(dir.path().join("mixed.txt")).unwrap(), b"VISIBLE\n");
}

#[test]
fn clipping_limits_report_the_cause_without_partial_export() {
    let dir = tempfile::tempdir().unwrap();
    // A non-axis-aligned clip is emitted on every operation; rectangle elision cannot turn
    // this into a fake depth test. This stays far below the interpreter content byte limit.
    let mut ops = "0 0 m 50 0 l 60 50 l 10 50 l h W n\n".repeat(1024);
    ops.push_str("BT /F1 6 Tf 10 10 Td (INSIDE) Tj ET");
    std::fs::write(dir.path().join("deep.pdf"), fixture(&ops, "/Fm Do", "0 0 50 50", "1 0 0 1 0 0")).unwrap();
    std::fs::write(dir.path().join("export.txt"), b"keep existing output").unwrap();
    let mut api = Automation::new().with_root(dir.path()).unwrap();
    let doc = call(&mut api, "doc_open", json!({"path":"deep.pdf"}))["doc"].clone();
    let error = api.call("text_extract", &json!({"doc":doc})).unwrap_err().to_string();
    assert!(error.contains("text clipping limit exceeded"), "{error}");
    let found = call(&mut api, "text_find", json!({"doc":doc,"query":"INSIDE"}));
    assert_eq!(found["complete"], false);
    assert_eq!(found["count"], 0);
    assert_eq!(found["warning_count"], 1);
    assert_eq!(found["warnings"][0]["page"], 1);
    assert!(found["warnings"][0]["error"].as_str().unwrap().contains("text clipping limit exceeded"));
    let error = api.call("doc_export_text", &json!({"doc":doc,"path":"export.txt"})).unwrap_err().to_string();
    assert!(error.contains("text clipping limit exceeded"), "{error}");
    assert_eq!(std::fs::read(dir.path().join("export.txt")).unwrap(), b"keep existing output");
    assert_eq!(call(&mut api, "doc_info", json!({"doc":doc}))["document"]["dirty"], false);
}

#[test]
fn sequential_clips_preserve_extraction_find_and_export_after_save() {
    let dir = tempfile::tempdir().unwrap();
    let mut ops = "q 0 0 m 50 0 l 60 50 l 10 50 l h W n Q\n".repeat(40_000);
    // After the old page-total allowance is exhausted, both inner and ancestor clips
    // must still exclude text. Q must restore the parent's visible region.
    ops.push_str("q 0 0 m 20 0 l 25 50 l 5 50 l h W n BT /F1 6 Tf 35 10 Td (HIDDEN) Tj ET Q ");
    let visible = "BT /F1 6 Tf 10 10 Td (INSIDE) Tj ET";
    ops.push_str(visible);
    ops.push_str(" BT /F1 6 Tf 100 100 Td (OUTSIDE) Tj ET");
    let original = fixture(&ops, "/Fm Do", "0 0 50 50", "1 0 0 1 0 0");
    std::fs::write(dir.path().join("sequential.pdf"), &original).unwrap();
    std::fs::write(dir.path().join("control.pdf"), fixture(visible, "/Fm Do", "0 0 50 50", "1 0 0 1 0 0")).unwrap();
    let mut api = Automation::new().with_root(dir.path()).unwrap();
    let doc = call(&mut api, "doc_open", json!({"path":"sequential.pdf"}))["doc"].as_u64().unwrap();
    let control = call(&mut api, "doc_open", json!({"path":"control.pdf"}))["doc"].as_u64().unwrap();
    let pixels = api.render_pixels(control, 1, 1.0).unwrap();
    assert!(pixels.2.as_chunks::<4>().0.iter().any(|p| p[0] < 255 || p[1] < 255 || p[2] < 255));
    for stage in ["before", "reopened"] {
        let id = if stage == "before" {
            doc
        } else {
            call(&mut api, "doc_save", json!({"doc":doc,"path":"saved.pdf"}));
            call(&mut api, "doc_open", json!({"path":"saved.pdf"}))["doc"].as_u64().unwrap()
        };
        assert_eq!(call(&mut api, "text_extract", json!({"doc":id}))["pages"], json!([{"page":1,"text":"INSIDE"}]));
        for (query, count) in [("INSIDE", 1), ("HIDDEN", 0), ("OUTSIDE", 0)] {
            let found = call(&mut api, "text_find", json!({"doc":id,"query":query}));
            assert_eq!(found["count"], count, "{stage} {query}");
            assert_eq!(found["complete"], true);
            assert_eq!(found["warning_count"], 0);
        }
        let output = format!("{stage}.txt");
        call(&mut api, "doc_export_text", json!({"doc":id,"path":output}));
        assert_eq!(std::fs::read(dir.path().join(output)).unwrap(), b"INSIDE\n");
        assert_eq!(api.render_pixels(id, 1, 1.0).unwrap().2.as_ref(), pixels.2.as_ref(), "{stage}");
        assert_eq!(call(&mut api, "doc_info", json!({"doc":id}))["document"]["dirty"], false);
    }
    assert_eq!(std::fs::read(dir.path().join("sequential.pdf")).unwrap(), original);
}

fn reachable_streams(bytes: Vec<u8>) -> Vec<Vec<u8>> {
    use pdfcraft_cos::{Document, Object};
    let doc = Document::open(std::sync::Arc::new(bytes)).unwrap();
    let mut queue = vec![doc.trailer().get(b"Root").unwrap().clone()];
    let mut seen = std::collections::HashSet::new();
    let mut streams = Vec::new();
    while let Some(object) = queue.pop() {
        match object {
            Object::Ref(r) => {
                if seen.insert(r) {
                    queue.push((*doc.resolve(&Object::Ref(r))).clone());
                }
            }
            Object::Array(a) => queue.extend(a),
            Object::Dict(d) => queue.extend(d.iter().map(|(_, v)| v.clone())),
            Object::Stream(s) => {
                streams.push(s.decoded_strict().unwrap());
                queue.extend(s.dict.iter().map(|(_, v)| v.clone()));
            }
            _ => {}
        }
    }
    streams
}

#[test]
fn query_redaction_removes_clipped_copies_without_polluting_find() {
    let font = redaction_font();
    for selector in ["find", "words", "pattern"] {
        for warm_find in [false, true] {
            let query = if selector == "pattern" { "x@y.test" } else { "QUERY" };
            let source = fixture_with_font(
                &format!("BT /F1 10 Tf 200 200 Td ({query}) Tj ET"),
                &format!("BT /F1 10 Tf 20 20 Td ({query}) Tj ET /Fm Do"),
                "0 0 50 50",
                "1 0 0 1 0 0",
                &font,
            );
            let count =
                |streams: &[Vec<u8>]| streams.iter().map(|s| s.windows(query.len()).filter(|s| *s == query.as_bytes()).count()).sum::<usize>();
            assert_eq!(count(&reachable_streams(source.clone())), 2);
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("source.pdf"), source).unwrap();
            let mut api = Automation::new().with_root(dir.path()).unwrap();
            let doc = call(&mut api, "doc_open", json!({"path":"source.pdf"}))["doc"].as_u64().unwrap();
            if warm_find {
                assert_eq!(call(&mut api, "text_find", json!({"doc":doc,"query":query}))["count"], 1);
            }
            let mut args = json!({"doc":doc});
            args[selector] = match selector {
                "pattern" => json!("email"),
                "words" => json!([query]),
                _ => json!(query),
            };
            assert_eq!(call(&mut api, "redact_mark", args)["marked"], 2, "{selector}, warm={warm_find}");
            // The preserving result must not enter the visual cache, in either operation order.
            assert_eq!(call(&mut api, "text_find", json!({"doc":doc,"query":query}))["count"], 1);
            let applied = call(&mut api, "redact_apply", json!({"doc":doc}));
            assert_eq!(applied["applied"], 2);
            assert_eq!(applied["marks_pending"], 0);
            call(&mut api, "doc_save", json!({"doc":doc,"path":"saved.pdf","full":true}));
            let reopened = call(&mut api, "doc_open", json!({"path":"saved.pdf"}))["doc"].as_u64().unwrap();
            assert_eq!(call(&mut api, "text_extract", json!({"doc":reopened}))["pages"], json!([{"page":1,"text":""}]));
            assert_eq!(count(&reachable_streams(std::fs::read(dir.path().join("saved.pdf")).unwrap())), 0, "{selector}");
        }
    }
}

fn redaction_font() -> String {
    let widths = "600 ".repeat(95);
    format!(
        "<< /Type /Font /Subtype /Type1 /BaseFont /Courier /Encoding /WinAnsiEncoding /FirstChar 32 /LastChar 126 /Widths [{widths}] /FontDescriptor << /Type /FontDescriptor /FontName /Courier /Flags 33 /FontBBox [-23 -250 715 805] /ItalicAngle 0 /Ascent 629 /Descent -157 /CapHeight 562 /StemV 51 >> >>"
    )
}

#[test]
fn redaction_verification_rejects_retained_clipped_text_under_a_mark() {
    let source = fixture_with_font("BT /F1 10 Tf 200 200 Td (QUERY) Tj ET", "/Fm Do", "0 0 50 50", "1 0 0 1 0 0", &redaction_font());
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("source.pdf"), &source).unwrap();
    let mut api = Automation::new().with_root(dir.path()).unwrap();
    let doc = call(&mut api, "doc_open", json!({"path":"source.pdf"}))["doc"].as_u64().unwrap();
    // View coordinates cover the retained glyphs, while the form BBox misses this mark.
    assert_eq!(call(&mut api, "redact_mark", json!({"doc":doc,"page":1,"rect":[195,85,275,110]}))["marked"], 1);
    let before = call(&mut api, "doc_info", json!({"doc":doc}));
    let error = api.call("redact_apply", &json!({"doc":doc})).unwrap_err();
    let pdfcraft_automation::ToolError::Failed(reason) = error else { panic!("unexpected error: {error:?}") };
    assert_eq!(reason, "redaction could not be verified: 5 item(s) were still found under the marks, so nothing was changed");
    assert_eq!(call(&mut api, "doc_info", json!({"doc":doc})), before);
    assert_eq!(std::fs::read(dir.path().join("source.pdf")).unwrap(), source);
}
