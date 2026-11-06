use pdfcraft_engine::{
    Edit, Session,
    export::Exporter,
    ocr::{self, OcrSettings, PlacedWord},
};

fn render(session: &Session, id: pdfcraft_engine::DocId) -> Vec<u8> {
    Exporter::new(session.get(id).unwrap()).png(0, 150.0).unwrap()
}

#[test]
fn crop_scanner_borders_preserves_pixels_rotation_text_and_undo() {
    if !ocr::available() {
        return;
    }
    for rotation in [0, 90, 180, 270] {
        let mut s = Session::new();
        let image = image::RgbImage::from_fn(600, 800, |x, y| {
            if (120..560).contains(&x) && (50..690).contains(&y) { image::Rgb([235, 235, 235]) } else { image::Rgb([25, 25, 25]) }
        });
        let mut png = std::io::Cursor::new(Vec::new());
        image.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let pdf = s.create_from_images(&[("paper.png".into(), png.into_inner())]).unwrap();
        let id = s.open("scan.pdf", None, pdf, None).unwrap();
        // An existing off-origin CropBox and /Rotate must not shift or mirror the crop.
        let info = s.get(id).unwrap().info.pages[0].clone();
        s.apply(
            id,
            Edit::SetPageBox {
                pages: vec![0],
                which: pdfcraft_engine::PageBox::Crop,
                spec: pdfcraft_engine::BoxSpec::Rect([8.0, 9.0, info.width as f64 - 7.0, info.height as f64 - 8.0]),
            },
        )
        .unwrap();
        s.apply(id, Edit::RotatePages { pages: vec![0], degrees: rotation }).unwrap();
        s.apply(
            id,
            Edit::AddOcrText {
                page: 0,
                words: vec![PlacedWord {
                    text: "Kept text".into(),
                    origin: [150.0, 150.0],
                    across: [100.0, 0.0],
                    up: [0.0, 20.0],
                    confidence: None,
                    source: "ocrs".into(),
                }],
            },
        )
        .unwrap();
        let before = s.get(id).unwrap().bytes.clone();
        let settings = OcrSettings {
            auto_crop: true,
            crop_margin: 0.0,
            recognize: false,
            rotate_pages: false,
            straighten_pages: false,
            skip_text_pages: false,
            ..Default::default()
        };
        let found = s.recognize_text(id, &[], settings.clone()).unwrap();
        assert!(found[0].crop.is_some(), "{rotation}: {found:?}");
        let after = s.get(id).unwrap().info.pages[0].clone();
        let (w, h) = if rotation % 180 == 0 { (440.0, 640.0) } else { (640.0, 440.0) };
        let unit = info.width as f64 / 600.0;
        assert!((after.width as f64 - w * unit).abs() < 12.0, "{rotation}: {after:?}");
        assert!((after.height as f64 - h * unit).abs() < 12.0);
        assert!(text(&s, id).contains("Kept text"));
        let saved = s.save_bytes(id).unwrap();
        let reopened = s.open("cropped.pdf", None, saved, None).unwrap();
        assert_eq!(after.width, s.get(reopened).unwrap().info.pages[0].width);
        assert_eq!(render(&s, id), render(&s, reopened));
        s.undo(id).unwrap();
        assert_eq!(before, s.get(id).unwrap().bytes);
        s.redo(id).unwrap();
        assert_eq!(after.width, s.get(id).unwrap().info.pages[0].width);
        // Correction-only file processing must save the crop even with zero recognized words.
        let file = ocr::recognize_file("scan.pdf", before.clone(), None, settings, &ocr::recognizers(&OcrSettings::default()).unwrap(), |_, _| true)
            .unwrap();
        assert_ne!(file.bytes, before);
        assert_eq!(file.words(), 0);
        let crop_only = s.open("file.pdf", None, file.bytes, None).unwrap();
        assert_eq!(after.width, s.get(crop_only).unwrap().info.pages[0].width);
        // A margin is validated instead of silently clamped, and refusal leaves the PDF alone.
        for margin in [-1.0, 21.0, f64::NAN] {
            assert!(s.recognize_text(id, &[], OcrSettings { crop_margin: margin, ..Default::default() }).is_err());
        }
    }
}
fn text(session: &Session, id: pdfcraft_engine::DocId) -> String {
    Exporter::new(session.get(id).unwrap()).text(0).unwrap()
}
fn scan(sentence: &str, rotation: u16, skew: f64) -> (Session, pdfcraft_engine::DocId) {
    let mut session = Session::new();
    let japanese = sentence.chars().any(|c| ('\u{3000}'..='\u{9fff}').contains(&c));
    let pdf = session.create_from_text("Scan fixture", if japanese { "Placeholder" } else { sentence }).unwrap();
    let doc = session.open("text.pdf", None, pdf, None).unwrap();
    if japanese {
        session.apply(doc, Edit::EditTextLine { page: 0, line: 0, text: sentence.into() }).unwrap();
    }
    let image = image::load_from_memory(&render(&session, doc)).unwrap().to_rgb8();
    let image = pdfcraft_ocr::scan::Correction { rotation, deskew: skew }.apply(&image);
    let mut png = std::io::Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png).unwrap();
    let pdf = session.create_from_images(&[("scan.png".into(), png.into_inner())]).unwrap();
    let id = session.open("scan.pdf", None, pdf, None).unwrap();
    (session, id)
}

