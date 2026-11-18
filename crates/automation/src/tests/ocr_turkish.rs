//! Deterministic review acceptance: recognition accuracy is measured separately with Tesseract.
//! Retain synthetic words just as ocr_words does, then exercise the public CLI/MCP tool table.

use super::*;
use pdfcraft_cos::Object;
use pdfcraft_engine::ocr::{OcrPage, PlacedWord};

const CORRECTED: &str = "ılık İstanbul Iğdır Çağrı Şişli Öğrenci GÜNEŞ ışık İŞ SÖZLEŞMESİ çÇğĞıIiİöÖşŞüÜâÂîÎûÛ";

fn scan(a: &mut Automation) -> DocId {
    // A synthetic scan with a dark border; it contains no text or third-party assets.
    let pixels: Vec<u8> = (0..400).flat_map(|y| (0..800).map(move |x| if x < 3 || y < 3 { 30 } else { 255 })).collect();
    let image = craft_image_codecs::Image::from_u8(800, 400, craft_image_codecs::ChannelLayout::Gray, pixels).unwrap();
    let png = craft_image_codecs::encode(&image, craft_image_codecs::Format::Png, &craft_image_codecs::EncodeOptions::default()).unwrap();
    std::fs::write(a.root.as_ref().unwrap().join("scan.png"), png).unwrap();
    let id = json(a, "doc_create", json!({"from":"images","paths":["scan.png","scan.png"]}))["doc"].as_u64().unwrap();
    DocId(id)
}

fn retain(a: &mut Automation, id: DocId, text: &str) -> u64 {
    let guard = a.session.ocr_guard(id).unwrap();
    a.next_ocr_review += 1;
    let handle = a.next_ocr_review;
    let words = vec![PlacedWord {
        text: text.into(),
        origin: [20.0, 60.0],
        across: [740.0, 0.0],
        up: [0.0, 18.0],
        confidence: Some(45.0),
        source: "synthetic".into(),
    }];
    let pages = vec![OcrPage { page: 0, words, already_on_page: 0, skipped: None, notes: vec![] }];
    a.ocr_reviews.insert(id, OcrReview { handle, guard, pages, settings: Default::default() });
    handle
}

fn text(a: &mut Automation, id: DocId) -> String {
    json(a, "text_extract", json!({"doc":id.0,"pages":[1]}))["pages"][0]["text"].as_str().unwrap().trim().into()
}

fn found(a: &mut Automation, id: DocId) {
    for query in ["ılık", "Iğdır", "Çağrı", "İŞ SÖZLEŞMESİ", "ğĞıIiİ"] {
        let result = json(a, "text_find", json!({"doc":id.0,"query":query}));
        assert_eq!(result["count"], 1, "{query}: {result}");
        assert_eq!(result["complete"], true, "{result}");
    }
}

#[test]
fn corrected_turkish_searchable_review_is_exact_invisible_and_one_undo_step() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap().with_clock(|| 1_700_000_000);
    let id = scan(&mut a);
    let before = a.render_pixels(id.0, 1, 1.0).unwrap();
    let review = retain(&mut a, id, "Ilık");
    let result = json(&mut a, "ocr_review_apply", json!({"doc":id.0,"review":review,"corrections":[{"page":1,"word":1,"text":CORRECTED}]}));
    assert_eq!(result["applied"], 1);
    assert_eq!(text(&mut a, id), CORRECTED);
    found(&mut a, id);
    assert_eq!(a.render_pixels(id.0, 1, 1.0).unwrap(), before, "searchable output does not draw corrected text");
    assert!(a.call("ocr_review_apply", &json!({"doc":id.0,"review":review})).is_err(), "review is consumed once");
    json(&mut a, "edit_undo", json!({"doc":id.0}));
    assert_eq!(text(&mut a, id), "");
    assert!(a.call("edit_undo", &json!({"doc":id.0})).is_err(), "the whole reviewed layer has one undo step");
    json(&mut a, "edit_redo", json!({"doc":id.0}));
    assert_eq!(text(&mut a, id), CORRECTED);
    json(&mut a, "doc_save", json!({"doc":id.0,"path":"Düzeltilmiş – ışık.pdf"}));
    let mut b = Automation::new().with_root(dir.path()).unwrap();
    let re = DocId(json(&mut b, "doc_open", json!({"path":"Düzeltilmiş – ışık.pdf"}))["doc"].as_u64().unwrap());
    assert_eq!(text(&mut b, re), CORRECTED);
    found(&mut b, re);
    assert_eq!(b.render_pixels(re.0, 1, 1.0).unwrap(), before);
}

