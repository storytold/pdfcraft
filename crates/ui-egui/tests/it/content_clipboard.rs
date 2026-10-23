//! Edit ▸ Cut, Copy and Paste of added text boxes and images in the real shell (#2636): the
//! keys and the Edit menu, in one document and between two.

use egui::Pos2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::{ADDED_PASTE_STEP, AddedContent, AddedImage, AddedText, Edit, FontFamily, TextAlign};
use pdfcraft_ui_egui::{Clip, PdfCraftApp};

const FORM: &[u8] = include_bytes!("../data/form.pdf");

fn harness_bytes(bytes: Vec<u8>) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(0.25).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("form.pdf", None, bytes.clone()).unwrap();
        app.set_option("zoom", "150").unwrap();
        // Tests never touch the clipboard of the desktop running them.
        app.system_clipboard = false;
        app
    });
    settle(&mut h);
    h
}

fn harness() -> Harness<'static, PdfCraftApp> {
    harness_bytes(FORM.to_vec())
}

fn settle(h: &mut Harness<'static, PdfCraftApp>) {
    for _ in 0..60 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// The 300 × 400 pt page's point (x, y from the top-left) on screen.
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

/// A menu item with a shortcut: its accessible label is its name, then the shortcut ("Copy Ctrl+C").
fn item<'a>(h: &'a Harness<'static, PdfCraftApp>, name: &'a str) -> egui_kittest::Node<'a> {
    h.get(egui_kittest::kittest::by().predicate(move |n| n.label().is_some_and(|l| l.strip_prefix(name).is_some_and(|r| r.starts_with(' ')))))
}

/// ⌘C / ⌘X arrive as clipboard events (egui-winit turns the keys into them).
fn key(h: &mut Harness<'static, PdfCraftApp>, e: egui::Event) {
    h.event(e);
    h.run_steps(3);
}

/// ⌘V: egui-winit swallows the press, so only the release of V arrives.
fn paste(h: &mut Harness<'static, PdfCraftApp>) {
    h.event(egui::Event::Key { key: egui::Key::V, physical_key: None, pressed: false, repeat: false, modifiers: egui::Modifiers::COMMAND });
    h.run_steps(3);
}

fn added_in(h: &Harness<'static, PdfCraftApp>, view: usize) -> Vec<pdfcraft_engine::Added> {
    let s = h.state();
    s.session.get(s.views[view].id).unwrap().added().to_vec()
}

fn added(h: &Harness<'static, PdfCraftApp>) -> Vec<pdfcraft_engine::Added> {
    added_in(h, 0)
}

fn undo_label(h: &Harness<'static, PdfCraftApp>) -> Option<String> {
    let s = h.state();
    let i = s.active.unwrap();
    s.session.get(s.views[i].id).unwrap().can_undo().map(str::to_string)
}

/// `r` moved `steps` paste steps down and to the right.
fn stepped(r: [f64; 4], steps: f64) -> [f64; 4] {
    let d = ADDED_PASTE_STEP * steps;
    [r[0] + d, r[1] - d, r[2] + d, r[3] - d]
}

fn styled() -> AddedText {
    AddedText {
        rect: [40.0, 195.0, 160.0, 209.4],
        text: "Styled note".into(),
        family: FontFamily::Times,
        bold: true,
        italic: true,
        size: 12.0,
        color: [0.8, 0.1, 0.2],
        align: TextAlign::Center,
    }
}

fn png(width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, width, height);
    enc.set_color(png::ColorType::Rgb);
    let px: Vec<u8> = (0..width * height).flat_map(|i| [rgb[0], rgb[1].wrapping_add(i as u8), rgb[2]]).collect();
    enc.write_header().unwrap().write_image_data(&px).unwrap();
    out
}

/// Open `bytes` as a second tab, which becomes the active one.
fn open_second(h: &mut Harness<'static, PdfCraftApp>, bytes: Vec<u8>) {
    h.state_mut().open_bytes("second.pdf", None, bytes).unwrap();
    settle(h);
    assert_eq!(h.state().active, Some(1));
}

