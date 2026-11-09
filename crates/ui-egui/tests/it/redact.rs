//! Redact a PDF in the real shell (egui_kittest): the Redact tool, Redact pages, the apply
//! confirmation and Clear all.

use egui::Pos2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{Dialog, PdfCraftApp, SaveTarget};

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("form.pdf", None, include_bytes!("../data/form.pdf").to_vec()).unwrap();
        app.set_option("zoom", "150").unwrap();
        app
    });
    crate::support::settle(&mut h);
    h
}

fn at(h: &Harness<'static, PdfCraftApp>, x: f32, y: f32) -> Pos2 {
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    let k = r.width() / 300.0;
    r.min + egui::vec2(x * k, y * k)
}

fn marks(h: &Harness<'static, PdfCraftApp>) -> usize {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().redaction_marks()
}

#[test]
fn marking_applying_and_clearing() {
    let mut h = harness();
    assert!(h.state_mut().execute("redact.mark"));
    h.run_steps(2);
    h.get_by_label("Redact a PDF");
    // Draw a box in an empty part of the page.
    let (a, b) = (at(&h, 40.0, 280.0), at(&h, 200.0, 320.0));
    h.hover_at(a);
    h.run_steps(1);
    h.drag_at(a);
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(a + (b - a) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    h.drop_at(b);
    h.run_steps(4);
    assert_eq!(marks(&h), 1);
    // Redact pages ▸ current page.
    h.state_mut().execute("redact.pages");
    h.run_steps(2);
    h.get_by_label("Mark Page Range");
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(marks(&h), 2);
    // Clear all, then undo it.
    h.get_by_label("Clear all").click();
    h.run_steps(3);
    assert_eq!(marks(&h), 0);
    h.state_mut().execute("edit.undo");
    h.run_steps(3);
    assert_eq!(marks(&h), 2);
    // Redact all asks first; Apply removes the marks (and the page's fields under them).
    h.get_by_label("Redact all").click();
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::RedactApply));
    h.get_by_label("Apply").click();
    h.run_steps(4);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!((doc.redaction_marks(), doc.can_undo()), (0, Some("Apply redactions")));
    assert!(doc.form.is_empty(), "the whole page was redacted, fields included");
}

/// After Apply redactions, Save asks where instead of writing over the opened file, as Acrobat
/// shows Save As with `_Redacted` appended to the name. Once that copy is written, Save writes
/// it in place again.
#[test]
fn saving_after_applying_redactions_asks_where() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-redacted-save-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let original = dir.join("contract.pdf");
    let source = include_bytes!("../data/form.pdf").to_vec();
    std::fs::write(&original, &source).unwrap();
    let path = original.to_string_lossy().into_owned();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_path(&path);
        app
    });
    crate::support::settle(&mut h);
    h.state_mut().execute("redact.pages");
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(marks(&h), 1);
    assert!(h.state_mut().execute("redact.apply"));
    h.run_steps(2);
    h.get_by_label("Apply").click();
    h.run_steps(4);
    assert_eq!(marks(&h), 0);
    assert_eq!(h.state().views[0].redactions_applied, 1);
    let copy = dir.join("contract_Redacted.pdf");
    h.state_mut().pick_override = Some(vec![copy.to_string_lossy().into_owned()]);
    assert!(!h.state_mut().save_active(SaveTarget::InPlace), "Save asks where first");
    h.run_steps(3);
    assert_eq!(std::fs::read(&original).unwrap(), source, "the opened file is left as it was");
    assert!(copy.exists(), "the redacted copy is written where the user chose");
    let s = h.state();
    assert_eq!(s.session.get(s.views[0].id).unwrap().name, "contract_Redacted.pdf");
    assert_eq!(s.views[0].redactions_applied, 0);
    h.state_mut().pick_override = None;
    assert!(h.state_mut().save_active(SaveTarget::InPlace), "the next Save writes the copy in place");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Undo past an Apply redactions that is already saved in the `_Redacted` copy makes Save ask
