//! An attachment's row in the Attachments panel answers a double-click and a right-click, as
//! in Acrobat (#3466), while its buttons keep taking their own clicks.

use std::path::Path;
use std::sync::Arc;

use craft::host::{Picked, ScriptedFiles};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

/// A one-page PDF with no embedded files, to attach to.
const INNER: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// A one-page PDF with two embedded files: `inner.pdf` (a PDF) and `notes.txt`.
fn attachments_pdf() -> Vec<u8> {
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /Names << /EmbeddedFiles << /Names [(inner.pdf) 5 0 R (notes.txt) 7 0 R] >> >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> >>".to_vec(),
        [format!("<< /Type /EmbeddedFile /Length {} >>\nstream\n", INNER.len()).as_bytes(), INNER, b"\nendstream"].concat(),
        b"<< /Type /Filespec /F (inner.pdf) /UF (inner.pdf) /EF << /F 4 0 R >> >>".to_vec(),
        b"<< /Type /EmbeddedFile /Length 11 >>\nstream\nhello notes\nendstream".to_vec(),
        b"<< /Type /Filespec /F (notes.txt) /UF (notes.txt) /Desc (Original) /EF << /F 6 0 R >> >>".to_vec(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// The app with [`attachments_pdf`] open and the Attachments panel showing, at 60 frames a
/// second so two clicks a frame apart make a double-click.
fn harness() -> Harness<'static, PdfCraftApp> {
    harness_with_pdf(attachments_pdf())
}

fn harness_with_pdf(pdf: Vec<u8>) -> Harness<'static, PdfCraftApp> {
    harness_answering(pdf, Arc::new(ScriptedFiles::new()))
}

/// [`harness_with_pdf`] whose file picks `files` answers.
fn harness_answering(pdf: Vec<u8>, files: Arc<ScriptedFiles>) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::support::test_app(crate::support::host_with_files(files));
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "doc.pdf", None, pdf).unwrap();
            app.set_option(&egui::Context::default(), "panel", "attachments").unwrap();
            app
        }
    });
    h.run_steps(4);
    h
}

#[test]
fn submitting_blank_removes_an_existing_whitespace_description() {
    let mut pdf = attachments_pdf();
    let desc = pdf.windows(b"(Original)".len()).position(|window| window == b"(Original)").unwrap();
    pdf[desc..desc + b"(Original)".len()].copy_from_slice(b"(        )");
    let mut h = harness_with_pdf(pdf);
    let doc_id = h.state().views[0].id;
    assert!(h.state().session.get(doc_id).unwrap().info.attachments.iter().find(|a| a.name == "notes.txt").unwrap().has_description);
    right_click(&mut h, "notes.txt");
    h.get_by_role(egui::accesskit::Role::Menu).get_by_label("Edit description").click();
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(3);
    let doc = h.state().session.get(doc_id).unwrap();
    let attachment = doc.info.attachments.iter().find(|a| a.name == "notes.txt").unwrap();
    assert!(!attachment.has_description);
    assert!(doc.dirty);
}

#[test]
fn submitting_blank_leaves_an_absent_description_untouched() {
    let mut h = harness();
    right_click(&mut h, "inner.pdf");
    h.get_by_role(egui::accesskit::Role::Menu).get_by_label("Edit description").click();
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(3);
    let doc = h.state().session.get(h.state().views[0].id).unwrap();
    let attachment = doc.info.attachments.iter().find(|a| a.name == "inner.pdf").unwrap();
    assert!(!attachment.has_description);
    assert!(!doc.dirty);
    assert_eq!(doc.can_undo(), None);
}

/// Two pointer clicks a frame apart on `label` (an accessibility click is never a double-click).
fn double_click(h: &mut Harness<'static, PdfCraftApp>, label: &str) {
    let at = row_name(h, label);
    h.hover_at(at);
    h.run_steps(1);
    for _ in 0..2 {
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
        h.run_steps(1);
    }
    h.run_steps(3);
}

fn right_click(h: &mut Harness<'static, PdfCraftApp>, label: &str) {
    let at = row_name(h, label);
    h.hover_at(at);
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: true, modifiers: Default::default() });
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: false, modifiers: Default::default() });
    h.run_steps(2);
}

