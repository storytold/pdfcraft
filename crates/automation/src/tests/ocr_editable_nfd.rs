//! Retained synthetic recognition isolates correction fidelity from recognizer accuracy.
//! These tests use the CLI/MCP tool table, temporary original fixtures and embedded fonts;
//! they do not discover Tesseract, system fonts, a GUI, or the operating-system clipboard.
use super::*;
use pdfcraft_engine::ocr::{OcrPage, PlacedWord};

const NFC: &str = "Çağrı İstanbul Şişli Öğrenci GÜNEŞ ığdır Âğîşû çÇğĞıIiİöÖşŞüÜâÂîÎûÛ";
const NFD: &str = "C\u{327}ag\u{306}rı I\u{307}stanbul S\u{327}is\u{327}li O\u{308}g\u{306}renci GU\u{308}NES\u{327} ıg\u{306}dır A\u{302}g\u{306}i\u{302}s\u{327}u\u{302} c\u{327}C\u{327}g\u{306}G\u{306}ıIiI\u{307}o\u{308}O\u{308}s\u{327}S\u{327}u\u{308}U\u{308}a\u{302}A\u{302}i\u{302}I\u{302}u\u{302}U\u{302}";

fn scan(a: &mut Automation) -> DocId {
    // An original, textless raster makes a copied scan distinguishable from visible text.
    let pixels: Vec<u8> = (0..400).flat_map(|y| (0..800).map(move |x| if x < 3 || y < 3 { 30 } else { 255 })).collect();
    let image = craft_image_codecs::Image::from_u8(800, 400, craft_image_codecs::ChannelLayout::Gray, pixels).unwrap();
    let png = craft_image_codecs::encode(&image, craft_image_codecs::Format::Png, &craft_image_codecs::EncodeOptions::default()).unwrap();
    std::fs::write(a.root.as_ref().unwrap().join("scan.png"), png).unwrap();
    DocId(json(a, "doc_create", json!({"from":"images","paths":["scan.png","scan.png","scan.png"]}))["doc"].as_u64().unwrap())
}

fn placed(a: &Automation, id: DocId, page: usize, text: &str, x: f32, width: f32) -> PlacedWord {
    let info = &a.session.get(id).unwrap().info.pages[page];
    // Recognition stores PDF user-space boxes; every rotated fixture has the same view box.
    let (o, right, top) = (info.view_to_user(x, 80.0), info.view_to_user(x + width, 80.0), info.view_to_user(x, 60.0));
    PlacedWord {
        text: text.into(),
        origin: o.map(f64::from),
        across: [f64::from(right[0] - o[0]), f64::from(right[1] - o[1])],
        up: [f64::from(top[0] - o[0]), f64::from(top[1] - o[1])],
        confidence: Some(45.0),
        source: "synthetic".into(),
    }
}

fn page(page: usize, words: Vec<PlacedWord>) -> OcrPage {
    OcrPage { page, words, already_on_page: 0, skipped: None, notes: Vec::new() }
}

fn retain(a: &mut Automation, id: DocId, pages: Vec<OcrPage>) -> u64 {
    // Same retained guard/result boundary as ocr_words(retain_review=true), with no provider.
    let guard = a.session.ocr_guard(id).unwrap();
    a.next_ocr_review += 1;
    let handle = a.next_ocr_review;
    a.ocr_reviews.insert(id, OcrReview { handle, guard, pages, settings: Default::default() });
    handle
}

fn retain_word(a: &mut Automation, id: DocId, text: &str) -> u64 {
    let word = placed(a, id, 0, text, 20.0, 700.0);
    retain(a, id, vec![page(0, vec![word])])
}

fn text(a: &mut Automation, id: DocId, page: usize) -> String {
    // Only the extractor's outside line whitespace is tolerated. Internal spaces, marks,
    // scalar order and normalization must be exact; never split_whitespace or normalize.
    json(a, "text_extract", json!({"doc":id.0,"pages":[page]}))["pages"][0]["text"].as_str().unwrap().trim_matches(['\r', '\n']).to_owned()
}

fn open(a: &mut Automation, path: &str) -> DocId {
    DocId(json(a, "doc_open", json!({"path":path}))["doc"].as_u64().unwrap())
}