/// where, so the restored content is not written over the copy. Redo makes Save write in place.
#[test]
fn undoing_saved_redactions_does_not_save_over_the_redacted_copy() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-redacted-saved-undo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let original = dir.join("contract.pdf");
    std::fs::write(&original, include_bytes!("../data/form.pdf")).unwrap();
    let path = original.to_string_lossy().into_owned();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_path(&path);
        app
    });
    crate::support::settle(&mut h);
    h.state_mut().execute("redact.pages");
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert!(h.state_mut().execute("redact.apply"));
    h.run_steps(2);
    h.get_by_label("Apply").click();
    h.run_steps(4);
    let copy = dir.join("contract_Redacted.pdf");
    h.state_mut().pick_override = Some(vec![copy.to_string_lossy().into_owned()]);
    assert!(!h.state_mut().save_active(SaveTarget::InPlace));
    h.run_steps(3);
    h.state_mut().pick_override = None;
    assert!(copy.exists());
    // Undo and Redo leave the saved redactions in effect: Save writes the copy in place.
    h.state_mut().execute("edit.undo");
    h.run_steps(3);
    h.state_mut().execute("edit.redo");
    h.run_steps(3);
    assert_eq!((h.state().views[0].redactions_applied, h.state().views[0].redactions_undone_after_save), (0, 0));
    assert!(h.state_mut().save_active(SaveTarget::InPlace), "with the redactions back, Save writes the copy in place");
    let saved = std::fs::read(&copy).unwrap();
    h.state_mut().execute("edit.undo");
    h.run_steps(3);
    assert_eq!(h.state().views[0].redactions_undone_after_save, 1);
    let elsewhere = dir.join("contract_restored.pdf");
    h.state_mut().pick_override = Some(vec![elsewhere.to_string_lossy().into_owned()]);
    assert!(!h.state_mut().save_active(SaveTarget::InPlace), "Save asks where instead of writing over the copy");
    h.run_steps(3);
    h.state_mut().pick_override = None;
    assert_eq!(std::fs::read(&copy).unwrap(), saved, "the redacted copy is left as it was");
    assert!(elsewhere.exists(), "the restored version is written where the user chose");
    assert_eq!(h.state().views[0].redactions_undone_after_save, 0);
    // The restored version is now the saved one: Redo applies the redactions to it again.
    h.state_mut().execute("edit.redo");
    h.run_steps(3);
    assert_eq!(h.state().views[0].redactions_applied, 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A new Apply redactions after Undo past a saved one clears Redo, so Undo and Redo of the new
/// step do not bring Save in place back over the `_Redacted` copy.
#[test]
fn redoing_a_new_apply_after_undoing_saved_redactions_still_asks_where() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-redacted-branch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let original = dir.join("contract.pdf");
    std::fs::write(&original, include_bytes!("../data/form.pdf")).unwrap();
    let path = original.to_string_lossy().into_owned();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_path(&path);
        app
    });
    crate::support::settle(&mut h);
    h.state_mut().execute("redact.pages");
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert!(h.state_mut().execute("redact.apply"));
    h.run_steps(2);
    h.get_by_label("Apply").click();
    h.run_steps(4);
    let copy = dir.join("contract_Redacted.pdf");
    h.state_mut().pick_override = Some(vec![copy.to_string_lossy().into_owned()]);
    assert!(!h.state_mut().save_active(SaveTarget::InPlace));
    h.run_steps(3);
    h.state_mut().pick_override = None;
    let saved = std::fs::read(&copy).unwrap();
    h.state_mut().execute("edit.undo");
    h.run_steps(3);
    // Apply the marks again as a new step, then Undo and Redo it.
    assert!(h.state_mut().execute("redact.apply"));
    h.run_steps(2);
    h.get_by_label("Apply").click();
    h.run_steps(4);
    h.state_mut().execute("edit.undo");
    h.run_steps(3);
    let view = &h.state().views[0];
    assert_eq!((view.redactions_applied, view.redactions_undone_after_save, view.saved_redactions_cut_off), (0, 0, true));
    h.state_mut().execute("edit.redo");
    h.run_steps(3);
    let elsewhere = dir.join("contract_again.pdf");
    h.state_mut().pick_override = Some(vec![elsewhere.to_string_lossy().into_owned()]);
    assert!(!h.state_mut().save_active(SaveTarget::InPlace), "Save asks where instead of writing over the copy");
    h.run_steps(3);
    h.state_mut().pick_override = None;
    assert_eq!(std::fs::read(&copy).unwrap(), saved, "the redacted copy is left as it was");
    assert!(!h.state().views[0].saved_redactions_cut_off);
    let _ = std::fs::remove_dir_all(&dir);
}