#[test]
fn unicode_ocr_survives_save_search_and_undo_without_changing_pixels() {
    let (mut s, id) = scan("Invisible text layer", 0, 0.0);
    let before = render(&s, id);
    let words = vec![PlacedWord {
        text: "Grüße 日本語 français español 😀".into(),
        origin: [50.0, 50.0],
        across: [300.0, 0.0],
        up: [0.0, 20.0],
        confidence: None,
        source: "ocrs".into(),
    }];
    s.apply(id, Edit::AddOcrText { page: 0, words }).unwrap();
    assert_eq!(before, render(&s, id));
    let found = text(&s, id);
    assert!(found.contains("Grüße 日本語 français español 😀"), "{found:?}");
    let saved = s.save_bytes(id).unwrap();
    let reopened = s.open("saved.pdf", None, saved, None).unwrap();
    assert!(text(&s, reopened).contains("日本語"));
    s.undo(id).unwrap();
    assert!(text(&s, id).trim().is_empty());
    s.redo(id).unwrap();
    assert!(text(&s, id).contains("日本語"));
}

#[test]
fn annotated_scan_refuses_deskew_atomically() {
    let (mut s, id) = scan("Keep comments attached", 0, 0.0);
    s.apply(
        id,
        Edit::AddAnnotation(pdfcraft_engine::NewAnnotation {
            page: 0,
            shape: pdfcraft_engine::Shape::Rectangle { rect: [20.0, 20.0, 100.0, 50.0] },
            style: Default::default(),
            contents: "Keep this comment".into(),
            author: "Test".into(),
        }),
    )
    .unwrap();
    let before = s.get(id).unwrap().bytes.clone();
    let result = s.apply(
        id,
        Edit::Batch {
            label: "Correct scans".into(),
            edits: vec![Edit::RotatePages { pages: vec![0], degrees: 90 }, Edit::DeskewPage { page: 0, degrees: 3.0 }],
        },
    );
    assert!(result.unwrap_err().to_string().contains("annotations"));
    assert_eq!(s.get(id).unwrap().bytes, before);
    for degrees in [f64::NAN, f64::INFINITY, 13.0] {
        assert!(s.apply(id, Edit::DeskewPage { page: 0, degrees }).is_err());
    }
}