fn editable(a: &mut Automation, id: DocId, review: u64, out: &str, correction: Option<&str>) -> Value {
    let mut args = json!({"doc":id.0,"review":review,"output_mode":"editable","out":out});
    if let Some(text) = correction {
        args["corrections"] = json!([{"page":1,"word":1,"text":text}]);
    }
    json(a, "ocr_review_apply", args)
}

fn assert_search_and_copy(a: &mut Automation, id: DocId, original: &str) {
    for (query, expected) in [("Çağrı", "C\u{327}ag\u{306}rı"), ("İstanbul", "I\u{307}stanbul"), ("Şişli", "S\u{327}is\u{327}li")] {
        let found = json(a, "text_find", json!({"doc":id.0,"query":query,"language":"tr","case_sensitive":true,"whole_words":true}));
        assert_eq!(found["count"], 1, "{query}: {found}");
        assert_eq!(found["complete"], true, "{found}");
        assert_eq!(found["warning_count"], 0, "{found}");
        assert_eq!(found["matches"][0]["text"], if original == NFD { expected } else { query });
        let rects = found["matches"][0]["rects"].as_array().unwrap();
        assert!(!rects.is_empty());
        for rect in rects {
            let r: Vec<f64> = rect.as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
            assert!(r.iter().all(|v| v.is_finite()) && r[2] > r[0] && r[3] > r[1], "{rect}");
        }
    }
    let page = a.page_texts(id, &[0]).unwrap().remove(0);
    assert_eq!(page.text_of(0..page.glyphs.len()), original, "selection/copy must retain original Unicode");
    let info = a.session.get(id).unwrap().info.pages[0].clone();
    let copied = json(a, "text_extract", json!({"doc":id.0,"pages":[1],"rect":[0,0,info.width,info.height]}));
    assert_eq!(copied["pages"][0]["text"], original, "column selection does not normalize");
}

fn assert_nfd_cluster_maps_draw(bytes: Arc<Vec<u8>>) {
    use pdfcraft_cos::{Document, ObjRef, Object};

    // ActualText alone could hide a normalized/incorrect ToUnicode map or blank glyph.
    let doc = Document::open(bytes).unwrap();
    let mut drawn = std::collections::BTreeSet::new();
    let expected = ["00430327", "00670306", "00490307", "00530327", "006F0308", "00750308", "00410302"];
    for number in doc.object_numbers() {
        let object = doc.get(ObjRef::new(number, doc.generation(number)));
        let Some(font) = object.as_dict().filter(|d| d.name(b"Subtype") == Some(b"Type3")) else { continue };
        let map = doc.resolve(font.get(b"ToUnicode").unwrap());
        let Object::Stream(map) = &*map else { panic!("Type3 ToUnicode must be a stream") };
        let map = String::from_utf8(map.decoded().unwrap()).unwrap();
        let procs = doc.resolve(font.get(b"CharProcs").unwrap());
        let procs = procs.as_dict().unwrap();
        for sequence in expected {
            let target = format!("<{sequence}>");
            let Some(line) = map.lines().find(|line| line.ends_with(&target)) else { continue };
            let code = line.split_whitespace().next().unwrap().trim_matches(['<', '>']);
            let glyph = doc.resolve(procs.get(format!("g{code}").as_bytes()).unwrap());
            let Object::Stream(glyph) = &*glyph else { panic!("Type3 CharProc must be a stream") };
            let commands = String::from_utf8(glyph.decoded().unwrap()).unwrap();
            assert!(commands.contains(" m\n") && commands.contains("f\n"), "{sequence}: mapped cluster must draw real outlines");
            drawn.insert(sequence);
        }
    }
    assert_eq!(drawn, expected.into_iter().collect(), "base/mark sequences must survive in ToUnicode as well as ActualText");
}