/// In the web build a delivered save keeps `redactions_applied` and records it in
/// `redactions_downloaded`. Undo past a delivered Apply redactions, or a new Apply after it, makes
/// Save ask where, so a host does not write the restored content over the redacted copy.
#[test]
fn undoing_delivered_redactions_asks_where() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-redacted-delivered-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let original = dir.join("contract.pdf");
    std::fs::write(&original, include_bytes!("../data/form.pdf")).unwrap();
    let path = original.to_string_lossy().into_owned();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_path(&path);
        app
    });
    crate::support::settle(&mut h);
    h.state_mut().execute("redact.pages");
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert!(h.state_mut().execute("redact.apply"));
    h.run_steps(2);
    h.get_by_label("Apply").click();
    h.run_steps(4);
    // The state a web save leaves: the step is delivered and stays counted.
    h.state_mut().views[0].redactions_downloaded = 1;
    h.state_mut().execute("edit.undo");
    h.run_steps(3);
    let elsewhere = dir.join("contract_restored.pdf");
    h.state_mut().pick_override = Some(vec![elsewhere.to_string_lossy().into_owned()]);
    assert!(!h.state_mut().save_active(SaveTarget::InPlace), "Save asks where after Undo past a delivered step");
    h.run_steps(3);
    h.state_mut().pick_override = None;
    assert!(elsewhere.exists());
    // A new Apply after Undo past a delivered step cannot be redone back to the delivered one.
    h.state_mut().views[0].redactions_downloaded = 1;
    h.state_mut().views[0].redactions_applied = 0;
    h.state_mut().execute("redact.pages");
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert!(h.state_mut().execute("redact.apply"));
    h.run_steps(2);
    h.get_by_label("Apply").click();
    h.run_steps(4);
    assert!(h.state().views[0].saved_redactions_cut_off);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Once every Apply redactions step is delivered, Save writes in place again even though the web
/// build keeps the steps counted for the `_Redacted` name.
#[test]
fn delivered_redactions_save_in_place() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-redacted-delivered-save-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let original = dir.join("contract.pdf");
    std::fs::write(&original, include_bytes!("../data/form.pdf")).unwrap();
    let path = original.to_string_lossy().into_owned();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_path(&path);
        app
    });
    crate::support::settle(&mut h);
    h.state_mut().execute("redact.pages");
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert!(h.state_mut().execute("redact.apply"));
    h.run_steps(2);
    h.get_by_label("Apply").click();
    h.run_steps(4);
    // The state a web save leaves: the step is delivered and stays counted.
    h.state_mut().views[0].redactions_downloaded = 1;
    assert!(h.state_mut().save_active(SaveTarget::InPlace), "Save writes in place after the step is delivered");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Undo past a saved Apply redactions while the Save dialog is open makes Save ask again, so the
