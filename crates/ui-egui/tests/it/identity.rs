//! Preferences ▸ Identity: the name new comments are signed with.

use crate::support::AppHarness as _;
use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

const PDF: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >> endobj
4 0 obj << /Length 55 >> stream
BT /F1 14 Tf 20 150 Td (The quick brown fox) Tj ET
endstream endobj
5 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj
trailer << /Root 1 0 R >>
%%EOF";

#[test]
fn the_identity_name_signs_new_comments_and_is_remembered() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "doc.pdf", None, PDF.to_vec()).expect("opens");
            app.set_option(&egui::Context::default(), "author", "Tester").unwrap();
            app.set_option(&egui::Context::default(), "dialog", "preferences").unwrap();
            app
        }
    });
    h.run_steps(4);
    // Replace the name in the field (the preferences scroll to it in a 900-point window).
    h.get_by_role_and_label(Role::TextInput, "Name on new comments").scroll_to_me();
    h.run_steps(4);
    h.get_by_role_and_label(Role::TextInput, "Name on new comments").click();
    h.run_steps(1);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.get_by_role_and_label(Role::TextInput, "Name on new comments").type_text("Grace Hopper");
    h.run_steps(2);
    // The window applies its values on OK.
    h.get_by_role_and_label(Role::Button, "OK").click();
    h.run_steps(2);
    assert_eq!(h.state().comment_prefs.author, "Grace Hopper");
    // New comments carry it. Wait for the page (and its text) to finish loading first: selecting
    // text that hasn't loaded selects nothing, and no highlight is made (seen on slow CI runners).
    h.set_option("dialog", "none").unwrap();
    crate::support::wait_until(&mut h, "the page's text layer", |app| app.views[0].glyph_screen_pos(0, 8).is_some() && !app.render_pending());
    h.state_mut().views[0].select_text(0, 4, 8);
    assert!(h.execute("comment.highlight"));
    // Wait for the edit to land in the document (it applies on a later frame).
    let author = |h: &Harness<'static, PdfCraftApp>| {
        let doc = h.state().session.get(h.state().views[0].id).unwrap();
        doc.info.annotations.first().and_then(|a| a.author.clone())
    };
    crate::support::wait_until(&mut h, "the highlight", |app| app.session.get(app.views[0].id).is_some_and(|d| !d.info.annotations.is_empty()));
    assert_eq!(author(&h).as_deref(), Some("Grace Hopper"));
    // It survives a restart; an empty or missing name keeps the default, and a huge one is cut.
    let mut restored = crate::control::new_app();
    restored.set_option(&egui::Context::default(), "language", "en").unwrap();
    // PLAN: assumed `PdfCraftApp::settings_snapshot(&self) -> PdfCraftSettings` and `apply_settings(&mut self, &PdfCraftSettings)` (pdf-shell-w9); neither exists in this tree until that set lands.
    restored.apply_settings(&h.state().settings_snapshot());
    assert_eq!(restored.comment_prefs.author, "Grace Hopper");
    restored.apply_settings(&pdfcraft_ui_egui::settings::import_json(r#"{"author": "  "}"#).0);
    assert_eq!(restored.comment_prefs.author, "Grace Hopper");
    restored.apply_settings(&pdfcraft_ui_egui::settings::import_json(&format!(r#"{{"author": "{}"}}"#, "x".repeat(10_000))).0);
    assert_eq!(restored.comment_prefs.author.chars().count(), 200);
}
