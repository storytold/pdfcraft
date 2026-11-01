//! Document font selection and contextual glyph save/reopen acceptance on synthetic PDFs.
use crate::support;
use pdfcraft_cos::{Dict, Document, Object, SaveOptions, Stream};
use pdfcraft_engine::{BlockStyle, Edit, Session};
use pdfcraft_render::{PageRenderer, RenderRequest};
use std::sync::Arc;
fn document() -> Arc<Vec<u8>> {
    let mut d = Document::new_empty();
    let pages = d.add(Object::Dict(Dict::new()));
    let contents = d.add(Object::Stream(Stream::flate(Dict::new(), b"BT /F1 18 Tf 30 160 Td (Source) Tj ET")));
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("Type1"));
    font.set(b"BaseFont".to_vec(), Object::name("Helvetica"));
    let mut fonts = Dict::new();
    fonts.set(b"F1".to_vec(), Object::Dict(font));
    let mut res = Dict::new();
    res.set(b"Font".to_vec(), Object::Dict(fonts));
    let mut p = Dict::new();
    p.set(b"Type".to_vec(), Object::name("Page"));
    p.set(b"Parent".to_vec(), Object::Ref(pages));
    p.set(b"Contents".to_vec(), Object::Ref(contents));
    p.set(b"Resources".to_vec(), Object::Dict(res));
    p.set(b"MediaBox".to_vec(), Object::Array([0, 0, 300, 240].map(Object::Int).to_vec()));
    let page = d.add(Object::Dict(p));
    d.update_dict(pages, |p| {
        p.set(b"Type".to_vec(), Object::name("Pages"));
        p.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
        p.set(b"Count".to_vec(), Object::Int(1));
    })
    .unwrap();
    let mut root = Dict::new();
    root.set(b"Type".to_vec(), Object::name("Catalog"));
    root.set(b"Pages".to_vec(), Object::Ref(pages));
    let root = d.add(Object::Dict(root));
    d.trailer_mut().set(b"Root".to_vec(), Object::Ref(root));
    Arc::new(pdfcraft_cos::write_full(&d, &SaveOptions::default()).unwrap())
}
fn pixels(bytes: Arc<Vec<u8>>) -> Vec<u8> {
    let mut renderer = PageRenderer::new(bytes, Default::default());
    let p = renderer.render(RenderRequest { page: 0, scale: 2.0, ..Default::default() });
    assert!(p.error.is_none(), "{:?}", p.error);
    assert!(p.warnings.is_empty(), "{:?}", p.warnings);
    p.rgba.to_vec()
}
fn exercise(font: Vec<u8>, text: &str, expected_family: &str) {
    let mut s = Session::new();
    let id = s.open("source.pdf", None, document(), None).unwrap();
    let original = pixels(s.get(id).unwrap().display.clone());
    s.apply(id, Edit::RegisterTextFont { page: 0, bytes: Arc::new(font.clone()) }).unwrap();
    let resource = s.get(id).unwrap().text_editor_fonts(0).into_iter().find(|f| f.bytes == font).unwrap().resource;
    let style = BlockStyle { resource: Some(resource), width: Some(240.0), ..Default::default() };
    s.apply(id, Edit::EditTextBlock { page: 0, block: 0, text: text.into(), style: style.clone() }).unwrap();
    let edited = s.get(id).unwrap();
    let lines = edited.text_lines(0);
    assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<String>(), text);
    assert!(lines.iter().all(|l| l.base_font == expected_family));
    assert!((lines[0].baseline()[1] - 160.0).abs() < 0.01);
    let current = pixels(edited.display.clone());
    assert_ne!(current, original);
    let saved = s.save_bytes(id).unwrap();
    let re = s.open("saved.pdf", None, saved, None).unwrap();
    assert_eq!(pixels(s.get(re).unwrap().display.clone()), current);
    assert_eq!(s.get(re).unwrap().text_blocks(0)[0].editor_text, text);
    let twice = format!("{text} {text}");
    s.apply(re, Edit::EditTextBlock { page: 0, block: 0, text: twice.clone(), style: style.clone() }).unwrap();
    assert_eq!(s.get(re).unwrap().text_blocks(0)[0].editor_text, twice);
    let again = s.save_bytes(re).unwrap();
    let next = s.open("twice.pdf", None, again, None).unwrap();
    assert_eq!(s.get(next).unwrap().text_blocks(0)[0].editor_text, twice);
    assert_eq!(pixels(s.get(next).unwrap().display.clone()), pixels(s.get(re).unwrap().display.clone()));
    s.apply(re, Edit::EditTextBlock { page: 0, block: 0, text: String::new(), style }).unwrap();
    assert!(s.get(re).unwrap().text_lines(0).is_empty(), "every contextual glyph is deleted");
    let empty = pixels(s.get(re).unwrap().display.clone());
    assert!(empty.as_chunks::<4>().0.iter().all(|p| p[..3] == [255; 3]));
    s.undo(id).unwrap();
    assert_eq!(pixels(s.get(id).unwrap().display.clone()), original);
}
#[test]
fn opentype_cff_document_font_preserves_program_codes_and_rendering_after_reopen() {
    exercise(support::font_fixture::cff_font(), "A A", "RectFont-Regular");
}
#[test]
fn contextual_arabic_marks_ligatures_and_mixed_bidi_keep_logical_text_after_reopen() {
    if let Some(path) = std::env::var_os("PDFCRAFT_TEST_ARABIC_FONT") {
        exercise(std::fs::read(path).unwrap(), "سَلَام 123", "NotoSansArabic-Regular");
        return;
    }
    let Some(font) = pdfcraft_fonts::document_arabic_font() else {
        eprintln!("set CRAFT_FONTS_DIR to test full Arabic glyph programs");
        return;
    };
    if pdfcraft_fonts::embedding::validate(font.bytes).is_err() {
        eprintln!("set PDFCRAFT_TEST_ARABIC_FONT to an openly licensed static Noto Sans Arabic face");
        return;
    }
    exercise(font.bytes.to_vec(), "سَلَام 123", "NotoSansArabic-Regular");
}

