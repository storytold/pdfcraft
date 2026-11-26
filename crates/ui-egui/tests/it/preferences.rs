//! Preferences rows apply as they change and persist when the window closes.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
// PLAN: assumed public `pdfcraft_ui_egui::{Mode, units::Unit}` and `craft::theme::selection::MotionChoice` for tests
use craft::theme::selection::MotionChoice;
use pdfcraft_ui_egui::units::Unit;
use pdfcraft_ui_egui::{Mode, PdfCraftApp, side_panels};

const PDF: &[u8] = include_bytes!("../data/form.pdf");

type H = Harness<'static, PdfCraftApp>;

/// One row edit and the app state it must produce immediately.
struct Case {
    name: &'static str,
    page: &'static str,
    edit: fn(&mut H),
    applied: fn(&PdfCraftApp) -> bool,
}

fn harness() -> H {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "form.pdf", None, PDF.to_vec()).unwrap();
            app.recent.extend((0..5).map(|n| pdfcraft_ui_egui::RecentFile {
                name: format!("{n}.pdf"),
                path: format!("/nowhere/{n}.pdf"),
                pages: 1,
                size: 0,
            }));
            app
        }
    });
    h.run_steps(4);
    h
}

/// Whether the Preferences window is on screen: its search field shows while it is open.
fn preferences_shown(h: &H) -> bool {
    h.query_by_role_and_label(egui::accesskit::Role::TextInput, "Search settings").is_some()
}

fn click(h: &mut H, label: &str) {
    // Preferences is one scrolling page: bring the row into view first.
    h.get_by(|node| !node.is_hidden() && node.role() != egui::accesskit::Role::Label && node.label().as_deref() == Some(label)).scroll_to_me();
    h.run_steps(3);
    h.get_by(|node| !node.is_hidden() && node.role() != egui::accesskit::Role::Label && node.label().as_deref() == Some(label)).click();
    h.run_steps(3);
}

/// Picks `option` of the choice row `row`: a short choice is a row of segments (a radio group), a long one a
/// drop-down whose options show once it opens.
fn choose(h: &mut H, row: &str, option: &str) {
    use egui::accesskit::Role;
    if h.query_by_role_and_label(Role::RadioGroup, row).is_some() {
        h.get_by_role_and_label(Role::RadioGroup, row).scroll_to_me();
        h.run_steps(3);
        h.get_by_role_and_label(Role::RadioGroup, row).get_by_label(option).click();
        h.run_steps(3);
    } else {
        click(h, row);
        click(h, option);
    }
}

/// Replaces the text of the field named `label` through the keyboard, as a user would.
fn type_into(h: &mut H, label: &str, text: &str) {
    let role = if label == "Recent files to keep" { egui::accesskit::Role::SpinButton } else { egui::accesskit::Role::TextInput };
    h.get_by_role_and_label(role, label).scroll_to_me();
    h.run_steps(3);
    h.get_by_role_and_label(role, label).click();
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run_steps(1);
    h.get_by_role_and_label(role, label).type_text(text);
    h.run_steps(3);
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "default mode",
            page: "Documents and view",
            edit: |h| {
                // A choice row is a drop-down: its options show once it opens.
                choose(h, "Default workspace mode", "E-Sign");
            },
            applied: |a| a.default_mode == Mode::Sign,
        },
        Case {
            name: "layout and continuous on the active view",
            page: "Documents and view",
            edit: |h| {
                // A choice row is a drop-down: its options show once it opens.
                choose(h, "Default page display", "Two-page view");
                click(h, "Continuous scrolling");
            },
            applied: |a| {
                let v = &a.views[0].view;
                v.display == craft::pages::PageDisplay::TwoUp && v.continuous != crate::control::new_app().views.first().is_none_or(|_| true)
            },
        },
        Case {
            name: "units",
            page: "Documents and view",
            edit: |h| {
                choose(h, "Units", Unit::Millimetres.label());
            },
            applied: |a| a.units == Unit::Millimetres,
        },
        Case {
            name: "tools side and rail side",
            page: "General & Interface",
            edit: |h| {
                // Each side row is a drop-down of Left and Right.
                choose(h, "Tools panel side", "Right");
                choose(h, "Navigation rail side", "Left");
            },
            applied: |a| {
                let dock = side_panels::state(a);
                dock["tools_side"] == "right" && dock["rail_side"] == "left"
            },
        },
        Case {
            name: "animation effects",
            page: "General & Interface",
            edit: |h| {
                choose(h, "Animation effects", "Off");
            },
            applied: |a| a.animation_effects == MotionChoice::Off,
        },
        Case {
            name: "recent limit truncates",
            page: "General & Interface",
            edit: |h| type_into(h, "Recent files to keep", "2"),
            applied: |a| a.recent_limit == 2 && a.recent.len() == 2,
        },
        Case {
            name: "comment author",
            page: "Comment",
            edit: |h| type_into(h, "Name on new comments", "Ada"),
            applied: |a| a.comment_prefs.author == "Ada",
        },
        Case {
            name: "date format",
            page: "Fill & Sign",
            edit: |h| type_into(h, "Pattern", "dd.mm.yyyy"),
            applied: |a| a.session.date_format() == "dd.mm.yyyy",
        },
        Case { name: "javascript", page: "JavaScript", edit: |h| click(h, "Enable Acrobat JavaScript"), applied: |a| !a.session.javascript() },
    ]
}

fn edited(case: &Case) -> H {
    let mut h = harness();
    h.state_mut().open_preferences();
    h.run_steps(4);
    (case.edit)(&mut h);
    h
}

#[test]
fn preference_rows_apply_live_and_survive_closing() {
    for case in cases() {
        let mut h = edited(&case);
        assert!((case.applied)(h.state()), "{} ({}): a row applies immediately", case.name, case.page);
        click(&mut h, "OK");
        h.run_steps(3);
        assert!(!preferences_shown(&h), "{}: OK closes the window", case.name);
        assert!((case.applied)(h.state()), "{}: OK keeps the change", case.name);

        let mut h = edited(&case);
        h.key_press(egui::Key::Escape);
        h.run_steps(3);
        assert!(!preferences_shown(&h), "{}: Escape closes the window", case.name);
        assert!((case.applied)(h.state()), "{}: Escape keeps the change", case.name);
    }
}