#[test]
fn a_text_box_is_copied_cut_and_pasted_with_its_style() {
    let mut h = harness();
    assert!(h.state_mut().apply_edit(Edit::AddText { page: 0, text: styled() }));
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(3);
    let original = added(&h)[0].content.clone();
    // The box selected (after moving it, or ✓): ⌘C keeps it, ⌘V pastes a copy beside it.
    h.state_mut().views[0].content.selected = Some((0, 0));
    key(&mut h, egui::Event::Copy);
    assert!(h.state().content_clipboard.is_some(), "⌘C kept the box");
    assert_eq!(added(&h).len(), 1, "copying changes nothing");
    paste(&mut h);
    let a = added(&h);
    assert_eq!(a.len(), 2);
    assert_eq!(a[1].content, original.with_rect(stepped(original.rect(), 1.0)), "font, size, colour, alignment and box");
    assert_eq!(undo_label(&h).as_deref(), Some("Paste"));
    h.run_steps(2);
    assert_eq!(h.state().views[0].content.selected, Some((0, 1)), "the copy is selected");
    // Each further paste steps on, so copies don't pile up out of sight.
    paste(&mut h);
    assert_eq!(added(&h)[2].content.rect(), stepped(original.rect(), 2.0));
    h.run_steps(2);
    assert_eq!(h.state().views[0].content.selected, Some((0, 2)));
    // ⌘X takes the selected copy off the page (one undo step)…
    key(&mut h, egui::Event::Cut);
    assert_eq!(added(&h).len(), 2);
    assert_eq!(undo_label(&h).as_deref(), Some("Delete content"));
    assert!(h.state().views[0].content.selected.is_none());
    // …and ⌘V puts it back where it was.
    paste(&mut h);
    let a = added(&h);
    assert_eq!(a.len(), 3);
    assert_eq!(a[2].content, original.with_rect(stepped(original.rect(), 2.0)));
    // Undo takes the paste back.
    h.state_mut().undo();
    h.run_steps(2);
    assert_eq!(added(&h).len(), 2);
    // Into another document: the same box, in the same place on its page.
    open_second(&mut h, FORM.to_vec());
    paste(&mut h);
    let theirs = added_in(&h, 1);
    assert_eq!(theirs.len(), 1);
    assert_eq!(theirs[0].content, original.with_rect(stepped(original.rect(), 2.0)));
    assert_eq!(added(&h).len(), 2, "the first document is unchanged");
    // It survives a save.
    let bytes = h.state().session.save_bytes(h.state().views[1].id).unwrap();
    let mut reopened = pdfcraft_engine::Session::new();
    let id = reopened.open("again.pdf", None, bytes, None).unwrap();
    assert_eq!(reopened.get(id).unwrap().added()[0].content, theirs[0].content);
}

#[test]
fn an_image_is_pasted_with_its_picture_turns_flips_and_crop() {
    let mut h = harness();
    h.state_mut().add_image("logo.png".into(), png(30, 20, [200, 40, 90]));
    h.run_steps(3);
    let AddedContent::Image(mut img) = added(&h)[0].content.clone() else { panic!("an image was added") };
    (img.rotation, img.flip_v, img.crop) = (3, true, [0.0, 0.1, 0.0, 0.2]);
    assert!(h.state_mut().apply_edit(Edit::UpdateContent { page: 0, index: 0, content: AddedContent::Image(img.clone()) }));
    h.run_steps(2);
    assert_eq!(h.state().views[0].content.selected, Some((0, 0)), "the added image is selected");
    key(&mut h, egui::Event::Copy);
    paste(&mut h);
    let a = added(&h);
    assert_eq!(a.len(), 2);
    let AddedContent::Image(copy) = &a[1].content else { panic!("an image was pasted") };
    assert_eq!(*copy, AddedImage { rect: stepped(img.rect, 1.0), ..img.clone() }, "the same picture object, turn, flip and crop");
    // Another document gets the picture as its own image, with the same box and transforms.
    let picture = {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().added_image_file(0, 0).unwrap().1
    };
    open_second(&mut h, FORM.to_vec());
    paste(&mut h);
    let theirs = added_in(&h, 1);
    let AddedContent::Image(their) = &theirs[0].content else { panic!("an image was pasted") };
    assert_eq!(AddedImage { image: img.image, ..their.clone() }, AddedImage { rect: img.rect, ..img.clone() });
    {
        let s = h.state();
        assert_eq!(s.session.get(s.views[1].id).unwrap().added_image_file(0, 0).unwrap().1, picture);
    }
    // The copy drawing the original's image object stays whole when the original goes, and
    // through a save.
    h.state_mut().active = Some(0);
    h.run_steps(2);
    assert!(h.state_mut().apply_edit(Edit::DeleteContent { page: 0, index: 0 }));
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!(doc.added().len(), 1);
    assert_eq!(doc.added_image_file(0, 0).unwrap().1, picture, "the copy keeps its picture");
    let mut reopened = pdfcraft_engine::Session::new();
    let id = reopened.open("again.pdf", None, s.session.save_bytes(s.views[0].id).unwrap(), None).unwrap();
    let again = reopened.get(id).unwrap();
    assert!(matches!(&again.added()[0].content, AddedContent::Image(i) if i.rotation == 3 && i.flip_v));
    assert_eq!(again.added_image_file(0, 0).unwrap().1, picture);
}

