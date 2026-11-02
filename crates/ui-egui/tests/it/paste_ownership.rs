//! Context paste coordinates, insertion identity and visible image ownership.
use egui::{Key, Modifiers, PointerButton, Pos2};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use pdfcraft_engine::{AddedContent, AddedImage, Edit};
use pdfcraft_ui_egui::{Clip, PdfCraftApp};

fn fixture(rotation: u16, cropped: bool, shared: bool) -> Vec<u8> {
    fixture_at(rotation, cropped, shared, false)
}
fn fixture_at(rotation: u16, cropped: bool, shared: bool, collision: bool) -> Vec<u8> {
    let stream = |body: &str, dict: &str| format!("<< {dict} /Length {} >>\nstream\n{body}\nendstream", body.len());
    let (media, crop, x, y) = if cropped { ("0 0 600 800", "150 250 450 550", 170, 270) } else { ("0 0 300 300", "0 0 300 300", 20, 20) };
    let (x, y, w, height) = if collision { (60, 80, 40, 20) } else { (x, y, 30, 30) };
    let extra = if shared { format!(" q 30 0 0 30 {} {y} cm /RedAgain Do Q", x + 50) } else { String::new() };
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [{media}] /CropBox [{crop}] /Rotate {rotation} /Resources << /XObject << /Red 5 0 R /RedAgain 5 0 R >> >> /Contents 4 0 R >>"
        ),
        stream(&format!("q {w} 0 0 {height} {x} {y} cm /Red Do Q{extra}"), ""),
        stream("FF0000>", "/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /ASCIIHexDecode"),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    out
}

fn harness(bytes: Vec<u8>) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1000.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.system_clipboard = false;
        app.open_bytes("ownership.pdf", None, bytes.clone()).unwrap();
        app.clipboard_override = Some(Clip::Image { width: 40, height: 20, rgba: [0, 0, 255, 255].repeat(800) });
        app
    });
    h.run_steps(4);
    h
}

fn point(h: &Harness<'static, PdfCraftApp>, user: [f64; 2]) -> Pos2 {
    let app = h.state();
    let info = &app.session.get(app.views[0].id).unwrap().info.pages[0];
    let v = info.user_to_view(user[0] as f32, user[1] as f32);
    app.views[0].page_xform(0).unwrap().norm_to_screen(v[0] / info.width, v[1] / info.height)
}

fn right_click(h: &mut Harness<'static, PdfCraftApp>, user: [f64; 2]) {
    let p = point(h, user);
    h.hover_at(p);
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: p, button: PointerButton::Secondary, pressed, modifiers: Modifiers::NONE });
    }
    h.run_steps(2);
}

fn reopened(app: &mut PdfCraftApp) -> PdfCraftApp {
    let id = app.views[0].id;
    let bytes = app.session.save_bytes(id).unwrap();
    let mut saved = PdfCraftApp::new();
    saved.open_bytes("saved.pdf", None, bytes.as_ref().clone()).unwrap();
    saved
}