fn correction_roundtrip(corrected: &str, path: &str) {
    let dir = tempfile::tempdir().unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap().with_clock(|| 1_700_000_000);
    let id = scan(&mut a);
    json(&mut a, "doc_save", json!({"doc":id.0,"path":"Kaynak – ığdır.pdf"}));
    let before = a.session.save_bytes(id).unwrap();
    let pixels = a.render_pixels(id.0, 1, 1.0).unwrap();
    let review = retain_word(&mut a, id, "Ilik");
    let result = editable(&mut a, id, review, path, Some(corrected));
    assert_eq!(result["words"], 1);
    assert!(!a.ocr_reviews.contains_key(&id));
    assert!(a.call("ocr_review_apply", &json!({"doc":id.0,"review":review})).is_err(), "a successful review is consumed once");
    assert_eq!(a.session.save_bytes(id).unwrap(), before, "editable export leaves the open source unchanged");
    assert_eq!(std::fs::read(dir.path().join("Kaynak – ığdır.pdf")).unwrap().as_slice(), before.as_slice());
    assert_eq!(text(&mut a, id, 1), "");
    assert!(a.render_pixels(id.0, 1, 1.0).unwrap() == pixels);
    assert!(a.call("edit_undo", &json!({"doc":id.0})).is_err(), "export creates no source undo step");
    assert!(a.call("edit_redo", &json!({"doc":id.0})).is_err(), "export creates no source redo step");

    let mut reopened = Automation::new().with_root(dir.path()).unwrap();
    let out = open(&mut reopened, path);
    assert_eq!(text(&mut reopened, out, 1), corrected);
    assert_search_and_copy(&mut reopened, out, corrected);
    let ink = reopened.render_pixels(out.0, 1, 1.0).unwrap();
    assert!(ink != pixels, "editable output must draw text");
    assert!(ink.2.chunks_exact(4).any(|p| p[0] < 128 && p[1] < 128 && p[2] < 128), "visible output must not be an empty white page");
    if corrected == NFD {
        assert_nfd_cluster_maps_draw(reopened.session.save_bytes(out).unwrap());
    }
    let doc = reopened.session.get(out).unwrap();
    assert_eq!(doc.info.pages.len(), 3);
    assert!(doc.page_images(0).is_empty(), "the scan must not disguise missing visible glyphs");
    for p in [2, 3] {
        assert_eq!(text(&mut reopened, out, p), "");
    }
    json(&mut reopened, "doc_save", json!({"doc":out.0,"path":"Tekrar – C\u{327}ag\u{306}rı.pdf"}));
    let mut fresh = Automation::new().with_root(dir.path()).unwrap();
    let saved = open(&mut fresh, "Tekrar – C\u{327}ag\u{306}rı.pdf");
    assert_eq!(text(&mut fresh, saved, 1), corrected);
    assert_search_and_copy(&mut fresh, saved, corrected);
    assert!(fresh.render_pixels(saved.0, 1, 1.0).unwrap() == ink, "save/reopen retains visible ink");
    assert_eq!(a.session.save_bytes(id).unwrap(), before);
}

#[test]
fn corrected_nfc_control_exercises_the_same_export_roundtrip_contract() {
    correction_roundtrip(NFC, "Çalışmalar – NFC.pdf");
}

#[test]
fn corrected_nfd_editable_roundtrip_keeps_exact_unicode_search_copy_and_visible_ink() {
    correction_roundtrip(NFD, "Çalışmalar – C\u{327}ag\u{306}rı NFD.pdf");
}

fn canonical_pair(degrees: i64) {
    let dir = tempfile::tempdir().unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap().with_clock(|| 1_700_000_000);
    let id = scan(&mut a);
    if degrees != 0 {
        json(&mut a, "page_rotate", json!({"doc":id.0,"pages":[1],"degrees":degrees}));
        json(&mut a, "page_rotate", json!({"doc":id.0,"pages":[2],"degrees":(degrees+90)%360}));
    }
    let before = a.session.save_bytes(id).unwrap();
    let mut exports = Vec::new();
    for (form, words) in [("NFC", ["Çağrı", "İstanbul"]), ("NFD", ["C\u{327}ag\u{306}rı", "I\u{307}stanbul"])] {
        let pages = (0..2).map(|p| page(p, vec![placed(&a, id, p, words[p], 20.0, 150.0), placed(&a, id, p, "ASCII", 185.0, 70.0)])).collect();
        let review = retain(&mut a, id, pages);
        let path = format!("{form}-{degrees}.pdf");
        assert_eq!(editable(&mut a, id, review, &path, None)["words"], 4);
        assert_eq!(a.session.save_bytes(id).unwrap(), before);
        let out = open(&mut a, &path);
        for (p, word) in words.iter().enumerate() {
            assert_eq!(text(&mut a, out, p + 1), format!("{word} ASCII"), "{form}, rotation {degrees}, page {p}");
            let (source, output) = (a.session.get(id).unwrap(), a.session.get(out).unwrap());
            assert_eq!((output.info.pages[p].width, output.info.pages[p].height), (source.info.pages[p].width, source.info.pages[p].height));
            assert_eq!(output.info.pages[p].rotation, 0, "boxes are mapped to the displayed source page");
            assert!(output.page_images(p).is_empty());
        }
        assert_eq!(text(&mut a, out, 3), "");
        exports.push(out);
    }
    for p in 0..2 {
        assert!(
            a.render_pixels(exports[0].0, p + 1, 1.0).unwrap() == a.render_pixels(exports[1].0, p + 1, 1.0).unwrap(),
            "canonical ink, rotation {degrees}, page {p}"
        );
        let nfc = a.page_texts(exports[0], &[p]).unwrap().remove(0);
        let nfd = a.page_texts(exports[1], &[p]).unwrap().remove(0);
        let bounds = |t: &pdfcraft_render::PageText| t.line_rects(0..t.glyphs.len());
        let (left, right) = (bounds(&nfc), bounds(&nfd));
        assert_eq!(left.len(), right.len());
        for (left, right) in left.iter().zip(&right) {
            assert!(left.iter().zip(right).all(|(a, b)| (a - b).abs() < 0.02), "canonical geometry: {left:?} != {right:?}");
        }
    }
}

