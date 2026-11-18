//! Preferences rows apply on OK and change nothing on Cancel.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
// PLAN: assumed public `pdfcraft_ui_egui::{Mode, units::Unit}` and `craft::theme::selection::MotionChoice` for tests
use craft::theme::selection::MotionChoice;
use pdfcraft_ui_egui::units::Unit;
use pdfcraft_ui_egui::{Mode, PdfCraftApp};

const PDF: &[u8] = include_bytes!("../data/form.pdf");

type H = Harness<'static, PdfCraftApp>;

/// One row edit and the app state it must produce after OK; the same state must be absent after Cancel.
struct Case {
    name: &'static str,
    page: &'static str,
    edit: fn(&mut H),
    applied: fn(&PdfCraftApp) -> bool,
}

fn harness() -> H {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = crate::control::new_app();
        app.set_option(&egui::Context::default(), "language", "en").unwrap();
        app.open_bytes("form.pdf", None, PDF.to_vec()).unwrap();
        app.recent.extend((0..5).map(|n| pdfcraft_ui_egui::RecentFile { name: format!("{n}.pdf"), path: format!("/nowhere/{n}.pdf"), pages: 1, size: 0 }));
        app
    });
    h.run_steps(4);
    h
}

fn click(h: &mut H, label: &str) {
    h.get_by_label(label).click();
    h.run_steps(3);
}

/// Replaces the text of the field named `label` through the keyboard, as a user would.
fn type_into(h: &mut H, label: &str, text: &str) {
    h.get_by_label(label).click();
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run_steps(1);
    h.get_by_label(label).type_text(text);
    h.run_steps(3);
}

fn cases() -> Vec<Case> {
    vec![
        Case { name: "default mode", page: "Documents and view", edit: |h| click(h, "E-Sign"), applied: |a| a.default_mode == Mode::Sign },
        Case {
            name: "layout and continuous on the active view",
            page: "Documents and view",
            edit: |h| {
                click(h, "Two-page view");
                click(h, "Continuous scrolling");
            },
            applied: |a| {
                let v = &a.views[0].view;
                v.display == craft::pages::PageDisplay::TwoUp && v.continuous != crate::control::new_app().views.first().map_or(true, |_| true)
            },
        },
        Case { name: "units", page: "Documents and view", edit: |h| click(h, Unit::Millimetres.label()), applied: |a| a.units == Unit::Millimetres },
        Case {
            name: "tools side and rail side",
            page: "General & Interface",
            edit: |h| {
                // The tools row comes first, then the rail row; each has a Left and a Right option.
                h.get_all_by_label("Right").next().unwrap().click();
                h.run_steps(3);
                h.get_all_by_label("Left").nth(1).unwrap().click();
                h.run_steps(3);
            },
            applied: |a| a.side_panels.tools_on_right() && !a.side_panels.rail_on_right(),
        },
        Case { name: "animation effects", page: "General & Interface", edit: |h| click(h, "Off"), applied: |a| a.animation_effects == MotionChoice::Off },
        Case {
            name: "recent limit truncates",
            page: "General & Interface",
            edit: |h| type_into(h, "Recent files to keep", "2"),
            applied: |a| a.recent_limit == 2 && a.recent.len() == 2,
        },
        Case { name: "comment author", page: "Comment", edit: |h| type_into(h, "Name on new comments", "Ada"), applied: |a| a.comment_prefs.author == "Ada" },
        Case { name: "date format", page: "Fill & Sign", edit: |h| type_into(h, "Pattern", "dd.mm.yyyy"), applied: |a| a.session.date_format() == "dd.mm.yyyy" },
        Case {
            name: "javascript",
            page: "JavaScript",
            edit: |h| click(h, "Enable Acrobat JavaScript"),
            applied: |a| !a.session.javascript(),
        },
    ]
}

fn edited(case: &Case) -> H {
    let mut h = harness();
    h.state_mut().open_preferences();
    h.run_steps(4);
    click(&mut h, case.page);
    (case.edit)(&mut h);
    h
}

#[test]
fn preference_rows_apply_on_ok_and_nothing_changes_on_cancel() {
    for case in cases() {
        let mut h = edited(&case);
        assert!(!(case.applied)(h.state()), "{}: a row edits the draft, not the app", case.name);
        click(&mut h, "OK");
        h.run_steps(3);
        assert!(h.state().preferences.is_none(), "{}: OK closes the window", case.name);
        assert!((case.applied)(h.state()), "{}: OK applies the draft", case.name);

        let mut h = edited(&case);
        click(&mut h, "Cancel");
        h.run_steps(3);
        assert!(h.state().preferences.is_none(), "{}: Cancel closes the window", case.name);
        assert!(!(case.applied)(h.state()), "{}: Cancel leaves the app as it was", case.name);
    }
}
