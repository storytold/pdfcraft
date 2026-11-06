//! Dialog cards keep their padding, and text fields are tall enough to use.

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

fn fixture() -> Vec<u8> {
    fixture_with_info(None)
}

/// A one-page file, with `info` as its document information dictionary when given.
fn fixture_with_info(info: Option<&str>) -> Vec<u8> {
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] >>",
    ];
    objs.extend(info);
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    let info = if info.is_some() { " /Info 4 0 R" } else { "" };
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R{info} >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// Return the rectangle recorded for a dialog card, failing the test when the card is missing.
fn card(h: &Harness<'_, PdfCraftApp>, id: &str) -> egui::Rect {
    h.ctx.data(|d| d.get_temp(egui::Id::unique(id).with("card"))).unwrap_or_else(|| panic!("missing dialog card {id}"))
}

fn protected() -> Vec<u8> {
    let mut doc = pdfcraft_cos::Document::open(std::sync::Arc::new(fixture())).unwrap();
    doc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes256,
        user_password: "pw",
        owner_password: "owner",
        permissions: -1,
        encrypt_metadata: true,
        seed: [4; 32],
    })
    .unwrap();
    pdfcraft_cos::write_full(&doc, &Default::default()).unwrap()
}

#[test]
fn document_properties_fields_are_tall_and_inset() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("doc.pdf", None, fixture()).unwrap();
        app.set_option("dialog", "properties").unwrap();
        app
    });
    h.run_steps(4);
    let card = card(&h, "dialog");
    let heading = h.get_by_label("Document Properties").rect();
    assert!(heading.left() - card.left() >= 16.0, "heading inset {heading:?} in {card:?}");
    assert!(heading.top() - card.top() >= 16.0, "heading inset {heading:?} in {card:?}");
    let field = h.get_by_role_and_label(Role::TextInput, "Title").rect();
    assert!(field.height() >= 30.0, "title field {field:?}");
    assert!(field.left() >= card.left() + 16.0, "title field {field:?} in {card:?}");
    assert!(field.right() <= card.right() - 16.0, "title field {field:?} in {card:?}");
}

#[test]
fn password_prompt_field_is_tall_and_inset() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("secret.pdf", None, protected()).unwrap();
        app
    });
    h.run_steps(4);
    let card = card(&h, "password");
    let field = h.get_by_role(Role::PasswordInput).rect();
    assert!(field.height() >= 30.0, "password field {field:?}");
    assert!(field.left() - card.left() >= 16.0, "password field {field:?} in {card:?}");
    assert!(card.right() - field.right() >= 16.0, "password field {field:?} in {card:?}");
    let heading = h.get_by_label("Password required").rect();
    assert!(heading.top() - card.top() >= 16.0, "heading inset {heading:?} in {card:?}");
}

#[test]
fn document_properties_long_filename_does_not_overflow_card() {
    let long_name = "Global_sociology_introducing_five_contemporary_societies_Schneider_Linda_Silverman_Arnold_2000_Boston_isbn13_9780072294309_hash_3d2960a9212a8573bba2536e91f36e59_archive_copy.pdf";
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes(long_name, None, fixture()).unwrap();
        app.set_option("dialog", "properties").unwrap();
        app
    });
    h.run_steps(4);
    let card = card(&h, "dialog");
    assert!(card.width() <= 700.0, "dialog card should not blow out horizontally on long filename, width was: {}", card.width());
}

#[test]
fn document_properties_values_line_up_and_long_fields_wrap() {
    let info = "<< /Title (Showcase) /Subject (A specimen document exercising typography, world scripts, vector art, forms and annotations) /Creator (demo-pdf) >>";
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("doc.pdf", None, fixture_with_info(Some(info))).unwrap();
        app.set_option("dialog", "properties").unwrap();
        app
    });
    h.run_steps(4);
    let card = card(&h, "dialog");
    let title = h.get_by_role_and_label(Role::TextInput, "Title").rect();
    // The fields reach the card's padding instead of stopping short of it.
    assert!(card.right() - title.right() <= 24.0, "title field {title:?} in {card:?}");
    // Plain values start where the fields' text does.
    for value in ["doc.pdf", "demo-pdf"] {
        // The file name is also the document tab's label.
        let v = h.query_all_by_label(value).map(|n| n.rect()).find(|r| card.contains_rect(*r)).unwrap_or_else(|| panic!("{value} in the dialog"));
        assert!((v.left() - (title.left() + 10.0)).abs() <= 1.0, "{value} at {v:?}, title field {title:?}");
    }
    // A long subject wraps onto a second line rather than running past the field's edge.
    let subject = h.get_by_role_and_label(Role::MultilineTextInput, "Subject").rect();
    assert!(subject.height() > title.height() + 8.0, "subject field {subject:?}, title field {title:?}");
}

