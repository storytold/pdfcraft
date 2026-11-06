//! Finger-sized comment controls, exercised with actual touch release and PointerGone events.

use egui::{Event, Pos2, Rect, TouchPhase, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use pdfcraft_ui_egui::{PdfCraftApp, comments::CommentTool};

fn page_bytes() -> Vec<u8> {
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in
        ["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [3 0 R] /Count 1 >>", "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] >>"]
            .iter()
            .enumerate()
    {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 4\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer << /Size 4 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    bytes
}

fn harness(tool: &'static str, touch: bool, size: egui::Vec2) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(size).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.set_option("touch-mode", if touch { "on" } else { "off" }).unwrap();
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "none").unwrap();
        app.set_option("author", "Tester").unwrap();
        app.open_bytes("touch-controls.pdf", None, page_bytes()).unwrap();
        app.set_option("quick", tool).unwrap();
        app
    });
    h.run_steps(5);
    h
}

fn tap(h: &mut Harness<'static, PdfCraftApp>, pos: Pos2) {
    let contact = |phase| Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(1), phase, pos, force: None };
    let button = |pressed| Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
    h.input_mut().events.extend([Event::PointerMoved(pos), contact(TouchPhase::Start), button(true)]);
    h.run_steps(1);
    h.input_mut().events.extend([button(false), contact(TouchPhase::End), Event::PointerGone]);
    h.run_steps(3);
}

fn page_point(h: &Harness<'static, PdfCraftApp>) -> Pos2 {
    let page = h.state().views[0].page_screen_rect(0).unwrap();
    page.min + page.size() * 0.3
}

fn target(h: &Harness<'static, PdfCraftApp>, label: &str) -> Rect {
    let r = h.get_by_label(label).rect();
    assert!(r.width() >= 44.0 && r.height() >= 44.0, "{label}: {r:?}");
    assert!(h.ctx.content_rect().contains_rect(r), "{label} is reachable: {r:?}");
    r
}

fn shot(h: &mut Harness<'static, PdfCraftApp>, name: &str) {
    // One GPU-backed harness per process also works on tablet graphics drivers that cannot
    // create multiple headless adapters concurrently. Normal tests never need a GPU.
    if std::env::var("PDFCRAFT_TOUCH_SHOT_NAME").ok().as_deref() != Some(name) {
        return;
    }
    if let Some(directory) = std::env::var_os("PDFCRAFT_TOUCH_SHOTS") {
        // Only one wgpu device at a time in this test binary.
        let _gpu = crate::gpu();
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        h.render().unwrap().save(directory.join(format!("{name}.png"))).unwrap();
    }
}

#[test]
fn touch_notes_and_text_boxes_post_cancel_undo_and_save_without_hover() {
    for size in [vec2(1024.0, 768.0), vec2(768.0, 1024.0)] {
        for (tool, subtype) in [("note", "Text"), ("freetext", "FreeText")] {
            let mut h = harness(tool, true, size);
            for label in ["Hand (H)", "Select (V)", "Draw", "Highlight", "Add a sticky note", "Add a text box", "Eraser", "Undo", "Redo"] {
                target(&h, label);
            }
            let p = page_point(&h);
            tap(&mut h, p);
            assert!(h.state().views[0].comments.composer.is_some(), "touch opens {tool}");
            h.input_mut().events.push(Event::Text("A tablet note".into()));
            h.run_steps(2);
            let post = target(&h, "Post");
            target(&h, "Cancel");
            shot(&mut h, &format!("{tool}-{}x{}", size.x, size.y));
            tap(&mut h, post.center());
            assert!(h.state().views[0].comments.composer.is_none());
            let id = h.state().views[0].id;
            let annotations = &h.state().session.get(id).unwrap().info.annotations;
            assert_eq!(annotations.len(), 1);
            assert_eq!(annotations[0].subtype, subtype);
            assert_eq!(annotations[0].contents.as_deref(), Some("A tablet note"));

            h.state_mut().undo();
            h.run_steps(2);
            assert!(h.state().session.get(id).unwrap().info.annotations.is_empty());
            h.state_mut().redo();
            h.run_steps(2);
            let bytes = h.state().session.save_bytes(id).unwrap();
            let mut reopened = PdfCraftApp::new();
            reopened.open_bytes("saved.pdf", None, bytes.as_ref().clone()).unwrap();
            let saved = &reopened.session.get(reopened.views[0].id).unwrap().info.annotations;
            assert_eq!(saved.len(), 1);
            assert_eq!(saved[0].contents.as_deref(), Some("A tablet note"));

            h.state_mut().set_option("quick", tool).unwrap();
            h.run_steps(2);
            let p = page_point(&h);
            tap(&mut h, p);
            let cancel = target(&h, "Cancel");
            tap(&mut h, cancel.center());
            assert!(h.state().views[0].comments.composer.is_none());
            assert_eq!(h.state().session.get(id).unwrap().info.annotations.len(), 1, "cancel adds nothing");
        }
    }
}

#[test]
fn touch_palette_and_no_fill_have_separate_finger_sized_targets() {
    for size in [vec2(1024.0, 768.0), vec2(768.0, 1024.0)] {
        let mut h = harness("square", true, size);
        let color = target(&h, "Colour");
        tap(&mut h, color.center());
        let mut previous = Vec::<Rect>::new();
        for (name, _) in pdfcraft_ui_egui::comments::SWATCHES {
            let r = target(&h, name);
            assert!(previous.iter().all(|old| !old.intersects(r)), "{name} overlaps another colour");
            previous.push(r);
        }
        shot(&mut h, &format!("palette-{}x{}", size.x, size.y));
        let red = target(&h, "Red");
        tap(&mut h, red.center());
        assert_eq!(h.state().comment_prefs.style(CommentTool::Rectangle).color, [0.89, 0.13, 0.13]);
        let fill = target(&h, "Fill colour: none");
        tap(&mut h, fill.center());
        let blue = target(&h, "Blue");
        tap(&mut h, blue.center());
        assert_eq!(h.state().comment_prefs.style(CommentTool::Rectangle).fill, Some([0.0, 0.47, 0.84]));
        let fill = target(&h, "Fill colour");
        tap(&mut h, fill.center());
        let none = target(&h, "No fill");
        tap(&mut h, none.center());
        assert_eq!(h.state().comment_prefs.style(CommentTool::Rectangle).fill, None);
        assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty, "controls do not mark the PDF");
    }
}

#[test]
fn mouse_only_composer_keeps_compact_controls() {
    let mut h = harness("note", false, vec2(1024.0, 768.0));
    // Touch mode Off remains an explicit size choice, including on a touch-capable device.
    let p = page_point(&h);
    tap(&mut h, p);
    assert!(h.get_by_label("Post").rect().height() < 44.0);
    assert!(h.get_by_label("Cancel").rect().height() < 44.0);
}
