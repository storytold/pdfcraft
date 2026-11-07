//! Annotation context selection over page artwork (#6236).
use super::*;

fn highlighted_image_pdf() -> Vec<u8> {
    let draw = stream("q 200 0 0 200 50 150 cm /Im0 Do Q");
    pdf(&[
        CATALOG,
        PAGES,
        "<< /Type /Page /Parent 2 0 R /Resources << /XObject << /Im0 4 0 R >> >> /Contents 5 0 R /Annots [6 0 R] >>",
        "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /ASCIIHexDecode /Length 8 >>\nstream\nFFEECC>\nendstream",
        &draw,
        "<< /Type /Annot /Subtype /Highlight /Rect [100 220 180 240] /QuadPoints [100 240 180 240 100 220 180 220] /C [1 1 0] /F 4 /T (Ana) /Contents (Over artwork) >>",
    ])
}

fn frame(h: &mut Harness<'_, PdfCraftApp>, events: impl IntoIterator<Item = egui::Event>) {
    h.input_mut().events.extend(events);
    let ctx = h.ctx.clone();
    let mut input = std::mem::take(h.input_mut());
    h.state_mut().prepare_raw_input(&ctx, &mut input);
    *h.input_mut() = input;
    h.step();
}

pub(super) fn click(h: &mut Harness<'_, PdfCraftApp>, point: egui::Pos2, button: egui::PointerButton) {
    frame(h, [egui::Event::PointerMoved(point)]);
    for pressed in [true, false] {
        frame(h, [egui::Event::PointerButton { pos: point, button, pressed, modifiers: egui::Modifiers::NONE }]);
    }
    h.run_steps(3);
}

#[test]
fn select_tool_selects_a_highlight_over_an_image() {
    let mut h = open(highlighted_image_pdf(), "150", false);
    h.state_mut().system_clipboard = false;
    let point = at(&h, 140.0, 230.0);
    click(&mut h, point, egui::PointerButton::Primary);
    assert_eq!(h.state().views[0].comments.selected, Some((0, 0)));
    assert!(h.state().views[0].picked_image.is_none(), "the underlying artwork is not selected");
}

enum ImageState {
    Unselected,
    Selected,
    Added,
}

