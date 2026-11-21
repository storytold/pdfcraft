//! Redact a PDF in the real shell (egui_kittest): the Redact tool, Redact pages, the apply
//! confirmation and Clear all.

use egui::Pos2;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
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
    let (prefs, codes) = (&h.state().redact_prefs, &h.state().redaction_codes);
    assert_eq!(prefs.overlay_text(codes), "(b)(6), (b)(7)(C)", "codes in the set's order");
    let pdfcraft_engine::Edit::AddAnnotation(a) = prefs.mark(codes, 0, vec![pdfcraft_engine::rect_quad([10.0, 10.0, 50.0, 30.0])], "T") else {
        panic!("a mark adds an annotation")
    };
    assert!(matches!(a.shape, pdfcraft_engine::Shape::Redact { ref overlay, .. } if overlay == "(b)(6), (b)(7)(C)"));
    h.state_mut().redact_prefs.use_code = false;
    assert_eq!(h.state().redact_prefs.overlay_text(&h.state().redaction_codes), "CONFIDENTIAL", "custom text is kept while codes are used");
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
        p.code_set = pdfcraft_engine::REDACTION_CODE_SETS[0].id.into();
        p.codes = vec!["(b)(6)".into(), "(b)(7)(C)".into()];
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
    assert_eq!(p.overlay_text(&fresh.redaction_codes), "(b)(6), (b)(7)(C)");
    assert_eq!(p.overlay, "CONFIDENTIAL");
    assert_eq!(p.look.font, pdfcraft_engine::OverlayFont::Courier);
    assert_eq!((p.look.size, p.look.color, p.look.align, p.look.repeat), (12.0, [0.0, 0.0, 1.0], 2, true));
    assert_eq!((p.outline, p.hover_fill, p.opacity), ([0.0, 0.5, 0.0], [1.0, 1.0, 0.0], 0.4));
    // The next mark takes the outline and opacity.
    let pdfcraft_engine::Edit::AddAnnotation(a) = p.mark(&fresh.redaction_codes, 0, vec![pdfcraft_engine::rect_quad([10.0, 10.0, 50.0, 30.0])], "T")
    else {
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
    assert_eq!(p.code_set, pdfcraft_engine::REDACTION_CODE_SETS[0].id);
    assert_eq!(p.overlay_text(&odd.redaction_codes), "(b)(6)");
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

/// A one-page PDF showing "Codename BLUEFIN report", with bookmarks "Bluefin plans" and
/// "Annual figures".
fn bookmarked_pdf() -> Vec<u8> {
    let content = "BT /F1 18 Tf 20 150 Td (Codename BLUEFIN report) Tj ET";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /Outlines 6 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_string(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /Outlines /First 7 0 R /Last 8 0 R /Count 2 >>".to_string(),
        "<< /Title (Bluefin plans) /Parent 6 0 R /Next 8 0 R /Dest [3 0 R /Fit] >>".to_string(),
        "<< /Title (Annual figures) /Parent 6 0 R /Prev 7 0 R /Dest [3 0 R /Fit] >>".to_string(),
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

/// #2539 review: applying redactions says when the removed text is still in a place it leaves
/// as it is (here a bookmark title), in the same notification as the count of marks applied.
#[test]
fn applying_says_where_the_removed_text_is_still_held() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("bluefin.pdf", None, bookmarked_pdf()).unwrap();
        app
    });
    crate::support::settle(&mut h);
    assert!(h.state_mut().execute("redact.mark"));
    h.run_steps(2);
    // Redact pages ▸ current page, then Redact all ▸ Apply.
    h.state_mut().execute("redact.pages");
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(marks(&h), 1);
    h.get_by_label("Redact all").click();
    h.run_steps(3);
    h.get_by_label("Apply").click();
    h.run_steps(4);
    assert_eq!(marks(&h), 0);
    let toast = h.state().toast.as_ref().map(|(m, _)| m.clone()).unwrap_or_default();
    assert_eq!(
        toast,
        "Applied 1 redaction mark. Save to remove the content from the file. The redacted text also appears in 1 bookmark title, which redaction doesn't change. Remove hidden information removes them."
    );
}

