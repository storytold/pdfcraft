//! The annotation quick bar's pin is a preference, including across older settings files.

use egui::{Pos2, pos2};
use egui_kittest::{Harness, kittest::Queryable};
use pdfcraft_ui_egui::{PdfCraftApp, QuickTool, comments::CommentTool};
use serde_json::{Value, json};

// Contributor-original blank page: no document assets or fonts are needed.
const PDF: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] >> endobj
xref
0 4
0000000000 65535 f\r
0000000009 00000 n\r
0000000058 00000 n\r
0000000115 00000 n\r
trailer << /Size 4 /Root 1 0 R >>
startxref
186
%%EOF";

/// Start a fresh shell with the state a previous process saved.
fn harness(saved: Option<String>) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        if let Some(saved) = saved {
            app.restore(&saved);
        }
        app.set_option("language", "en").unwrap();
        app.set_option("author", "Tester").unwrap();
        app.open_bytes("pin.pdf", None, PDF.to_vec()).unwrap();
        app.set_option("left", "closed").unwrap();
        app.set_option("quick", "callout").unwrap();
        app
    });
    crate::support::wait_until(&mut h, "the blank page", |app| app.views[0].page_screen_rect(0).is_some() && !app.render_pending());
    h
}

/// Place and post a callout at known PDF coordinates, independently of panel and zoom changes.
fn callout(h: &mut Harness<'static, PdfCraftApp>, y: f32) {
    let r = h.state().views[0].page_screen_rect(0).unwrap();
    let at = |x: f32| -> Pos2 { pos2(r.left() + x / 300.0 * r.width(), r.top() + (200.0 - y) / 200.0 * r.height()) };
    h.hover_at(at(40.0));
    h.run_steps(1);
    h.drag_at(at(40.0));
    h.run_steps(1);
    h.hover_at(at(140.0));
    h.run_steps(1);
    h.drop_at(at(140.0));
    h.run_steps(3);
    h.query_all_by(|node| node.placeholder() == Some("Type text")).next().unwrap().type_text("Remember this");
    h.run_steps(2);
    h.get_by_label("Post").click();
    h.run_steps(3);
}

/// Optional private visual evidence, kept outside the repository by the test runner.
fn shot(h: &mut Harness<'static, PdfCraftApp>, name: &str) {
    if let Ok(dir) = std::env::var("PDFCRAFT_PIN_SHOTS") {
        h.render().unwrap().save(std::path::Path::new(&dir).join(name)).unwrap();
    }
}

#[test]
fn pin_from_quick_bar_survives_restart_and_unpinning() {
    let _gpu = crate::gpu();
    let mut h = harness(None);
    h.get_by_label("Keep tool selected").click();
    h.run_steps(3);
    assert!(h.state().comment_prefs.pinned);
    let doc = h.state().session.get(h.state().views[0].id).unwrap();
    assert!(doc.repair_log().is_empty(), "the synthetic page opens without repair");
    assert_eq!(doc.bytes.as_slice(), PDF, "changing this preference leaves the PDF unchanged");
    assert!(!doc.dirty);
    let saved = h.state().persist();
    drop(h);

    let mut h = harness(Some(saved));
    assert!(h.state().comment_prefs.pinned, "the pin survives a fresh shell");
    h.get_by_label("Keep tool selected: on");
    shot(&mut h, "pin-restored.png");
    callout(&mut h, 140.0);
    crate::support::wait_until(&mut h, "the pinned callout", |app| app.session.get(app.views[0].id).unwrap().info.annotations.len() == 1);
    assert!(matches!(h.state().quick_tool, QuickTool::Comment(CommentTool::Callout)), "the restored pin keeps the tool selected after use");

    h.get_by_label("Keep tool selected: on").click();
    h.run_steps(3);
    assert!(!h.state().comment_prefs.pinned);
    shot(&mut h, "pin-disabled.png");
    let saved = h.state().persist();
    callout(&mut h, 60.0);
    crate::support::wait_until(&mut h, "the unpinned callout returning to Select", |app| {
        app.session.get(app.views[0].id).unwrap().info.annotations.len() == 2 && matches!(app.quick_tool, QuickTool::Select)
    });
    assert!(!matches!(h.state().quick_tool, QuickTool::Comment(_)), "unpinning restores one-shot use");

    let mut restored = PdfCraftApp::new();
    restored.comment_prefs.pinned = true;
    restored.restore(&saved);
    assert!(!restored.comment_prefs.pinned, "unpinning survives the next restart too");
}

#[test]
fn saved_pin_round_trips_both_boolean_values() {
    for pinned in [true, false] {
        let mut app = PdfCraftApp::new();
        app.comment_prefs.pinned = pinned;
        let saved = app.persist();
        let value: Value = serde_json::from_str(&saved).unwrap();
        assert_eq!(value["comment_tool_pinned"], json!(pinned));
        let mut restored = PdfCraftApp::new();
        restored.comment_prefs.pinned = !pinned;
        restored.restore(&saved);
        assert_eq!(restored.comment_prefs.pinned, pinned);
        assert!(matches!(restored.quick_tool, QuickTool::Select), "remembering the pin does not select a drawing tool at startup");
    }
}

#[test]
fn restoring_explicit_pin_choice_overrides_current_choice() {
    for pinned in [true, false] {
        let mut app = PdfCraftApp::new();
        app.comment_prefs.pinned = !pinned;
        app.restore(&json!({"comment_tool_pinned": pinned}).to_string());
        assert_eq!(app.comment_prefs.pinned, pinned);
    }
}

#[test]
fn legacy_settings_keep_the_default_and_other_preferences() {
    let mut app = PdfCraftApp::new();
    assert!(!app.comment_prefs.pinned);
    let legacy = r#"{"author":"Legacy reviewer","comment_styles":[{"tool":"comment.line","color":[0.25,0.5,1.0]}]}"#;
    app.restore(legacy);
    assert!(!app.comment_prefs.pinned, "old settings retain the default one-shot behavior");
    assert_eq!(app.comment_prefs.author, "Legacy reviewer");
    assert_eq!(app.comment_prefs.style(CommentTool::Line).color, [0.25, 0.5, 1.0]);
    app.comment_prefs.pinned = true;
    app.restore(legacy);
    assert!(app.comment_prefs.pinned, "a partial restore without this key leaves the current choice alone");
}

#[test]
fn malformed_pin_values_preserve_the_current_choice() {
    for pinned in [false, true] {
        let mut app = PdfCraftApp::new();
        app.comment_prefs.pinned = pinned;
        for value in [json!(null), json!(0), json!(1), json!(-1), json!("true"), json!("false"), json!([]), json!({})] {
            app.restore(&json!({"comment_tool_pinned": value, "author": "Still restored"}).to_string());
            assert_eq!(app.comment_prefs.pinned, pinned, "invalid pin value: {value}");
            assert_eq!(app.comment_prefs.author, "Still restored", "other valid preferences still restore");
        }
        for invalid in ["", "{", r#"{"comment_tool_pinned":true"#, "[]", "null", "false", "0"] {
            app.restore(invalid);
            assert_eq!(app.comment_prefs.pinned, pinned);
        }
    }
}