/// restored content is not written to the redacted copy the dialog proposed.
#[test]
fn undoing_saved_redactions_while_the_save_dialog_is_open_asks_again() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-redacted-dialog-undo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let original = dir.join("contract.pdf");
    std::fs::write(&original, include_bytes!("../data/form.pdf")).unwrap();
    let path = original.to_string_lossy().into_owned();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_path(&path);
        app
    });
    crate::support::settle(&mut h);
    let apply = |h: &mut Harness<'static, PdfCraftApp>| {
        h.state_mut().execute("redact.pages");
        h.run_steps(2);
        h.get_by_label("OK").click();
        h.run_steps(3);
        assert!(h.state_mut().execute("redact.apply"));
        h.run_steps(2);
        h.get_by_label("Apply").click();
        h.run_steps(4);
    };
    apply(&mut h);
    let copy = dir.join("contract_Redacted.pdf");
    h.state_mut().pick_override = Some(vec![copy.to_string_lossy().into_owned()]);
    assert!(!h.state_mut().save_active(SaveTarget::InPlace));
    h.run_steps(3);
    let saved = std::fs::read(&copy).unwrap();
    // A second Apply redactions makes Save ask where. Before the answer, Undo takes back the
    // second Apply, its marks and the saved Apply.
    apply(&mut h);
    assert!(!h.state_mut().save_active(SaveTarget::InPlace));
    for _ in 0..3 {
        h.state_mut().execute("edit.undo");
    }
    assert_eq!(h.state().views[0].redactions_undone_after_save, 1);
    let elsewhere = dir.join("contract_restored.pdf");
    h.state_mut().pick_override = Some(vec![elsewhere.to_string_lossy().into_owned()]);
    h.run_steps(4);
    h.state_mut().pick_override = None;
    assert_eq!(std::fs::read(&copy).unwrap(), saved, "the redacted copy is left as it was");
    assert!(elsewhere.exists(), "Save asked again and wrote where the user chose");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Undo of Apply redactions while the Save dialog that proposed the `_Redacted` name is open
/// makes Save ask again, so the unredacted content is not saved under that name.
#[test]
fn undoing_apply_redactions_while_the_save_dialog_is_open_asks_again() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-redacted-dialog-unapply-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let original = dir.join("contract.pdf");
    std::fs::write(&original, include_bytes!("../data/form.pdf")).unwrap();
    let path = original.to_string_lossy().into_owned();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_path(&path);
        app
    });
    crate::support::settle(&mut h);
    h.state_mut().execute("redact.pages");
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert!(h.state_mut().execute("redact.apply"));
    h.run_steps(2);
    h.get_by_label("Apply").click();
    h.run_steps(4);
    let copy = dir.join("contract_Redacted.pdf");
    h.state_mut().pick_override = Some(vec![copy.to_string_lossy().into_owned()]);
    assert!(!h.state_mut().save_active(SaveTarget::InPlace));
    h.state_mut().execute("edit.undo");
    assert_eq!(h.state().views[0].redactions_applied, 0);
    let elsewhere = dir.join("contract_copy.pdf");
    h.state_mut().pick_override = Some(vec![elsewhere.to_string_lossy().into_owned()]);
    h.run_steps(4);
    h.state_mut().pick_override = None;
    assert!(!copy.exists(), "the unredacted content is not saved under the _Redacted name");
    assert!(elsewhere.exists(), "Save asked again and wrote where the user chose");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Undo of Apply redactions brings back Save in place; Redo makes Save ask where again.
