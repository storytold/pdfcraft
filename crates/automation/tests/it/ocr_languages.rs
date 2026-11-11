//! Russian OCR through the same tools the CLI and native interface use. Fixtures are
//! generated from the project's fonts; no scan or recognition model is bundled here.

use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

fn ok(auto: &mut Automation, tool: &str, args: Value) -> Value {
    match auto.call(tool, &args).unwrap().remove(0) {
        Content::Json(value) => value,
        _ => panic!("expected JSON from {tool}"),
    }
}

fn pixels(auto: &mut Automation, doc: u64) -> image::RgbaImage {
    match auto.call("page_render", &json!({ "doc": doc, "page": 1, "dpi": 72 })).unwrap().remove(0) {
        Content::Png { data, .. } => image::load_from_memory_with_format(&data, image::ImageFormat::Png).unwrap().to_rgba8(),
        _ => panic!("expected a page image"),
    }
}

#[test]
fn russian_tools_refuse_the_latin_only_engine_before_editing() {
    let mut auto = Automation::new();
    let doc = ok(&mut auto, "doc_create", json!({ "from": "blank" }))["doc"].as_u64().unwrap();
    for language in ["ru", "en+ru"] {
        let why = auto.call("ocr_recognize", &json!({ "doc": doc, "language": language, "engine": "ocrs" })).unwrap_err();
        assert!(why.to_string().contains("only English"), "{why}");
    }
    assert!(auto.call("edit_undo", &json!({ "doc": doc })).is_err(), "a refused run makes no edit");
}

#[test]
fn mixed_russian_dictionary_uses_its_language_filename() {
    let dir = tempfile::tempdir().unwrap();
    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let dictionary = ok(&mut auto, "ocr_dictionary", json!({ "path": "ocr-dictionary-en+ru.txt", "add": ["Русский", "English"] }));
    assert_eq!(dictionary["words"], json!(["english", "русский"]));
    let reopened = ok(&mut auto, "ocr_dictionary", json!({ "path": "ocr-dictionary-en+ru.txt" }));
    assert_eq!(reopened["words"], dictionary["words"]);
}

/// Run when Tesseract and both rus/eng data files are installed. Without those optional
/// runtime dependencies the language-status assertions still run and the recognition is skipped.
#[test]
fn russian_and_mixed_ocr_survive_save_reopen_and_batch_without_changing_scan_pixels() {
    let dir = tempfile::tempdir().unwrap();
    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let status = ok(&mut auto, "ocr_status", json!({}));
    let languages = status["languages"].as_array().unwrap();
    for code in ["ru", "en+ru"] {
        let language = languages.iter().find(|entry| entry["code"] == code).expect("Russian language is offered");
        assert!(language["available"].is_boolean(), "{status}");
        if language["available"] != true {
            assert!(language["unavailable"].as_str().is_some_and(|why| !why.is_empty()), "{status}");
            eprintln!("skipped recognition: {}", language["unavailable"]);
            return;
        }
    }
    let typed = ok(&mut auto, "doc_create", json!({ "from": "text", "text": "Русский текст\nEnglish text" }))["doc"].as_u64().unwrap();
    ok(&mut auto, "doc_export_images", json!({ "doc": typed, "folder": "scan", "dpi": 300, "pages": [1] }));
    let png = std::fs::read_dir(dir.path().join("scan")).unwrap().next().unwrap().unwrap().path();
    let scan = ok(&mut auto, "doc_create", json!({ "from": "images", "paths": [png.to_string_lossy()], "dpi": 300 }))["doc"].as_u64().unwrap();
    ok(&mut auto, "doc_save", json!({ "doc": scan, "path": "source.pdf" }));
    let original = std::fs::read(dir.path().join("source.pdf")).unwrap();
    let before = pixels(&mut auto, scan);

    let words = ok(&mut auto, "ocr_words", json!({ "doc": scan, "language": "en+ru" }));
    let words = words["pages"][0]["words"].as_array().unwrap();
    assert!(!words.is_empty());
    assert!(words.iter().all(|word| word["source"] == "tesseract"), "{words:?}");
    let reading = ok(&mut auto, "ocr_recognize", json!({ "doc": scan, "language": "en+ru", "engine": "auto" }));
    let text = reading["pages"][0]["text"].as_str().unwrap();
    assert!(text.contains("Русский") && text.contains("English"), "{reading}");
    ok(&mut auto, "doc_save", json!({ "doc": scan, "path": "searchable.pdf" }));
    let reopened = ok(&mut auto, "doc_open", json!({ "path": "searchable.pdf" }))["doc"].as_u64().unwrap();
    for query in ["Русский", "English"] {
        let found = ok(&mut auto, "text_find", json!({ "doc": reopened, "query": query }));
        assert!(found["count"].as_u64().unwrap() > 0, "{query}: {found}");
    }
    assert_eq!(before, pixels(&mut auto, reopened), "the invisible Unicode layer does not repaint the scan");
    assert_eq!(original, std::fs::read(dir.path().join("source.pdf")).unwrap());

    // The batch CLI exposes a language but no engine option. The shared settings must
    // choose Tesseract for Russian when no engine is explicitly named.
    let batch = ok(&mut auto, "doc_batch", json!({ "paths": ["source.pdf"], "steps": ["ocr"], "out_dir": "batch", "language": "ru" }));
    assert_eq!(batch["completed"], 1, "{batch}");
    assert_eq!(batch["failed"], 0, "{batch}");
    let result = ok(&mut auto, "doc_open", json!({ "path": "batch/source.pdf" }))["doc"].as_u64().unwrap();
    let found = ok(&mut auto, "text_find", json!({ "doc": result, "query": "Русский" }));
    assert!(found["count"].as_u64().unwrap() > 0, "{found}");
    assert_eq!(before, pixels(&mut auto, result));
    assert_eq!(original, std::fs::read(dir.path().join("source.pdf")).unwrap());
}
