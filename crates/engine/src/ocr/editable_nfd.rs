//! Synthetic regression for source-preserving editable OCR clusters.
use super::{OcrPage, PlacedWord};
use crate::Session;
use std::sync::Arc;

fn word(text: &str) -> PlacedWord {
    PlacedWord { text: text.into(), origin: [20.0, 200.0], across: [180.0, 0.0], up: [0.0, 15.0], confidence: None, source: "synthetic".into() }
}
fn page(page: usize, words: Vec<PlacedWord>) -> OcrPage {
    OcrPage { already_on_page: 0, page, words, skipped: None, notes: Vec::new() }
}

#[test]
fn editable_nfd_round_trips_original_turkish_unicode_and_source() {
    let mut session = Session::new().with_clock(|| 1_700_000_000);
    let id = session.open("synthetic.pdf", None, Arc::new(crate::tests::fixture(1)), None).unwrap();
    let before = session.save_bytes(id).unwrap();
    let original = "C\u{327}ag\u{306}rı I\u{307}stanbul S\u{327}is\u{327}li";
    let found = [page(0, vec![word(original)])];
    let bytes = session.editable_ocr_bytes(id, &found).expect("supported NFD must be shaped before drawing");
    let mut output = Session::new();
    let out = output.open("editable.pdf", None, bytes, None).unwrap();
    assert!(crate::tests::page_texts(&output, out)[0].contains(original));
    assert_eq!(session.save_bytes(id).unwrap(), before, "editable export must not mutate the source");
}

#[test]
fn editable_nfd_and_nfc_canonical_ink_and_line_bounds_are_identical() {
    assert_canonical_pair(["ÇağrıİŞÜ", "C\u{327}ag\u{306}rıI\u{307}S\u{327}U\u{308}"]);
}

#[test]
fn editable_nfd_win_ansi_latin_canonical_ink_and_bounds_are_identical() {
    for pair in [["Ü", "U\u{308}"], ["Ç", "C\u{327}"], ["Café", "Cafe\u{301}"], ["Å", "A\u{30a}"]] {
        assert!(pdfcraft_fonts::fits_win_ansi(pair[0]), "this regression specifically exercises the legacy WinAnsi branch");
        assert_canonical_pair(pair);
    }
}

fn assert_canonical_pair(originals: [&str; 2]) {
    let mut session = Session::new();
    let id = session.open("synthetic.pdf", None, Arc::new(crate::tests::fixture(1)), None).unwrap();
    let mut rendered = Vec::new();
    let mut bounds = Vec::new();
    for source in originals {
        let plan = pdfcraft_fonts::TextPlan::for_editable_words([source]).unwrap();
        eprintln!(
            "canonical source {source:?}: faces {:?}, advance {}",
            plan.fonts.iter().map(|f| f.family).collect::<Vec<_>>(),
            plan.advance_of(plan.encoding(0))
        );
        let bytes = session.editable_ocr_bytes(id, &[page(0, vec![word(source)])]).unwrap();
        let mut renderer = pdfcraft_render::PageRenderer::new(bytes, Default::default());
        let text = renderer.filtered_text(0, Default::default()).unwrap();
        assert_eq!(text.plain_text().trim(), source);
        bounds.push(text.glyphs.iter().fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |mut b, g| {
            b[0] = b[0].min(g.rect[0]);
            b[1] = b[1].min(g.rect[1]);
            b[2] = b[2].max(g.rect[2]);
            b[3] = b[3].max(g.rect[3]);
            b
        }));
        let raster = renderer.render(pdfcraft_render::RenderRequest { page: 0, scale: 2.0, ..Default::default() });
        assert!(raster.error.is_none(), "{:?}", raster.error);
        assert!(raster.rgba.chunks_exact(4).any(|p| p[..3] != [255; 3]));
        rendered.push(raster.rgba);
    }
    assert!(bounds[0].iter().zip(bounds[1]).all(|(a, b)| (a - b).abs() < 0.02), "{bounds:?}");
    assert!(
        rendered[0] == rendered[1],
        "canonical spellings differ in {} channels",
        rendered[0].iter().zip(rendered[1].iter()).filter(|(a, b)| a != b).count()
    );
}