/// #2539 review: applying redactions that remove the document's XMP packet, which named the
/// redacted word and said the file conforms to PDF/A, says that the identification was written
/// back, in the same notification as the count of marks applied.
#[test]
fn applying_says_the_pdfa_identification_was_written_back() {
    let xmp = r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?><x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:pdfaid="http://www.aiim.org/pdfa/ns/id/" pdfaid:part="2" pdfaid:conformance="B"/><rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Bluefin report</dc:title></rdf:Description></rdf:RDF></x:xmpmeta><?xpacket end="w"?>"#;
    let content = "BT /F1 18 Tf 20 150 Td (Codename BLUEFIN report) Tj ET";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /Metadata 6 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_string(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        format!("<< /Type /Metadata /Subtype /XML /Length {} >>\nstream\n{xmp}\nendstream", xmp.len()),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        bytes.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("pdfa.pdf", None, bytes).unwrap();
        app
    });
    crate::support::settle(&mut h);
    assert!(h.state_mut().execute("redact.mark"));
    h.run_steps(2);
    h.state_mut().execute("redact.pages");
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(marks(&h), 1);
    h.get_by_label("Redact all").click();
    h.run_steps(3);
    h.get_by_label("Apply").click();
    h.run_steps(4);
    assert_eq!(marks(&h), 0);
    let toast = h.state().toast.as_ref().map(|(m, _)| m.clone()).unwrap_or_default();
    assert_eq!(
        toast,
        "Applied 1 redaction mark. Save to remove the content from the file. The document's XMP metadata held the redacted text and was removed. Its PDF/A identification was written back on its own."
    );
}

/// Click `label` once it has stopped moving: a dialog that changes size is centred again over a
/// few frames.
fn click(h: &mut Harness<'static, PdfCraftApp>, label: &str) {
    let mut last = None;
    for _ in 0..40 {
        let rect = h.get_by_label(label).rect();
        if last == Some(rect) {
            h.get_by_label(label).click();
            return;
        }
        last = Some(rect);
        h.run_steps(1);
    }
    panic!("{label} kept moving");
}