fn edit_context(state: ImageState) {
    let _gpu = std::env::var_os("PDFCRAFT_6236_SHOT").map(|_| crate::gpu());
    let mut h = open(highlighted_image_pdf(), "150", false);
    h.state_mut().system_clipboard = false;
    h.state_mut().quick_tool = QuickTool::EditText;
    h.run_steps(4);
    match state {
        ImageState::Unselected => {}
        ImageState::Selected => {
            let point = at(&h, 75.0, 180.0);
            click(&mut h, point, egui::PointerButton::Primary);
            assert!(h.state().views[0].image_selection.is_some(), "real image selection control");
        }
        ImageState::Added => {
            let mut png = std::io::Cursor::new(Vec::new());
            image::RgbaImage::from_pixel(120, 120, image::Rgba([180, 210, 240, 255])).write_to(&mut png, image::ImageFormat::Png).unwrap();
            h.state_mut().add_image("added.png".into(), png.into_inner());
            h.run_steps(4);
            assert!(h.state().views[0].content.selected.is_some(), "the image's mirrored selection is active");
        }
    }
    let id = h.state().views[0].id;
    let original: Vec<_> = h.state().session.get(id).unwrap().page_images(0).iter().map(|image| image.rect).collect();
    let point = at(&h, 140.0, 230.0);
    click(&mut h, point, egui::PointerButton::Secondary);
    assert_eq!(h.state().views[0].comments.selected, Some((0, 0)), "the annotation wins the context click above the image");
    assert!(h.state().views[0].image_selection.is_none(), "the image controller no longer owns Delete");
    assert!(h.state().views[0].content.selected.is_none(), "no hidden image selection owns Delete");
    assert!(h.query_by_label_contains("Delete Image").is_none(), "the context menu belongs to the highlight");
    if matches!(state, ImageState::Selected)
        && let Some(path) = std::env::var_os("PDFCRAFT_6236_SHOT")
    {
        h.render().unwrap().save(path).unwrap();
    }
    h.get_by_label("Properties…").click();
    frame(&mut h, []);
    h.run_steps(2);
    assert_eq!(h.state().dialog, Some(Dialog::CommentProps));
    assert_eq!(h.state().comment_props.as_ref().unwrap().original.author, "Ana");
    h.get_by_label("General").click();
    frame(&mut h, []);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Author").click();
    frame(&mut h, []);
    frame(
        &mut h,
        [
            egui::Event::ModifiersChanged(egui::Modifiers::COMMAND),
            egui::Event::Key { key: egui::Key::A, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND },
        ],
    );
    frame(
        &mut h,
        [
            egui::Event::Key { key: egui::Key::A, physical_key: None, pressed: false, repeat: false, modifiers: egui::Modifiers::COMMAND },
            egui::Event::ModifiersChanged(egui::Modifiers::NONE),
            egui::Event::Text("Reviewed".into()),
        ],
    );
    h.get_by_label("OK").click();
    frame(&mut h, []);
    h.run_steps(3);
    assert_eq!(h.state().session.get(id).unwrap().comment_props(0, 0).unwrap().author, "Reviewed", "Properties edits the highlight");
    if matches!(state, ImageState::Added) {
        frame(
            &mut h,
            [egui::Event::Key { key: egui::Key::Delete, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }],
        );
    } else {
        let point = at(&h, 140.0, 230.0);
        click(&mut h, point, egui::PointerButton::Secondary);
        h.get_by_label("Delete").click();
        frame(&mut h, []);
    }
    h.run_steps(3);
    let doc = h.state().session.get(id).unwrap();
    assert!(doc.info.annotations.is_empty(), "the highlight was deleted");
    let images = doc.page_images(0);
    assert_eq!(images.iter().map(|image| image.rect).collect::<Vec<_>>(), original, "no image moved or was deleted");
    let saved = h.state_mut().session.save_bytes(id).unwrap();
    let mut reopened = PdfCraftApp::new();
    reopened.open_bytes("saved.pdf", None, saved.as_ref().clone()).unwrap();
    let doc = reopened.session.get(reopened.views[0].id).unwrap();
    assert!(doc.info.annotations.is_empty(), "deletion persists after reopen");
    assert_eq!(doc.page_images(0).iter().map(|image| image.rect).collect::<Vec<_>>(), original);
}

#[test]
fn edit_tool_opens_highlight_properties_over_an_unselected_image() {
    edit_context(ImageState::Unselected);
}

#[test]
fn edit_tool_opens_highlight_properties_over_a_selected_image() {
    edit_context(ImageState::Selected);
}

#[test]
fn highlight_context_releases_an_added_images_keyboard_selection() {
    edit_context(ImageState::Added);
}

#[test]
fn edit_primary_click_under_a_highlight_still_selects_the_picture() {
    let mut h = open(highlighted_image_pdf(), "150", false);
    h.state_mut().system_clipboard = false;
    h.state_mut().quick_tool = QuickTool::EditText;
    h.run_steps(4);
    let point = at(&h, 140.0, 230.0);
    click(&mut h, point, egui::PointerButton::Primary);
    assert_eq!(h.state().views[0].image_selection.as_ref().map(|image| (image.page, image.index)), Some((0, 0)));
    assert_eq!(h.state().views[0].comments.selected, None, "right-click is how Edit reaches the highlight");
    assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty);
}