#[test]
fn canonical_nfc_nfd_editable_ink_geometry_and_multipage_match() {
    canonical_pair(0);
}

#[test]
fn canonical_nfc_nfd_editable_rotated_boxes_keep_ink_geometry_and_multipage() {
    for degrees in [90, 180, 270] {
        canonical_pair(degrees);
    }
}

#[test]
fn editable_refuses_unsupported_orphan_and_excessive_marks_without_consuming_review() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let id = scan(&mut a);
    let before = a.session.save_bytes(id).unwrap();
    let review = retain_word(&mut a, id, "Original");
    for text in ["a\u{1AB0}".to_owned(), "\u{327}C".into(), format!("a{}", "\u{301}".repeat(33))] {
        let error = a
            .call(
                "ocr_review_apply",
                &json!({"doc":id.0,"review":review,"output_mode":"editable","out":"rejected.pdf","corrections":[{"page":1,"word":1,"text":text}]}),
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains("combining") || error.contains("mark") || error.contains("cluster"), "named shaping refusal: {error}");
        assert!(!dir.path().join("rejected.pdf").exists());
        assert_eq!(a.session.save_bytes(id).unwrap(), before);
        let retained = a.ocr_reviews.get(&id).unwrap();
        assert_eq!(retained.handle, review);
        assert_eq!(retained.pages[0].words[0].text, "Original", "failed correction must not alter retained text");
    }
    // A valid retry uses the same handle. NFC also proves this property on the old guard.
    assert_eq!(editable(&mut a, id, review, "retry.pdf", Some("Çağrı"))["words"], 1);
    assert!(!a.ocr_reviews.contains_key(&id));
    assert_eq!(a.session.save_bytes(id).unwrap(), before);
}

#[test]
fn editable_refuses_byte_and_word_limits_atomically_and_keeps_review_retryable() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let id = scan(&mut a);
    let before = a.session.save_bytes(id).unwrap();
    let review = retain_word(&mut a, id, "Original");
    let too_long = "a".repeat(4 * 1024 * 1024 + 1);
    let error = a
        .call(
            "ocr_review_apply",
            &json!({"doc":id.0,"review":review,"output_mode":"editable","out":"oversize.pdf","corrections":[{"page":1,"word":1,"text":too_long}]}),
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("4 MiB"), "{error}");
    assert!(!dir.path().join("oversize.pdf").exists());
    assert_eq!(a.session.save_bytes(id).unwrap(), before);
    assert_eq!(a.ocr_reviews.get(&id).unwrap().pages[0].words[0].text, "Original");
    editable(&mut a, id, review, "retry-byte.pdf", None);

    let word = placed(&a, id, 0, "a", 20.0, 50.0);
    let review = retain(&mut a, id, vec![page(0, vec![word; 65537])]);
    let error =
        a.call("ocr_review_apply", &json!({"doc":id.0,"review":review,"output_mode":"editable","out":"oversize.pdf"})).unwrap_err().to_string();
    assert!(error.contains("too large"), "{error}");
    assert!(!dir.path().join("oversize.pdf").exists());
    assert_eq!(a.session.save_bytes(id).unwrap(), before);
    assert_eq!(a.ocr_reviews.get(&id).unwrap().handle, review);
    assert_eq!(json(&mut a, "ocr_review_apply", json!({"doc":id.0,"review":review,"discard":true}))["discarded"], review);
}

