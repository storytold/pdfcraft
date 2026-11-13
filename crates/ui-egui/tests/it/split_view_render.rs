//! Split view: the two views of one document render through its one pool, and an edit made after
//! they collected the frame's demand (#3149) leaves nothing of the old pages in that pool.

use egui::{Key, Modifiers};
use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.run_inline = true;
        app.set_option("language", "en").unwrap();
        app.open_bytes("form.pdf", None, include_bytes!("../data/form.pdf").to_vec()).unwrap();
        app
    });
    h.run_steps(4);
    h
}

/// Run frames until `ready`, rendering in between (renders arrive between frames).
fn wait(h: &mut Harness<'static, PdfCraftApp>, what: &str, ready: impl Fn(&PdfCraftApp) -> bool) {
    for _ in 0..600 {
        h.run_steps(1);
        if ready(h.state()) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("timed out waiting for {what}");
}

/// The palette runs its command after the page area was drawn, as a dialog's OK does: the page
/// turns after both views asked for this frame's rasters at the old size. Neither view may keep
/// that demand (the single-view path drops it; the shared one handed it to the pool).
#[test]
fn a_late_edit_drops_both_views_stale_demand() {
    let mut h = harness();
    h.key_press_modifiers(Modifiers::COMMAND, Key::Backslash);
    h.run_steps(4);
    assert_eq!(h.state().views.len(), 2);
    assert_eq!(h.state().views[0].id, h.state().views[1].id, "one document in both panes");
    let portrait = |s: &PdfCraftApp, i: usize| s.views[i].drawn_page_size(0).is_some_and(|[w, hgt]| hgt > w);
    let landscape = |s: &PdfCraftApp, i: usize| s.views[i].drawn_page_size(0).is_some_and(|[w, hgt]| w > hgt);
    wait(&mut h, "both panes drawn", |s| portrait(s, 0) && portrait(s, 1));
    let id = h.state().views[0].id;
    let rotation = |h: &Harness<'static, PdfCraftApp>| h.state().session.get(id).unwrap().info.pages[0].rotation;
    h.state_mut().set_option("palette", "rotate pages clockwise").unwrap();
    h.run_steps(2);
    assert!(h.state().palette_open);
    h.key_press(Key::Enter);
    // Frame by frame, to look at the one that turned the page.
    for _ in 0..10 {
        h.run_steps(1);
        if rotation(&h) == 90 {
            break;
        }
    }
    assert_eq!(rotation(&h), 90, "the palette turned the page");
    for (k, v) in h.state().views.iter().enumerate() {
        assert_eq!(v.raster_memory().pending_requests, 0, "view {k} kept demand made for the page before it turned");
    }
    // And both panes draw the turned page.
    wait(&mut h, "both panes drawn again", |s| landscape(s, 0) && landscape(s, 1));
}