#[test]
fn space_audit_tab_measures_the_file_and_stays_in_the_card() {
    use pdfcraft_ui_egui::{Dialog, PropsTab};
    let open = |dialog: Dialog| {
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.open_bytes("doc.pdf", None, fixture()).unwrap();
            app.dialog = Some(dialog);
            app
        });
        h.run_steps(4);
        h
    };
    let fonts = card(&open(Dialog::Properties(PropsTab::Fonts)), "dialog");
    // The tab is reachable by name, like the other Properties tabs.
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("doc.pdf", None, fixture()).unwrap();
        app.set_option("dialog", "space-audit").unwrap();
        app
    });
    h.run_steps(4);
    assert_eq!(h.state().dialog, Some(Dialog::Properties(PropsTab::SpaceAudit)));
    assert!(!h.state().space_audit.is_empty(), "the tab measures the file when it opens");
    h.get_by_label("Total");
    // Switching tabs keeps the dialog the same size, and the table stays inside its padding,
    // in the tab and in the Optimizer's own Audit Space Usage dialog.
    let tab = card(&h, "dialog");
    assert!((tab.width() - fonts.width()).abs() < 0.5, "space audit card {tab:?}, fonts card {fonts:?}");
    for h in [h, open(Dialog::AuditSpace(true))] {
        let card = card(&h, "dialog");
        let pct = h.get_by_label("Percentage").rect();
        assert!(pct.right() <= card.right() - 16.0, "Percentage {pct:?} in {card:?}");
    }
}

#[test]
fn space_audit_tab_measures_again_after_an_edit() {
    use pdfcraft_engine::optimize::SpaceCategory;
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("doc.pdf", None, fixture()).unwrap();
        app.set_option("dialog", "space-audit").unwrap();
        app
    });
    h.run_steps(4);
    let bookmarks =
        |h: &Harness<'_, PdfCraftApp>| h.state().space_audit.iter().filter(|r| r.category == SpaceCategory::Bookmarks).map(|r| r.bytes).sum::<u64>();
    let before = h.state().space_audit.clone();
    assert_eq!(bookmarks(&h), 0, "the fixture has no outline: {before:?}");
    // Further frames with the file unchanged keep the same measurement.
    h.run_steps(2);
    assert_eq!(h.state().space_audit, before);
    // An edit while the tab is open is measured on the next frame.
    h.state_mut().apply_edit(pdfcraft_engine::Edit::AddBookmark {
        parent: vec![],
        index: 0,
        title: "Chapter".into(),
        page: 0,
        left: None,
        top: None,
    });
    h.run_steps(2);
    assert!(bookmarks(&h) > 0, "the new outline is counted: {:?}", h.state().space_audit);
    // The table shows the new row (the rail's Bookmarks button has the same label).
    let card = card(&h, "dialog");
    assert!(h.query_all_by_label("Bookmarks").any(|n| card.contains_rect(n.rect())), "a Bookmarks row in {card:?}");
}

/// Check that a dialog's Close button follows the content when switching from a taller tab to a shorter one.
#[test]
fn dialog_buttons_follow_the_content_after_a_taller_tab() {
    use pdfcraft_ui_egui::{Dialog, PropsTab};
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("doc.pdf", None, fixture()).unwrap();
        app.dialog = Some(Dialog::Properties(PropsTab::InitialView));
        app
    });
    h.run_steps(4);
    for name in ["Navigation tab", "Page layout", "Magnification"] {
        h.get_by_role_and_label(Role::ComboBox, name);
    }
    h.get_by_role_and_label(Role::SpinButton, "Open to page");
    h.state_mut().dialog = Some(Dialog::Properties(PropsTab::Description));
    h.run_steps(4);
    // The card shrinks back to the shorter tab: Close sits under the last row, not halfway
    // down the room the taller tab left behind.
    let last = h.get_by_label("PDF producer").rect();
    let close = h.get_by_role_and_label(Role::Button, "Close").rect();
    assert!(close.top() - last.bottom() <= 40.0, "Close {close:?} under the last row {last:?}");
    let card = card(&h, "dialog");
    assert!(card.bottom() - close.bottom() <= 24.0, "Close {close:?} at the foot of {card:?}");
}

#[test]
fn page_hover_cursors_stay_off_while_a_dialog_is_open() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("form.pdf", None, include_bytes!("../data/form.pdf").to_vec()).unwrap();
        // Small enough that the check box sits above the dialog card.
        app.set_option("zoom", "100").unwrap();
        app
    });
    for _ in 0..30 {
        h.run_steps(2);
    }
    // The middle of the "agree" check box (/Rect [20 310 36 326] on a 300 × 400 page).
    let page = h.state().views[0].page_screen_rect(0).expect("on screen");
    let k = page.width() / 300.0;
    let check = page.min + egui::vec2(28.0 * k, (400.0 - 318.0) * k);
    h.hover_at(check);
    h.run_steps(2);
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::PointingHand, "the check box takes clicks");
    h.state_mut().set_option("dialog", "security").unwrap();
    h.run_steps(4);
    let card = card(&h, "dialog");
    assert!(!card.contains(check), "the check box {check:?} is beside the dialog {card:?}, under its backdrop");
    h.hover_at(check + egui::vec2(1.0, 0.0));
    h.run_steps(2);
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::Default, "nothing under the dialog's backdrop can be clicked");
}