/// A skewed scan with a comment on it is still recognized: the run leaves that page unstraightened
/// and says so, instead of failing the whole batch on DeskewPage.
#[test]
fn annotated_skewed_scan_is_recognized_without_deskew() {
    if !ocr::available() {
        return;
    }
    let sentence = (0..18).map(|_| "Straight lines make this scanned document easier to read.\n").collect::<String>();
    let (mut s, id) = scan(&sentence, 0, 3.0);
    s.apply(
        id,
        Edit::AddAnnotation(pdfcraft_engine::NewAnnotation {
            page: 0,
            shape: pdfcraft_engine::Shape::Rectangle { rect: [20.0, 20.0, 100.0, 50.0] },
            style: Default::default(),
            contents: "Keep this comment".into(),
            author: "Test".into(),
        }),
    )
    .unwrap();
    let found = s.recognize_text(id, &[], OcrSettings { rotate_pages: false, ..Default::default() }).unwrap();
    assert!(found[0].deskew_skipped && found[0].correction.deskew == 0.0, "{found:?}");
    assert!(text(&s, id).contains("Straight"), "{:?}", text(&s, id));
    assert_eq!(s.get(id).unwrap().can_undo(), Some("Recognize text"));
}

#[test]
fn scan_correction_handles_all_quarter_turns_and_stale_results() {
    if !ocr::available() {
        eprintln!("skipped: run cargo xtask models");
        return;
    }
    let sentence = "Document orientation and searchable text\nThe quick brown fox jumps over the lazy dog.\nEvery page should be upright and easy to read.\nPlease keep this document for your records.\nA useful example with several lines of text.";
    for rotation in [0, 90, 180, 270] {
        let (mut s, id) = scan(sentence, rotation, 0.0);
        let before = s.get(id).unwrap().bytes.clone();
        let result = s.recognize_text(id, &[], OcrSettings { straighten_pages: false, ..Default::default() }).unwrap();
        assert_eq!(result[0].correction.rotation, (360 - rotation) % 360, "{result:?}");
        assert!(text(&s, id).to_lowercase().contains("orientation"), "{:?}", result);
        s.undo(id).unwrap();
        assert_eq!(s.get(id).unwrap().bytes, before);
        s.redo(id).unwrap();
        let job = s.ocr_job(id, &[], OcrSettings { skip_text_pages: false, ..Default::default() }).unwrap();
        let guard = job.guard.clone();
        let found = job.run(&ocr::recognizers(&OcrSettings::default()).unwrap(), |_, _| true);
        s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
        assert!(s.apply_ocr_checked(id, &found, &guard).unwrap_err().contains("changed"));
    }
}

#[test]
fn scan_deskew_without_ocr_persists_and_is_undoable() {
    if !ocr::available() {
        return;
    }
    let sentence = (0..18).map(|_| "Straight lines make this scanned document easier to read.\n").collect::<String>();
    for angle in [-4.0, 3.0] {
        let (mut s, id) = scan(&sentence, 0, angle);
        let before = s.get(id).unwrap().bytes.clone();
        let result = s.recognize_text(id, &[], OcrSettings { rotate_pages: false, recognize: false, ..Default::default() }).unwrap();
        assert!((result[0].correction.deskew + angle).abs() < 0.3, "{result:?}");
        assert!(text(&s, id).trim().is_empty());
        let saved = s.save_bytes(id).unwrap();
        assert!(saved.starts_with(before.as_ref()));
        let after = s.open("after.pdf", None, saved, None).unwrap();
        let img = image::load_from_memory(&render(&s, after)).unwrap().to_rgb8();
        assert!(pdfcraft_ocr::scan::deskew(&img).abs() < 0.2);
        s.undo(id).unwrap();
        assert_eq!(s.get(id).unwrap().bytes, before);
    }
}

#[test]
fn scan_recognizes_accents_in_requested_languages() {
    if !ocr::available() {
        return;
    }
    for (language, sentence, expected) in [
        ("de", "Grüße aus Österreich und schöne Grüße für alle.", "Österreich"),
        ("fr", "Une journée à Paris avec un café français.", "journée"),
        ("es", "El español también tiene acentos y una mañana.", "español"),
    ] {
        let (mut s, id) = scan(sentence, 0, 0.0);
        let found = s
            .recognize_text(id, &[], OcrSettings { language: language.into(), rotate_pages: false, straighten_pages: false, ..Default::default() })
            .unwrap();
        assert!(text(&s, id).contains(expected), "{language}: {found:?}; extracted {:?}", text(&s, id));
    }
}