/// A row event of the Attachments list becomes an attachment action: a double click saves a file that
/// is not a PDF and opens one that is; the row menu offers Open only for the PDF, and its Save, Open and
/// Delete act on the row it was opened on.
#[test]
fn attachment_rows_turn_their_events_into_attachment_actions() {
    let saved = Path::new("out/saved notes.txt");
    let from_menu = Path::new("out/from menu.pdf");
    let files = Arc::new(ScriptedFiles::answering([Picked::Paths(vec![saved.to_path_buf()]), Picked::Paths(vec![from_menu.to_path_buf()])]));
    let mut h = harness_answering(attachments_pdf(), files.clone());
    assert_eq!(h.state().views.len(), 1);
    double_click(&mut h, "notes.txt");
    assert_eq!(files.file(saved).as_deref(), Some(&b"hello notes"[..]), "PdfCraft opens only PDFs, so it saves the rest");
    assert_eq!(h.state().views.len(), 1, "nothing opens");
    double_click(&mut h, "inner.pdf");
    assert_eq!(h.state().views.len(), 2, "the attached PDF opens in a tab of its own");
    // Opening the same attachment again selects its existing tab. Close it and keep a different
    // document open so the menu action must create a third tab.
    h.state_mut().close_tab(1);
    let ctx = h.ctx.clone();
    h.state_mut().open_bytes(&ctx, "other.pdf", None, include_bytes!("../data/form.pdf").to_vec()).unwrap();
    h.run_steps(3);
    h.get_by_role_and_label(egui::accesskit::Role::Tab, "doc.pdf").click();
    h.run_steps(3);
    // The menu of a text file has no Open; the PDF's has.
    right_click(&mut h, "notes.txt");
    assert!(h.get_by_role(egui::accesskit::Role::Menu).query_by_label("Open in a new tab").is_none(), "no Open for notes.txt");
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    right_click(&mut h, "inner.pdf");
    h.get_by_role(egui::accesskit::Role::Menu).get_by_label("Open in a new tab");
    h.get_by_role(egui::accesskit::Role::Menu).get_by_label("Save attachment…").click();
    h.run_steps(3);
    assert_eq!(files.file(from_menu).as_deref(), Some(INNER), "Save from inner.pdf's menu");
    right_click(&mut h, "inner.pdf");
    h.get_by_role(egui::accesskit::Role::Menu).get_by_label("Open in a new tab").click();
    h.run_steps(3);
    assert_eq!(h.state().views.len(), 3, "Open from the menu");
    h.get_by_role_and_label(egui::accesskit::Role::Tab, "doc.pdf").click();
    h.run_steps(3);
    // Delete from inner.pdf's menu leaves notes.txt.
    right_click(&mut h, "inner.pdf");
    h.get_by_role(egui::accesskit::Role::Menu).get_by_label("Delete attachment").click();
    h.run_steps(3);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!(doc.info.attachments.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["notes.txt"], "Delete from the menu");
    assert!(doc.can_undo().is_some_and(|l| l.starts_with("Delete attachment")), "as one undoable edit");
}

#[test]
fn the_description_can_be_edited_from_the_row_menu() {
    let mut h = harness();
    right_click(&mut h, "notes.txt");
    h.get_by_role(egui::accesskit::Role::Menu).get_by_label("Edit description").click();
    h.run_steps(2);
    let description = h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Description");
    description.click();
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    description.type_text("Edited in the panel");
    h.get_by_label("OK").click();
    h.run_steps(3);
    let state = h.state();
    let doc = state.session.get(state.views[0].id).unwrap();
    let attachment = doc.info.attachments.iter().find(|a| a.name == "notes.txt").unwrap();
    assert_eq!(attachment.description.as_deref(), Some("Edited in the panel"));
    assert!(doc.can_undo().is_some_and(|label| label == "Edit attachment description"));
}

#[test]
fn keeping_the_attachment_description_does_not_create_an_edit() {
    let mut h = harness();
    right_click(&mut h, "notes.txt");
    h.get_by_role(egui::accesskit::Role::Menu).get_by_label("Edit description").click();
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(3);
    let state = h.state();
    let doc = state.session.get(state.views[0].id).unwrap();
    assert_eq!(doc.info.attachments.iter().find(|a| a.name == "notes.txt").unwrap().description.as_deref(), Some("Original"));
    assert!(!doc.dirty, "confirming the unchanged description does not dirty the document");
    assert_eq!(doc.can_undo(), None, "confirming the unchanged description does not add undo history");
}

#[test]
fn editing_the_document_while_description_dialog_is_open_discards_its_stale_draft() {
    let mut h = harness();
    right_click(&mut h, "notes.txt");
    h.get_by_role(egui::accesskit::Role::Menu).get_by_label("Edit description").click();
    h.run_steps(2);
    assert!(h.state_mut().apply_edit(
        &egui::Context::default(),
        pdfcraft_engine::Edit::EditAttachmentDescription { key: "notes.txt".into(), description: Some("Changed elsewhere".into()) }
    ));
    h.get_by_label("OK").click();
    h.run_steps(3);
    let state = h.state();
    let doc = state.session.get(state.views[0].id).unwrap();
    assert_eq!(doc.info.attachments.iter().find(|a| a.name == "notes.txt").unwrap().description.as_deref(), Some("Changed elsewhere"));
    assert_eq!(doc.can_undo(), Some("Edit attachment description"));
}

/// Where the attachment named `label` shows in the panel: the lowest text with that name, below
/// a tab of the same name once the attachment is open. The row's accessible name is its name
/// followed by its details.
fn row_name(h: &Harness<'static, PdfCraftApp>, label: &str) -> egui::Pos2 {
    let row = h.get_by(|node| {
        node.role() == egui::accesskit::Role::ListItem && node.label().is_some_and(|text| text.starts_with(label)) && !node.is_hidden()
    });
    let rect = row.rect();
    egui::pos2(rect.left() + rect.width() * 0.25, rect.center().y)
}