#[test]
fn editable_refuses_marked_row_and_glyph_budgets_before_publishing_output() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let id = scan(&mut a);
    let before = a.session.save_bytes(id).unwrap();
    let review = retain_word(&mut a, id, "Original");
    // Under the 4 MiB aggregate limit, but over shaped_text's 262144-byte row limit.
    let row_bytes = format!("g\u{306}{}", "a".repeat(262_144));
    // q+acute cannot compose: 32768 clusters need at least 65536 shaped glyphs.
    let positioned_glyphs = "q\u{301}".repeat(32_768);
    // 2401 distinct, individually valid Latin clusters exceed the 2400 Type3 code budget.
    // Four marks chosen from eight supported marks provide 4096 sequences under 33 scalars.
    let mut unique_clusters = String::new();
    for index in 0..2401usize {
        unique_clusters.push('q');
        for shift in [0, 3, 6, 9] {
            unique_clusters.push(['\u{300}', '\u{301}', '\u{302}', '\u{303}', '\u{304}', '\u{306}', '\u{308}', '\u{30A}'][(index >> shift) & 7]);
        }
    }
    for (label, text) in [("row-bytes", row_bytes), ("positioned-glyphs", positioned_glyphs), ("unique-clusters", unique_clusters)] {
        let error = a
            .call(
                "ocr_review_apply",
                &json!({"doc":id.0,"review":review,"output_mode":"editable","out":"budget.pdf","corrections":[{"page":1,"word":1,"text":text}]}),
            )
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("combining")
                || error.contains("budget")
                || error.contains("bounded")
                || error.contains("glyph")
                || error.contains("limit")
                || error.contains("large"),
            "{label}: {error}"
        );
        assert!(!dir.path().join("budget.pdf").exists(), "{label}");
        assert_eq!(a.session.save_bytes(id).unwrap(), before, "{label}");
        let retained = a.ocr_reviews.get(&id).unwrap();
        assert_eq!(retained.handle, review);
        assert_eq!(retained.pages[0].words[0].text, "Original");
    }
    assert_eq!(editable(&mut a, id, review, "retry-budget.pdf", None)["words"], 1);
    assert_eq!(a.session.save_bytes(id).unwrap(), before);
}

#[test]
fn editable_nfd_output_paths_are_confined_and_existing_files_are_preserved() {
    let outer = tempfile::tempdir().unwrap();
    let root = outer.path().join("root");
    std::fs::create_dir(&root).unwrap();
    let mut a = Automation::new().with_root(&root).unwrap();
    let id = scan(&mut a);
    json(&mut a, "doc_save", json!({"doc":id.0,"path":"Kaynak.pdf"}));
    let before = a.session.save_bytes(id).unwrap();
    let review = retain_word(&mut a, id, "Original");
    std::fs::write(root.join("Mevcut.pdf"), b"existing-output-sentinel").unwrap();
    std::fs::write(outer.path().join("outside.pdf"), b"outside-root-sentinel").unwrap();
    for (path, reason) in [("Kaynak.pdf", "source"), ("Mevcut.pdf", "exists"), ("../outside.pdf", "outside the allowed directory")] {
        let error = a
            .call(
                "ocr_review_apply",
                &json!({"doc":id.0,"review":review,"output_mode":"editable","out":path,"corrections":[{"page":1,"word":1,"text":NFD}]}),
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains(reason), "{path}: {error}");
        assert_eq!(a.session.save_bytes(id).unwrap(), before);
        assert_eq!(a.ocr_reviews.get(&id).unwrap().pages[0].words[0].text, "Original");
    }
    assert_eq!(std::fs::read(root.join("Kaynak.pdf")).unwrap().as_slice(), before.as_slice());
    assert_eq!(std::fs::read(root.join("Mevcut.pdf")).unwrap(), b"existing-output-sentinel");
    assert_eq!(std::fs::read(outer.path().join("outside.pdf")).unwrap(), b"outside-root-sentinel");
    assert_eq!(editable(&mut a, id, review, "retry-path.pdf", None)["words"], 1);
}