/// Page text selected after an added item is what ⌘C copies (and ⌘X leaves the item alone): the
/// newest selection wins, and selecting an item drops a text selection.
#[test]
fn copy_takes_the_newest_selection() {
    let mut h = harness();
    assert!(h.state_mut().apply_edit(Edit::AddText { page: 0, text: styled() }));
    // The Edit panel with the Select tool: a click selects a box (Edit text & images types in it).
    (h.state_mut().left, h.state_mut().left_open) = (pdfcraft_ui_egui::LeftPanel::Tool("edit"), true);
    h.run_steps(3);
    h.state_mut().views[0].content.selected = Some((0, 0));
    // The page's text (the box is page content too) is read in the background.
    let mut selected = false;
    for _ in 0..200 {
        h.run_steps(2);
        if h.state_mut().views[0].select_all() {
            selected = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(selected, "the page has text to select");
    h.run_steps(2);
    key(&mut h, egui::Event::Copy);
    assert!(h.state().content_clipboard.is_none(), "the page text was copied, not the box");
    key(&mut h, egui::Event::Cut);
    assert_eq!(added(&h).len(), 1, "the box stays");
    // Page text selected first, then a click on the box: the box is selected, and the text
    // selection goes.
    h.state_mut().views[0].content.selected = None;
    assert!(h.state_mut().views[0].select_all());
    let on_box = at(&h, 60.0, 198.0);
    click(&mut h, on_box);
    assert_eq!(h.state().views[0].content.selected, Some((0, 0)));
    assert!(h.state().views[0].selected_text().is_none_or(|t| t.is_empty()));
    key(&mut h, egui::Event::Copy);
    assert!(h.state().content_clipboard.is_some(), "now the box is the newest selection");
}

/// Something copied in another app since wins: a picture is pasted instead, and text leaves ⌘V
/// alone. The copy pastes while the system clipboard still holds what it put there.
#[test]
fn a_newer_system_clipboard_wins_over_the_copied_item() {
    let mut h = harness();
    assert!(h.state_mut().apply_edit(Edit::AddText { page: 0, text: styled() }));
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(3);
    h.state_mut().views[0].content.selected = Some((0, 0));
    key(&mut h, egui::Event::Copy);
    // Text copied elsewhere since: nothing is pasted.
    h.state_mut().clipboard_override = Some(Clip::Text("Copied in a browser".into()));
    paste(&mut h);
    assert_eq!(added(&h).len(), 1);
    // The box's own text (as the system clipboard hands it back): the box.
    h.state_mut().clipboard_override = Some(Clip::Text("Styled note\r\n".into()));
    paste(&mut h);
    let a = added(&h);
    assert_eq!(a.len(), 2);
    assert!(matches!(&a[1].content, AddedContent::Text(t) if t.bold && t.family == FontFamily::Times));
    // A picture copied since: the picture.
    h.state_mut().clipboard_override = Some(Clip::Image { width: 30, height: 20, rgba: [90u8, 90, 90, 255].repeat(30 * 20) });
    paste(&mut h);
    let a = added(&h);
    assert_eq!(a.len(), 3);
    assert!(matches!(a[2].content, AddedContent::Image(_)));
    assert_eq!(undo_label(&h).as_deref(), Some("Add image"));
    // A copied image wins while its pixels are still there (the clipboard held text before).
    h.state_mut().clipboard_override = Some(Clip::Text("older".into()));
    h.run_steps(2);
    h.state_mut().views[0].content.selected = Some((0, 2));
    key(&mut h, egui::Event::Copy);
    let rgba: Vec<u8> = [90u8, 90, 90, 255].repeat(30 * 20);
    h.state_mut().clipboard_override = Some(Clip::Image { width: 30, height: 20, rgba });
    paste(&mut h);
    assert_eq!(undo_label(&h).as_deref(), Some("Paste"), "the copied image itself");
    let a = added(&h);
    assert_eq!(a[3].content.rect(), stepped(a[2].content.rect(), 1.0), "beside the original, not centred");
}

#[test]
fn edit_menu_cut_copy_and_paste_act_on_added_content() {
    let mut h = harness();
    assert!(h.state_mut().apply_edit(Edit::AddText { page: 0, text: styled() }));
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(3);
    // Nothing selected or copied yet: each says what it needs.
    assert!(!h.state_mut().execute("edit.cut"));
    assert_eq!(h.state().toast.as_ref().map(|t| t.0.as_str()), Some("Select a text box or an image first"));
    assert!(!h.state_mut().execute("edit.paste"));
    assert_eq!(h.state().toast.as_ref().map(|t| t.0.as_str()), Some("Copy a text box or an image first"));
    h.state_mut().views[0].content.selected = Some((0, 0));
    assert!(h.state_mut().execute("edit.copy"));
    assert!(h.state_mut().execute("edit.paste"));
    h.run_steps(3);
    assert_eq!(added(&h).len(), 2);
    h.state_mut().views[0].content.selected = Some((0, 1));
    assert!(h.state_mut().execute("edit.cut"));
    h.run_steps(3);
    assert_eq!(added(&h).len(), 1);
    // Menu ▸ Edit lists them with their keys; Paste runs from there.
    h.get_by_label("Menu").click();
    h.run_steps(2);
    h.get_by_label("Edit ⏵").hover();
    h.run_steps(3);
    item(&h, "Cut");
    item(&h, "Copy");
    item(&h, "Paste").click();
    h.run_steps(3);
    assert_eq!(added(&h).len(), 2, "the cut box is pasted back");
}

/// A document that doesn't allow copying keeps its added content in: Copy and Cut are refused, and
/// a refused Cut deletes nothing.
#[test]
fn copying_honours_the_documents_copy_permission() {
    let mut doc = pdfcraft_cos::Document::open(std::sync::Arc::new(FORM.to_vec())).unwrap();
    doc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes256,
        user_password: "",
        owner_password: "owner",
        permissions: -1 ^ 0b1_0000,
        encrypt_metadata: true,
        seed: [4; 32],
    })
    .unwrap();
    let mut h = harness_bytes(pdfcraft_cos::write_full(&doc, &Default::default()).unwrap());
    assert!(h.state_mut().apply_edit(Edit::AddText { page: 0, text: styled() }), "changes are allowed");
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(3);
    h.state_mut().views[0].content.selected = Some((0, 0));
    key(&mut h, egui::Event::Copy);
    assert!(h.state().content_clipboard.is_none());
    key(&mut h, egui::Event::Cut);
    assert_eq!(added(&h).len(), 1, "a refused cut deletes nothing");
    assert_eq!(h.state().toast.as_ref().map(|t| t.0.as_str()), Some("The document's security settings don't allow copying text and images"));
}

