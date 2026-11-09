//! The Destinations panel (View ▸ Side panels ▸ Destinations), driven as a user would: New
//! destination at the view on screen, rename, sort, go to, Set destination and Delete.

use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

const PAGES: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R >> endobj
4 0 obj << /Type /Page /Parent 2 0 R >> endobj
5 0 obj << /Type /Page /Parent 2 0 R >> endobj
trailer << /Root 1 0 R >>
%%EOF";

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("pages.pdf", None, PAGES.to_vec()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "destinations").unwrap();
        app
    });
    h.run_steps(6);
    h
}

/// The document's destinations: (name, 0-based page).
fn destinations(h: &Harness<'static, PdfCraftApp>) -> Vec<(String, Option<usize>)> {
    let id = h.state().views[0].id;
    h.state().session.named_destinations(id).unwrap().into_iter().map(|d| (d.name, d.page)).collect()
}

/// Go to page `n` (1-based).
fn go_to(h: &mut Harness<'static, PdfCraftApp>, n: usize) {
    h.state_mut().set_option("page", &n.to_string()).unwrap();
    h.run_steps(3);
}

/// Destination options ▸ New destination, named by typing over "Untitled".
fn add(h: &mut Harness<'static, PdfCraftApp>, name: &str) {
    h.get_by_label("Destination options").click();
    h.run_steps(2);
    h.get_by_label("New destination").click();
    h.run_steps(3);
    let editor = || "Untitled";
    h.query_all_by_value(editor()).next().expect("the new destination opens in rename mode");
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(1);
    h.query_all_by_value(editor()).next().unwrap().type_text(name);
    h.run_steps(1);
    h.key_press(Key::Enter);
    h.run_steps(3);
}

/// Right-click destination `name` and choose `item`.
fn menu(h: &mut Harness<'static, PdfCraftApp>, name: &str, item: &str) {
    h.get_by_label(name).click_secondary();
    h.run_steps(2);
    h.get_by_label(item).click();
    h.run_steps(3);
}

/// The top of each listed destination's row, by name.
fn top(h: &Harness<'static, PdfCraftApp>, name: &str) -> f32 {
    h.get_by_label(name).rect().top()
}

#[test]
fn new_rename_sort_go_to_set_and_delete_destinations() {
    let mut h = harness();
    h.get_by_label("This document has no named destinations.");
    go_to(&mut h, 3);
    add(&mut h, "Results");
    go_to(&mut h, 1);
    add(&mut h, "Appendix");
    assert_eq!(destinations(&h), [("Appendix".into(), Some(0)), ("Results".into(), Some(2))]);

    // By name first; the Page label sorts by page.
    assert!(top(&h, "Appendix") < top(&h, "Results"));
    menu(&mut h, "Appendix", "Set destination");
    assert_eq!(destinations(&h), [("Appendix".into(), Some(0)), ("Results".into(), Some(2))], "set to the page on screen: still page 1");
    go_to(&mut h, 3);
    // Now Appendix goes to page 3 like Results; sorted by page then name it stays first.
    menu(&mut h, "Appendix", "Set destination");
    assert_eq!(destinations(&h), [("Appendix".into(), Some(2)), ("Results".into(), Some(2))]);
    h.get_by_label("Page").click();
    h.run_steps(2);
    assert!(h.state().destinations.by_page);

    // A click goes there.
    go_to(&mut h, 2);
    h.get_by_label("Results").click();
    h.run_steps(3);
    assert_eq!(h.state().views[0].current, 2);

    // Rename from the menu; Escape leaves the name as it was.
    menu(&mut h, "Results", "Rename");
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert_eq!(destinations(&h)[1].0, "Results");
    menu(&mut h, "Results", "Rename");
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(1);
    h.query_all_by_value("Results").next().unwrap().type_text("Findings");
    h.run_steps(1);
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert_eq!(destinations(&h), [("Appendix".into(), Some(2)), ("Findings".into(), Some(2))]);

    menu(&mut h, "Appendix", "Delete");
    assert_eq!(destinations(&h), [("Findings".into(), Some(2))]);
    assert!(h.state_mut().execute("edit.undo"));
    h.run_steps(3);
    assert_eq!(destinations(&h).len(), 2);
    h.get_by_label("Appendix");
}

#[test]
fn a_second_new_destination_is_untitled_2() {
    let mut h = harness();
    for _ in 0..2 {
        h.get_by_label("Destination options").click();
        h.run_steps(2);
        h.get_by_label("New destination").click();
        h.run_steps(3);
        h.key_press(Key::Enter);
        h.run_steps(3);
    }
    assert_eq!(destinations(&h), [("Untitled".into(), Some(0)), ("Untitled 2".into(), Some(0))]);
}
