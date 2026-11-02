//! A complete embedded program can have a sparse original PDF text encoding.
use pdfcraft_automation::{Automation, Content};
use pdfcraft_cos::{Dict, Document, Object, Stream, write_incremental};
use serde_json::{Value, json};
use std::sync::Arc;

const INTER: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");

/// Build a PDF with the exact Inter program and either sparse WinAnsi or complete Unicode encoding.
fn fixture(complete_mapping: bool) -> Vec<u8> {
    let bytes = pdfcraft_engine::Session::new().create_blank(600.0, 800.0, 1).unwrap();
    let mut doc = Document::open(bytes).unwrap();
    let page = pdfcraft_model::pages(&doc).remove(0);
    let mut fonts = Dict::new();
    let (resource, encoded) = if complete_mapping {
        let resource = pdfcraft_fonts::embedding::embed(&mut doc, &mut fonts, INTER, "Inter").unwrap();
        let font = doc.dict(fonts.get(resource.as_bytes()).unwrap()).unwrap();
        let metrics = pdfcraft_fonts::pdf::Metrics::from_dict(&doc, &font);
        assert!(metrics.encode("ő").is_some());
        (resource, metrics.encode("ABC").unwrap())
    } else {
        let program = doc.add(Object::Stream(Stream::flate(Dict::new(), INTER)));
        let mut descriptor = Dict::new();
        descriptor.set(b"Type".to_vec(), Object::name("FontDescriptor"));
        descriptor.set(b"FontName".to_vec(), Object::name("Inter-Regular"));
        descriptor.set(b"FontFile2".to_vec(), Object::Ref(program));
        let mut font = Dict::new();
        font.set(b"Type".to_vec(), Object::name("Font"));
        font.set(b"Subtype".to_vec(), Object::name("TrueType"));
        font.set(b"BaseFont".to_vec(), Object::name("Inter-Regular"));
        font.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
        font.set(b"FirstChar".to_vec(), Object::Int(32));
        font.set(b"LastChar".to_vec(), Object::Int(126));
        font.set(b"Widths".to_vec(), Object::Array((32..=126).map(|_| Object::Int(560)).collect()));
        font.set(b"FontDescriptor".to_vec(), Object::Dict(descriptor));
        let metrics = pdfcraft_fonts::pdf::Metrics::from_dict(&doc, &font);
        assert!(metrics.encode("ABC").is_some() && metrics.encode("ő").is_none());
        fonts.set(b"F1".to_vec(), Object::Ref(doc.add(Object::Dict(font))));
        ("F1".into(), b"ABC".to_vec())
    };
    let hex: String = encoded.iter().map(|byte| format!("{byte:02X}")).collect();
    let stream = doc.add(Object::Stream(Stream::flate(Dict::new(), format!("BT /{resource} 16 Tf 1 0 0 1 72 700 Tm <{hex}> Tj ET").as_bytes())));
    let mut resources = Dict::new();
    resources.set(b"Font".to_vec(), Object::Dict(fonts));
    doc.update_dict(page.obj, |page| {
        page.set(b"Resources".to_vec(), Object::Dict(resources));
        page.set(b"Contents".to_vec(), Object::Ref(stream));
    })
    .unwrap();
    write_incremental(&doc, &Default::default()).unwrap()
}

/// Run one automation request and require its first result to be JSON.
fn call(auto: &mut Automation, name: &str, args: Value) -> Value {
    match auto.call(name, &args).unwrap().remove(0) {
        Content::Json(value) => value,
        other => panic!("expected JSON from {name}: {other:?}"),
    }
}

/// Check font-resource count and exact embedded Inter bytes, refusing outline fallback resources.
fn assert_source_fonts(bytes: Vec<u8>, expected: usize) {
    let doc = Document::open(Arc::new(bytes)).unwrap();
    let page = pdfcraft_model::pages(&doc).remove(0);
    let resources = doc.dict(page.dict.get(b"Resources").unwrap()).unwrap();
    let fonts = doc.dict(resources.get(b"Font").unwrap()).unwrap();
    assert_eq!(fonts.len(), expected, "no unnecessary font resource should be added");
    for (_, font) in fonts.iter() {
        let font = doc.dict(font).unwrap();
        assert_ne!(font.name(b"Subtype"), Some(b"Type3".as_slice()), "no fallback outlines");
        assert_eq!(pdfcraft_fonts::pdf::embedded_sfnt(&doc, &font).as_deref(), Some(INTER), "retain the exact complete document program");
    }
}

#[test]
/// Verify sparse RTL encoding completion, complete-source/LTR controls and literal font requests through preview, editing and reopen.
fn rtl_latin_completion_uses_one_full_source_encoding_for_preview_apply_and_reopen() {
    for (complete, direction, expected_fonts) in [(false, "rtl", 2), (true, "rtl", 1), (false, "ltr", 2)] {
        let dir = tempfile::tempdir().unwrap();
        let original = fixture(complete);
        std::fs::write(dir.path().join("source.pdf"), &original).unwrap();
        let mut auto = Automation::new().with_root(dir.path()).unwrap();
        let doc = call(&mut auto, "doc_open", json!({"path":"source.pdf"}))["doc"].clone();
        let before = call(&mut auto, "text_paragraphs", json!({"doc":doc,"page":1}));
        assert_eq!(before["paragraphs"][0]["editor_text"], "ABC");
        let args = json!({"doc":doc,"page":1,"paragraph":1,"text":"ABő","width":400,"direction":direction});
        let id = pdfcraft_engine::DocId(doc.as_u64().unwrap());
        let before_bytes = auto.session().save_bytes(id).unwrap();
        call(&mut auto, "text_edit_layout", args.clone());
        assert_eq!(auto.session().save_bytes(id).unwrap(), before_bytes, "preview preserves the complete document bytes");
        assert_eq!(call(&mut auto, "text_paragraphs", json!({"doc":doc,"page":1})), before, "preview is read-only");
        call(&mut auto, "text_edit", args);
        assert_eq!(call(&mut auto, "text_paragraphs", json!({"doc":doc,"page":1}))["paragraphs"][0]["editor_text"], "ABő");
        let pixels = auto.render_pixels(doc.as_u64().unwrap(), 1, 1.0).unwrap().2;
        call(&mut auto, "doc_save", json!({"doc":doc,"path":"saved.pdf"}));
        let saved = std::fs::read(dir.path().join("saved.pdf")).unwrap();
        assert_source_fonts(saved.clone(), expected_fonts);
        let reopened = call(&mut auto, "doc_open", json!({"path":"saved.pdf"}))["doc"].clone();
        assert_eq!(call(&mut auto, "text_paragraphs", json!({"doc":reopened,"page":1}))["paragraphs"][0]["editor_text"], "ABő");
        assert_eq!(auto.render_pixels(reopened.as_u64().unwrap(), 1, 1.0).unwrap().2, pixels);
        if direction == "rtl" {
            assert!(
                auto.call("text_edit", &json!({"doc":reopened,"page":1,"paragraph":1,"ranges":[{"range":[0,1],"color":"#cc0000"}]})).is_err(),
                "mixed-style RTL rows remain unsupported"
            );
        }
        if !complete {
            let explicit = call(&mut auto, "doc_open", json!({"path":"source.pdf"}))["doc"].clone();
            assert!(
                auto.call("text_edit", &json!({"doc":explicit,"page":1,"paragraph":1,"text":"ABő","direction":direction,"font_resource":"F1"}))
                    .is_err(),
                "an explicit sparse resource must not silently switch fonts"
            );
        }
    }
}
