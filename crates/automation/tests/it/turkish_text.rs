//! Turkish page content must remain drawable, searchable and editable after saving.
//! Synthetic documents exercise the public CLI/MCP tool table without installed fonts.

use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

const ALPHABET: &str = "ç Ç ğ Ğ ı I i İ ö Ö ş Ş ü Ü â î û";
const WORDS: &str = "İstanbul Iğdır Çağrı Şişli Öğrenci GÜNEŞ ışık İŞ SÖZLEŞMESİ";

#[test]
fn manual_word_candidates_tool_is_read_only_and_requires_a_retained_review() {
    let defs = pdfcraft_automation::tools();
    let def = defs.iter().find(|t| t.name == "ocr_word_candidates").unwrap();
    assert!(def.read_only);
    assert!(!def.destructive);
    assert_eq!(def.input_schema["required"], json!(["doc", "review", "page", "word"]));
    assert_eq!(def.input_schema["additionalProperties"], false);
    let mut a = Automation::new();
    let doc = tool(&mut a, "doc_create", json!({"from":"blank"}))["doc"].as_u64().unwrap();
    let before = a.render_pixels(doc, 1, 1.0).unwrap();
    let error = a.call("ocr_word_candidates", &json!({"doc":doc,"review":1,"page":1,"word":1})).unwrap_err().to_string();
    assert!(error.contains("unknown OCR review"), "{error}");
    assert!(a.call("ocr_word_candidates", &json!({"doc":doc,"review":1,"page":0,"word":1})).is_err());
    assert!(a.call("ocr_word_candidates", &json!({"doc":doc,"review":1,"page":1,"word":1,"model_profile":"standard"})).is_err());
    assert_eq!(a.render_pixels(doc, 1, 1.0).unwrap(), before);
}

#[test]
fn turkish_ocr_status_reports_the_selected_language_and_engine() {
    let mut a = Automation::new();
    let general = tool(&mut a, "ocr_status", json!({}));
    assert!(general["available"].is_boolean());
    assert!(general.get("engine").is_none(), "no-argument status remains the general inventory");
    assert!(general["languages"].as_array().unwrap().iter().any(|l| l["code"] == "tr"));
    let unsupported = tool(&mut a, "ocr_status", json!({"language":"tr","engine":"ocrs"}));
    assert_eq!(unsupported["available"], false, "English OCR availability must never mean Turkish OCR works");
    assert_eq!(unsupported["language"], "tr");
    assert_eq!(unsupported["engine"], "ocrs");
    assert!(unsupported["unavailable"].as_str().unwrap().contains("Turkish"), "{unsupported}");
    let automatic = tool(&mut a, "ocr_status", json!({"language":"tr"}));
    assert_eq!(automatic["engine"], "auto", "status checks any capable reader when no engine is named");
    for engine in ["auto", "tesseract"] {
        let status = tool(&mut a, "ocr_status", json!({"language":"tr","engine":engine}));
        assert_eq!(status["language"], "tr");
        assert_eq!(status["engine"], engine);
        assert!(status["available"].is_boolean());
        if status["available"] == false {
            assert!(status["unavailable"].as_str().is_some_and(|why| !why.is_empty()), "{status}");
        }
    }
    assert!(a.call("ocr_status", &json!({"language":"xx","engine":"auto"})).is_err());
    assert!(a.call("ocr_status", &json!({"language":"tr","engine":"unknown"})).is_err());
}

fn tool(a: &mut Automation, name: &str, args: Value) -> Value {
    match a.call(name, &args) {
        Ok(mut result) => match result.remove(0) {
            Content::Json(value) => value,
            other => panic!("{name}: expected JSON, got {other:?}"),
        },
        Err(error) => panic!("{name} {args}: {error}"),
    }
}

fn page_text(a: &mut Automation, doc: u64, page: u64) -> String {
    tool(a, "text_extract", json!({"doc": doc, "pages": [page]}))["pages"][0]["text"].as_str().unwrap().trim().to_owned()
}