/// Redaction Properties ▸ Edit Codes: a custom set is made, given codes, renamed and edited, used
/// for the overlay text, exported, deleted and imported again, and kept in the settings. Built-in
/// sets stay as they are.
#[test]
fn custom_code_sets_from_edit_codes() {
    use pdfcraft_engine::redaction_codes::CodeEdit;
    let dir = std::env::temp_dir().join(format!("pdfcraft-redaction-codes-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut h = harness();
    h.state_mut().export_dir_override = Some(dir.to_string_lossy().into_owned());
    assert!(h.state_mut().execute("redact.properties"));
    h.run_steps(2);
    click(&mut h, "Use overlay text");
    h.run_steps(2);
    click(&mut h, "Redaction code:");
    h.run_steps(2);
    click(&mut h, "Edit Codes…");
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::RedactCodes));
    h.get_by_label("U.S. FOIA (built-in)");
    h.get_by_label("Codes in U.S. FOIA");
    h.get_by_label("Built-in sets can't be changed. Make a new set for your own codes.");
    // A built-in set refuses edits, here and headlessly.
    assert!(!h.state_mut().edit_redaction_codes(CodeEdit::AddCode { set: "foia".into(), code: "(b)(10)".into() }));
    assert!(h.state().code_editor.message.as_ref().is_some_and(|(m, refused)| *refused && m.contains("built-in")));

    h.state_mut().code_editor.set_name = "Court".into();
    click(&mut h, "New set");
    h.run_steps(2);
    h.get_by_label("Codes in Court");
    for code in ["Sealed", "Minor", "Juror"] {
        h.state_mut().code_editor.code_text = code.into();
        click(&mut h, "Add code");
        h.run_steps(2);
    }
    click(&mut h, "Juror");
    h.run_steps(2);
    h.state_mut().code_editor.code_text = "Jury".into();
    click(&mut h, "Rename code");
    h.run_steps(2);
    // A code the set already has is refused, with the reason shown.
    h.state_mut().code_editor.code_text = "Sealed".into();
    click(&mut h, "Add code");
    h.run_steps(2);
    h.get_by_label("the code set \"Court\" already has the code \"Sealed\"");
    click(&mut h, "Minor");
    h.run_steps(2);
    click(&mut h, "Remove code");
    h.run_steps(2);
    h.state_mut().code_editor.set_name = "Bench".into();
    click(&mut h, "Rename set");
    h.run_steps(2);
    assert_eq!(h.state().redaction_codes.find("Bench").unwrap().codes(), ["Sealed", "Jury"]);
    click(&mut h, "Close");
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::RedactProps), "Close goes back to the properties");

    // The custom set is chosen like a built-in one (the set list offers it), and its codes
    // become the overlay text.
    h.state_mut().redact_prefs.code_set = "Bench".into();
    h.run_steps(2);
    click(&mut h, "Jury");
    h.run_steps(2);
    click(&mut h, "Sealed");
    h.run_steps(2);
    click(&mut h, "OK");
    h.run_steps(2);
    let s = h.state();
    assert_eq!(s.redact_prefs.code_set, "Bench");
    assert_eq!(s.redact_prefs.overlay_text(&s.redaction_codes), "Sealed, Jury", "the set's order");
    let pdfcraft_engine::Edit::AddAnnotation(a) =
        s.redact_prefs.mark(&s.redaction_codes, 0, vec![pdfcraft_engine::rect_quad([10.0, 10.0, 50.0, 30.0])], "T")
    else {
        panic!("a mark adds an annotation")
    };
    assert!(matches!(a.shape, pdfcraft_engine::Shape::Redact { ref overlay, .. } if overlay == "Sealed, Jury"));

    // The sets and the choice survive a restart.
    let json = h.state().persist();
    let mut fresh = PdfCraftApp::new();
    fresh.restore(&json);
    assert_eq!(fresh.redaction_codes, h.state().redaction_codes);
    assert_eq!(fresh.redact_prefs.overlay_text(&fresh.redaction_codes), "Sealed, Jury");

    // Export, delete, import.
    assert!(h.state_mut().execute("redact.properties"));
    h.run_steps(2);
    click(&mut h, "Edit Codes…");
    h.run_steps(3);
    click(&mut h, "Export…");
    h.run_steps(3);
    let file = dir.join("redaction-codes.json");
    let written = std::fs::read_to_string(&file)
        .unwrap_or_else(|e| panic!("{e}: {:?} {:?} {:?}", h.state().dialog, h.state().code_editor.message, h.state().toast));
    assert!(written.contains("\"pdfcraft-redaction-codes\"") && written.contains("\"Bench\""), "{written}");
    click(&mut h, "Delete set");
    h.run_steps(2);
    assert!(h.state().redaction_codes.custom().is_empty());
    assert_eq!(h.state().redact_prefs.code_set, "foia", "a deleted set is no longer chosen");
    assert!(h.state().redact_prefs.codes.is_empty());
    h.state_mut().pick_override = Some(vec![file.to_string_lossy().into_owned()]);
    click(&mut h, "Import…");
    h.run_steps(4);
    assert_eq!(h.state().redaction_codes.find("Bench").unwrap().codes(), ["Sealed", "Jury"]);
    h.get_by_label("Imported the code set Bench from redaction-codes.json.");
    // The same file again: the name is taken, so nothing is imported, and the reason is shown.
    click(&mut h, "Import…");
    h.run_steps(4);
    assert!(h.state().code_editor.message.as_ref().is_some_and(|(m, refused)| *refused && m.contains("already a code set named")));
    assert_eq!(h.state().redaction_codes.custom().len(), 1);
    // A damaged file is refused whole.
    let bad = dir.join("bad.json");
    std::fs::write(&bad, r#"{"format":"pdfcraft-redaction-codes","version":1,"sets":[{"name":"X","codes":["a\u0000"]}]}"#).unwrap();
    h.state_mut().pick_override = Some(vec![bad.to_string_lossy().into_owned()]);
    click(&mut h, "Import…");
    h.run_steps(4);
    assert!(h.state().code_editor.message.as_ref().is_some_and(|(m, _)| m.contains("bad.json was not imported") && m.contains("control character")));
    assert_eq!(h.state().redaction_codes.custom().len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// CodeRabbit on #7992. A set of 200 codes keeps OK on screen (the codes scroll); Escape on Edit
/// Codes goes back to the properties; Import uses the replace choice of the click, not one made
/// while the file was on its way; a replacing import spelled in another case keeps the set chosen
/// and resets the editor's fields; and a code the standard fonts can't draw can't be picked.
#[test]
fn edit_codes_import_dismissal_long_sets_and_undrawable_codes() {
    use pdfcraft_engine::redaction_codes::CodeEdit;
    let dir = std::env::temp_dir().join(format!("pdfcraft-redaction-codes-review-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut h = harness();
    let s = h.state_mut();
    assert!(s.edit_redaction_codes(CodeEdit::CreateSet { name: "Bench".into() }));
    for code in ["Sealed", "Jury", "Ωmega"] {
        assert!(s.edit_redaction_codes(CodeEdit::AddCode { set: "Bench".into(), code: code.into() }));
    }
    assert!(s.edit_redaction_codes(CodeEdit::CreateSet { name: "Long".into() }));
    for n in 0..200 {
        assert!(s.edit_redaction_codes(CodeEdit::AddCode { set: "Long".into(), code: format!("Code {n}") }));
    }

    // 200 codes: OK and Cancel stay on screen.
    s.redact_prefs.use_overlay = true;
    s.redact_prefs.use_code = true;
    s.redact_prefs.code_set = "Long".into();
    assert!(s.execute("redact.properties"));
    h.run_steps(3);
    let screen = h.ctx.content_rect();
    assert!(screen.contains_rect(h.get_by_label("OK").rect()), "OK is on screen");
    let (first, second) = (h.get_by_label("Code 0").rect(), h.get_by_label("Code 2").rect());
    assert!(second.top() > first.top(), "the codes go in rows, one under the other");
    // A code in the scrolled list takes a click.
    click(&mut h, "Code 1");
    h.run_steps(2);
    click(&mut h, "OK");
    h.run_steps(2);
    assert_eq!(h.state().redact_prefs.codes, ["Code 1"]);
    h.state_mut().redact_prefs.codes.clear();

    // An undrawable code is shown but can't be picked; the others can.
    h.state_mut().redact_prefs.code_set = "Bench".into();
    assert!(h.state_mut().execute("redact.properties"));
    h.run_steps(3);
    assert!(h.get_by_label("Ωmega").accesskit_node().is_disabled(), "the standard fonts can't draw it");
    assert!(!h.get_by_label("Sealed").accesskit_node().is_disabled());
    click(&mut h, "Sealed");
    h.run_steps(2);

    // Escape on Edit Codes goes back to the properties, whose draft still waits.
    click(&mut h, "Edit Codes…");
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::RedactCodes));
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::RedactProps), "back to the properties");
    click(&mut h, "OK");
    h.run_steps(2);
    assert_eq!(h.state().redact_prefs.codes, ["Sealed"]);

    // A file holding "BENCH": Import clicked with replace off refuses it, even if replace is
    // turned on before the file arrives.
    let file = dir.join("bench.json");
    std::fs::write(&file, r#"{"format":"pdfcraft-redaction-codes","version":1,"sets":[{"name":"BENCH","codes":["Sealed","Minor"]}]}"#).unwrap();
    assert!(h.state_mut().execute("redact.properties"));
    h.run_steps(3);
    click(&mut h, "Edit Codes…");
    h.run_steps(3);
    h.state_mut().code_editor.replace = false;
    h.state_mut().pick_override = Some(vec![file.to_string_lossy().into_owned()]);
    click(&mut h, "Import…");
    // The click's frame asks for the file; it arrives on a later one.
    h.run_steps(1);
    h.state_mut().code_editor.replace = true;
    h.run_steps(4);
    assert!(
        h.state().code_editor.message.as_ref().is_some_and(|(m, refused)| *refused && m.contains("already a code set named")),
        "{:?}",
        h.state().code_editor.message
    );
    assert_eq!(h.state().redaction_codes.find("Bench").unwrap().codes(), ["Sealed", "Jury", "Ωmega"]);

    // With replace on when clicked, it replaces the set under the file's spelling, which stays
    // chosen with the codes it still has; the editor shows that set with no code or text left
    // from before.
    h.state_mut().code_editor.set_name = "Long".into();
    h.state_mut().code_editor.code_text = "Leftover".into();
    h.state_mut().pick_override = Some(vec![file.to_string_lossy().into_owned()]);
    click(&mut h, "Import…");
    h.run_steps(4);
    let s = h.state();
    assert_eq!(s.redaction_codes.find("BENCH").map(|b| b.codes()), Some(vec!["Sealed", "Minor"]), "{:?}", s.code_editor.message);
    assert_eq!((s.redact_prefs.code_set.as_str(), s.redact_prefs.codes.as_slice()), ("BENCH", &["Sealed".to_string()][..]));
    assert_eq!((s.code_editor.set_name.as_str(), s.code_editor.code_text.as_str()), ("BENCH", ""));
    let _ = std::fs::remove_dir_all(&dir);
}

/// CodeRabbit on #7992: while the saved code sets can't be read, the saved choice of set and
/// codes is written back too, until the user chooses other defaults.
#[test]
fn an_unreadable_library_keeps_the_saved_code_choice() {
    let damaged = r#"{"redaction_code_sets":{"format":"pdfcraft-redaction-codes","version":2,"sets":[]},"redact_prefs":{"use_code":true,"code_set":"Court","codes":["Sealed"]}}"#;
    let mut odd = PdfCraftApp::new();
    odd.restore(damaged);
    assert!(odd.redaction_codes_unread.is_some(), "the newer library isn't read");
    let saved: serde_json::Value = serde_json::from_str(&odd.persist()).unwrap();
    assert_eq!((&saved["redact_prefs"]["code_set"], &saved["redact_prefs"]["codes"]), (&serde_json::json!("Court"), &serde_json::json!(["Sealed"])));
    // Defaults the user chooses replace it.
    odd.redact_defaults.code_set = "foia".into();
    odd.redact_defaults.codes = vec!["(b)(1)".into()];
    let saved: serde_json::Value = serde_json::from_str(&odd.persist()).unwrap();
    assert_eq!(saved["redact_prefs"]["code_set"], "foia");
}

/// Saved code sets that can't be read are neither dropped nor overwritten: the user is told, the
/// entry is written back as it was, and Edit Codes can discard it.
#[test]
fn unreadable_saved_code_sets_are_kept_until_discarded() {
    use pdfcraft_engine::redaction_codes::CodeEdit;
    let damaged = r#"{"redaction_code_sets":{"format":"pdfcraft-redaction-codes","version":1,"sets":[{"name":"A","codes":["x","x"]}]}}"#;
    let mut odd = PdfCraftApp::new();
    odd.restore(damaged);
    assert!(odd.redaction_codes.custom().is_empty());
    let (_, why) = odd.redaction_codes_unread.clone().expect("kept, with the reason");
    assert!(why.contains("already has the code"), "{why}");
    assert!(odd.toast.as_ref().is_some_and(|(m, _)| m.contains("could not be read")), "the user is told");
    let saved: serde_json::Value = serde_json::from_str(&odd.persist()).unwrap();
    let original: serde_json::Value = serde_json::from_str(damaged).unwrap();
    assert_eq!(saved["redaction_code_sets"], original["redaction_code_sets"], "written back unchanged");
    assert!(!odd.edit_redaction_codes(CodeEdit::CreateSet { name: "New".into() }), "nothing overwrites them");
    odd.discard_unread_redaction_codes();
    assert!(odd.edit_redaction_codes(CodeEdit::CreateSet { name: "New".into() }));
    let saved: serde_json::Value = serde_json::from_str(&odd.persist()).unwrap();
    assert_eq!(saved["redaction_code_sets"]["sets"][0]["name"], "New");
}