/// An added image over a picture that is part of the page: whichever was clicked last is what
/// ⌘C copies. Selecting the item drops the page picture's selection.
#[test]
fn copy_takes_the_newest_of_an_item_and_a_page_picture() {
    let pdf = pdfcraft_engine::Session::new().create_from_images(&[("p.png".into(), png(300, 400, [10, 200, 30]))]).unwrap();
    let mut h = harness_bytes(pdf.to_vec());
    let add = Edit::AddImage {
        page: 0,
        rect: Some([40.0, 300.0, 120.0, 360.0]),
        name: "logo.png".into(),
        bytes: std::sync::Arc::new(png(40, 30, [200, 0, 0])),
    };
    assert!(h.state_mut().apply_edit(add));
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(3);
    let (on_item, on_picture) = (at(&h, 80.0, 70.0), at(&h, 150.0, 330.0));
    click(&mut h, on_item);
    assert_eq!(h.state().views[0].content.selected, Some((0, 0)), "the added image");
    click(&mut h, on_picture);
    assert!(h.state().views[0].image_selection.is_some(), "the page's picture");
    key(&mut h, egui::Event::Copy);
    assert!(h.state().content_clipboard.is_none(), "not the item selected before it");
    click(&mut h, on_item);
    assert_eq!(h.state().views[0].content.selected, Some((0, 0)));
    assert!(h.state().views[0].image_selection.is_none(), "the item again, and only the item");
    key(&mut h, egui::Event::Copy);
    assert!(h.state().content_clipboard.is_some());
}