fn assert_found_once(a: &mut Automation, doc: u64, query: &str) {
    let found = tool(a, "text_find", json!({"doc": doc, "query": query}));
    assert_eq!(found["count"], 1, "{query}: {found}");
    assert_eq!(found["complete"], true, "{query}: {found}");
    assert_eq!(found["warning_count"], 0, "{query}: {found}");
}

#[test]
fn turkish_added_text_round_trips_with_search_and_render() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap().with_clock(|| 1_700_000_000);
    let doc = tool(&mut a, "doc_create", json!({"from": "blank", "width": 800, "height": 600}))["doc"].as_u64().unwrap();
    let blank = a.render_pixels(doc, 1, 1.0).unwrap();
    let text = format!("{ALPHABET}\n{WORDS}\nPDF 2026: Türkçe + English (123,45)");
    tool(&mut a, "page_add_text", json!({"doc": doc, "page": 1, "text": text, "at": [30, 40], "width": 740, "size": 14}));
    assert_eq!(page_text(&mut a, doc, 1), text);
    for query in ["Çağrı", "Öğrenci", "ışık", "İŞ SÖZLEŞMESİ"] {
        assert_found_once(&mut a, doc, query);
    }
    let drawn = a.render_pixels(doc, 1, 1.0).unwrap();
    assert_ne!(drawn, blank, "text must reach the actual page appearance");
    let lines = tool(&mut a, "text_lines", json!({"doc": doc, "page": 1}));
    // Unicode file specifications exercise Windows paths as well as the PDF's text encoding.
    let filename = "İş sözleşmesi – Şubat.pdf";
    tool(&mut a, "doc_save", json!({"doc": doc, "path": filename}));
    assert!(dir.path().join(filename).is_file());

    // A fresh session must recover text and appearance from the PDF alone.
    let mut b = Automation::new().with_root(dir.path()).unwrap().with_clock(|| 1_700_000_000);
    let reopened = tool(&mut b, "doc_open", json!({"path": filename}))["doc"].as_u64().unwrap();
    assert_eq!(page_text(&mut b, reopened, 1), text);
    assert_eq!(tool(&mut b, "text_lines", json!({"doc": reopened, "page": 1})), lines);
    assert_eq!(b.render_pixels(reopened, 1, 1.0).unwrap(), drawn);
    for query in ["Çağrı", "Öğrenci", "ışık", "İŞ SÖZLEŞMESİ"] {
        assert_found_once(&mut b, reopened, query);
    }
}

