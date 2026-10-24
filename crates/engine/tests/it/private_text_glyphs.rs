//! Private acceptance input stays outside the repository; no private pixels/fonts are emitted.
use pdfcraft_engine::{BlockStyle, Edit, Session};
use pdfcraft_render::{PageRenderer, RenderRequest};
use std::sync::Arc;

fn pixels(bytes: Arc<Vec<u8>>, page: usize) -> Vec<u8> {
    let mut renderer = PageRenderer::new(bytes, Default::default());
    let rendered = renderer.render(RenderRequest { page, scale: 1.5, ..Default::default() });
    assert!(rendered.error.is_none(), "{:?}", rendered.error);
    assert!(rendered.warnings.is_empty(), "{:?}", rendered.warnings);
    rendered.rgba.to_vec()
}

#[test]
#[ignore = "requires a private Chinese PDF and its full approved face in craft-fonts"]
fn private_new_chinese_glyphs_keep_face_position_save_reopen_and_undo() {
    let path = std::env::var("PDFCRAFT_CHINESE_TEXT_PDF").expect("provide a local PDF");
    let original = Arc::new(std::fs::read(&path).unwrap());
    let mut session = Session::new();
    let id = session.open("private.pdf", None, original.clone(), None).unwrap();
    let pages = session.get(id).unwrap().info.pages.len();
    let all: String = (0..pages).flat_map(|p| session.get(id).unwrap().text_lines(p)).map(|l| l.text).collect();
    let new: String = "测试编辑删除龘".chars().filter(|c| !all.contains(*c)).take(4).collect();
    assert!(new.chars().count() >= 2, "the test must insert characters not already in this PDF");
    for page in 0..pages {
        let before = session.get(id).unwrap();
        let blocks = before.text_blocks(page);
        let (index, block) = blocks.iter().enumerate().find(|(_, b)| b.editor_text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))).unwrap();
        let source_lines = before.text_lines(page);
        let anchor = source_lines[block.lines[0]].baseline();
        let original_pixels = pixels(before.display.clone(), page);
        let source_font_bytes: Vec<_> =
            before.text_editor_fonts(page).into_iter().filter(|f| !f.resource.starts_with("PCApproved")).map(|f| (f.resource, f.bytes)).collect();
        let draft = format!("{}{new}", block.editor_text);
        let generation = before.edit_generation();
        let layout = before.text_edit_layout(page, index, &draft, &BlockStyle::default()).unwrap();
        assert!(layout.draft_rect.is_some());
        assert_eq!(before.edit_generation(), generation);
        session.apply(id, Edit::EditTextBlock { page, block: index, text: draft.clone(), style: BlockStyle::default() }).unwrap();
        let edited = session.get(id).unwrap();
        let lines = edited.text_lines(page);
        let added: Vec<_> = lines.iter().filter(|l| l.text.chars().any(|c| new.contains(c))).collect();
        assert!(!added.is_empty());
        for line in added {
            assert_eq!(line.base_font, "DroidSansFallback", "same family, not a substitute");
            assert!(line.size > 0.0);
            assert_eq!(line.color, source_lines[*block.lines.last().unwrap()].color);
        }
        let first = lines.iter().find(|l| l.text.starts_with(source_lines[block.lines[0]].text.trim())).unwrap();
        assert!(first.baseline().iter().zip(anchor).all(|(a, b)| (a - b).abs() < 0.01));
        let editor_fonts = edited.text_editor_fonts(page);
        for (resource, bytes) in &source_font_bytes {
            assert_eq!(&editor_fonts.iter().find(|f| &f.resource == resource).unwrap().bytes, bytes, "source subset is untouched");
        }
        let live = pixels(edited.display.clone(), page);
        assert_ne!(live, original_pixels);
        let saved = session.save_bytes(id).unwrap();
        let scratch = std::env::temp_dir().join(format!("pdfcraft-private-glyphs-{}.pdf", std::process::id()));
        std::fs::write(&scratch, saved.as_slice()).unwrap();
        let reopened = session.open("reopened.pdf", None, Arc::new(std::fs::read(&scratch).unwrap()), None).unwrap();
        let reopen = session.get(reopened).unwrap();
        assert_eq!(pixels(reopen.display.clone(), page), live);
        assert!(reopen.text_lines(page).iter().any(|l| l.text.contains(&new)));
        let content = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
        assert_eq!(content(&reopen.text_blocks(page)[index].editor_text), content(&draft));
        let shortened = reopen.text_blocks(page)[index].editor_text.replace(&new, &new.chars().skip(1).collect::<String>());
        session.apply(reopened, Edit::EditTextBlock { page, block: index, text: shortened.clone(), style: BlockStyle::default() }).unwrap();
        let saved = session.save_bytes(reopened).unwrap();
        let after_delete = pixels(session.get(reopened).unwrap().display.clone(), page);
        let twice = session.open("deleted-reopened.pdf", None, saved, None).unwrap();
        assert_eq!(pixels(session.get(twice).unwrap().display.clone(), page), after_delete);
        assert_eq!(content(&session.get(twice).unwrap().text_blocks(page)[index].editor_text), content(&shortened));
        session.close(twice);
        session.close(reopened);
        std::fs::remove_file(scratch).unwrap();
        session.undo(id).unwrap();
        assert_eq!(pixels(session.get(id).unwrap().display.clone(), page), original_pixels);
        // The app conservatively stays dirty after Undo; the source bytes remain exact.
        assert_eq!(session.get(id).unwrap().bytes.as_slice(), original.as_slice());
    }
    assert_eq!(std::fs::read(path).unwrap(), original.as_slice());
}
