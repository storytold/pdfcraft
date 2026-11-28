//! Preferences ▸ Date format: the pattern Fill & Sign dates use.

use crate::support::AppHarness as _;
use egui::accesskit::Role;
use egui::{Key, Modifiers, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;
use pdfcraft_ui_egui::settings::import_json;

const PDF: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// Some widget shows `text`, as its label or its value (a combo box shows its choice as a value).
fn shows(h: &Harness<'static, PdfCraftApp>, text: &str) -> bool {
    h.query_all_by(|n| n.label().is_some_and(|l| l.contains(text)) || n.value().is_some_and(|v| v.contains(text))).next().is_some()
}

/// What the Pattern box holds, valid or not.
fn pattern(h: &Harness<'static, PdfCraftApp>) -> Option<String> {
    h.get_by_role_and_label(Role::TextInput, "Pattern").value()
}

/// Opens the Preferences page `page`: its entry in the dialog's list, the first thing the dialog names so (the
/// window's own "Fill & Sign" names another thing, and the open page its heading).
fn preferences_page(h: &mut Harness<'static, PdfCraftApp>, page: &str) {
    h.get_by_role(Role::Dialog).query_all_by_label(page).next().expect("the page's entry").click();
    h.run_steps(3);
}

fn retype(h: &mut Harness<'static, PdfCraftApp>, text: &str) {
    // The preferences scroll; bring the field into view before clicking it.
    h.get_by_role_and_label(Role::TextInput, "Pattern").scroll_to_me();
    h.run_steps(4);
    h.get_by_role_and_label(Role::TextInput, "Pattern").click();
    h.run_steps(1);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.get_by_role_and_label(Role::TextInput, "Pattern").type_text(text);
    h.run_steps(2);
}

#[test]
fn fill_and_sign_dates_follow_the_date_format_preference_and_it_is_remembered() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "form.pdf", None, PDF.to_vec()).expect("opens");
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "dialog", "preferences").unwrap();
            app
        }
    });
    h.run_steps(4);
    assert_eq!(h.state().session.date_format(), "m/d/yyyy");
    // The window applies valid changes live and opens again for the next step.
    fn reopen(h: &mut Harness<'static, PdfCraftApp>) {
        h.state_mut().open_preferences();
        h.run_steps(4);
        preferences_page(h, "Fill & Sign");
    }
    fn ok(h: &mut Harness<'static, PdfCraftApp>) {
        h.get_by_label("OK").click();
        h.run_steps(3);
    }
    preferences_page(&mut h, "Fill & Sign");
    // A valid pattern shows a preview of today and applies while it is typed.
    retype(&mut h, "d \\de mmmm yyyy");
    assert_eq!(h.state().session.date_format(), "d \\de mmmm yyyy", "typing applies the pattern");
    h.get_by_label_contains("Today: ");
    ok(&mut h);
    assert_eq!(h.state().session.date_format(), "d \\de mmmm yyyy");
    reopen(&mut h);
    // An invalid one stays in the box with the reason, and the last valid one is kept.
    retype(&mut h, "dd/mm HH");
    assert!(h.query_all_by_label_contains("is a time letter (H h M s t)").next().is_some());
    assert_eq!(h.state().session.date_format(), "d \\de mmmm yyyy");
    assert_eq!(pattern(&h).as_deref(), Some("dd/mm HH"));
    h.set_option("date-language", "es").unwrap();
    h.run_steps(2);
    assert_eq!(h.state().session.date_language(), Some("es"));
    assert_eq!(pattern(&h).as_deref(), Some("dd/mm HH"), "changing the language must not discard the invalid draft pattern");
    h.set_option("date-language", "auto").unwrap();
    // Typed a key at a time, spaces stay where they are typed (the box isn't trimmed under the cursor).
    h.get_by_role_and_label(Role::TextInput, "Pattern").click();
    h.run_steps(1);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    for c in "yyyy mm dd".chars() {
        h.get_by_role_and_label(Role::TextInput, "Pattern").type_text(&c.to_string());
        h.run_steps(1);
    }
    assert_eq!(pattern(&h).as_deref(), Some("yyyy mm dd"));
    ok(&mut h);
    assert_eq!(h.state().session.date_format(), "yyyy mm dd");
    reopen(&mut h);
    // A letter group that isn't a date code is refused, not silently shortened.
    retype(&mut h, "yyyyyyyy");
    assert!(h.query_all_by_label_contains("`yyyyyyyy` is not a date code").next().is_some());
    assert_eq!(h.state().session.date_format(), "yyyy mm dd");
    retype(&mut h, "yyyy.mm.dd.");
    ok(&mut h);
    assert_eq!(h.state().session.date_format(), "yyyy.mm.dd.");

    // Fill & Sign ▸ Date stamps today in it.
    h.set_option("dialog", "none").unwrap();
    h.run_steps(2);
    assert!(h.execute("sign.fill.date"));
    h.run_steps(2);
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    let p = pos2(r.left() + r.width() * 0.2, r.top() + r.height() * 0.2);
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(4);
    let expected = h.state().date_text(None).unwrap();
    let s = h.state();
    let dates: Vec<_> = s.session.get(s.views[0].id).unwrap().info.annotations.iter().filter_map(|a| a.contents.clone()).collect();
    assert_eq!(dates, std::slice::from_ref(&expected));
    assert!(expected.ends_with('.') && expected.matches('.').count() == 3, "{expected}");

    // It survives a restart; an unusable saved pattern keeps the default; the control channel checks it too.
    let mut restored = crate::control::new_app();
    restored.apply_settings(&h.state().settings_snapshot());
    assert_eq!(restored.session.date_format(), "yyyy.mm.dd.");
    let mut fresh = crate::control::new_app();
    fresh.apply_settings(&import_json(&format!(r#"{{"date_format": "{}"}}"#, "d".repeat(10_000))).0);
    fresh.apply_settings(&import_json(r#"{"date_format": "HH:MM"}"#).0);
    assert_eq!(fresh.session.date_format(), "m/d/yyyy");
    assert!(fresh.set_option(&egui::Context::default(), "date-format", "Year").is_err());
    fresh.set_option(&egui::Context::default(), "date-format", "dd/mm/yyyy").unwrap();
    assert_eq!(fresh.session.date_format(), "dd/mm/yyyy");
}

#[test]
fn month_names_follow_the_date_language_or_else_the_interface_language() {
    if crate::support::skip_without_catalogs("month_names_follow_the_date_language_or_else_the_interface_language") {
        return;
    }
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.set_option(&egui::Context::default(), "date-format", "d mmmm yyyy").unwrap();
            app.set_option(&egui::Context::default(), "language", "cs").unwrap();
            app.set_option(&egui::Context::default(), "dialog", "preferences").unwrap();
            app
        }
    });
    h.run_steps(3);
    let text = |h: &Harness<'static, PdfCraftApp>| h.state().date_text(None).unwrap();
    assert_eq!(text(&h), h.state().session.today_text(None, Some("cs")).unwrap(), "follows the interface (Czech)");
    // The picker offers following the interface first, then the interface languages.
    assert!(shows(&h, "Stejný jako rozhraní (Čeština)"));
    h.set_option("date-language", "es").unwrap();
    assert_eq!(h.state().session.date_language(), Some("es"));
    h.run_steps(2);
    assert_eq!(h.state().session.date_language(), Some("es"), "open live Preferences must not reapply the previous language");
    assert_eq!(text(&h), h.state().session.today_text(None, Some("es")).unwrap());
    assert!(shows(&h, "Español"));
    h.set_option("date-format", "dd mmmm yyyy").unwrap();
    h.run_steps(2);
    assert_eq!(h.state().session.date_format(), "dd mmmm yyyy");
    assert_eq!(h.state().session.date_language(), Some("es"), "changing the format leaves the chosen language alone");
    h.set_option("date-format", "d mmmm yyyy").unwrap();

    // Remembered; an unknown saved language is ignored; auto follows the interface again.
    let mut restored = crate::control::new_app();
    restored.apply_settings(&h.state().settings_snapshot());
    assert_eq!(restored.session.date_language(), Some("es"));
    let mut fresh = crate::control::new_app();
    fresh.apply_settings(&import_json(r#"{"date_language": "klingon"}"#).0);
    assert_eq!(fresh.session.date_language(), None);
    assert!(fresh.set_option(&egui::Context::default(), "date-language", "klingon").is_err());
    h.set_option("date-language", "auto").unwrap();
    assert_eq!(h.state().session.date_language(), None);
}

#[test]
fn a_date_the_pdf_cannot_hold_is_refused_with_a_warning_first() {
    if crate::support::skip_without_glyphs(
        "a_date_the_pdf_cannot_hold_is_refused_with_a_warning_first",
        "en",
        "日曜日月曜日火曜日水曜日木曜日金曜日土曜日",
    ) {
        return;
    }
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "form.pdf", None, PDF.to_vec()).expect("opens");
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            // Japanese weekday names: Fill & Sign text can't hold them yet.
            app.set_option(&egui::Context::default(), "date-format", "dddd").unwrap();
            app.set_option(&egui::Context::default(), "date-language", "ja").unwrap();
            app.set_option(&egui::Context::default(), "dialog", "preferences").unwrap();
            app
        }
    });
    h.run_steps(3);
    h.get_by_label_contains("can't be written into the PDF yet");

    h.set_option("dialog", "none").unwrap();
    h.run_steps(2);
    assert!(h.query_by_label_contains("can't be written into the PDF yet").is_none(), "Preferences was closed");
    assert!(h.execute("sign.fill.date"));
    h.run_steps(2);
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    let p = pos2(r.left() + r.width() * 0.2, r.top() + r.height() * 0.2);
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(4);
    {
        let s = h.state();
        assert!(s.session.get(s.views[0].id).unwrap().info.annotations.is_empty(), "nothing placed");
    }
    let notice = crate::support::shown_notice(&h).unwrap_or_default();
    assert!(notice.contains("can't be written into the PDF yet"), "{notice}");
}
