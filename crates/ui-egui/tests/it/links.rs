//! Help / About: community social destinations are no longer shown in the UI.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::links;
use pdfcraft_ui_egui::{Dialog, PdfCraftApp};

fn harness(setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        setup(&mut app);
        app
    });
    h.run_steps(4);
    h
}

#[test]
fn no_discord_button_in_the_top_bar() {
    let h = harness(|_| {});
    assert!(h.query_by_label("Discord").is_none());
}

#[test]
fn home_screen_has_no_community_card() {
    let h = harness(|_| {});
    assert!(h.query_by_label("Join the ArtCraft community").is_none());
    assert!(h.query_by_label("Join our Discord").is_none());
    assert!(h.query_by_label("PdfCraft web page").is_none());
    assert!(h.query_by_label("PdfCraft on GitHub").is_none());
    assert!(h.query_by_label("ArtCraft website").is_none());
}

#[test]
fn about_dialog_shows_the_brand_without_social_links() {
    let pdf = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF";
    // With a document open, so the home screen's own controls are not on screen.
    let h = harness(move |app| {
        app.open_bytes("one.pdf", None, pdf.to_vec()).unwrap();
        app.dialog = Some(Dialog::About);
    });
    assert!(h.query_all_by_label("ArtCraft").count() >= 2, "the mark and the wordmark (alt text)");
    assert!(h.query_by_label("Join our Discord").is_none());
    assert!(h.query_by_label("PdfCraft web page").is_none());
    assert!(h.query_by_label("PdfCraft on GitHub").is_none());
    assert!(h.query_by_label("ArtCraft website").is_none());
}

#[test]
fn no_outbound_community_link_commands() {
    assert!(links::LINKS.is_empty());
    for id in ["help.discord", "help.app_page", "help.github", "help.website"] {
        assert!(pdfcraft_engine::commands::command(id).is_none(), "{id}");
        assert!(links::for_command(id).is_none(), "{id}");
    }
}

/// Runs frames until the node labelled `label` exists and has stopped moving, then returns it.
///
/// The About dialog is a modal centred on its own size, and switching tabs changes that size, so
/// for a few frames afterwards the modal grows and recentres and every control in it moves. A
/// fixed number of frames is a guess at how long that takes: it was too few after the 0.5.0
/// contributors refresh (78 names instead of 5) on some machines, the click on Table landed where
/// the button had been, and the table never opened (#671).
fn settled<'a>(h: &'a mut Harness<'static, PdfCraftApp>, label: &'a str) -> egui_kittest::Node<'a> {
    let mut last = None;
    for _ in 0..60 {
        h.run_steps(1);
        let now = h.query_by_label(label).map(|n| n.rect());
        if now.is_some() && now == last {
            return h.get_by_label(label);
        }
        last = now;
    }
    panic!("{label:?} never appeared or never stopped moving");
}

#[test]
fn about_dialog_has_contributors_and_models_tabs() {
    let mut h = harness(|app| app.dialog = Some(Dialog::About));
    settled(&mut h, "Contributors").click();
    // The owner is always in the compiled-in credits (contributors/contributors.json), shown by username.
    settled(&mut h, "@echelon");
    settled(&mut h, "Table").click();
    settled(&mut h, "PRs");
    settled(&mut h, "Display name").click();
    settled(&mut h, "Brandon Thomas");
    settled(&mut h, "Models").click();
    h.run_steps(4);
    assert!(h.query_all_by_label("Anthropic").count() >= 1);
}