#[test]
fn undoing_apply_redactions_saves_in_place_again() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-redacted-undo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let original = dir.join("contract.pdf");
    std::fs::write(&original, include_bytes!("../data/form.pdf")).unwrap();
    let path = original.to_string_lossy().into_owned();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_path(&path);
        app
    });
    crate::support::settle(&mut h);
    for _ in 0..2 {
        h.state_mut().execute("redact.pages");
        h.run_steps(2);
        h.get_by_label("OK").click();
        h.run_steps(3);
        assert!(h.state_mut().execute("redact.apply"));
        h.run_steps(2);
        h.get_by_label("Apply").click();
        h.run_steps(4);
    }
    assert_eq!(h.state().views[0].redactions_applied, 2);
    h.state_mut().execute("edit.undo");
    h.run_steps(3);
    assert_eq!(h.state().views[0].redactions_applied, 1, "the first Apply redactions is still in effect");
    // Undoing the second mark leaves the first Apply redactions in effect.
    h.state_mut().execute("edit.undo");
    h.run_steps(3);
    assert_eq!(h.state().views[0].redactions_applied, 1);
    h.state_mut().execute("edit.undo");
    h.run_steps(3);
    assert_eq!(h.state().views[0].redactions_applied, 0);
    h.state_mut().execute("edit.redo");
    h.run_steps(3);
    assert_eq!(h.state().views[0].redactions_applied, 1);
    h.state_mut().execute("edit.undo");
    h.run_steps(3);
    assert!(h.state_mut().save_active(SaveTarget::InPlace), "with the redactions undone, Save writes in place");
    let s = h.state();
    assert_eq!(s.session.get(s.views[0].id).unwrap().name, "contract.pdf");
    let _ = std::fs::remove_dir_all(&dir);
}

/// File ▸ Revert discards the applied redactions, so Save writes the opened file in place again.
#[test]
fn reverting_after_apply_redactions_saves_in_place_again() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-redacted-revert-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let original = dir.join("contract.pdf");
    std::fs::write(&original, include_bytes!("../data/form.pdf")).unwrap();
    let path = original.to_string_lossy().into_owned();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_path(&path);
        app
    });
    crate::support::settle(&mut h);
    h.state_mut().execute("redact.pages");
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert!(h.state_mut().execute("redact.apply"));
    h.run_steps(2);
    h.get_by_label("Apply").click();
    h.run_steps(4);
    assert_eq!(h.state().views[0].redactions_applied, 1);
    assert!(h.state_mut().execute("file.revert"));
    h.run_steps(2);
    assert_eq!(h.state().dialog, Some(Dialog::Revert));
    h.get_all_by_label("Revert").last().expect("the button").click();
    h.run_steps(3);
    assert_eq!(h.state().views[0].redactions_applied, 0);
    assert!(h.state_mut().save_active(SaveTarget::InPlace), "after Revert, Save writes in place");
    let s = h.state();
    assert_eq!(s.session.get(s.views[0].id).unwrap().name, "contract.pdf");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn redaction_codes_become_the_overlay_text() {
    let mut h = harness();
    assert!(h.state_mut().execute("redact.properties"));
    h.run_steps(2);
    h.get_by_label("Redaction Tool Properties");
    h.get_by_label("Use overlay text").click();
    h.run_steps(2);
    h.state_mut().redact_prefs.overlay = "CONFIDENTIAL".into();
    h.get_by_label("Redaction code:").click();
    h.run_steps(2);
    h.get_by_label("(b)(7)(C)").click();
    h.run_steps(2);
    h.get_by_label("(b)(6)").click();
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(2);
    let prefs = &h.state().redact_prefs;
    assert_eq!(prefs.overlay_text(), "(b)(6), (b)(7)(C)", "codes in the set's order");
    let pdfcraft_engine::Edit::AddAnnotation(a) = prefs.mark(0, vec![pdfcraft_engine::rect_quad([10.0, 10.0, 50.0, 30.0])], "T") else {
        panic!("a mark adds an annotation")
    };
    assert!(matches!(a.shape, pdfcraft_engine::Shape::Redact { ref overlay, .. } if overlay == "(b)(6), (b)(7)(C)"));
    h.state_mut().redact_prefs.use_code = false;
    assert_eq!(h.state().redact_prefs.overlay_text(), "CONFIDENTIAL", "custom text is kept while codes are used");
}