#[test]
fn skewed_lines_are_read_straightened_without_deskewing_the_page() {
    if !ocr::available() {
        return;
    }
    let sentence = "Quarterly invoice numbers match the signed delivery receipts.
Warehouse staff confirmed every pallet before shipping.
";
    let (mut s, id) = scan(&sentence.repeat(6), 0, 4.0);
    // Lines are straightened by the multilingual recognizer, which reads every language but
    // English (English is the ocrs model's); German reads this Latin text the same way.
    let settings = OcrSettings { language: "de".into(), rotate_pages: false, straighten_pages: false, ..Default::default() };
    let found = s.recognize_text(id, &[], settings).unwrap();
    assert_eq!(found[0].correction.deskew, 0.0, "{found:?}");
    let extracted = text(&s, id);
    // Read from their bounding boxes, these lines each take in their neighbours and nothing
    // survives; straightened, the recognizer still slips now and then, so most is enough.
    for expected in ["Quarterly", "invoice", "delivery", "receipts.", "Warehouse", "confirmed", "pallet", "shipping."] {
        assert!(extracted.matches(expected).count() >= 4, "{expected:?} in {extracted:?}");
    }
}

#[test]
fn french_ocr_preserves_capitals_cedilla_ligature_and_apostrophes_after_save() {
    if !ocr::available() {
        eprintln!("skipped: run cargo xtask models");
        return;
    }
    let (mut s, id) = scan("ÉTÉ français cœur l'été l’été à Paris.", 0, 0.0);
    let before = render(&s, id);
    s.recognize_text(id, &[], OcrSettings { language: "fr".into(), rotate_pages: false, straighten_pages: false, ..Default::default() }).unwrap();
    let saved = s.save_bytes(id).unwrap();
    let reopened = s.open("french.pdf", None, saved, None).unwrap();
    let extracted = text(&s, reopened);
    for expected in ["ÉTÉ", "français", "cœur", "l'été", "Paris."] {
        assert!(extracted.contains(expected), "missing {expected:?}: {extracted:?}");
    }
    assert_eq!(extracted.replace('’', "'").matches("l'été").count(), 2, "both apostrophe forms: {extracted:?}");
    assert_eq!(before, render(&s, reopened), "searchable text must not change scan pixels");
}

#[test]
fn italian_ocr_preserves_all_six_accents_after_save() {
    if !ocr::available() {
        eprintln!("skipped: run cargo xtask models");
        return;
    }
    let (mut s, id) = scan("Lunedì visiterò la città perché è più tranquilla.", 0, 0.0);
    let before = render(&s, id);
    s.recognize_text(id, &[], OcrSettings { language: "it".into(), rotate_pages: false, straighten_pages: false, ..Default::default() }).unwrap();
    let saved = s.save_bytes(id).unwrap();
    let reopened = s.open("italian.pdf", None, saved, None).unwrap();
    let extracted = text(&s, reopened);
    for expected in ["Lunedì", "visiterò", "città", "perché", "è", "più"] {
        assert!(extracted.split_whitespace().any(|word| word == expected), "missing {expected:?}: {extracted:?}");
    }
    assert_eq!(before, render(&s, reopened), "searchable text must not change the scan pixels");
}

#[test]
fn additional_latin_languages_keep_accented_words_after_save() {
    if !ocr::available() {
        eprintln!("skipped: run cargo xtask models");
        return;
    }
    let samples: &[(&str, &str, &[&str])] = &[
        ("nl", "Een coöperatie in België koopt één café.", &["coöperatie", "België", "één"]),
        ("da", "Vi går på øen og læser bøger.", &["går", "på", "øen", "læser", "bøger"]),
        ("nb", "Vi går på øya og leser bøker.", &["går", "på", "øya", "bøker"]),
        ("sv", "Vi läser böcker på en ö nära staden.", &["läser", "böcker", "på", "ö", "nära"]),
        ("fi", "Hyvää päivää ystävä. Täällä on lämmin kesä.", &["Hyvää", "päivää", "ystävä", "Täällä", "lämmin", "kesä"]),
        ("pt", "A ação começa amanhã com café e música.", &["ação", "começa", "amanhã", "café", "música"]),
    ];
    for (language, sentence, expected) in samples {
        let (mut s, id) = scan(sentence, 0, 0.0);
        let before = render(&s, id);
        s.recognize_text(id, &[], OcrSettings { language: (*language).into(), rotate_pages: false, straighten_pages: false, ..Default::default() })
            .unwrap();
        let saved = s.save_bytes(id).unwrap();
        let reopened = s.open("recognized.pdf", None, saved, None).unwrap();
        let extracted = text(&s, reopened);
        for word in *expected {
            assert!(
                extracted.split_whitespace().any(|w| w.trim_matches(|c: char| !c.is_alphabetic()) == *word),
                "{language} missing {word:?}: {extracted:?}"
            );
        }
        assert_eq!(before, render(&s, reopened), "{language}: OCR must preserve scan pixels");
    }
}

