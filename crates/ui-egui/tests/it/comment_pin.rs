//! The annotation quick bar's pin is a preference that survives a restart.

use egui::{Pos2, pos2};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use pdfcraft_ui_egui::settings::PdfCraftSettings;
use pdfcraft_ui_egui::{PdfCraftApp, QuickTool, comments::CommentTool};

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

/// Start a fresh shell with the settings a previous process saved.
fn harness(saved: Option<PdfCraftSettings>) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            if let Some(saved) = &saved {
                app.apply_settings(saved);
            }
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.set_option(&egui::Context::default(), "author", "Tester").unwrap();
            app.open_bytes(&egui::Context::default(), "pin.pdf", None, PDF.to_vec()).unwrap();
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "quick", "callout").unwrap();
            app
        }
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
    let saved = h.state().settings_snapshot();
    drop(h);

    let mut h = harness(Some(saved));
    assert!(h.state().comment_prefs.pinned, "the pin survives a fresh shell");
    assert_eq!(
        h.get_by_role_and_label(egui::accesskit::Role::Button, "Keep tool selected").accesskit_node().toggled(),
        Some(egui::accesskit::Toggled::True),
        "the restored pin button is toggled on"
    );
    shot(&mut h, "pin-restored.png");
    callout(&mut h, 140.0);
    crate::support::wait_until(&mut h, "the pinned callout", |app| app.session.get(app.views[0].id).unwrap().info.annotations.len() == 1);
    assert!(matches!(h.state().quick_tool, QuickTool::Comment(CommentTool::Callout)), "the restored pin keeps the tool selected after use");

    h.get_by_role_and_label(egui::accesskit::Role::Button, "Keep tool selected").click();
    h.run_steps(3);
    assert!(!h.state().comment_prefs.pinned);
    assert_eq!(
        h.get_by_role_and_label(egui::accesskit::Role::Button, "Keep tool selected").accesskit_node().toggled(),
        None,
        "unpinning toggles the pin button off"
    );
    shot(&mut h, "pin-disabled.png");
    let saved = h.state().settings_snapshot();
    callout(&mut h, 60.0);
    crate::support::wait_until(&mut h, "the unpinned callout returning to Select", |app| {
        app.session.get(app.views[0].id).unwrap().info.annotations.len() == 2 && matches!(app.quick_tool, QuickTool::Select)
    });
    assert!(!matches!(h.state().quick_tool, QuickTool::Comment(_)), "unpinning restores one-shot use");
    drop(h);

    let h = harness(Some(saved));
    assert!(!h.state().comment_prefs.pinned, "unpinning survives the next restart too");
}