/// A complete source program behind a deliberately partial PDF encoding, not a registered
/// replacement font. Sin has one advancing body per cluster, isolating source-face selection
/// from the separate empty-auxiliary and ToUnicode ownership regressions.
fn complete_arabic_source() -> Option<(Vec<u8>, Arc<Vec<u8>>)> {
    let Some(path) = std::env::var_os("PDFCRAFT_TEST_ARABIC_FONT") else {
        assert!(std::env::var_os("PDFCRAFT_ARABIC_CORPUS_REQUIRED").is_none(), "set PDFCRAFT_TEST_ARABIC_FONT to run required Arabic source editing");
        eprintln!("Arabic source editing not exercised: set PDFCRAFT_TEST_ARABIC_FONT to an openly licensed static Noto Sans Arabic face");
        return None;
    };
    let font = std::fs::read(path).unwrap();
    pdfcraft_fonts::embedding::validate_document_input(&font).unwrap();
    let mut doc = Document::open(document()).unwrap();
    let shaped = pdfcraft_fonts::source_shaping::shape(&font, "سسس").unwrap();
    assert!(shaped.glyphs.iter().all(|glyph| !glyph.text.is_empty()), "this fixture must not depend on empty CMap destinations");
    let mappings: Vec<_> = shaped.glyphs.iter().map(|glyph| (glyph.text.clone(), glyph.id)).collect();
    let mut fonts = Dict::new();
    let (_, postscript) = pdfcraft_fonts::embedding::names(&font).unwrap();
    let name = pdfcraft_fonts::embedding::embed_mapping(&mut doc, &mut fonts, &font, &postscript, &mappings).unwrap();
    let mut content = format!("BT /{name} 16 Tf\n");
    for (i, glyph) in shaped.glyphs.iter().enumerate() {
        content.push_str(&format!("1 0 0 1 {} {} Tm <{:04X}> Tj\n", 30.0 + 16.0 * glyph.position[0], 160.0 + 16.0 * glyph.position[1], i + 1));
    }
    content.push_str("ET\n");
    let contents = doc.add(Object::Stream(Stream::flate(Dict::new(), content.as_bytes())));
    let page = pdfcraft_model::pages(&doc)[0].obj;
    doc.update_dict(page, |page| {
        let mut resources = Dict::new();
        resources.set(b"Font".to_vec(), Object::Dict(fonts));
        page.set(b"Resources".to_vec(), Object::Dict(resources));
        page.set(b"Contents".to_vec(), Object::Ref(contents));
        page.set(b"ArabicEditSentinel".to_vec(), Object::Int(42));
    })
    .unwrap();
    Some((font, Arc::new(pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap())))
}

