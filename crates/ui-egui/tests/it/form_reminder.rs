//! Persistent dismissal of the ordinary AcroForm reminder (#2725).

use egui_kittest::{Harness, kittest::Queryable};
use pdfcraft_testkit::Pdf;
use pdfcraft_ui_egui::PdfCraftApp;

const REMINDER: &str = "This document contains 1 interactive form fields.";
const PREFERENCE: &str = "Show form field reminder";

/// Build one synthetic AcroForm text field for reminder-visibility regressions.
fn form() -> Vec<u8> {
    Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] >> >>")
        .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>")
        .obj("<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>")
        .obj("<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V (Synthetic) /Rect [50 300 250 330] /P 3 0 R >>")
        .build()
}

/// Restore optional saved preferences and open the synthetic document in a headless UI.
fn harness(saved: Option<String>, bytes: Vec<u8>) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1000.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        if let Some(saved) = saved {
            app.restore(&saved);
        }
        app.set_option("language", "en").unwrap();
        app.open_bytes("synthetic-form.pdf", None, bytes).unwrap();
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "none").unwrap();
        app
    });
    h.run_steps(6);
    h
}

/// Optional private visual checks use the real egui shell with only our synthetic form.
fn shot(h: &mut Harness<'static, PdfCraftApp>, name: &str) {
    if let Ok(dir) = std::env::var("PDFCRAFT_FORM_REMINDER_SHOT_DIR") {
        let path = std::path::Path::new(&dir).join(format!("{name}.png"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(std::path::Path::new(&dir).join("synthetic-form.pdf"), form()).unwrap();
        h.render().unwrap().save(path).unwrap();
    }
}

/// Closing the existing reminder applies to another document and a freshly restored app.
#[test]
fn dismissing_form_reminder_survives_new_documents_and_restart() {
    let mut h = harness(None, form());
    let id = h.state().views[0].id;
    let original = h.state().session.get(id).unwrap().bytes.clone();
    assert!(h.state().session.get(id).unwrap().repair_log().is_empty(), "use a valid fixture");
    h.get_by_label(REMINDER);
    let highlights = h.state().views[0].highlight_fields;
    h.get_by_label("Dismiss").click();
    h.run_steps(4);
    assert!(h.query_by_label(REMINDER).is_none());
    assert!(!h.state().views[0].notice_dismissed, "the general warning dismissal must remain session-local");
    assert!(!h.state().session.get(id).unwrap().dirty);
    assert_eq!(h.state().views[0].highlight_fields, highlights);
    assert_eq!(h.state().session.get(id).unwrap().bytes, original, "a UI preference must not change PDF bytes");
    let saved = h.state().persist();
    h.state_mut().open_bytes("another-form.pdf", None, form()).unwrap();
    h.run_steps(4);
    assert!(h.query_by_label(REMINDER).is_none(), "another form must keep the preference");
    let h = harness(Some(saved), form());
    assert!(h.query_by_label(REMINDER).is_none(), "the reminder must stay dismissed after restarting");
}

/// Preferences can restore a dismissed reminder immediately and keep the choice on restart.
#[test]
fn preferences_can_restore_the_form_reminder() {
    use egui::accesskit::Role;
    let mut h = harness(None, form());
    shot(&mut h, "reminder-on");
    h.get_by_label("Dismiss").click();
    h.run_steps(4);
    shot(&mut h, "reminder-dismissed");
    h.state_mut().set_option("dialog", "preferences").unwrap();
    h.run_steps(4);
    h.get_by_role_and_label(Role::CheckBox, PREFERENCE).scroll_to_me();
    h.run_steps(4);
    shot(&mut h, "preferences-reminder-off");
    h.get_by_role_and_label(Role::CheckBox, PREFERENCE).click();
    h.run_steps(4);
    h.get_by_label("OK").scroll_to_me();
    h.run_steps(4);
    h.get_by_label("OK").click();
    h.run_steps(4);
    assert!(h.state().dialog.is_none(), "Preferences must actually close before checking the restored reminder");
    h.get_by_label(REMINDER);
    shot(&mut h, "reminder-restored");
    let saved = h.state().persist();
    let h = harness(Some(saved), form());
    h.get_by_label(REMINDER);
}

/// Older or malformed settings retain the previous default, while a stored false is honoured.
#[test]
fn stored_form_reminder_choice_preserves_old_settings_defaults() {
    for (saved, expected) in [
        ("{}", true),
        (r#"{"show_form_field_reminder":null}"#, true),
        (r#"{"show_form_field_reminder":"off"}"#, true),
        (r#"{"show_form_field_reminder":0}"#, true),
        (r#"{"show_form_field_reminder":true}"#, true),
        (r#"{"show_form_field_reminder":false}"#, false),
    ] {
        let h = harness(Some(saved.to_string()), form());
        assert_eq!(h.query_by_label(REMINDER).is_some(), expected, "{saved}");
    }
}

/// Hiding a normal form reminder must leave damage warnings visible and session-local.
#[test]
fn a_hidden_form_reminder_does_not_hide_repair_warnings() {
    let mut bytes = form();
    let xref = bytes.windows(5).position(|w| w == b"xref\n").unwrap();
    bytes.truncate(xref);
    bytes.extend_from_slice(b"trailer << /Root 1 0 R >>\n%%EOF\n");
    let saved = r#"{"show_form_field_reminder":false}"#.to_string();
    let mut h = harness(Some(saved), bytes.clone());
    assert!(h.query_by_label(REMINDER).is_none());
    h.get_by_label("This file was damaged and has been repaired. Saving keeps the repaired version.");
    h.get_by_label("Dismiss").click();
    h.run_steps(4);
    let h = harness(Some(h.state().persist()), bytes);
    h.get_by_label("This file was damaged and has been repaired. Saving keeps the repaired version.");
}

/// Restoring the ordinary hint must neither be blocked by nor revive a dismissed warning.
#[test]
fn a_dismissed_repair_warning_does_not_block_restoring_the_form_reminder() {
    let mut bytes = form();
    let xref = bytes.windows(5).position(|w| w == b"xref\n").unwrap();
    bytes.truncate(xref);
    bytes.extend_from_slice(b"trailer << /Root 1 0 R >>\n%%EOF\n");
    let mut h = harness(Some(r#"{"show_form_field_reminder":false}"#.to_string()), bytes);
    let warning = "This file was damaged and has been repaired. Saving keeps the repaired version.";
    h.get_by_label(warning);
    h.get_by_label("Dismiss").click();
    h.run_steps(4);
    assert!(h.state().views[0].notice_dismissed);
    assert!(h.query_by_label(warning).is_none());
    shot(&mut h, "warning-dismissed");
    h.state_mut().set_option("form-reminder", "on").unwrap();
    h.run_steps(4);
    h.get_by_label(REMINDER);
    assert!(h.query_by_label(warning).is_none());
    shot(&mut h, "reminder-after-dismissed-warning");
    assert!(h.state().views[0].notice_dismissed, "do not clear the independent warning dismissal");
    h.get_by_label("Dismiss").click();
    h.run_steps(4);
    assert!(h.query_by_label(REMINDER).is_none());
    assert!(h.query_by_label(warning).is_none(), "the warning must stay dismissed after hiding the reminder again");
}

/// The persistent preference is independent of the document's security restrictions.
#[test]
fn a_hidden_form_reminder_does_not_hide_security_notices() {
    let mut doc = pdfcraft_cos::Document::open(std::sync::Arc::new(form())).unwrap();
    doc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes256,
        user_password: "",
        owner_password: "synthetic-owner",
        permissions: 0b0100,
        encrypt_metadata: true,
        seed: [7; 32],
    })
    .unwrap();
    let bytes = pdfcraft_cos::write_full(&doc, &Default::default()).unwrap();
    for show in [true, false] {
        let saved = serde_json::json!({"show_form_field_reminder":show}).to_string();
        let mut h = harness(Some(saved), bytes.clone());
        let id = h.state().views[0].id;
        h.get_by_label_contains("interactive form fields. This document is secured. Some changes are restricted by its security settings.");
        h.get_by_label("Security settings");
        let before = h.state().views[0].highlight_fields;
        let label = if before { "Hide field highlights" } else { "Highlight fields" };
        h.get_by_label(label).click();
        h.run_steps(4);
        assert_eq!(h.state().views[0].highlight_fields, !before);
        assert_eq!(h.state().session.get(id).unwrap().bytes.as_ref(), bytes.as_slice());
        assert!(!h.state().session.get(id).unwrap().dirty);
        if !show {
            shot(&mut h, "secured-form-with-reminder-off");
        }
        h.get_by_label("Dismiss").click();
        h.run_steps(4);
        assert!(h.query_by_label_contains("This document is secured").is_none());
        assert!(h.state().views[0].notice_dismissed);
        let saved = h.state().persist();
        assert_eq!(serde_json::from_str::<serde_json::Value>(&saved).unwrap()["show_form_field_reminder"], show);
        let h = harness(Some(saved), bytes.clone());
        h.get_by_label_contains("interactive form fields. This document is secured.");
        h.get_by_label("Security settings");
    }
}

/// XFA status and disabled-script warnings keep their existing dismissal behaviour.
#[test]
fn a_hidden_form_reminder_does_not_hide_xfa_notices() {
    let template = pdfcraft_xfa::fixtures::scripted_template();
    let h = harness(Some(r#"{"show_form_field_reminder":false}"#.to_string()), pdfcraft_xfa::fixtures::shell(&template));
    h.get_by_label_contains("Dynamic XFA form laid out from its template:");
    let runaway = "function f() { for (var j = 0; j &lt; 99999; j++) {} } for (var k = 0; k &lt; 5000; k++) f();";
    let template = template.replace("if (qty.rawValue === null) qty.rawValue = 2;", runaway);
    let mut h = harness(Some(r#"{"show_form_field_reminder":false}"#.to_string()), pdfcraft_xfa::fixtures::shell(&template));
    let id = h.state().views[0].id;
    assert!(h.state().session.get(id).unwrap().xfa_scripts_off());
    h.state_mut().toast = None;
    h.run_steps(4);
    h.get_by_label_contains("This form's scripts are turned off because one ran too long.");
    h.get_by_label("JavaScript Console");
}

/// The existing control channel changes and reports the same persistent preference.
#[test]
fn control_form_reminder_is_persistent_and_rejects_invalid_values() {
    use serde_json::json;
    let mut h = harness(None, form());
    let ctx = h.ctx.clone();
    let control = h.state_mut().attach_control(&ctx);
    let call = |h: &mut Harness<'static, PdfCraftApp>, method: &str, params| {
        let reply = control.send(method, params);
        for _ in 0..30 {
            h.step();
            if let Ok(result) = reply.try_recv() {
                return result;
            }
        }
        panic!("no control reply for {method}");
    };
    call(&mut h, "ui.set", json!({"key":"form-reminder","value":"off"})).unwrap();
    h.run_steps(4);
    assert!(h.query_by_label(REMINDER).is_none());
    assert_eq!(call(&mut h, "ui.state", json!({})).unwrap()["show_form_field_reminder"], false);
    assert!(call(&mut h, "ui.set", json!({"key":"form-reminder","value":"maybe"})).is_err());
    assert_eq!(call(&mut h, "ui.state", json!({})).unwrap()["show_form_field_reminder"], false);
    let saved = h.state().persist();
    let h = harness(Some(saved), form());
    assert!(h.query_by_label(REMINDER).is_none());
    let mut app = PdfCraftApp::new();
    app.set_option("form-reminder", "off").unwrap();
    assert_eq!(serde_json::from_str::<serde_json::Value>(&app.persist()).unwrap()["show_form_field_reminder"], false);
}
