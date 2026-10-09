//! Preferences ▸ Date format: the pattern Fill & Sign dates use.

use egui::accesskit::Role;
use egui::{Key, Modifiers, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

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

fn retype(h: &mut Harness<'static, PdfCraftApp>, text: &str) {
    h.get_by_role_and_label(Role::TextInput, "Pattern").click();
    h.run_steps(1);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.get_by_role_and_label(Role::TextInput, "Pattern").type_text(text);
    h.run_steps(2);
}

#[test]
fn fill_and_sign_dates_follow_the_date_format_preference_and_it_is_remembered() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("form.pdf", None, PDF.to_vec()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("dialog", "preferences").unwrap();
        app
    });
    h.run_steps(4);
    assert_eq!(h.state().session.date_format(), "m/d/yyyy");
    // A valid pattern applies as it's typed, with a preview of today.
    retype(&mut h, "d \\de mmmm yyyy");
    assert_eq!(h.state().session.date_format(), "d \\de mmmm yyyy");
    h.get_by_label_contains("Today: ");
    // An invalid one stays in the box with the reason, and the last valid one is kept.
    retype(&mut h, "dd/mm HH");
    h.get_by_label_contains("is a time letter (H h M s t)");
    assert_eq!(h.state().session.date_format(), "d \\de mmmm yyyy");
    assert_eq!(h.state().date_format_draft.as_deref(), Some("dd/mm HH"));
    // Typed a key at a time, spaces stay where they are typed (the box isn't trimmed under the cursor).
    h.get_by_role_and_label(Role::TextInput, "Pattern").click();
    h.run_steps(1);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    for c in "yyyy mm dd".chars() {
        h.get_by_role_and_label(Role::TextInput, "Pattern").type_text(&c.to_string());
        h.run_steps(1);
    }
    assert_eq!(h.state().date_format_draft.as_deref(), Some("yyyy mm dd"));
    assert_eq!(h.state().session.date_format(), "yyyy mm dd");
    // A letter group that isn't a date code is refused, not silently shortened.
    retype(&mut h, "yyyyyyyy");
    h.get_by_label_contains("`yyyyyyyy` is not a date code");
    assert_eq!(h.state().session.date_format(), "yyyy mm dd");
    retype(&mut h, "yyyy.mm.dd.");
    assert_eq!(h.state().session.date_format(), "yyyy.mm.dd.");

    // Fill & Sign ▸ Date stamps today in it.
    h.state_mut().set_option("dialog", "none").unwrap();
    h.run_steps(2);
    assert!(h.state_mut().execute("sign.fill.date"));
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
    let mut restored = PdfCraftApp::new();
    restored.restore(&h.state().persist());
    assert_eq!(restored.session.date_format(), "yyyy.mm.dd.");
    let mut fresh = PdfCraftApp::new();
    fresh.restore(&format!(r#"{{"date_format": "{}"}}"#, "d".repeat(10_000)));
    fresh.restore(r#"{"date_format": "HH:MM"}"#);
    assert_eq!(fresh.session.date_format(), "m/d/yyyy");
    assert!(fresh.set_option("date-format", "Year").is_err());
    fresh.set_option("date-format", "dd/mm/yyyy").unwrap();
    assert_eq!(fresh.session.date_format(), "dd/mm/yyyy");
}

#[test]
fn month_names_follow_the_date_language_or_else_the_interface_language() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("date-format", "d mmmm yyyy").unwrap();
        app.set_option("language", "cs").unwrap();
        app.set_option("dialog", "preferences").unwrap();
        app
    });
    h.run_steps(3);
    let text = |h: &Harness<'static, PdfCraftApp>| h.state().date_text(None).unwrap();
    assert_eq!(text(&h), h.state().session.today_text(None, Some("cs")).unwrap(), "follows the interface (Czech)");
    // The picker offers following the interface first, then the interface languages.
    assert!(shows(&h, "Same as interface (Čeština)"));
    h.state_mut().set_option("date-language", "es").unwrap();
    h.run_steps(2);
    assert_eq!(text(&h), h.state().session.today_text(None, Some("es")).unwrap());
    assert!(shows(&h, "Español"));

    // Remembered; an unknown saved language is ignored; auto follows the interface again.
    let mut restored = PdfCraftApp::new();
    restored.restore(&h.state().persist());
    assert_eq!(restored.session.date_language(), Some("es"));
    let mut fresh = PdfCraftApp::new();
    fresh.restore(r#"{"date_language": "klingon"}"#);
    assert_eq!(fresh.session.date_language(), None);
    assert!(fresh.set_option("date-language", "klingon").is_err());
    h.state_mut().set_option("date-language", "auto").unwrap();
    assert_eq!(h.state().session.date_language(), None);
}

#[test]
fn a_date_the_pdf_cannot_hold_is_refused_with_a_warning_first() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("form.pdf", None, PDF.to_vec()).expect("opens");
        app.set_option("left", "closed").unwrap();
        // Japanese weekday names: Fill & Sign text can't hold them yet.
        app.set_option("date-format", "dddd").unwrap();
        app.set_option("date-language", "ja").unwrap();
        app.set_option("dialog", "preferences").unwrap();
        app
    });
    h.run_steps(3);
    h.get_by_label_contains("can't be written into the PDF yet");

    h.state_mut().set_option("dialog", "none").unwrap();
    h.run_steps(2);
    assert!(h.state_mut().execute("sign.fill.date"));
    h.run_steps(2);
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    let p = pos2(r.left() + r.width() * 0.2, r.top() + r.height() * 0.2);
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(4);
    let s = h.state();
    assert!(s.session.get(s.views[0].id).unwrap().info.annotations.is_empty(), "nothing placed");
    let toast = s.toast.as_ref().map(|t| t.0.clone()).unwrap_or_default();
    assert!(toast.contains("can't be written into the PDF yet"), "{toast}");
}