fn arabic_edit_state(session: &Session, id: pdfcraft_engine::DocId, expected: &str, font: &[u8]) -> Vec<u8> {
    let doc = session.get(id).unwrap();
    assert_eq!(doc.text_blocks(0)[0].editor_text, expected);
    assert!(doc.edit_notices.is_empty(), "no undrawn-text or substitution notices: {:?}", doc.edit_notices);
    let source_faces: Vec<_> = doc.text_editor_fonts(0).into_iter().filter(|face| !face.resource.starts_with("PCApproved")).collect();
    assert!(!source_faces.is_empty());
    assert!(source_faces.iter().all(|face| face.bytes == font), "reuse the exact embedded source program");
    let mut renderer = PageRenderer::new(doc.display.clone(), Default::default());
    let text = renderer.filtered_text(0, Default::default()).unwrap();
    assert_eq!(text.plain_text(), expected);
    assert_eq!(text.text_of(0..text.glyphs.len()), expected, "full selection retains exact Unicode");
    assert!(!text.find(expected).is_empty(), "the edited logical phrase remains searchable");
    pixels(doc.display.clone())
}

#[test]
fn complete_arabic_source_edit_history_and_incremental_reopen_keep_unicode_and_pixels() {
    let Some((font, source)) = complete_arabic_source() else { return };
    let artifact_dir = std::env::var_os("PDFCRAFT_ARABIC_EDIT_ARTIFACT_DIR").map(std::path::PathBuf::from);
    if let Some(dir) = &artifact_dir {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("source.pdf"), source.as_slice()).unwrap();
    }
    for (case, replacement) in
        [("latin", "Hello 123"), ("marks", "سَسَ"), ("greeting", "السَّلَامُ عَلَيْكُمْ"), ("numbers", "سَلَام 123"), ("mixed", "مرحبا Hello 123")]
    {
        let mut session = Session::new();
        let id = session.open("source.pdf", None, source.clone(), None).unwrap();
        let original_pixels = arabic_edit_state(&session, id, "سسس", &font);
        let style = BlockStyle { width: Some(240.0), ..Default::default() };
        let before_preview = session.get(id).unwrap().display.clone();
        let layout = session.get(id).unwrap().text_edit_layout(0, 0, replacement, &style).unwrap();
        let draft_rect = layout.draft_rect.unwrap();
        let background = session.get(id).unwrap().text_edit_preview(0, 0).unwrap();
        let background = background.render_background(2.0).unwrap();
        assert!(background.error.is_none() && background.warnings.is_empty());
        assert!(background.rgba.as_chunks::<4>().0.iter().all(|pixel| pixel[..3] == [255; 3]));
        let unmodified = session.get(id).unwrap();
        assert_eq!(unmodified.display, before_preview);
        assert!(!unmodified.dirty && unmodified.can_undo().is_none());
        session.apply(id, Edit::EditTextBlock { page: 0, block: 0, text: replacement.into(), style: style.clone() }).unwrap();
        let applied_rect = session.get(id).unwrap().text_blocks(0)[0].rect;
        assert!(
            draft_rect.iter().zip(applied_rect).all(|(predicted, applied)| (predicted - applied).abs() < 0.01),
            "{case}: preview {draft_rect:?}, applied {applied_rect:?}"
        );
        let edited_pixels = arabic_edit_state(&session, id, replacement, &font);
        let original_geometry = session
            .get(id)
            .unwrap()
            .text_lines(0)
            .into_iter()
            .map(|line| (line.font.clone(), line.size, line.baseline(), line.rect))
            .collect::<Vec<_>>();
        assert!(edited_pixels != original_pixels, "{case}: editing changes the page");
        session.undo(id).unwrap();
        assert_eq!(arabic_edit_state(&session, id, "سسس", &font), original_pixels);
        session.redo(id).unwrap();
        assert_eq!(arabic_edit_state(&session, id, replacement, &font), edited_pixels);
        let saved = session.save_bytes(id).unwrap();
        assert!(saved.starts_with(source.as_slice()), "incremental Save retains the original source bytes");
        let cos = Document::open(saved.clone()).unwrap();
        assert_eq!(pdfcraft_model::pages(&cos)[0].dict.get(b"ArabicEditSentinel"), Some(&Object::Int(42)), "unknown page data survives");
        if let Some(dir) = &artifact_dir {
            std::fs::write(dir.join(format!("edited-{case}.pdf")), saved.as_slice()).unwrap();
            std::fs::write(dir.join(format!("edited-{case}.txt")), replacement).unwrap();
        }
        let reopened = session.open("saved.pdf", None, saved, None).unwrap();
        assert_eq!(arabic_edit_state(&session, reopened, replacement, &font), edited_pixels);
        let inserted = format!("{replacement} س");
        session.apply(reopened, Edit::EditTextBlock { page: 0, block: 0, text: inserted.clone(), style: style.clone() }).unwrap();
        arabic_edit_state(&session, reopened, &inserted, &font);
        session.apply(reopened, Edit::EditTextBlock { page: 0, block: 0, text: replacement.into(), style }).unwrap();
        let deleted_pixels = arabic_edit_state(&session, reopened, replacement, &font);
        let deleted_geometry = session
            .get(reopened)
            .unwrap()
            .text_lines(0)
            .into_iter()
            .map(|line| (line.font.clone(), line.size, line.baseline(), line.rect))
            .collect::<Vec<_>>();
        if deleted_pixels != edited_pixels
            && let Some(dir) = &artifact_dir
        {
            std::fs::write(dir.join(format!("deleted-{case}.pdf")), session.save_bytes(reopened).unwrap().as_slice()).unwrap();
        }
        assert!(deleted_pixels == edited_pixels, "{case}: original {original_geometry:?}; deleted {deleted_geometry:?}");
        session.undo(reopened).unwrap();
        arabic_edit_state(&session, reopened, &inserted, &font);
        session.redo(reopened).unwrap();
        assert!(arabic_edit_state(&session, reopened, replacement, &font) == deleted_pixels, "{case}: redo retains its saved page appearance");
        let deleted = session.save_bytes(reopened).unwrap();
        let after_delete = session.open("deleted.pdf", None, deleted, None).unwrap();
        assert!(arabic_edit_state(&session, after_delete, replacement, &font) == deleted_pixels, "{case}: the repeated edit survives serialization");
    }
}

