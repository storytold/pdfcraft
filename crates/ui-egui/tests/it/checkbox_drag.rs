//! A press on a checkbox and a drag over the ones below it sets them all (#1662), here in
//! Preferences, whose settings scroll.

use egui::{Pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

const BOXES: [&str; 4] = [
    "Reopen the files that were open when PdfCraft last closed",
    "Restore the last view (page, zoom and page display) when a document is opened again",
    "Reload documents when their files change on disk",
    "Show a notice when a document is secured",
];

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1000.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.set_option("dialog", "preferences").unwrap();
        app
    });
    h.run_steps(6);
    h
}

/// The four settings the boxes in [`BOXES`] stand for, in that order.
fn values(app: &PdfCraftApp) -> [bool; 4] {
    [app.reopen_last_session, app.restore_last_view, app.reload_changed_files, app.show_secured_notice]
}

/// The middle of a checkbox's box, at the start of its row.
fn box_of(h: &Harness<'static, PdfCraftApp>, label: &str) -> Pos2 {
    h.get_by_label(label).rect().left_center() + vec2(7.0, 0.0)
}

/// Press, move in steps and release, as a user drags.
fn drag(h: &mut Harness<'static, PdfCraftApp>, from: Pos2, to: Pos2) {
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    for k in 1..=8 {
        h.hover_at(from + (to - from) * (k as f32 / 8.0));
        h.run_steps(1);
    }
    h.drop_at(to);
    h.run_steps(3);
}

#[test]
fn a_drag_down_the_preferences_checkboxes_sets_them_all_and_a_drag_back_up_clears_them() {
    let mut h = harness();
    {
        let app = h.state_mut();
        (app.reopen_last_session, app.restore_last_view, app.reload_changed_files, app.show_secured_notice) = (false, true, false, false);
    }
    h.run_steps(2);
    // They start out below the visible settings; a box out of view isn't set by a drag.
    h.get_by_label(BOXES[3]).scroll_to_me();
    h.run_steps(4);
    let (first, last) = (box_of(&h, BOXES[0]), box_of(&h, BOXES[3]));
    let ok = h.get_by_label("OK").rect();
    assert!(last.y < ok.min.y, "the boxes are in view, above the OK row: {last:?} vs {ok:?}");
    drag(&mut h, first, last);
    assert_eq!(values(h.state()), [true; 4], "one drag ticked every box it went over");
    assert_eq!(box_of(&h, BOXES[0]), first, "the settings didn't scroll under the drag");
    drag(&mut h, last, first);
    assert_eq!(values(h.state()), [false; 4], "a drag back up from a ticked box cleared them");
    // A click still toggles one.
    h.get_by_label(BOXES[2]).click();
    h.run_steps(2);
    assert_eq!(values(h.state()), [false, false, true, false]);
}