/// The Redaction Tool Properties are the next run's too, like every other tool's defaults
/// (Acrobat's "Make Properties Default" without the box): fill, overlay text or codes, the
/// text's look, and the mark's outline colour and opacity all come back from the settings.
#[test]
fn redaction_properties_survive_a_restart() {
    let mut h = harness();
    {
        let p = &mut h.state_mut().redact_prefs;
        p.fill = None;
        p.use_overlay = true;
        p.use_code = true;
        p.code_set = pdfcraft_engine::REDACTION_CODE_SETS[0];
        p.codes = vec!["(b)(6)", "(b)(7)(C)"];
        p.overlay = "CONFIDENTIAL".into();
        p.look.font = pdfcraft_engine::OverlayFont::Courier;
        p.look.size = 12.0;
        p.look.color = [0.0, 0.0, 1.0];
        p.look.align = 2;
        p.look.repeat = true;
        p.outline = [0.0, 0.5, 0.0];
        p.hover_fill = [1.0, 1.0, 0.0];
        p.opacity = 0.4;
    }
    // The defaults are what OK with "Make properties default" ticked leaves: go through the
    // dialog.
    assert!(h.state_mut().execute("redact.properties"));
    h.run_steps(6);
    h.get_by_label("Make properties default");
    h.get_by_label("OK").click();
    h.run_steps(2);
    let json = h.state().persist();
    let mut fresh = PdfCraftApp::new();
    fresh.restore(&json);
    let p = &fresh.redact_prefs;
    assert_eq!(p.fill, None, "no colour is a saved choice, not a missing one");
    assert_eq!(p.overlay_text(), "(b)(6), (b)(7)(C)");
    assert_eq!(p.overlay, "CONFIDENTIAL");
    assert_eq!(p.look.font, pdfcraft_engine::OverlayFont::Courier);
    assert_eq!((p.look.size, p.look.color, p.look.align, p.look.repeat), (12.0, [0.0, 0.0, 1.0], 2, true));
    assert_eq!((p.outline, p.hover_fill, p.opacity), ([0.0, 0.5, 0.0], [1.0, 1.0, 0.0], 0.4));
    // The next mark takes the outline and opacity.
    let pdfcraft_engine::Edit::AddAnnotation(a) = p.mark(0, vec![pdfcraft_engine::rect_quad([10.0, 10.0, 50.0, 30.0])], "T") else {
        panic!("a mark adds an annotation")
    };
    assert_eq!((a.style.color, a.style.opacity, a.style.fill), ([0.0, 0.5, 0.0], 0.4, None));
    // Settings are untrusted: a bad colour, an unknown code set and a code outside the set are
    // dropped, the rest kept.
    let mut odd = PdfCraftApp::new();
    odd.restore(
        r#"{"redact_prefs":{"outline":[2,0,0],"opacity":0.7,"code_set":"nope","codes":["(b)(6)","bogus"],"use_overlay":true,"use_code":true}}"#,
    );
    let p = &odd.redact_prefs;
    assert_eq!((p.outline, p.opacity), ([0.89, 0.13, 0.13], 0.7));
    assert_eq!(p.code_set.id, pdfcraft_engine::REDACTION_CODE_SETS[0].id);
    assert_eq!(p.overlay_text(), "(b)(6)");
}

/// "Make properties default" off: OK keeps the values for this run only, the saved defaults
/// stay what they were; Cancel puts back what the dialog opened with.
#[test]
fn redaction_properties_for_this_run_only_and_cancel() {
    let mut h = harness();
    assert!(h.state_mut().execute("redact.properties"));
    h.run_steps(6);
    h.get_by_label("Redaction mark appearance");
    h.get_by_label("Outline colour:");
    h.get_by_label("Fill colour:");
    h.get_by_label("Opacity:");
    h.get_by_label("Make properties default").click();
    h.run_steps(3);
    assert!(!h.state().redact_prefs.remember);
    h.state_mut().redact_prefs.outline = [0.0, 0.0, 1.0];
    h.get_by_label("OK").click();
    h.run_steps(2);
    assert_eq!(h.state().redact_prefs.outline, [0.0, 0.0, 1.0], "used for this run");
    assert_eq!(h.state().redact_defaults.outline, [0.89, 0.13, 0.13], "not made the default");
    let mut fresh = PdfCraftApp::new();
    fresh.restore(&h.state().persist());
    assert_eq!(fresh.redact_prefs.outline, [0.89, 0.13, 0.13], "the next run starts from the saved defaults");
    // Cancel discards the edits made while the dialog was open.
    assert!(h.state_mut().execute("redact.properties"));
    h.run_steps(2);
    h.state_mut().redact_prefs.opacity = 0.2;
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert_eq!(h.state().redact_prefs.opacity, 1.0, "cancelled");
    assert_eq!(h.state().redact_prefs.outline, [0.0, 0.0, 1.0], "the run's values stay");
}

