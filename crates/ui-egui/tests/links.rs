//! Community links: the Discord button is one click away everywhere; Help menu, About dialog and
//! home screen open the ArtCraft and PdfCraft pages.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::links;
use pdfcraft_ui_egui::{Dialog, PdfCraftApp};

fn harness(setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        setup(&mut app);
        app
    });
    h.run_steps(4);
    h
}

#[test]
fn discord_button_in_the_top_bar_opens_discord() {
    let mut h = harness(|_| {});
    h.get_by_label("Discord").click();
    h.run_steps(2);
    assert_eq!(h.state().last_opened_url.as_deref(), Some(links::DISCORD));
    assert_eq!(links::DISCORD, "https://discord.gg/artcraft");
}

#[test]
fn home_screen_links() {
    for (label, url) in [
        ("Join our Discord", links::DISCORD),
        ("PdfCraft web page", "https://getartcraft.com/apps/pdfcraft"),
        ("PdfCraft on GitHub", "https://github.com/storytold/pdfcraft"),
        ("ArtCraft website", "https://getartcraft.com"),
    ] {
        let mut h = harness(|_| {});
        h.get_by_label("Join the ArtCraft community");
        h.get_by_label(label).click();
        h.run_steps(2);
        assert_eq!(h.state().last_opened_url.as_deref(), Some(url), "{label}");
    }
}

#[test]
fn about_dialog_shows_the_brand_and_links() {
    let pdf = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF";
    // With a document open, so the home screen's own links are not on screen.
    let mut h = harness(move |app| {
        app.open_bytes("one.pdf", None, pdf.to_vec()).unwrap();
        app.dialog = Some(Dialog::About);
    });
    assert!(h.query_all_by_label("ArtCraft").count() >= 2, "the mark and the wordmark (alt text)");
    h.get_by_label("Join our Discord").click();
    h.run_steps(2);
    assert_eq!(h.state().last_opened_url.as_deref(), Some(links::DISCORD));
}

#[test]
fn help_commands_open_each_link() {
    for l in links::LINKS {
        let mut h = harness(|_| {});
        assert!(h.state_mut().execute(l.command), "{}", l.command);
        assert_eq!(h.state().last_opened_url.as_deref(), Some(l.url));
        assert_eq!(pdfcraft_engine::commands::command(l.command).unwrap().menu, Some("Help"));
    }
}

#[test]
fn about_dialog_has_contributors_and_models_tabs() {
    let mut h = harness(|app| app.dialog = Some(Dialog::About));
    h.get_by_label("Contributors").click();
    h.run_steps(2);
    // The owner is always in the compiled-in credits (contributors/contributors.json), shown by username.
    h.get_by_label("@echelon");
    h.get_by_label("Table").click();
    h.run_steps(2);
    h.get_by_label("PRs");
    h.get_by_label("Display name").click();
    h.run_steps(2);
    h.get_by_label("Brandon Thomas");
    h.get_by_label("Models").click();
    h.run_steps(2);
    assert!(h.query_all_by_label("Anthropic").count() >= 1);
}