#[test]
fn turkish_added_text_retyping_undo_preserves_neighboring_content() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap().with_clock(|| 1_700_000_000);
    let doc = tool(&mut a, "doc_create", json!({"from": "blank", "width": 800, "height": 600}))["doc"].as_u64().unwrap();
    let neighbor = "Existing ASCII invoice 123.45";
    tool(&mut a, "page_add_text", json!({"doc": doc, "page": 1, "text": neighbor, "at": [30, 40], "width": 740}));
    let original = "Çağrı İstanbul Iğdır";
    tool(&mut a, "page_add_text", json!({"doc": doc, "page": 1, "text": original, "at": [30, 120], "width": 740, "size": 16}));
    let before = a.render_pixels(doc, 1, 1.0).unwrap();
    let before_items = tool(&mut a, "content_list", json!({"doc": doc}));
    let replacement = "Şişli Öğrenci GÜNEŞ ışık çığ";
    tool(&mut a, "content_update", json!({"doc": doc, "page": 1, "index": 2, "text": replacement}));
    assert_eq!(page_text(&mut a, doc, 1), format!("{neighbor}\n{replacement}"));
    assert_found_once(&mut a, doc, neighbor);
    assert_found_once(&mut a, doc, replacement);
    assert_eq!(tool(&mut a, "text_find", json!({"doc": doc, "query": original}))["count"], 0);
    let after = a.render_pixels(doc, 1, 1.0).unwrap();
    assert_ne!(after, before, "retyping must replace the displayed glyphs");

    tool(&mut a, "edit_undo", json!({"doc": doc}));
    assert_eq!(page_text(&mut a, doc, 1), format!("{neighbor}\n{original}"));
    assert_eq!(tool(&mut a, "content_list", json!({"doc": doc})), before_items);
    assert_eq!(a.render_pixels(doc, 1, 1.0).unwrap(), before);
    tool(&mut a, "edit_redo", json!({"doc": doc}));
    assert_eq!(page_text(&mut a, doc, 1), format!("{neighbor}\n{replacement}"));
    assert_eq!(a.render_pixels(doc, 1, 1.0).unwrap(), after);
    tool(&mut a, "doc_save", json!({"doc": doc, "path": "retyped.pdf"}));

    let mut b = Automation::new().with_root(dir.path()).unwrap().with_clock(|| 1_700_000_000);
    let reopened = tool(&mut b, "doc_open", json!({"path": "retyped.pdf"}))["doc"].as_u64().unwrap();
    assert_eq!(page_text(&mut b, reopened, 1), format!("{neighbor}\n{replacement}"));
    assert_eq!(b.render_pixels(reopened, 1, 1.0).unwrap(), after);
    // Both incremental and full saves must keep the non-ASCII resource usable for another edit.
    for full in [false, true] {
        let updated = format!("{ALPHABET} — {}", if full { "tam kayıt" } else { "artımlı kayıt" });
        tool(&mut b, "content_update", json!({"doc": reopened, "page": 1, "index": 2, "text": updated}));
        let saved = tool(&mut b, "doc_save", json!({"doc": reopened, "full": full}));
        assert_eq!(saved["incremental"], !full);
        let mut check = Automation::new().with_root(dir.path()).unwrap();
        let check_doc = tool(&mut check, "doc_open", json!({"path": "retyped.pdf"}))["doc"].as_u64().unwrap();
        assert_eq!(page_text(&mut check, check_doc, 1), format!("{neighbor}\n{updated}"));
        assert_found_once(&mut check, check_doc, neighbor);
    }
}

#[test]
fn turkish_added_text_keeps_letters_for_standard_font_and_style_choices() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let doc = tool(&mut a, "doc_create", json!({"from": "blank", "width": 800, "height": 600, "pages": 12}))["doc"].as_u64().unwrap();
    let mut page = 1;
    for family in ["helvetica", "times", "courier"] {
        for (bold, italic) in [(false, false), (true, false), (false, true), (true, true)] {
            tool(
                &mut a,
                "page_add_text",
                json!({"doc": doc, "page": page, "text": ALPHABET, "at": [30, 40], "width": 740, "size": 16, "font": family, "bold": bold, "italic": italic}),
            );
            assert_eq!(page_text(&mut a, doc, page), ALPHABET, "{family}, bold={bold}, italic={italic}");
            page += 1;
        }
    }
    let items = tool(&mut a, "content_list", json!({"doc": doc}));
    tool(&mut a, "doc_save", json!({"doc": doc, "path": "styles.pdf"}));
    let mut b = Automation::new().with_root(dir.path()).unwrap();
    let reopened = tool(&mut b, "doc_open", json!({"path": "styles.pdf"}))["doc"].as_u64().unwrap();
    assert_eq!(tool(&mut b, "content_list", json!({"doc": reopened})), items, "source style choices survive Unicode fallback");
    for page in 1..=12 {
        assert_eq!(page_text(&mut b, reopened, page), ALPHABET, "page {page}");
        assert_eq!(b.render_pixels(reopened, page as usize, 1.0).unwrap(), a.render_pixels(doc, page as usize, 1.0).unwrap(), "page {page}");
    }
}