#[test]
fn redacting_an_imported_word_list() {
    let memo = pdfcraft_engine::Session::new().create_from_text("memo", "Call Ada today\nAda is private\nPublic line").unwrap().to_vec();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("memo.pdf", None, memo.clone()).unwrap();
        app
    });
    h.run_steps(4);
    h.state_mut().execute("redact.search");
    h.run_steps(2);
    h.get_by_label("Multiple words or phrases");
    let list = b"  Ada \n\nprivate\nAda\nabsent\n".to_vec();
    h.state_mut().use_files(pdfcraft_ui_egui::FilePurpose::RedactWords, vec![("names.txt".into(), list)]);
    let d = &h.state().redact_search;
    assert_eq!((d.mode, d.words.as_str()), (pdfcraft_ui_egui::RedactSearchMode::Words, "Ada\nprivate\nabsent"));
    h.run_steps(2);
    h.get_by_label("3 word(s) or phrase(s)");
    h.get_by_label("Mark all").click();
    h.run_steps(3);
    assert_eq!((marks(&h), h.state().redact_search.found), (3, Some(3)));
    let too_big = vec![b'a'; 2 << 20];
    h.state_mut().use_files(pdfcraft_ui_egui::FilePurpose::RedactWords, vec![("huge.txt".into(), too_big)]);
    assert_eq!(h.state().redact_search.words, "Ada\nprivate\nabsent", "an oversized list changes nothing");
}

#[test]
fn redaction_search_can_match_case_and_whole_words() {
    let memo = pdfcraft_engine::Session::new().create_from_text("memo", "Ada ada Adaline\nAda beta betamax").unwrap().to_vec();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("memo.pdf", None, memo.clone()).unwrap();
        app
    });
    h.run_steps(4);
    h.state_mut().execute("redact.search");
    h.state_mut().redact_search.text = "Ada".into();
    h.run_steps(2);
    h.get_by_label("Whole words only").click();
    h.get_by_label("Case-sensitive").click();
    h.run_steps(2);
    assert!(h.state().redact_search.whole_words && h.state().redact_search.case_sensitive);
    h.get_by_label("Mark all").click();
    h.run_steps(3);
    assert_eq!((marks(&h), h.state().redact_search.found), (2, Some(2)));
}

#[test]
fn removing_hidden_information_and_sanitizing() {
    let mut h = harness();
    h.state_mut().execute("protect.remove_hidden");
    h.run_steps(2);
    h.get_by_label("Remove hidden information");
    {
        let d = &h.state().hidden_draft;
        let fields = d.found.iter().find(|f| f.0 == pdfcraft_engine::Hidden::FormFields).unwrap();
        assert_eq!((fields.1, fields.2), (5, true), "five fields, checked");
    }
    h.get_by_label("Remove").click();
    h.run_steps(3);
    {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        assert!(doc.form.is_empty());
        assert_eq!(doc.can_undo(), Some("Remove hidden information"));
    }
    h.state_mut().execute("redact.sanitize");
    h.run_steps(2);
    h.get_by_label("Sanitize").click();
    h.run_steps(3);
    let s = h.state();
    assert_eq!(s.session.get(s.views[0].id).unwrap().can_undo(), Some("Sanitize document"));
}