fn draft_and_image() -> Harness<'static, PdfCraftApp> {
    let mut h = open(highlighted_image_pdf(), "150", false);
    h.state_mut().system_clipboard = false;
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::AddText {
        page: 0,
        text: pdfcraft_engine::AddedText { rect: [40.0, 55.0, 240.0, 75.0], text: "Original".into(), ..Default::default() },
    }));
    assert!(h.state_mut().execute("edit.text"));
    h.run_steps(3);
    let point = at(&h, 45.0, 65.0);
    click(&mut h, point, egui::PointerButton::Primary);
    assert_eq!(h.state().views[0].content.draft.as_ref().unwrap().index, Some(0));
    frame(
        &mut h,
        [
            egui::Event::Key { key: egui::Key::End, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE },
            egui::Event::Key { key: egui::Key::End, physical_key: None, pressed: false, repeat: false, modifiers: egui::Modifiers::NONE },
            egui::Event::Text(" kept".into()),
        ],
    );
    assert_eq!(h.state().views[0].content.draft.as_ref().unwrap().text, "Original kept");
    // The shared leave-Edit caller must preserve a draft's own Text-typed selection.
    assert!(h.state_mut().execute("view.select_tool"));
    frame(&mut h, []);
    assert_eq!(h.state().views[0].content.selected, Some((0, 0)));
    assert_eq!(h.state().views[0].content.draft.as_ref().unwrap().text, "Original kept");
    // Image-picker completion can arrive while that existing text draft remains open.
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(120, 120, image::Rgba([180, 210, 240, 255])).write_to(&mut png, image::ImageFormat::Png).unwrap();
    h.state_mut().add_image("selected.png".into(), png.into_inner());
    h.run_steps(3);
    assert_eq!(h.state().views[0].content.selected, Some((0, 1)));
    assert!(h.state().views[0].image_selection.is_some());
    assert_eq!(h.state().views[0].content.draft.as_ref().unwrap().text, "Original kept");
    h
}

fn text_item(h: &Harness<'_, PdfCraftApp>) -> pdfcraft_engine::AddedText {
    h.state()
        .session
        .get(h.state().views[0].id)
        .unwrap()
        .added()
        .iter()
        .find_map(|item| match &item.content {
            pdfcraft_engine::AddedContent::Text(text) => Some(text.clone()),
            _ => None,
        })
        .unwrap()
}

#[test]
fn comment_context_with_a_draft_cannot_leave_an_image_owning_delete() {
    let mut h = draft_and_image();
    let id = h.state().views[0].id;
    let draft = h.state().views[0].content.draft.as_ref().unwrap().clone();
    let source = text_item(&h);
    let point = at(&h, 140.0, 230.0);
    click(&mut h, point, egui::PointerButton::Secondary);
    assert_eq!(h.state().views[0].comments.selected, Some((0, 0)));
    assert!(h.state().views[0].image_selection.is_none());
    assert_eq!(draft.style, source, "the draft edits the original text item");
    assert!(h.state().views[0].content.draft.is_none(), "the right-click commits the draft by normal focus handling");
    let expected = pdfcraft_engine::AddedText { text: draft.text, rect: draft.rect, ..draft.style };
    assert_eq!(text_item(&h), expected, "the draft and its text formatting were kept");
    h.get_by_label("Properties…").click();
    frame(&mut h, []);
    h.run_steps(3);
    assert!(h.state().views[0].content.draft.is_none());
    assert_eq!(text_item(&h).text, "Original kept");
    h.get_by_label("Cancel").click();
    frame(&mut h, []);
    h.run_steps(3);
    frame(&mut h, [egui::Event::Key { key: egui::Key::Delete, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }]);
    h.run_steps(3);
    let doc = h.state().session.get(id).unwrap();
    assert_eq!(doc.page_images(0).len(), 2, "Delete must not remove the hidden selected image");
    assert_eq!(text_item(&h).text, "Original kept", "the committed draft is retained");
    assert!(doc.info.annotations.is_empty(), "Delete acts on the selected comment");
}

#[test]
fn leaving_edit_with_a_draft_releases_only_an_image_selection() {
    let mut h = draft_and_image();
    let draft = h.state().views[0].content.draft.as_ref().unwrap().clone();
    let source = text_item(&h);
    // This is the control-channel command, so switching tools does not move keyboard focus.
    assert!(h.state_mut().execute("view.select_tool"));
    frame(&mut h, []);
    assert!(h.state().views[0].content.selected.is_none(), "the image-only keyboard selection is released");
    assert!(h.state().views[0].image_selection.is_none());
    let after = h.state().views[0].content.draft.as_ref().unwrap();
    assert_eq!((after.page, after.index, &after.text, &after.style), (draft.page, draft.index, &draft.text, &draft.style));
    assert_eq!(text_item(&h), source);
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().page_images(0).len(), 2);
}