/// A document that allows copying but not changes: ⌘X is refused before anything is copied, so
/// what was copied before stays on the clipboard, and the item stays on the page.
#[test]
fn cut_honours_the_documents_change_permission() {
    let mut s = pdfcraft_engine::Session::new();
    let id = s.open("form.pdf", None, std::sync::Arc::new(FORM.to_vec()), None).unwrap();
    s.apply(id, Edit::AddText { page: 0, text: styled() }).unwrap();
    let mut doc = pdfcraft_cos::Document::open(s.save_bytes(id).unwrap()).unwrap();
    doc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes256,
        user_password: "",
        owner_password: "owner",
        permissions: -1 ^ 0b1000,
        encrypt_metadata: true,
        seed: [5; 32],
    })
    .unwrap();
    let mut h = harness_bytes(pdfcraft_cos::write_full(&doc, &Default::default()).unwrap());
    // Edit text & images is refused for such a document; the Edit panel still shows its items.
    assert!(!h.state_mut().execute("edit.edit_text"));
    (h.state_mut().left, h.state_mut().left_open) = (pdfcraft_ui_egui::LeftPanel::Tool("edit"), true);
    h.run_steps(3);
    assert_eq!(added(&h).len(), 1);
    h.state_mut().views[0].content.selected = Some((0, 0));
    key(&mut h, egui::Event::Cut);
    assert!(h.state().content_clipboard.is_none(), "nothing copied by a refused cut");
    assert_eq!(added(&h).len(), 1, "and nothing deleted");
    assert_eq!(h.state().toast.as_ref().map(|t| t.0.as_str()), Some("The document's security settings don't allow this change"));
    key(&mut h, egui::Event::Copy);
    assert!(h.state().content_clipboard.is_some(), "copying is allowed");
}

/// Copying something else in PdfCraft (page text here) replaces the copied item, also where there
/// is no system clipboard to tell (the web): ⌘V doesn't paste the older item.
#[test]
fn copying_page_text_replaces_the_copied_item() {
    let mut h = harness();
    assert!(h.state_mut().apply_edit(Edit::AddText { page: 0, text: styled() }));
    (h.state_mut().left, h.state_mut().left_open) = (pdfcraft_ui_egui::LeftPanel::Tool("edit"), true);
    h.run_steps(3);
    h.state_mut().views[0].content.selected = Some((0, 0));
    key(&mut h, egui::Event::Copy);
    assert!(h.state().content_clipboard.is_some());
    let mut selected = false;
    for _ in 0..200 {
        h.run_steps(2);
        if h.state_mut().views[0].select_all() {
            selected = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(selected, "the page has text to select");
    key(&mut h, egui::Event::Copy);
    assert!(h.state().content_clipboard.is_none(), "the page text is the newest copy");
    paste(&mut h);
    assert_eq!(added(&h).len(), 1);
}

/// ⌘V while an item is being dragged pastes nothing: the drag would otherwise end on the pasted
/// copy, which a paste selects, and move it instead of the item dragged.
#[test]
fn a_paste_mid_drag_is_ignored() {
    let mut h = harness();
    assert!(h.state_mut().apply_edit(Edit::AddText { page: 0, text: styled() }));
    (h.state_mut().left, h.state_mut().left_open) = (pdfcraft_ui_egui::LeftPanel::Tool("edit"), true);
    h.run_steps(3);
    h.state_mut().views[0].content.selected = Some((0, 0));
    key(&mut h, egui::Event::Copy);
    let (from, to) = (at(&h, 60.0, 198.0), at(&h, 60.0, 250.0));
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    h.hover_at(to);
    h.run_steps(2);
    paste(&mut h);
    h.drop_at(to);
    h.run_steps(3);
    let a = added(&h);
    assert_eq!(a.len(), 1, "nothing pasted mid-drag");
    assert!(a[0].content.rect()[3] < styled().rect[3] - 30.0, "the dragged box moved: {:?}", a[0].content.rect());
}