#[test]
fn searchable_invisible_nfd_correction_roundtrips_one_undo_step_and_consumes_review() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap().with_clock(|| 1_700_000_000);
    let id = scan(&mut a);
    let before = a.render_pixels(id.0, 1, 1.0).unwrap();
    let review = retain_word(&mut a, id, "Ilik");
    let result = json(&mut a, "ocr_review_apply", json!({"doc":id.0,"review":review,"corrections":[{"page":1,"word":1,"text":NFD}]}));
    assert_eq!(result["applied"], 1);
    assert_eq!(text(&mut a, id, 1), NFD);
    assert_search_and_copy(&mut a, id, NFD);
    assert!(a.render_pixels(id.0, 1, 1.0).unwrap() == before, "searchable NFD has no visible ink");
    assert!(!a.ocr_reviews.contains_key(&id));
    json(&mut a, "edit_undo", json!({"doc":id.0}));
    assert_eq!(text(&mut a, id, 1), "");
    assert!(a.call("edit_undo", &json!({"doc":id.0})).is_err(), "the reviewed layer is one undo step");
    json(&mut a, "edit_redo", json!({"doc":id.0}));
    assert_eq!(text(&mut a, id, 1), NFD);
    assert!(a.call("ocr_review_apply", &json!({"doc":id.0,"review":review})).is_err(), "undo/redo does not resurrect a review handle");
    json(&mut a, "doc_save", json!({"doc":id.0,"path":"Görünmez – C\u{327}ag\u{306}rı.pdf"}));
    let mut fresh = Automation::new().with_root(dir.path()).unwrap();
    let out = open(&mut fresh, "Görünmez – C\u{327}ag\u{306}rı.pdf");
    assert_eq!(text(&mut fresh, out, 1), NFD);
    assert_search_and_copy(&mut fresh, out, NFD);
    assert!(fresh.render_pixels(out.0, 1, 1.0).unwrap() == before);
}

fn editable_retype_has_ink(pixels: &[u8]) -> bool {
    pixels.chunks_exact(4).any(|p| p[0] < 128 && p[1] < 128 && p[2] < 128)
}

fn retype_refusals_preserve_output(editor: &mut Automation, out: DocId, original: &str) {
    let before = editor.render_pixels(out.0, 1, 1.0).unwrap();
    // A fallback's scalar cmap coverage must not admit orphan/unsupported/over-budget marks.
    // A rejected draft must leave both the imported PDF and its undo history untouched.
    let export_bytes = editor.session.save_bytes(out).unwrap();
    for (label, invalid) in [
        ("orphan U+0306", "\u{306}".to_owned()),
        ("unsupported U+1AB0", "a\u{1AB0}".to_owned()),
        ("34-scalar cluster", format!("q{}", "\u{301}".repeat(33))),
        ("shaped cache budget", "a\u{301}\n".repeat(512)),
    ] {
        let result = editor.call("text_edit", &json!({"doc":out.0,"page":1,"paragraph":1,"text":invalid}));
        assert!(result.is_err(), "{label}: unsupported draft must be refused: {result:?}");
        if label == "shaped cache budget" {
            let error = result.unwrap_err().to_string();
            assert!(error.contains("cache budget"), "{label}: expected the named cache budget refusal: {error}");
        }
        assert_eq!(editor.session.save_bytes(out).unwrap(), export_bytes, "{label}: refusal must not change PDF bytes");
        assert_eq!(text(editor, out, 1), original, "{label}: refusal must preserve exact original NFD");
        assert!(editor.render_pixels(out.0, 1, 1.0).unwrap() == before, "{label}: refusal must preserve visible ink");
        assert!(editor.call("edit_undo", &json!({"doc":out.0})).is_err(), "{label}: refusal must not create an undo step");
    }
}

