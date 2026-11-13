//! Edit ▸ Attach a file… attaches to the document that was active when the picker opened. If the
//! user switches tabs while choosing, the pick is dropped (and they are told) rather than the file
//! landing in whichever document is active by then.

use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

/// A one-page document with a proper xref table.
fn fixture() -> Vec<u8> {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_string(),
        "<< /Type /Page /Parent 2 0 R >>".to_string(),
    ];
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
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("a.pdf", None, fixture()).unwrap();
        app.open_bytes("b.pdf", None, fixture()).unwrap();
        app
    });
    h.run_steps(2);
    h
}

fn dirty(app: &PdfCraftApp, view: usize) -> bool {
    app.session.get(app.views[view].id).is_some_and(|d| d.dirty)
}

#[test]
fn an_attachment_pick_is_dropped_when_the_user_switches_tabs() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-attach-target-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("note.txt");
    std::fs::write(&file, b"hello").unwrap();

    let mut h = harness();
    h.state_mut().active = Some(0);
    h.state_mut().pick_override = Some(vec![file.to_string_lossy().into_owned()]);
    assert!(h.state_mut().execute("edit.attach"));
    // The user switches tabs before the pick arrives.
    h.state_mut().active = Some(1);
    h.run_steps(2);
    assert!(!dirty(h.state(), 0), "the document the pick was for is unchanged");
    assert!(!dirty(h.state(), 1), "the newly active document did not get the attachment");
    let toast = h.state().toast.as_ref().map(|(m, _)| m.clone()).unwrap_or_default();
    assert!(toast.contains("document changed"), "the user is told why nothing happened: {toast:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
