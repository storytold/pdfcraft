//! Split view: an edit made after both views drew their frame (#3149) leaves neither showing the
//! old page.

use crate::support::AppHarness as _;
use craft::ui::primitives::textures::TextureState; // PLAN: assumed facade path of `TextureState`
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::support::inline_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "form.pdf", None, include_bytes!("../data/form.pdf").to_vec()).unwrap();
            app
        }
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

/// The size of view `i`'s first page picture once it is drawn; a stand-in for a page still loading
/// is never counted.
fn page_size(s: &PdfCraftApp, i: usize) -> Option<egui::Vec2> {
    match &s.views[i].session.placed.first()?.picture {
        TextureState::Ready(texture) => Some(texture.size_vec2()),
        _ => None,
    }
}

/// The palette runs its command after the page area was drawn, as a dialog's OK does: the page
/// turns after both views drew the old one. Both panes must end up drawing the turned page, never
/// the stale portrait one.
#[ignore = "known gap of the egui-to-craft port: see docs/port-known-gaps.md"]
#[test]
fn a_late_edit_turns_the_page_in_both_panes() {
    let mut h = harness();
    h.key_press_modifiers(Modifiers::COMMAND, Key::Backslash);
    h.run_steps(4);
    assert_eq!(h.state().views.len(), 2);
    assert_eq!(h.state().views[0].id, h.state().views[1].id, "one document in both panes");
    let portrait = |s: &PdfCraftApp, i: usize| page_size(s, i).is_some_and(|size| size.y > size.x);
    let landscape = |s: &PdfCraftApp, i: usize| page_size(s, i).is_some_and(|size| size.x > size.y);
    wait(&mut h, "both panes drawn", |s| portrait(s, 0) && portrait(s, 1));
    let id = h.state().views[0].id;
    let rotation = |h: &Harness<'static, PdfCraftApp>| h.state().session.get(id).unwrap().info.pages[0].rotation;
    h.set_option("palette", "rotate pages clockwise").unwrap();
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
    // Both panes draw the turned page.
    wait(&mut h, "both panes drawn again", |s| landscape(s, 0) && landscape(s, 1));
}