#[test]
fn editable_nfd_unsupported_input_is_atomic_and_searchable_keeps_original_unicode() {
    let mut session = Session::new().with_clock(|| 1_700_000_000);
    let id = session.open("synthetic.pdf", None, Arc::new(crate::tests::fixture(2)), None).unwrap();
    let before = session.save_bytes(id).unwrap();
    let excessive = format!("q{}", "\u{301}".repeat(33));
    for invalid in ["\u{306}", "\u{1ab0}", "a\u{1ab0}", excessive.as_str(), "a\u{301}\u{1f600}"] {
        let found = [page(0, vec![word("C\u{327}ag\u{306}rı")]), page(1, vec![word(invalid)])];
        assert!(session.editable_ocr_bytes(id, &found).is_err());
        assert_eq!(session.save_bytes(id).unwrap(), before);
    }
    let original = "\u{306}a\u{1ab0} C\u{327}ag\u{306}rı";
    session.apply(id, crate::Edit::AddOcrText { page: 0, words: vec![word(original)] }).unwrap();
    assert!(crate::tests::page_texts(&session, id)[0].contains(original));
    session.undo(id).unwrap();
    assert_eq!(session.save_bytes(id).unwrap(), before);
}

#[test]
fn editable_nfd_multiple_pages_rotations_and_shared_cluster_fonts() {
    for rotation in [0, 90, 180, 270] {
        let mut doc = pdfcraft_cos::Document::open(Arc::new(crate::tests::fixture(3))).unwrap();
        for p in pdfcraft_model::pages(&doc) {
            doc.update_dict(p.obj, |d| d.set(b"Rotate".to_vec(), pdfcraft_cos::Object::Int(rotation))).unwrap();
        }
        let mut session = Session::new();
        let id = session.open("synthetic.pdf", None, Arc::new(pdfcraft_cos::write_full(&doc, &Default::default()).unwrap()), None).unwrap();
        let first = "C\u{327}ag\u{306}rı";
        let second = "I\u{307}stanbul";
        let bytes = session.editable_ocr_bytes(id, &[page(0, vec![word(first)]), page(2, vec![word(second)])]).unwrap();
        let output = pdfcraft_cos::Document::open(bytes.clone()).unwrap();
        let pages = pdfcraft_model::pages(&output);
        assert_eq!(pages.len(), 3);
        assert!(pages.iter().all(|p| p.dict.int(b"Rotate").unwrap_or(0) == 0));
        let fonts = |page: usize| {
            let res = output.resolve(pages[page].dict.get(b"Resources").unwrap());
            let font = output.resolve(res.as_dict().unwrap().get(b"Font").unwrap());
            font.as_dict().unwrap().clone()
        };
        assert_eq!(fonts(0), fonts(2), "font objects are shared across nonadjacent pages");
        let full: Vec<_> = fonts(0).iter().filter_map(|(_, font)| output.dict(font)).filter(|font| font.name(b"Subtype") == Some(b"Type0")).collect();
        assert_eq!(full.len(), 1, "one full source resource, shared by all OCR pages");
        assert_eq!(pdfcraft_fonts::pdf::embedded_sfnt(&output, &full[0]).unwrap(), pdfcraft_fonts::INTER_REGULAR);
        for font in fonts(0).iter().filter_map(|(_, font)| output.dict(font)).filter(|font| font.name(b"Subtype") == Some(b"Type3")) {
            let descriptor = font.get(b"FontDescriptor").unwrap();
            assert!(descriptor.as_ref().is_some(), "Type3 descriptors remain indirect");
            let descriptor = output.dict(descriptor).unwrap();
            assert_eq!(font.name(b"Name"), descriptor.name(b"FontName"));
            for key in [b"FontFile".as_slice(), b"FontFile2", b"FontFile3"] {
                assert!(!descriptor.contains(key), "font programs belong to the separate Type0 resource");
            }
        }
        let mut renderer = pdfcraft_render::PageRenderer::new(bytes, Default::default());
        for (page, expected) in [(0, first), (1, ""), (2, second)] {
            let text = renderer.filtered_text(page, Default::default()).unwrap();
            assert_eq!(text.plain_text().trim(), expected, "rotation {rotation}, page {page}");
        }
    }
}