#[test]
fn unsupported_mixed_style_arabic_edit_preserves_source_and_history() {
    let Some((_, source)) = complete_arabic_source() else { return };
    let mut session = Session::new();
    let id = session.open("source.pdf", None, source, None).unwrap();
    let before = session.get(id).unwrap().display.clone();
    let range = pdfcraft_edit::text::RangeStyle {
        range: 0..2,
        family: None,
        resource: None,
        size: None,
        color: Some([1.0, 0.0, 0.0]),
        underline: None,
        char_spacing: None,
        scale: None,
    };
    let style = BlockStyle { width: Some(240.0), ranges: vec![range], ..Default::default() };
    let error = session.apply(id, Edit::EditTextBlock { page: 0, block: 0, text: "سَسَ".into(), style }).unwrap_err();
    assert!(error.to_string().contains("uniform full-face row"), "{error}");
    let after = session.get(id).unwrap();
    assert_eq!(after.display, before);
    assert!(!after.dirty);
    assert!(after.can_undo().is_none() && after.can_redo().is_none());
    assert_eq!(after.text_blocks(0)[0].editor_text, "سسس");
}

#[test]
fn leading_arabic_marks_retain_position_when_an_inserted_suffix_is_deleted() {
    let Some((font, _)) = complete_arabic_source() else { return };
    for logical in ["سَسَ", "السَّلَامُ عَلَيْكُمْ", "مرحباً بكم في جنيف", "لا لأ لإ لآ سلام", "سَلَام 123", "مرحبا Bonjour Genève Hello PDF", "المبلغ ١٢٣٫٤٥ و 123.45"]
    {
        for rise in [0.0, 5.0, -4.0] {
            let mut source = Document::open(document()).unwrap();
            let content = format!("BT /F1 18 Tf 30 160 Td {rise} Ts (Source) Tj ET");
            let stream = source.add(Object::Stream(Stream::flate(Dict::new(), content.as_bytes())));
            let page = pdfcraft_model::pages(&source)[0].obj;
            source
                .update_dict(page, |page| {
                    page.set(b"Contents".to_vec(), Object::Ref(stream));
                    page.set(b"MediaBox".to_vec(), Object::Array([0, 0, 600, 240].map(Object::Int).to_vec()));
                })
                .unwrap();
            let mut session = Session::new();
            let mut id =
                session.open("source.pdf", None, Arc::new(pdfcraft_cos::write_full(&source, &SaveOptions::default()).unwrap()), None).unwrap();
            // Explicit registration isolates positioning from source-font completion.
            session.apply(id, Edit::RegisterTextFont { page: 0, bytes: Arc::new(font.clone()) }).unwrap();
            let resource = session.get(id).unwrap().text_editor_fonts(0).into_iter().find(|face| face.bytes == font).unwrap().resource;
            let style = BlockStyle { resource: Some(resource), size: Some(16.0), width: Some(540.0), ..Default::default() };
            session.apply(id, Edit::EditTextBlock { page: 0, block: 0, text: logical.into(), style: style.clone() }).unwrap();
            let before = arabic_edit_state(&session, id, logical, &font);
            let position = session.get(id).unwrap().text_lines(0)[0].baseline();
            assert!((position[1] - (160.0 + rise)).abs() < 0.01, "{logical:?}, rise {rise}: {position:?}");
            for suffix in [" س", " 123", " Hello"] {
                let inserted = format!("{logical}{suffix}");
                session.apply(id, Edit::EditTextBlock { page: 0, block: 0, text: inserted.clone(), style: style.clone() }).unwrap();
                arabic_edit_state(&session, id, &inserted, &font);
                session.apply(id, Edit::EditTextBlock { page: 0, block: 0, text: logical.into(), style: style.clone() }).unwrap();
                let after = arabic_edit_state(&session, id, logical, &font);
                let next_position = session.get(id).unwrap().text_lines(0)[0].baseline();
                assert!(before == after, "{logical:?}, rise {rise}: before {position:?}, after {next_position:?}");
                session.undo(id).unwrap();
                arabic_edit_state(&session, id, &inserted, &font);
                session.redo(id).unwrap();
                assert!(arabic_edit_state(&session, id, logical, &font) == before);
                let saved = session.save_bytes(id).unwrap();
                let reopened = session.open("reopened.pdf", None, saved, None).unwrap();
                assert!(arabic_edit_state(&session, reopened, logical, &font) == before);
                session.close(id);
                id = reopened;
            }
        }
    }
}