fn check_placement(rotation: u16, cropped: bool, view_rotation: u16, shared: bool, existing_added: bool, collision: bool) {
    let mut h = harness(fixture_at(rotation, cropped, shared, collision));
    if existing_added {
        let mut png = Vec::new();
        image::RgbImage::from_pixel(40, 40, image::Rgb([0, 0, 255])).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        h.state_mut().add_image("existing.png".into(), png);
        assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().added().len(), 1);
        h.state_mut().quick_tool = pdfcraft_ui_egui::QuickTool::Select;
        h.state_mut().views[0].image_selection = None;
        h.state_mut().views[0].content.selected = None;
    }
    h.state_mut().views[0].rotation = view_rotation;
    h.run_steps(3);
    let id = h.state().views[0].id;
    let originals: Vec<_> = h.state().session.get(id).unwrap().page_images(0).iter().map(|image| image.object.unwrap()).collect();
    let at = if cropped { [250.0, 400.0] } else { [210.0, 80.0] };
    right_click(&mut h, at);
    h.get_by_label_contains("Paste").click();
    h.run_steps(3);
    let doc = h.state().session.get(id).unwrap();
    let inserted = doc
        .added()
        .iter()
        .find_map(|item| match &item.content {
            AddedContent::Image(image) if !originals.contains(&image.image) => Some(image.image),
            _ => None,
        })
        .expect("a new image XObject was inserted");
    assert!(!originals.contains(&inserted), "embedding must allocate a fresh image identity");
    let selected = &doc.page_images(0)[h.state().views[0].image_selection.as_ref().unwrap().index];
    assert_eq!(selected.object, Some(inserted), "the visible selection must name the inserted XObject");
    let center = [(selected.rect[0] + selected.rect[2]) * 0.5, (selected.rect[1] + selected.rect[3]) * 0.5];
    assert!((center[0] - at[0]).abs() < 0.001 && (center[1] - at[1]).abs() < 0.001, "rotation={rotation}, crop={cropped}: {center:?} vs {at:?}");
    let saved = reopened(h.state_mut());
    let placed = saved.session.get(saved.views[0].id).unwrap().page_images(0).into_iter().find(|image| image.object == Some(inserted)).unwrap();
    assert!(((placed.rect[0] + placed.rect[2]) * 0.5 - at[0]).abs() < 0.001);
    assert!(((placed.rect[1] + placed.rect[3]) * 0.5 - at[1]).abs() < 0.001);
    h.key_press(Key::Delete);
    h.run_steps(3);
    let saved = reopened(h.state_mut());
    let doc = saved.session.get(saved.views[0].id).unwrap();
    assert_eq!(doc.added().len(), usize::from(existing_added));
    let remaining = doc.page_images(0);
    assert_eq!(remaining.len(), originals.len());
    assert!(remaining.iter().any(|image| image.name == "Red"));
    if shared {
        assert!(remaining.iter().any(|image| image.name == "RedAgain"));
    }
}

#[test]
fn context_paste_ordinary_page_control() {
    check_placement(0, false, 0, false, false, false);
}
#[test]
fn context_paste_rotate90_places_and_selects_inserted_image() {
    check_placement(90, false, 0, false, false, false);
}
#[test]
fn context_paste_rotate180_places_and_selects_inserted_image() {
    check_placement(180, false, 0, false, false, false);
}
#[test]
fn context_paste_rotate270_places_and_selects_inserted_image() {
    check_placement(270, false, 0, false, false, false);
}
#[test]
fn context_paste_crop_offset_places_and_selects_inserted_image() {
    check_placement(0, true, 0, false, false, false);
}
#[test]
fn context_paste_rotated_crop_places_and_selects_inserted_image() {
    check_placement(90, true, 0, false, false, false);
}
#[test]
fn context_paste_view_rotation_control() {
    check_placement(0, false, 90, false, false, false);
}
#[test]
fn context_paste_shared_original_xobject_control() {
    check_placement(90, false, 0, true, false, false);
}

#[test]
fn context_paste_with_existing_added_image_control() {
    check_placement(90, false, 0, false, true, false);
}

#[test]
fn context_paste_identity_ignores_original_at_display_rectangle() {
    check_placement(90, false, 0, false, false, true);
}

fn check_delete_owner(original: bool, shared_added: bool) {
    let mut h = harness(fixture(0, false, false));
    let id = h.state().views[0].id;
    if shared_added {
        let object = h.state().session.get(id).unwrap().page_images(0)[0].object.unwrap();
        let (extension, bytes) = h.state().session.get(id).unwrap().page_image_copy_file(0, 0).unwrap();
        assert!(h.state_mut().apply_edit(Edit::PasteContent {
            page: 0,
            content: AddedContent::Image(AddedImage::new([130.0, 130.0, 170.0, 170.0], object)),
            file: Some((format!("shared.{extension}"), std::sync::Arc::new(bytes)))
        }));
        assert_eq!(h.state().session.get(id).unwrap().page_images(0).len(), 2);
        assert!(h.state().session.get(id).unwrap().page_images(0).iter().all(|image| image.object == Some(object)));
        h.state_mut().execute("edit.edit_text");
        h.state_mut().views[0].content.selected = Some((0, 0));
    } else {
        let mut png = Vec::new();
        image::RgbImage::from_pixel(40, 40, image::Rgb([0, 0, 255])).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        h.state_mut().add_image("blue.png".into(), png);
    }
    h.run_steps(3);
    let p = point(&h, [150.0, 150.0]);
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(2);
    // The legacy editor can retain the added-image selection while its frame is hidden.
    h.state_mut().views[0].content.selected = Some((0, 0));
    let target = if original { [35.0, 35.0] } else { [150.0, 150.0] };
    right_click(&mut h, target);
    h.get_by_label("Copy Image").click();
    h.run_steps(2);
    let selection = h.state().views[0].image_selection.as_ref().unwrap();
    let selected = &h.state().session.get(id).unwrap().page_images(0)[selection.index];
    assert_eq!(selected.name == "Red", original, "the frame belongs to the requested occurrence");
    h.key_press(Key::Delete);
    h.run_steps(3);
    let saved = reopened(h.state_mut());
    let doc = saved.session.get(saved.views[0].id).unwrap();
    assert_eq!(doc.page_images(0).len(), 1);
    assert_eq!(doc.page_images(0).iter().any(|image| image.name == "Red"), !original, "Delete must remove the visibly selected occurrence");
    assert_eq!(doc.added().len(), usize::from(original), "the other item must survive save/reopen");
}

