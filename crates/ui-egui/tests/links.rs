//! Community links: the Discord button is one click away everywhere; Help menu, About dialog and
//! home screen open the ArtCraft and PdfCraft pages.

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
    // A modal is centred on the size it reached last frame and its list shrinks to its content, so
    // the dialog keeps moving for a few frames after it opens. Let it settle before each click, or
    // the click lands where a button was rather than where it is.
    let mut h = harness(|app| app.dialog = Some(Dialog::About));
    h.run_steps(6);
    h.get_by_label("Contributors").click();
    // Tab changes resize and recenter the modal as the compiled-in credits grow.
    // Let its layout settle before clicking the next control at its new position.
    h.run_steps(4);
    // The owner is always in the compiled-in credits (contributors/contributors.json), shown by username.
    h.get_by_label("@echelon");
    h.get_by_label("Table").click();
    h.run_steps(4);
    h.get_by_label("PRs");
    h.get_by_label("Display name").click();
    h.run_steps(4);
    h.get_by_label("Brandon Thomas");
    h.get_by_label("Models").click();
    h.run_steps(4);
    assert!(h.query_all_by_label("Anthropic").count() >= 1);
}

/// The smallest window the app allows (820×520, `apps/pdfcraft/src/main.rs`). A dialog that sizes to
/// its content can push its own buttons off the bottom of the window, and a dialog you cannot close
/// is worse than a dialog with no margins.
#[test]
fn about_dialog_keeps_its_close_button_reachable() {
    let mut h = Harness::builder().with_size(egui::vec2(820.0, 520.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.dialog = Some(Dialog::About);
        app
    });
    h.run_steps(6);
    for tab in ["Contributors", "Models"] {
        h.get_by_label(tab).click();
        // A modal is centred on the size it reached last frame, so give it the frames it needs to
        // settle before treating a button as where it is drawn.
        h.run_steps(6);
        let close = h.get_by_label("Close");
        let r = close.rect();
        assert!(r.bottom() <= 520.0 && r.top() >= 0.0, "the {tab} tab put Close at {r:?} of a 520 pt window");
        close.click();
        h.run_steps(2);
        assert!(h.state().dialog.is_none(), "Close did not close the {tab} tab");
        h.state_mut().dialog = Some(Dialog::About);
        h.run_steps(6);
    }
}