#[test]
fn editable_nfd_tounicode_survives_removing_actual_text_and_stacked_ink_is_not_clipped() {
    use pdfcraft_cos::{Dict, Object, Stream};
    let mut session = Session::new();
    let id = session.open("synthetic.pdf", None, Arc::new(crate::tests::fixture(1)), None).unwrap();
    let original = format!("I\u{307}q{}", "\u{301}".repeat(12));
    let bytes = session.editable_ocr_bytes(id, &[page(0, vec![word(&original)])]).unwrap();
    let mut doc = pdfcraft_cos::Document::open(bytes.clone()).unwrap();
    let p = &pdfcraft_model::pages(&doc)[0];
    let content = doc.resolve(p.dict.get(b"Contents").unwrap());
    let Object::Stream(content) = content.as_ref() else { panic!("expected output content") };
    let decoded = content.decoded().unwrap();
    let parsed = pdfcraft_content::parse(&decoded);
    assert!(parsed.ops.iter().any(|op| op.is("BDC")), "ActualText supplements ToUnicode");
    let ops: Vec<_> = parsed.ops.into_iter().filter(|op| !op.is("BMC") && !op.is("BDC") && !op.is("EMC")).collect();
    let replacement = doc.add(Object::Stream(Stream::flate(Dict::new(), &pdfcraft_content::serialize_ops(&ops))));
    doc.update_dict(p.obj, |d| d.set(b"Contents".to_vec(), Object::Ref(replacement))).unwrap();
    let mut renderer = pdfcraft_render::PageRenderer::new(Arc::new(pdfcraft_cos::write_full(&doc, &Default::default()).unwrap()), Default::default());
    let text = renderer.filtered_text(0, Default::default()).unwrap();
    assert_eq!(text.plain_text().trim(), original, "the font map alone owns exact UTF-16 clusters");
    let mut renderer = pdfcraft_render::PageRenderer::new(bytes, Default::default());
    let raster = renderer.render(pdfcraft_render::RenderRequest { page: 0, scale: 2.0, ..Default::default() });
    assert!(raster.error.is_none());
    let mut ink = [u32::MAX, u32::MAX, 0, 0];
    for (i, pixel) in raster.rgba.chunks_exact(4).enumerate() {
        if pixel[..3].iter().any(|v| *v < 240) {
            let (x, y) = (i as u32 % raster.width, i as u32 / raster.width);
            ink[0] = ink[0].min(x);
            ink[1] = ink[1].min(y);
            ink[2] = ink[2].max(x);
            ink[3] = ink[3].max(y);
        }
    }
    assert!(ink[0] < ink[2] && ink[1] < ink[3], "real cluster ink");
    let height = raster.height as f64 / 2.0;
    assert!(ink[1] as f64 / 2.0 >= height - 215.5 && ink[3] as f64 / 2.0 <= height - 199.5, "stacked ink fits its word's vertical box: {ink:?}");
}

#[test]
fn editable_nfd_recognition_notes_use_the_checked_cluster_plan() {
    struct Fixed(&'static str);
    impl pdfcraft_ocr::Recognizer for Fixed {
        fn id(&self) -> &'static str {
            "synthetic"
        }
        fn available(&self) -> bool {
            true
        }
        fn languages(&self) -> Vec<(String, String)> {
            vec![("en".into(), "English".into())]
        }
        fn recognize(
            &self,
            _: &pdfcraft_ocr::OcrImage,
            _: &pdfcraft_ocr::RecognizeOptions,
        ) -> Result<Vec<pdfcraft_ocr::Line>, pdfcraft_ocr::OcrError> {
            Ok(vec![pdfcraft_ocr::Line { words: vec![pdfcraft_ocr::Word::new(self.0, [1.0, 1.0, 90.0, 12.0], "synthetic")] }])
        }
    }
    let mut session = Session::new();
    let id = session.open("synthetic.pdf", None, Arc::new(crate::tests::fixture(1)), None).unwrap();
    for source in ["C\u{327}ag\u{306}rı", "a\u{1ab0}"] {
        let settings = super::OcrSettings { output_mode: super::OutputMode::EditableText, force_ocr: true, ..Default::default() };
        let job = session.ocr_job(id, &[0], settings).unwrap();
        let providers = super::Recognizers { primary: Arc::new(Fixed(source)), secondary: None };
        let found = job.run(&providers, |_, _| true);
        assert_eq!(found[0].words[0].text, source);
        assert!(
            !found[0].notes.iter().any(|note| note.contains("appear as ?")),
            "export refuses losses rather than silently substituting: {:?}",
            found[0].notes
        );
        if source.contains('\u{1ab0}') {
            assert!(
                found[0].notes.iter().any(|note| note.contains("editable") && note.contains("searchable")),
                "unsupported output needs an actionable note"
            );
        } else {
            assert!(found[0].notes.is_empty(), "supported NFD must not get a false missing-font warning: {:?}", found[0].notes);
        }
    }
}

#[test]
fn editable_nfd_export_refuses_geometry_lost_in_page_coordinate_conversion() {
    let mut session = Session::new().with_clock(|| 1_700_000_000);
    let id = session.open("synthetic.pdf", None, Arc::new(crate::tests::fixture(1)), None).unwrap();
    let before = session.save_bytes(id).unwrap();
    let mut tiny = word("a\u{301}");
    tiny.up = [0.0, 0.00000001];
    assert!(
        session.editable_ocr_bytes(id, &[page(0, vec![tiny])]).is_err(),
        "f32 page conversion must not turn a nonempty marked word into a blank successful export"
    );
    assert_eq!(session.save_bytes(id).unwrap(), before);
}