#[test]
fn corrected_turkish_editable_review_writes_visible_glyphs_to_a_new_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap().with_clock(|| 1_700_000_000);
    let id = scan(&mut a);
    json(&mut a, "doc_save", json!({"doc":id.0,"path":"Kaynak.pdf"}));
    let source = a.session.save_bytes(id).unwrap();
    let before = a.render_pixels(id.0, 1, 1.0).unwrap();
    let review = retain(&mut a, id, "Ilık");
    std::fs::write(dir.path().join("exists.pdf"), b"keep this file").unwrap();
    for path in ["Kaynak.pdf", "exists.pdf", "../outside.pdf"] {
        assert!(a.call("ocr_review_apply", &json!({"doc":id.0,"review":review,"output_mode":"editable","out":path})).is_err(), "{path}");
        assert!(a.ocr_reviews.contains_key(&id), "a refused output keeps the review");
        assert_eq!(a.session.save_bytes(id).unwrap(), source);
    }
    assert_eq!(std::fs::read(dir.path().join("exists.pdf")).unwrap(), b"keep this file");
    assert_eq!(std::fs::read(dir.path().join("Kaynak.pdf")).unwrap().as_slice(), source.as_slice());
    json(
        &mut a,
        "ocr_review_apply",
        json!({"doc":id.0,"review":review,"output_mode":"editable","out":"Çağrı.pdf","corrections":[{"page":1,"word":1,"text":CORRECTED}]}),
    );
    assert_eq!(a.session.save_bytes(id).unwrap(), source, "editable output must leave source bytes alone");
    assert_eq!(text(&mut a, id), "");
    assert!(a.call("edit_undo", &json!({"doc":id.0})).is_err(), "new-file export never edits the source");
    let mut b = Automation::new().with_root(dir.path()).unwrap();
    let re = DocId(json(&mut b, "doc_open", json!({"path":"Çağrı.pdf"}))["doc"].as_u64().unwrap());
    assert_eq!(text(&mut b, re), CORRECTED);
    found(&mut b, re);
    assert_ne!(b.render_pixels(re.0, 1, 1.0).unwrap(), before, "real visible ink must be produced");
    assert_eq!(json(&mut b, "text_extract", json!({"doc":re.0,"pages":[2]}))["pages"][0]["text"], "", "blank source pages stay blank");
    let doc = pdfcraft_cos::Document::open(Arc::new(std::fs::read(dir.path().join("Çağrı.pdf")).unwrap())).unwrap();
    let mut drawn_turkish = std::collections::BTreeSet::new();
    for number in doc.object_numbers() {
        let object = doc.get(pdfcraft_cos::ObjRef::new(number, doc.generation(number)));
        let Some(font) = object.as_dict() else { continue };
        if font.name(b"Subtype") != Some(b"Type3") {
            continue;
        }
        let map = font.get(b"ToUnicode").map(|r| doc.resolve(r)).unwrap();
        let Object::Stream(map) = &*map else { panic!("Type3 ToUnicode is a stream") };
        let map = String::from_utf8(map.decoded().unwrap()).unwrap();
        let procs = font.get(b"CharProcs").map(|r| doc.resolve(r)).unwrap();
        let procs = procs.as_dict().unwrap();
        for ch in "ğĞıİşŞ".chars() {
            let target = format!("<{:04X}>", ch as u32);
            let Some(line) = map.lines().find(|line| line.ends_with(&target)) else { continue };
            let code = line.split_whitespace().next().unwrap().trim_matches(['<', '>']);
            let glyph = procs.get(format!("g{code}").as_bytes()).map(|r| doc.resolve(r)).unwrap();
            let Object::Stream(glyph) = &*glyph else { panic!("Type3 CharProc is a stream") };
            let commands = String::from_utf8(glyph.decoded().unwrap()).unwrap();
            assert!(commands.contains(" m\n") && commands.contains("f\n"), "{ch} must have real outline drawing commands: {commands}");
            drawn_turkish.insert(ch);
        }
    }
    assert_eq!(drawn_turkish.len(), 6, "ToUnicode alone is insufficient: every Turkish fallback glyph must draw");
}

#[test]
fn unsupported_mark_review_is_preserved_searchably_but_editable_refuses_atomically() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let id = scan(&mut a);
    // Supported Latin NFD is covered by ocr_editable_nfd. Keep the refusal oracle
    // on a mark outside that supported range, which invisible OCR still preserves.
    let nfd = "C\u{0327}ag\u{0306}rı I\u{0307}stanbul\u{1ab0}";
    let before = a.session.save_bytes(id).unwrap();
    let review = retain(&mut a, id, nfd);
    let error =
        a.call("ocr_review_apply", &json!({"doc":id.0,"review":review,"output_mode":"editable","out":"unshaped.pdf"})).unwrap_err().to_string();
    assert!(error.contains("U+1AB0"), "{error}");
    assert!(!dir.path().join("unshaped.pdf").exists());
    assert_eq!(a.session.save_bytes(id).unwrap(), before);
    assert!(a.ocr_reviews.contains_key(&id), "failed export retains the review");
    json(&mut a, "ocr_review_apply", json!({"doc":id.0,"review":review}));
    assert_eq!(text(&mut a, id), nfd, "source Unicode must not be normalized behind the user's back");
    json(&mut a, "doc_save", json!({"doc":id.0,"path":"decomposed.pdf"}));
    let mut b = Automation::new().with_root(dir.path()).unwrap();
    let re = DocId(json(&mut b, "doc_open", json!({"path":"decomposed.pdf"}))["doc"].as_u64().unwrap());
    assert_eq!(text(&mut b, re), nfd);
}
