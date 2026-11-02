//! An attachment's row in the Attachments panel answers a double-click and a right-click, as
//! in Acrobat (#3466), while its buttons keep taking their own clicks.

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
        b"<< /Type /Filespec /F (notes.txt) /UF (notes.txt) /EF << /F 6 0 R >> >>".to_vec(),
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
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("doc.pdf", None, attachments_pdf()).unwrap();
        app.set_option("panel", "attachments").unwrap();
        app
    });
    h.run_steps(4);
    h
}

/// A folder of its own for one test's saved file, removed afterwards.
struct Dir(std::path::PathBuf);

impl Dir {
    fn new(test: &str) -> Self {
        let path = std::env::temp_dir().join(format!("pdfcraft-attachment-row-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
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

#[test]
fn double_clicking_a_pdf_attachment_opens_it_in_a_new_tab() {
    let mut h = harness();
    assert_eq!(h.state().views.len(), 1);
    double_click(&mut h, "inner.pdf");
    assert_eq!(h.state().views.len(), 2, "the attached PDF opens in a tab of its own");
}

#[test]
fn double_clicking_another_attachment_saves_it() {
    let dir = Dir::new("double");
    let target = dir.0.join("saved notes.txt");
    let mut h = harness();
    h.state_mut().pick_override = Some(vec![target.to_string_lossy().into_owned()]);
    double_click(&mut h, "notes.txt");
    assert_eq!(std::fs::read(&target).unwrap(), b"hello notes", "PdfCraft opens only PDFs, so it saves the rest");
    assert_eq!(h.state().views.len(), 1, "nothing opens");
}

#[test]
fn the_right_click_menu_opens_saves_and_deletes() {
    let dir = Dir::new("menu");
    let target = dir.0.join("from menu.pdf");
    let mut h = harness();
    let count = |h: &Harness<'static, PdfCraftApp>, label: &str| h.query_all_by_label(label).count();
    // The rows have Save and Delete buttons each, and the PDF's row an Open button.
    assert_eq!((count(&h, "Save attachment…"), count(&h, "Open in a new tab"), count(&h, "Delete attachment")), (2, 1, 2));
    // A text file's menu has Save and Delete, no Open.
    right_click(&mut h, "notes.txt");
    assert_eq!((count(&h, "Save attachment…"), count(&h, "Open in a new tab"), count(&h, "Delete attachment")), (3, 1, 3));
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    // Scope actions to the menu: egui nests it under its row in the accessibility tree.
    // Save writes the PDF, not the text file whose row comes last.
    right_click(&mut h, "inner.pdf");
    assert_eq!(count(&h, "Open in a new tab"), 2, "the PDF's menu offers Open");
    h.state_mut().pick_override = Some(vec![target.to_string_lossy().into_owned()]);
    h.get_by_role(egui::accesskit::Role::Menu).get_by_label("Save attachment…").click();
    h.run_steps(3);
    assert_eq!(std::fs::read(&target).unwrap(), INNER, "Save from inner.pdf's menu");
    right_click(&mut h, "inner.pdf");
    h.get_by_role(egui::accesskit::Role::Menu).get_by_label("Open in a new tab").click();
    h.run_steps(3);
    assert_eq!(h.state().views.len(), 2, "Open from the menu");
    h.state_mut().active = Some(0);
    h.run_steps(3);
    // Delete from inner.pdf's menu leaves notes.txt, whose row's button would delete itself.
    right_click(&mut h, "inner.pdf");
    h.get_by_role(egui::accesskit::Role::Menu).get_by_label("Delete attachment").click();
    h.run_steps(3);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!(doc.info.attachments.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["notes.txt"], "Delete from the menu");
    assert!(doc.can_undo().is_some_and(|l| l.starts_with("Delete attachment")), "as one undoable edit");
}

#[test]
fn the_row_buttons_still_take_their_own_clicks() {
    let dir = Dir::new("buttons");
    let target = dir.0.join("from button.txt");
    let mut h = harness();
    h.state_mut().pick_override = Some(vec![target.to_string_lossy().into_owned()]);
    // The rows list inner.pdf first, then notes.txt: the second Save button is notes.txt's.
    h.get_all_by_label("Save attachment…").nth(1).unwrap().click();
    h.run_steps(3);
    assert_eq!(std::fs::read(&target).unwrap(), b"hello notes", "the Save button under the row's click area");
    h.get_by_label("Open in a new tab").click();
    h.run_steps(3);
    assert_eq!(h.state().views.len(), 2, "the Open button");
}

/// CodeRabbit on #4513: a right-click on one of a row's buttons opens that row's menu too, as a
/// right-click anywhere on the row does.
#[test]
fn a_right_click_on_a_row_button_opens_the_row_menu() {
    let mut h = harness();
    let count = |h: &Harness<'static, PdfCraftApp>, label: &str| h.query_all_by_label(label).count();
    // inner.pdf's row comes first: its Save button is the first one.
    let at = h.get_all_by_label("Save attachment…").next().expect("inner.pdf's Save button").rect().center();
    h.hover_at(at);
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: true, modifiers: Default::default() });
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: false, modifiers: Default::default() });
    h.run_steps(2);
    assert_eq!(count(&h, "Open in a new tab"), 2, "inner.pdf's menu, with Open, is open");
    assert_eq!(count(&h, "Save attachment…"), 3);
}

/// Where the attachment named `label` shows in the panel: the lowest text with that name, below
/// a tab of the same name once the attachment is open.
fn row_name(h: &Harness<'static, PdfCraftApp>, label: &str) -> egui::Pos2 {
    h.get_all_by_label(label).map(|n| n.rect()).max_by(|a, b| a.top().total_cmp(&b.top())).expect("the attachment is listed").center()
}