#[test]
fn reopened_editable_nfd_ocr_paragraph_retypes_undoes_redoes_and_saves_exact_unicode() {
    use sha2::{Digest, Sha256};

    // A successful OCR export must remain editable after losing its in-memory OCR plan.
    // These are the exact original/replacement strings from the headless acceptance failure.
    let original = "C\u{327}ag\u{306}rı I\u{307}stanbul S\u{327}is\u{327}li";
    let replacement = "Gu\u{308}ncellenmis\u{327}";
    let source_path = "Kaynak – OCR.pdf";
    let export_path = "Editable – C\u{327}ag\u{306}rı.pdf";
    let edited_path = "Düzenlenmiş – Gu\u{308}ncellenmis\u{327}.pdf";
    let dir = tempfile::tempdir().unwrap();
    let mut source = Automation::new().with_root(dir.path()).unwrap().with_clock(|| 1_700_000_000);
    let id = scan(&mut source);
    json(&mut source, "doc_save", json!({"doc":id.0,"path":source_path}));
    let source_bytes = source.session.save_bytes(id).unwrap();
    let source_hash = Sha256::digest(std::fs::read(dir.path().join(source_path)).unwrap());
    let review = retain_word(&mut source, id, "Ilik");
    assert_eq!(editable(&mut source, id, review, export_path, Some(original))["words"], 1);
    let export_hash = Sha256::digest(std::fs::read(dir.path().join(export_path)).unwrap());

    // A fresh session and disk doc_open are essential: editing an in-memory generated page
    // or explicitly choosing Inter would bypass the imported Type3 paragraph fallback bug.
    let mut editor = Automation::new().with_root(dir.path()).unwrap().with_clock(|| 1_700_000_000);
    let out = open(&mut editor, export_path);
    assert_eq!(text(&mut editor, out, 1), original);
    let paragraphs = json(&mut editor, "text_paragraphs", json!({"doc":out.0,"page":1}));
    assert_eq!(paragraphs["count"], 1, "{paragraphs}");
    assert_eq!(paragraphs["paragraphs"][0]["paragraph"], 1);
    assert_eq!(paragraphs["paragraphs"][0]["text"], original);
    assert_eq!(paragraphs["paragraphs"][0]["editor_text"], original);
    let before = editor.render_pixels(out.0, 1, 1.0).unwrap();
    assert!(editable_retype_has_ink(&before.2), "export must have visible text");

    retype_refusals_preserve_output(&mut editor, out, original);

    // Keep precisely the default paragraph-edit path: no font/font_resource/size overrides.
    json(&mut editor, "text_edit", json!({"doc":out.0,"page":1,"paragraph":1,"text":replacement}));
    assert_eq!(text(&mut editor, out, 1), replacement, "editing must not normalize the replacement to NFC");
    let edited = editor.render_pixels(out.0, 1, 1.0).unwrap();
    assert!(edited != before, "retyping must replace the original visible text");
    assert!(editable_retype_has_ink(&edited.2), "replacement must draw real ink");

    json(&mut editor, "edit_undo", json!({"doc":out.0}));
    assert_eq!(text(&mut editor, out, 1), original, "undo must retain the original NFD scalar sequence");
    assert!(editor.render_pixels(out.0, 1, 1.0).unwrap() == before, "undo restores original ink");
    assert!(editor.call("edit_undo", &json!({"doc":out.0})).is_err(), "one paragraph retype is one undo step");
    json(&mut editor, "edit_redo", json!({"doc":out.0}));
    assert_eq!(text(&mut editor, out, 1), replacement);
    assert!(editor.render_pixels(out.0, 1, 1.0).unwrap() == edited, "redo restores replacement ink");

    json(&mut editor, "doc_save", json!({"doc":out.0,"path":edited_path}));
    let mut reopened = Automation::new().with_root(dir.path()).unwrap();
    let saved = open(&mut reopened, edited_path);
    assert_eq!(text(&mut reopened, saved, 1), replacement, "saved replacement must remain exact NFD in a fresh session");
    assert!(reopened.render_pixels(saved.0, 1, 1.0).unwrap() == edited, "save/reopen retains the replacement ink");
    for page in [2, 3] {
        assert_eq!(text(&mut reopened, saved, page), "", "unreviewed pages stay blank");
    }

    assert_eq!(source.session.save_bytes(id).unwrap(), source_bytes, "paragraph editing cannot change the source scan");
    assert_eq!(Sha256::digest(std::fs::read(dir.path().join(source_path)).unwrap()), source_hash, "source disk hash stays unchanged");
    assert_eq!(
        Sha256::digest(std::fs::read(dir.path().join(export_path)).unwrap()),
        export_hash,
        "Save As must leave the old OCR output's disk hash unchanged"
    );
    let unchanged = open(&mut reopened, export_path);
    assert_eq!(text(&mut reopened, unchanged, 1), original);
}
