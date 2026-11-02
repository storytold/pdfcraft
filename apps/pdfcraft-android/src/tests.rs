//! Host tests of the API the Android app uses (`cargo test -p pdfcraft-android`).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use super::*;

/// A fresh folder for one test's files.
fn scratch() -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!("pdfcraft-android-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A blank PDF of `pages` US Letter pages, written to a fresh folder.
fn blank_pdf(pages: usize) -> String {
    let bytes = pdfcraft_engine::Session::new().create_blank(612.0, 792.0, pages).unwrap();
    let path = scratch().join("blank.pdf");
    std::fs::write(&path, bytes.as_slice()).unwrap();
    path.to_string_lossy().into_owned()
}

#[test]
fn opens_a_document_and_describes_its_pages() {
    let s = MobileSession::new();
    let info = s.open(blank_pdf(3), None).unwrap();
    assert_eq!(info.name, "blank.pdf");
    assert_eq!(info.pages.len(), 3);
    assert_eq!((info.pages[0].width, info.pages[0].height), (612.0, 792.0));
    assert_eq!(info.pages[2].label, "3");
    assert!(!info.dirty);
    assert!(info.undo.is_none());
    assert_eq!(s.documents(), vec![info.id]);
}

#[test]
fn renders_a_page_and_a_tile() {
    let s = MobileSession::new();
    let doc = s.open(blank_pdf(1), None).unwrap().id;
    let img = s.render_page(doc, 0, 0.5, None).unwrap();
    assert_eq!((img.width, img.height), (306, 396));
    assert_eq!(img.rgba.len(), 306 * 396 * 4);
    // a blank page is opaque white
    assert!(img.rgba.chunks(4).all(|p| p == [255, 255, 255, 255]));
    let tile = s.render_page(doc, 0, 2.0, Some(TileRect { x: 100, y: 200, width: 64, height: 32 })).unwrap();
    assert_eq!((tile.width, tile.height), (64, 32));
    // renderers are reused, and again after an edit changes the document
    s.render_page(doc, 0, 0.25, None).unwrap();
    s.execute("page_rotate".into(), format!(r#"{{"doc": {doc}, "degrees": 90}}"#)).unwrap();
    let turned = s.render_page(doc, 0, 0.5, None).unwrap();
    assert_eq!((turned.width, turned.height), (396, 306));
}

#[test]
fn refuses_bad_render_requests() {
    let s = MobileSession::new();
    let doc = s.open(blank_pdf(2), None).unwrap().id;
    assert!(matches!(s.render_page(doc, 2, 1.0, None), Err(MobileError::Render(_))));
    for scale in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        assert!(matches!(s.render_page(doc, 0, scale, None), Err(MobileError::Render(_))), "{scale}");
    }
    assert!(matches!(s.render_page(doc + 1, 0, 1.0, None), Err(MobileError::NoDocument(_))));
    // an absurd scale is capped, not an allocation of terabytes
    let huge = s.render_page(doc, 0, 1.0e9, None).unwrap();
    assert!(huge.width <= pdfcraft_render::MAX_SIDE as u32 && huge.height <= pdfcraft_render::MAX_SIDE as u32);
}

#[test]
fn runs_tools_and_tracks_undo() {
    let s = MobileSession::new();
    let doc = s.open(blank_pdf(2), None).unwrap().id;
    let before = s.document(doc).unwrap().generation;
    let out = s.execute("page_delete".into(), format!(r#"{{"doc": {doc}, "pages": [2]}}"#)).unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["pages"], 1);
    let info = s.document(doc).unwrap();
    assert_eq!(info.pages.len(), 1);
    assert!(info.dirty);
    assert!(info.undo.is_some());
    assert_ne!(info.generation, before);
    s.execute("edit_undo".into(), format!(r#"{{"doc": {doc}}}"#)).unwrap();
    assert_eq!(s.document(doc).unwrap().pages.len(), 2);
}

#[test]
fn reports_tool_errors_instead_of_failing_hard() {
    let s = MobileSession::new();
    assert!(matches!(s.execute("no_such_tool".into(), String::new()), Err(MobileError::Command(_))));
    assert!(matches!(s.execute("doc_info".into(), "{not json".into()), Err(MobileError::Command(_))));
    assert!(matches!(s.execute("doc_info".into(), r#"{"doc": 99}"#.into()), Err(MobileError::Command(_))));
    assert!(matches!(s.execute("doc_info".into(), "[1, 2]".into()), Err(MobileError::Command(_))));
    assert!(!s.tools().is_empty());
}

#[test]
fn open_failures_are_told_apart() {
    let s = MobileSession::new();
    assert!(matches!(s.open("relative.pdf".into(), None), Err(MobileError::Open(_))));
    let junk = scratch().join("junk.pdf");
    std::fs::write(&junk, b"this is not a pdf").unwrap();
    assert!(matches!(s.open(junk.to_string_lossy().into_owned(), None), Err(MobileError::Open(_))));
    let missing = scratch().join("missing.pdf");
    assert!(matches!(s.open(missing.to_string_lossy().into_owned(), None), Err(MobileError::Open(_))));
}

#[test]
fn asks_for_a_password_and_opens_with_the_right_one() {
    let s = MobileSession::new();
    let path = blank_pdf(1);
    let doc = s.open(path.clone(), None).unwrap().id;
    s.execute("doc_protect".into(), format!(r#"{{"doc": {doc}, "open_password": "secret"}}"#)).unwrap();
    let locked = scratch().join("locked.pdf").to_string_lossy().into_owned();
    s.execute("doc_save".into(), json!({ "doc": doc, "path": locked }).to_string()).unwrap();
    s.close(doc, true).unwrap();
    assert!(matches!(s.open(locked.clone(), None), Err(MobileError::NeedsPassword)));
    assert!(matches!(s.open(locked.clone(), Some("wrong".into())), Err(MobileError::WrongPassword)));
    let info = s.open(locked, Some("secret".into())).unwrap();
    assert!(info.encrypted);
    let img = s.render_page(info.id, 0, 0.1, None).unwrap();
    assert!(img.width > 0);
}

#[test]
fn flattens_the_outline() {
    let s = MobileSession::new();
    let doc = s.open(blank_pdf(3), None).unwrap().id;
    assert!(s.outline(doc).unwrap().is_empty());
    s.execute("bookmark_add".into(), format!(r#"{{"doc": {doc}, "title": "Intro", "page": 1}}"#)).unwrap();
    s.execute("bookmark_add".into(), format!(r#"{{"doc": {doc}, "title": "End", "page": 3}}"#)).unwrap();
    let outline = s.outline(doc).unwrap();
    assert_eq!(outline.iter().map(|e| (e.title.as_str(), e.level, e.page)).collect::<Vec<_>>(), vec![("Intro", 0, Some(0)), ("End", 0, Some(2))]);
    assert!(s.document(doc).unwrap().has_outline);
}

#[test]
fn closing_refuses_to_lose_edits() {
    let s = MobileSession::new();
    let doc = s.open(blank_pdf(2), None).unwrap().id;
    s.execute("page_delete".into(), format!(r#"{{"doc": {doc}, "pages": [1]}}"#)).unwrap();
    assert!(s.close(doc, false).is_err());
    s.close(doc, true).unwrap();
    assert!(s.documents().is_empty());
    assert!(matches!(s.document(doc), Err(MobileError::NoDocument(_))));
}

/// A PDF made from plain text, written to a fresh folder.
fn text_pdf(text: &str) -> String {
    let bytes = pdfcraft_engine::Session::new().create_from_text("Notes", text).unwrap();
    let path = scratch().join("notes.pdf");
    std::fs::write(&path, bytes.as_slice()).unwrap();
    path.to_string_lossy().into_owned()
}

#[test]
fn selects_words_and_reads_their_text() {
    let s = MobileSession::new();
    let doc = s.open(text_pdf("Hello brave world\nSecond line here"), None).unwrap().id;
    assert!(s.glyph_count(doc, 0).unwrap() > 20);
    let hits = s.find_in_page(doc, 0, "brave".into(), false, false).unwrap();
    assert_eq!(hits.len(), 1);
    let hit = &hits[0];
    assert_eq!(s.text_of(doc, hit.range).unwrap(), "brave");
    assert_eq!(hit.rects.len(), 4);
    assert!(hit.context.contains("Hello brave world"));
    // a long press in the middle of the word selects all of it
    let (cx, cy) = ((hit.rects[0] + hit.rects[2]) / 2.0, (hit.rects[1] + hit.rects[3]) / 2.0);
    let word = s.word_at(doc, 0, cx, cy, 4.0).unwrap().unwrap();
    assert_eq!(word, hit.range);
    // far from any text there is no word
    assert!(s.word_at(doc, 0, 600.0, 780.0, 4.0).unwrap().is_none());
    // a range across the line break reads both lines
    let all = TextRange { page: 0, start: 0, end: s.glyph_count(doc, 0).unwrap() };
    assert_eq!(s.text_of(doc, all).unwrap(), "Hello brave world\nSecond line here");
    assert_eq!(s.text_rects(doc, all).unwrap().len(), 8);
    // out-of-range glyph indices are clamped, not a crash
    let wild = TextRange { page: 0, start: 5, end: u32::MAX };
    assert!(s.text_of(doc, wild).is_ok());
    assert!(s.glyph_rect(doc, 0, u32::MAX).unwrap().is_empty());
    assert!(s.text_of(doc, TextRange { page: 9, start: 0, end: 1 }).is_err());
}

#[test]
fn search_options_and_fresh_text_after_an_edit() {
    let s = MobileSession::new();
    let doc = s.open(text_pdf("Cat cat CAT category"), None).unwrap().id;
    assert_eq!(s.find_in_page(doc, 0, "cat".into(), false, false).unwrap().len(), 4);
    assert_eq!(s.find_in_page(doc, 0, "cat".into(), true, false).unwrap().len(), 2);
    assert_eq!(s.find_in_page(doc, 0, "cat".into(), false, true).unwrap().len(), 3);
    assert!(s.find_in_page(doc, 0, "  ".into(), false, false).unwrap().is_empty());
    let before = s.find_in_page(doc, 0, "Cat".into(), true, false).unwrap()[0].rects.clone();
    // turning the page changes where the text is: the cache follows the document's version
    s.execute("page_rotate".into(), format!(r#"{{"doc": {doc}, "degrees": 90}}"#)).unwrap();
    let after = s.find_in_page(doc, 0, "Cat".into(), true, false).unwrap()[0].rects.clone();
    assert_ne!(before, after);
}

#[test]
fn lists_comments_links_widgets_and_thumbnails() {
    let s = MobileSession::new();
    let path = blank_pdf(2);
    let doc = s.open(path.clone(), None).unwrap().id;
    assert!(s.comments(doc).unwrap().is_empty());
    s.execute(
        "comment_add".into(),
        json!({"doc": doc, "page": 2, "type": "note", "at": [100, 120], "contents": "Look here", "author": "Ann"}).to_string(),
    )
    .unwrap();
    let comments = s.comments(doc).unwrap();
    assert_eq!(comments.len(), 1);
    let c = &comments[0];
    assert_eq!((c.page, c.subtype.as_str(), c.contents.as_deref(), c.author.as_deref()), (1, "Text", Some("Look here"), Some("Ann")));
    // its rectangle is in page points from the top-left, where it was put
    assert!((c.rect[0] - 100.0).abs() < 1.0 && (c.rect[1] - 120.0).abs() < 1.0, "{:?}", c.rect);
    // the index is the one the comment tools take (1-based there)
    s.execute("comment_delete".into(), json!({"doc": doc, "page": c.page + 1, "index": c.index + 1}).to_string()).unwrap();
    assert!(s.comments(doc).unwrap().is_empty());

    s.execute("link_add".into(), json!({"doc": doc, "page": 1, "rect": [50, 60, 150, 80], "to_page": 2}).to_string()).unwrap();
    s.execute("link_add".into(), json!({"doc": doc, "page": 1, "rect": [50, 100, 150, 120], "url": "https://example.org/a"}).to_string()).unwrap();
    let links = s.links(doc, 0).unwrap();
    assert_eq!(links.len(), 2);
    let to_page = links.iter().find(|l| l.page.is_some()).unwrap();
    assert_eq!(to_page.page, Some(1));
    assert!((to_page.rect[1] - 60.0).abs() < 1.0 && (to_page.rect[3] - 80.0).abs() < 1.0, "{:?}", to_page.rect);
    assert_eq!(links.iter().find_map(|l| l.uri.clone()).as_deref(), Some("https://example.org/a"));
    assert!(s.links(doc, 7).unwrap().is_empty());

    s.execute("form_add_field".into(), json!({"doc": doc, "page": 1, "type": "text", "rect": [72, 200, 272, 224], "name": "who"}).to_string())
        .unwrap();
    s.execute("form_add_field".into(), json!({"doc": doc, "page": 1, "type": "checkbox", "rect": [72, 240, 90, 258], "name": "ok"}).to_string())
        .unwrap();
    let widgets = s.widgets(doc).unwrap();
    let who = widgets.iter().find(|w| w.field == "who").unwrap();
    assert_eq!((who.kind.as_str(), who.page), ("text", 0));
    assert!((who.rect[0] - 72.0).abs() < 1.0 && (who.rect[1] - 200.0).abs() < 1.0, "{:?}", who.rect);
    s.execute("form_fill".into(), json!({"doc": doc, "values": {"who": "Bo", "ok": true}}).to_string()).unwrap();
    let widgets = s.widgets(doc).unwrap();
    assert_eq!(widgets.iter().find(|w| w.field == "who").unwrap().value.as_deref(), Some("Bo"));
    assert!(widgets.iter().find(|w| w.field == "ok").unwrap().on);

    let thumb = file_thumbnail(path, 100).unwrap();
    assert!(thumb.height <= 100 && thumb.height >= 99 && thumb.width < thumb.height);
    assert!(file_thumbnail("/no/such/file.pdf".into(), 100).is_err());
    assert!(s.attachments(doc).unwrap().is_empty());
    assert!(s.save_attachment(doc, 0, scratch().join("x").to_string_lossy().into_owned()).is_err());
}

/// A WAV file of 16-bit mono PCM at 16 kHz holding `samples`.
fn wav_file(samples: &[i16]) -> String {
    let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    let mut w = Vec::new();
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&[1, 0, 1, 0]);
    w.extend_from_slice(&16000u32.to_le_bytes());
    w.extend_from_slice(&32000u32.to_le_bytes());
    w.extend_from_slice(&[2, 0, 16, 0]);
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(data.len() as u32).to_le_bytes());
    w.extend_from_slice(&data);
    let path = scratch().join("voice.wav");
    std::fs::write(&path, w).unwrap();
    path.to_string_lossy().into_owned()
}

#[test]
fn a_voice_comment_keeps_its_recording() {
    let s = MobileSession::new();
    let doc = s.open(blank_pdf(1), None).unwrap().id;
    let samples: Vec<i16> = (0..1600).map(|i| ((i as f32 * 0.2).sin() * 12000.0) as i16).collect();
    let wav = wav_file(&samples);
    let args = serde_json::json!({ "doc": doc, "page": 1, "type": "sound", "at": [100, 100], "path": wav, "author": "Tester" });
    s.execute("comment_add".into(), args.to_string()).unwrap();
    let c = s.comments(doc).unwrap();
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].subtype, "Sound");
    // the icon sits where the page was tapped
    assert!((c[0].rect[0] - 100.0).abs() < 0.5 && (c[0].rect[1] - 100.0).abs() < 0.5, "{:?}", c[0].rect);
    // the recording comes back out as the same samples
    let out = scratch().join("out.wav");
    s.save_sound(doc, 0, c[0].index, out.to_string_lossy().into_owned()).unwrap();
    let back = std::fs::read(&out).unwrap();
    assert_eq!(&back[..4], b"RIFF");
    assert_eq!(u32::from_le_bytes([back[24], back[25], back[26], back[27]]), 16000);
    let pcm: Vec<i16> = back[44..].as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b)).collect();
    assert_eq!(pcm, samples);
    // it survives saving and reopening
    let saved = scratch().join("voice.pdf");
    s.execute("doc_save".into(), serde_json::json!({ "doc": doc, "path": saved, "full": true }).to_string()).unwrap();
    let again = s.open(saved.to_string_lossy().into_owned(), None).unwrap().id;
    let c = s.comments(again).unwrap();
    assert_eq!(c[0].subtype, "Sound");
    s.save_sound(again, 0, c[0].index, out.to_string_lossy().into_owned()).unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), back);
    // not a recording: a clear error, no comment
    let bad = scratch().join("bad.wav");
    std::fs::write(&bad, b"RIFF....WAVEjunk").unwrap();
    let args = serde_json::json!({ "doc": doc, "page": 1, "type": "sound", "at": [10, 10], "path": bad });
    assert!(s.execute("comment_add".into(), args.to_string()).is_err());
    assert!(s.save_sound(doc, 0, 99, out.to_string_lossy().into_owned()).is_err());
}

/// A one-page PDF with a file attached to the document (`data.csv`) and one on a comment (`note.txt`).
fn attachments_pdf() -> String {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R /Names << /EmbeddedFiles 6 0 R >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [4 0 R] >>",
        "<< /Type /Annot /Subtype /FileAttachment /Rect [10 10 30 30] /FS 5 0 R /Contents (note) >>",
        "<< /Type /Filespec /F (note.txt) /EF << /F 7 0 R >> >>",
        "<< /Names [(data.csv) 8 0 R] >>",
        "<< /Type /EmbeddedFile /Length 5 >>\nstream\nhello\nendstream",
        "<< /Type /Filespec /F (data.csv) /EF << /F 9 0 R >> >>",
        "<< /Type /EmbeddedFile /Length 3 >>\nstream\na,b\nendstream",
    ];
    let mut pdf = String::from("%PDF-1.7\n");
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.push_str(&format!("{} 0 obj\n{body}\nendobj\n", i + 1));
    }
    let xref = pdf.len();
    pdf.push_str(&format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1));
    for o in offsets {
        pdf.push_str(&format!("{o:010} 00000 n \n"));
    }
    pdf.push_str(&format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1));
    let path = scratch().join("attachments.pdf");
    std::fs::write(&path, pdf).unwrap();
    path.to_string_lossy().into_owned()
}

#[test]
fn tells_document_attachments_from_comment_attachments() {
    let s = MobileSession::new();
    let info = s.open(attachments_pdf(), None).unwrap();
    // only the document's own file counts for the Attachments list
    assert_eq!(info.attachments, 1);
    let list = s.attachments(info.id).unwrap();
    let doc_file = list.iter().find(|a| a.name == "data.csv").unwrap();
    let comment_file = list.iter().find(|a| a.name == "note.txt").unwrap();
    assert!(!doc_file.on_page);
    assert!(comment_file.on_page);
}

#[test]
fn reading_blocks_split_paragraphs_and_map_characters_to_glyphs() {
    let s = MobileSession::new();
    let doc = s.open(text_pdf("First paragraph here.\n\nSecond one follows."), None).unwrap().id;
    let blocks = s.reading_blocks(doc, 0).unwrap();
    let texts: Vec<&str> = blocks.iter().map(|b| b.text.as_str()).collect();
    assert_eq!(texts, vec!["First paragraph here.", "Second one follows."]);
    for b in &blocks {
        // one glyph per character, within the block's range and in order
        assert_eq!(b.glyph_of_char.len(), b.text.chars().count());
        assert!(b.glyph_of_char.windows(2).all(|w| w[0] <= w[1]));
        assert!(b.glyph_of_char.iter().all(|&g| g >= b.range.start && g < b.range.end));
    }
    // the word "Second" maps to glyphs whose text is that word
    let second = &blocks[1];
    let range = TextRange { page: 0, start: second.glyph_of_char[0], end: second.glyph_of_char[5] + 1 };
    assert_eq!(s.text_of(doc, range).unwrap(), "Second");
}

#[test]
fn signs_with_a_digital_id_and_trusts_its_signer() {
    let s = MobileSession::new();
    let dir = scratch();
    let id = dir.join("me.p12").to_string_lossy().into_owned();
    let made = json!({ "name": "Test Signer", "email": "signer@example.com", "password": "secret12", "path": id });
    s.execute("sign_id_create".into(), made.to_string()).unwrap();
    // the file's certificate, and a wrong password told apart
    let info = digital_id_info(id.clone(), "secret12".into()).unwrap();
    assert_eq!(info.name, "Test Signer");
    assert_eq!(info.email.as_deref(), Some("signer@example.com"));
    assert!(info.self_signed && info.valid_to > info.valid_from && info.problem.is_none());
    assert_eq!(info.sha256.replace(' ', "").len(), 64);
    assert!(matches!(digital_id_info(id.clone(), "wrong".into()), Err(MobileError::WrongPassword)));
    let junk = dir.join("junk.p12");
    std::fs::write(&junk, b"not a digital id").unwrap();
    assert!(matches!(digital_id_info(junk.to_string_lossy().into_owned(), "x".into()), Err(MobileError::Command(_))));

    // signing writes the signed file over the working copy the document was opened from
    let path = blank_pdf(1);
    let doc = s.open(path.clone(), None).unwrap().id;
    assert!(s.signer_certificate(doc, "Signature1".into()).unwrap().is_none());
    let args =
        json!({ "doc": doc, "id": id, "password": "secret12", "page": 1, "rect": [72, 72, 252, 122], "graphic": { "text": "Test" }, "out": path });
    let out: Value = serde_json::from_str(&s.execute("sign_document".into(), args.to_string()).unwrap()).unwrap();
    let field = out["signature"]["field"].as_str().unwrap().to_string();
    assert_eq!(out["signature"]["status"], "unknown");
    let info = s.document(doc).unwrap();
    assert!(info.signed && !info.dirty && info.undo.is_none());
    assert_eq!(std::fs::read(&path).unwrap().len() as u64, out["bytes"].as_u64().unwrap());

    // trusting the signer's certificate makes the signature valid
    let cert = s.signer_certificate(doc, field).unwrap().unwrap();
    let cer = dir.join("signer.cer");
    std::fs::write(&cer, &cert).unwrap();
    s.execute("sign_trust".into(), json!({ "paths": [cer.to_string_lossy()] }).to_string()).unwrap();
    let list: Value = serde_json::from_str(&s.execute("sign_list".into(), json!({ "doc": doc }).to_string()).unwrap()).unwrap();
    assert_eq!(list["all_valid"], true);

    // an edit after signing is saved incrementally: the signature stays valid
    s.execute("comment_add".into(), json!({ "doc": doc, "page": 1, "type": "note", "at": [300, 300], "contents": "after" }).to_string()).unwrap();
    s.execute("doc_save".into(), json!({ "doc": doc }).to_string()).unwrap();
    let list: Value = serde_json::from_str(&s.execute("sign_list".into(), json!({ "doc": doc }).to_string()).unwrap()).unwrap();
    assert_eq!(list["signatures"][0]["status"], "valid");
    assert_eq!(list["signatures"][0]["modification"], "allowed");
}

/// A form's scripts run through the calls the app makes: filling a field recalculates a scripted
/// total and returns its alert, and a button's script (from `form_actions`) runs with `js_run`.
#[test]
fn form_scripts_run_when_fields_are_filled_and_buttons_pressed() {
    let s = MobileSession::new();
    let doc = s.open(blank_pdf(1), None).unwrap().id;
    let run = |tool: &str, args: Value| -> Value { serde_json::from_str(&s.execute(tool.into(), args.to_string()).unwrap()).unwrap() };
    for (i, name) in ["a", "b", "total"].into_iter().enumerate() {
        let y = 100 + 30 * i;
        run("form_add_field", json!({"doc": doc, "page": 1, "type": "text", "rect": [200, y, 360, y + 20], "name": name}));
    }
    run("form_add_field", json!({"doc": doc, "page": 1, "type": "button", "rect": [200, 200, 360, 224], "name": "double", "caption": "Double a"}));
    let sum = "event.value = Number(this.getField(\"a\").value) + Number(this.getField(\"b\").value); if (event.value > 9) app.alert(\"Big\");";
    run("form_set_script", json!({"doc": doc, "field": "total", "event": "calculate", "script": sum}));
    let double = "var a = this.getField(\"a\"); a.value = Number(a.value) * 2; app.alert(\"a is now \" + a.value);";
    run("form_set_script", json!({"doc": doc, "field": "double", "event": "mouse_up", "script": double}));
    let total = |s: &MobileSession| s.widgets(doc).unwrap().into_iter().find(|w| w.field == "total").and_then(|w| w.value);

    run("form_fill", json!({"doc": doc, "values": {"a": "2"}}));
    let filled = run("form_fill", json!({"doc": doc, "values": {"b": "3"}}));
    assert_eq!(total(&s).as_deref(), Some("5"), "{filled}");
    let actions = run("form_actions", json!({"doc": doc, "field": "double"}));
    let script = actions["actions"][0]["javascript"].as_str().unwrap().to_owned();
    let pressed = run("js_run", json!({"doc": doc, "field": "double", "script": script}));
    assert_eq!(pressed["alerts"][0]["message"], "a is now 4", "{pressed}");
    assert_eq!(total(&s).as_deref(), Some("7"));
    let big = run("form_fill", json!({"doc": doc, "values": {"b": "8"}}));
    assert_eq!(big["alerts"][0]["message"], "Big", "{big}");
}
