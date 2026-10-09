//! Edit a PDF ▸ Add content in the real shell (egui_kittest): type text onto a page, move it,
//! restyle it from the panel, add an image, delete with the keyboard.

use egui::Pos2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{PdfCraftApp, QuickTool};

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("form.pdf", None, include_bytes!("data/form.pdf").to_vec()).unwrap();
        app.set_option("zoom", "150").unwrap();
        app
    });
    for _ in 0..60 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h
}

fn at(h: &Harness<'static, PdfCraftApp>, x: f32, y: f32) -> Pos2 {
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    let k = r.width() / 300.0;
    r.min + egui::vec2(x * k, y * k)
}

fn click(h: &mut Harness<'static, PdfCraftApp>, p: Pos2) {
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(3);
}

fn added(h: &Harness<'static, PdfCraftApp>) -> Vec<pdfcraft_engine::Added> {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().added.clone()
}

/// A click whose press and release arrive in the same frame, as a quick real mouse click does.
fn quick_click(h: &mut Harness<'static, PdfCraftApp>, p: Pos2) {
    h.hover_at(p);
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
    }
    h.run_steps(3);
}

fn texts(h: &Harness<'static, PdfCraftApp>) -> Vec<String> {
    added(h)
        .into_iter()
        .filter_map(|a| match a.content {
            pdfcraft_engine::AddedContent::Text(t) => Some(t.text),
            _ => None,
        })
        .collect()
}

#[test]
fn clicking_elsewhere_keeps_the_text_and_starts_a_new_box() {
    // #74: a click elsewhere on the page threw away the text being typed and opened a new box.
    let mut h = harness();
    assert!(h.state_mut().execute("edit.text"));
    h.run_steps(2);
    let p = at(&h, 40.0, 100.0);
    click(&mut h, p);
    h.event(egui::Event::Text("First".into()));
    h.run_steps(2);
    // A slow click (press and release in different frames)…
    let p = at(&h, 40.0, 200.0);
    click(&mut h, p);
    assert_eq!(texts(&h), ["First"]);
    assert!(h.state().views[0].content.draft.is_some(), "a new box opened");
    assert_eq!(h.state().quick_tool, QuickTool::AddText, "still adding text");
    h.event(egui::Event::Text("Second".into()));
    h.run_steps(2);
    // …and a quick one (both in one frame, as a real mouse click usually arrives).
    let p = at(&h, 40.0, 300.0);
    quick_click(&mut h, p);
    assert_eq!(texts(&h), ["First", "Second"]);
    // Discard throws away only the box being typed.
    h.event(egui::Event::Text("Third".into()));
    h.run_steps(2);
    h.get_by_label("Discard this text (Esc)").click();
    h.run_steps(3);
    assert_eq!(texts(&h), ["First", "Second"]);
    assert!(h.state().views[0].content.draft.is_none());
}

#[test]
fn typing_moving_styling_and_deleting_added_content() {
    let mut h = harness();
    assert!(h.state_mut().execute("edit.text"));
    h.run_steps(2);
    h.get_by_label("Edit a PDF");
    let p = at(&h, 40.0, 300.0);
    click(&mut h, p);
    assert!(h.state().views[0].content.draft.is_some(), "the editor opened");
    h.event(egui::Event::Text("Reviewed".into()));
    h.run_steps(2);
    // Done keeps the text and goes back to Select, with the new text selected.
    h.get_by_label("Done adding text").click();
    h.run_steps(3);
    let a = added(&h);
    assert_eq!(a.len(), 1);
    let pdfcraft_engine::AddedContent::Text(t) = &a[0].content else { panic!() };
    assert_eq!(t.text, "Reviewed");
    assert_eq!(h.state().quick_tool, QuickTool::Select);
    h.run_steps(2);
    assert_eq!(h.state().views[0].content.selected, Some((0, 0)), "the new text is selected");
    // Bold from the Format panel: one undoable change.
    h.get_by_label("B").click();
    h.run_steps(3);

    let pdfcraft_engine::AddedContent::Text(t) = &added(&h)[0].content else { panic!() };
    assert!(t.bold);
    {
        let s = h.state();
        assert_eq!(s.session.get(s.views[0].id).unwrap().can_undo(), Some("Edit content"));
    }
    // Delete.
    h.key_press(egui::Key::Delete);
    h.run_steps(3);
    assert!(added(&h).is_empty());
    // An image lands in the middle of the page, selected.
    let mut png = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png, 30, 20);
        enc.set_color(png::ColorType::Rgb);
        let mut w = enc.write_header().unwrap();
        w.write_image_data(&[90u8; 1800]).unwrap();
    }
    h.state_mut().add_image("logo.png".into(), png);
    h.run_steps(3);
    let a = added(&h);
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].content.rect(), [135.0, 190.0, 165.0, 210.0]);
    assert_eq!(h.state().views[0].content.selected, Some((0, 0)));
    // Edit image: rotate clockwise turns the box around its centre.
    h.get_by_label("Rotate clockwise").click();
    h.run_steps(3);
    let a = added(&h);
    let pdfcraft_engine::AddedContent::Image(img) = &a[0].content else { panic!() };
    assert_eq!((img.rotation, img.rect), (3, [140.0, 185.0, 160.0, 215.0]));
    h.get_by_label("Flip horizontal").click();
    h.run_steps(3);
    let pdfcraft_engine::AddedContent::Image(img) = &added(&h)[0].content else { panic!() };
    assert!(img.flip_h);
}