#[test]
fn scan_recognizes_japanese_with_release_fonts() {
    if !ocr::available() || pdfcraft_fonts::CRAFT_FONTS.is_empty() {
        eprintln!("skipped: models and CRAFT_FONTS_DIR required");
        return;
    }
    let (mut s, id) = scan("日本語の文字を認識します。\n今日は良い天気です。", 0, 0.0);
    let found =
        s.recognize_text(id, &[], OcrSettings { language: "ja".into(), rotate_pages: false, straighten_pages: false, ..Default::default() }).unwrap();
    assert!(text(&s, id).contains("日本語"), "{found:?}; extracted {:?}", text(&s, id));
}

#[test]
fn scan_rotation_and_deskew_keep_searchable_text_aligned() {
    if !ocr::available() {
        return;
    }
    let sentence = (0..12).map(|_| "Straight searchable lines stay aligned with the scan.\n").collect::<String>();
    let (mut s, id) = scan(&sentence, 90, 4.0);
    let original = image::load_from_memory(&render(&s, id)).unwrap().to_rgb8();
    let mut bordered = image::RgbImage::from_pixel(original.width() + 280, original.height() + 200, image::Rgb([25; 3]));
    image::imageops::replace(&mut bordered, &original, 180, 80);
    let mut png = std::io::Cursor::new(Vec::new());
    bordered.write_to(&mut png, image::ImageFormat::Png).unwrap();
    let pdf = s.create_from_images(&[("bordered.png".into(), png.into_inner())]).unwrap();
    let id = s.open("bordered.pdf", None, pdf, None).unwrap();
    let found = s.recognize_text(id, &[], OcrSettings { auto_crop: true, ..Default::default() }).unwrap();
    assert!(found[0].crop.is_some());
    assert_eq!(found[0].correction.rotation, 270);
    assert!((found[0].correction.deskew + 4.0).abs() < 0.3, "{found:?}");
    assert!(text(&s, id).to_lowercase().contains("searchable"));
    let bytes = s.save_bytes(id).unwrap();
    let reopened = s.open("result.pdf", None, bytes, None).unwrap();
    let img = image::load_from_memory(&render(&s, reopened)).unwrap().to_rgb8();
    assert!(
        pdfcraft_ocr::scan::deskew(&img).abs() < 0.2,
        "remaining angle {}, correction {:?}",
        pdfcraft_ocr::scan::deskew(&img),
        found[0].correction
    );
    // The extracted words must lie along horizontal lines after both PDF transformations.
    let doc = s.get(reopened).unwrap();
    let mut renderer = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), doc.export_source().config);
    let text =
        renderer.render(pdfcraft_render::RenderRequest { page: 0, kind: pdfcraft_render::RequestKind::Text, ..Default::default() }).text.unwrap();
    let range = text.find("searchable").into_iter().next().unwrap();
    for glyph in &text.glyphs[range] {
        assert!(glyph.direction[0] > 0.99 && glyph.direction[1].abs() < 0.01, "{glyph:?}");
    }
}