#[test]
fn source_glyph_unicode_without_actual_text_preserves_arabic_word_copy() {
    let Some((font, seed)) = complete_arabic_source() else { return };
    for logical in ["اللغة العربية جميلة", "مرحباً بكم في جنيف", "السَّلَامُ عَلَيْكُمْ"] {
        let mut doc = Document::open(seed.clone()).unwrap();
        let shaped = pdfcraft_fonts::source_shaping::shape(&font, logical).unwrap();
        let mut fonts = Dict::new();
        let (_, postscript) = pdfcraft_fonts::embedding::names(&font).unwrap();
        let mappings = shaped.pdf_mappings(&font).unwrap();
        let resource = pdfcraft_fonts::embedding::embed_mapping(&mut doc, &mut fonts, &font, &postscript, &mappings).unwrap();
        let mut content = format!("BT /{resource} 16 Tf\n");
        for (i, glyph) in shaped.glyphs.iter().enumerate() {
            content.push_str(&format!("1 0 0 1 {} {} Tm <{:04X}> Tj\n", 30.0 + 16.0 * glyph.position[0], 160.0 + 16.0 * glyph.position[1], i + 1));
        }
        content.push_str("ET");
        let contents = doc.add(Object::Stream(Stream::flate(Dict::new(), content.as_bytes())));
        let page = pdfcraft_model::pages(&doc)[0].obj;
        doc.update_dict(page, |page| {
            let mut resources = Dict::new();
            resources.set(b"Font".to_vec(), Object::Dict(fonts));
            page.set(b"Resources".to_vec(), Object::Dict(resources));
            page.set(b"Contents".to_vec(), Object::Ref(contents));
        })
        .unwrap();
        let initial = Arc::new(pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap());
        let mut session = Session::new();
        let id = session.open("mapped-source.pdf", None, initial.clone(), None).unwrap();
        let original = arabic_edit_state(&session, id, logical, &font);
        let mut renderer = PageRenderer::new(initial.clone(), Default::default());
        let text = renderer.filtered_text(0, Default::default()).unwrap();
        for word in logical.split_whitespace() {
            let matches = text.find(word);
            assert!(!matches.is_empty(), "{word}");
            for range in matches {
                assert_eq!(text.text_of(range.clone()), word);
                let (start, last) = text.word_at(range.start).unwrap();
                assert_eq!(text.text_of(start..last + 1), word);
            }
        }
        let reopened = Document::open(initial).unwrap();
        let saved = Arc::new(pdfcraft_cos::write_full(&reopened, &SaveOptions::default()).unwrap());
        let next = session.open("reserialized.pdf", None, saved, None).unwrap();
        assert!(arabic_edit_state(&session, next, logical, &font) == original);
    }
}