#[test]
fn original_image_selection_owns_delete_over_hidden_added_image() {
    check_delete_owner(true, false);
}
#[test]
fn added_image_selection_delete_control() {
    check_delete_owner(false, false);
}
#[test]
fn shared_xobject_original_occurrence_owns_delete() {
    check_delete_owner(true, true);
}

#[test]
fn whole_page_actions_do_not_require_a_selection() {
    let mut h = harness(fixture(0, false, false));
    right_click(&mut h, [210.0, 80.0]);
    assert!(h.get_by_label_contains("Copy").accesskit_node().is_disabled());
    assert!(!h.get_by_label("Take a snapshot").accesskit_node().is_disabled());
    assert!(!h.get_by_label("Export this PDF").accesskit_node().is_disabled());
}

#[test]
fn whole_page_actions_still_require_copy_permission() {
    let mut app = PdfCraftApp::new();
    app.open_bytes("source.pdf", None, fixture(0, false, false)).unwrap();
    assert!(app.apply_edit(Edit::Protect(pdfcraft_engine::Protection { permissions_password: Some("owner".into()), ..Default::default() })));
    let bytes = app.session.save_bytes(app.views[0].id).unwrap();
    let mut h = harness(bytes.as_ref().clone());
    right_click(&mut h, [210.0, 80.0]);
    assert!(h.get_by_label("Take a snapshot").accesskit_node().is_disabled());
    assert!(h.get_by_label("Export this PDF").accesskit_node().is_disabled());
}

#[test]
fn legacy_text_delete_survives_switching_out_of_the_image_tool() {
    let mut h = harness(fixture(0, false, false));
    h.state_mut().execute("edit.edit_text");
    right_click(&mut h, [35.0, 35.0]);
    h.get_by_label("Copy Image").click();
    h.run_steps(2);
    assert!(h.state().views[0].image_selection.is_some());
    assert!(h.state_mut().apply_edit(Edit::AddText {
        page: 0,
        text: pdfcraft_engine::AddedText { text: "note".into(), rect: [100.0, 100.0, 180.0, 130.0], ..Default::default() }
    }));
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().added().len(), 1);
    h.state_mut().views[0].content.selected = Some((0, 0));
    h.state_mut().quick_tool = pdfcraft_ui_egui::QuickTool::Select;
    h.key_press(Key::Delete);
    h.run_steps(3);
    let saved = reopened(h.state_mut());
    let doc = saved.session.get(saved.views[0].id).unwrap();
    assert!(doc.added().is_empty());
    assert_eq!(doc.page_images(0).len(), 1);
}

#[test]
fn leaving_image_controller_cannot_leave_hidden_image_owning_delete() {
    let mut h = harness(fixture(0, false, false));
    let mut png = Vec::new();
    image::RgbImage::from_pixel(40, 40, image::Rgb([0, 0, 255])).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    h.state_mut().add_image("blue.png".into(), png);
    h.run_steps(3);
    // Start from no selection, then let the controller select the added image itself.
    h.state_mut().views[0].image_selection = None;
    h.state_mut().views[0].content.selected = None;
    let p = point(&h, [150.0, 150.0]);
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(2);
    assert!(h.state().views[0].image_selection.is_some());
    h.key_press(Key::V);
    h.run_steps(2);
    assert_eq!(h.state().quick_tool, pdfcraft_ui_egui::QuickTool::Select);
    assert!(h.state().views[0].image_selection.is_none());
    h.key_press(Key::Delete);
    h.run_steps(3);
    let saved = reopened(h.state_mut());
    let doc = saved.session.get(saved.views[0].id).unwrap();
    assert_eq!(doc.page_images(0).len(), 2, "there is no visible image selection to delete");
    assert_eq!(doc.added().len(), 1);
}