/// #1012 + #1028: existing invisible words are still duplicates when recognition
/// crops scanner padding, turns the page and deskews before reading it.
#[test]
fn corrected_scans_do_not_duplicate_existing_ocr() {
    if !ocr::available() {
        eprintln!("skipped: run cargo xtask models");
        return;
    }
    let sentence =
        (0..8).map(|_| "Document orientation and searchable text. The quick brown fox jumps over the lazy dog.").collect::<Vec<_>>().join("\n");
    let (mut s, scan_id) = scan(&sentence, 90, 2.0);
    let image = image::load_from_memory(&render(&s, scan_id)).unwrap().to_rgb8();
    let mut framed = image::RgbImage::from_pixel(image.width() + 140, image.height() + 140, image::Rgb([25, 25, 25]));
    image::imageops::overlay(&mut framed, &image, 70, 70);
    let mut png = std::io::Cursor::new(Vec::new());
    framed.write_to(&mut png, image::ImageFormat::Png).unwrap();
    let bytes = s.create_from_images(&[("framed.png".into(), png.into_inner())]).unwrap();
    let id = s.open("framed.pdf", None, bytes, None).unwrap();
    let settings = OcrSettings { skip_text_pages: false, auto_crop: true, crop_margin: 0.0, ..Default::default() };
    let initial = s.ocr_job(id, &[], settings.clone()).unwrap().run(&ocr::recognizers(&OcrSettings::default()).unwrap(), |_, _| true);
    assert!(initial[0].crop.is_some(), "{initial:?}");
    assert_ne!(initial[0].correction.rotation, 0, "{initial:?}");
    assert!(initial[0].correction.deskew.abs() > 0.5, "{initial:?}");
    assert!(!initial[0].words.is_empty());
    // Seed just the invisible layer in source coordinates, leaving the crooked scan intact.
    s.apply(id, Edit::AddOcrText { page: 0, words: initial[0].words.clone() }).unwrap();
    let before = s.get(id).unwrap().bytes.clone();
    let found = s.recognize_text(id, &[], settings).unwrap();
    assert!(found[0].words.is_empty(), "{found:?}");
    assert_eq!(found[0].already_on_page, initial[0].words.len());
    let extracted = text(&s, id);
    let saved = s.save_bytes(id).unwrap();
    let reopened = s.open("corrected.pdf", None, saved, None).unwrap();
    assert_eq!(text(&s, reopened), extracted);
    s.undo(id).unwrap();
    assert_eq!(s.get(id).unwrap().bytes, before);
}

/// Decoded Han text needs no installed font to survive the invisible PDF text layer.
#[test]
fn chinese_ocr_text_layer_preserves_scripts_and_punctuation_after_save() {
    for sentence in ["繁體中文，臺灣。", "简体中文，中国。"] {
        let (mut s, id) = scan("Synthetic scan", 0, 0.0);
        let before = s.get(id).unwrap().bytes.clone();
        let pixels = render(&s, id);
        s.apply(
            id,
            Edit::AddOcrText {
                page: 0,
                words: vec![PlacedWord {
                    text: sentence.into(),
                    origin: [20.0, 20.0],
                    across: [150.0, 0.0],
                    up: [0.0, 20.0],
                    confidence: None,
                    source: "ocrs".into(),
                }],
            },
        )
        .unwrap();
        let saved = s.save_bytes(id).unwrap();
        let reopened = s.open("chinese.pdf", None, saved, None).unwrap();
        assert!(text(&s, reopened).contains(sentence), "{:?}", text(&s, reopened));
        assert_eq!(pixels, render(&s, reopened));
        s.undo(id).unwrap();
        assert_eq!(s.get(id).unwrap().bytes, before);
    }
}

#[test]
fn catalan_ocr_keeps_accents_after_save() {
    if !ocr::available() {
        eprintln!("skipped: run cargo xtask models");
        return;
    }
    let (mut s, id) = scan("La llengua catalana és útil per a la ciència i la música.", 0, 0.0);
    let before = render(&s, id);
    s.recognize_text(id, &[], OcrSettings { language: "ca".into(), rotate_pages: false, straighten_pages: false, ..Default::default() }).unwrap();
    let saved = s.save_bytes(id).unwrap();
    let reopened = s.open("catalan.pdf", None, saved, None).unwrap();
    let extracted = text(&s, reopened);
    for expected in ["catalana", "és", "útil", "ciència", "música"] {
        assert!(extracted.contains(expected), "missing {expected:?}: {extracted:?}");
    }
    assert_eq!(before, render(&s, reopened));
}