/// #125: text the standard fonts can't draw stays in the editor with a warning instead of being
/// written to the page as `?` (or lost).
#[test]
fn text_the_standard_fonts_cant_draw_stays_in_the_editor() {
    let mut h = harness();
    assert!(h.state_mut().execute("edit.text"));
    h.run_steps(2);
    let p = at(&h, 40.0, 300.0);
    click(&mut h, p);
    h.event(egui::Event::Text("Hello 世界".into()));
    h.run_steps(2);
    h.get_by_label_contains("can't draw “世” (U+4E16)");
    if let Ok(dir) = std::env::var("PDFCRAFT_SHOTS") {
        h.render().unwrap().save(format!("{dir}/undrawable-text.png")).unwrap();
    }
    h.get_by_label("Done adding text").click();
    h.run_steps(3);
    assert!(texts(&h).is_empty(), "nothing was written to the page");
    let draft = h.state().views[0].content.draft.as_ref().map(|d| d.text.clone());
    assert_eq!(draft.as_deref(), Some("Hello 世界"), "the typed text is kept");
    // A click elsewhere on the page doesn't finish it, or replace it with a new box.
    let elsewhere = at(&h, 40.0, 150.0);
    click(&mut h, elsewhere);
    let draft = h.state().views[0].content.draft.as_ref().map(|d| d.text.clone());
    assert_eq!(draft.as_deref(), Some("Hello 世界"), "clicking elsewhere keeps the typed text");
    assert!(texts(&h).is_empty());
    // Once the text can be drawn, Done adds it (Done took the focus, so edit the draft directly).
    if let Some(d) = h.state_mut().views[0].content.draft.as_mut() {
        d.text = "Hello world".into();
    }
    h.run_steps(2);
    h.get_by_label("Done adding text").click();
    h.run_steps(3);
    assert_eq!(texts(&h), ["Hello world"]);
}

/// Text kept because it can't be drawn goes with its page when the page is deleted, so nothing
/// is left blocking Save out of sight.
#[test]
fn a_kept_draft_goes_with_its_deleted_page() {
    let mut h = harness();
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::InsertBlankPage { at: 1, width: 612.0, height: 792.0 }));
    h.run_steps(2);
    assert!(h.state_mut().execute("edit.text"));
    h.state_mut().views[0].go_to_page(1);
    h.run_steps(4);
    let p = h.state().views[0].page_screen_rect(1).expect("page 2 on screen").min + egui::vec2(40.0, 40.0);
    click(&mut h, p);
    h.event(egui::Event::Text("日本語".into()));
    h.run_steps(2);
    assert_eq!(h.state().views[0].content.draft.as_ref().map(|d| (d.page, d.text.as_str())), Some((1, "日本語")));
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::DeletePages { pages: vec![1] }));
    h.run_steps(2);
    assert!(h.state().views[0].content.draft.is_none());
}
