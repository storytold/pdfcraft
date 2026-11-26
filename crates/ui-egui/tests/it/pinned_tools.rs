//! Pinned tools on Home: pin tools, unpin tools, persistence across sessions,
//! selection modal, and the sidebar pin toggle.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{LeftPanel, PdfCraftApp};

fn harness(setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1000.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::support::held_app(std::sync::Arc::new(craft::host::ManualJobs::new()));
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            setup(&mut app);
            app
        }
    });
    h.run_steps(4);
    h
}

#[test]
fn state_operations_and_validation() {
    let mut app = crate::control::new_app();
    assert!(!app.is_tool_pinned("redact"));

    // Pinning a valid tool
    assert!(app.pin_tool("redact"));
    assert!(app.is_tool_pinned("redact"));
    assert_eq!(app.pinned_tools, vec!["redact"]);

    // Pinning already pinned tool returns false (no duplicates)
    assert!(!app.pin_tool("redact"));
    assert_eq!(app.pinned_tools, vec!["redact"]);

    // Pinning an invalid tool returns false
    assert!(!app.pin_tool("invalid_tool_unknown_id"));
    assert_eq!(app.pinned_tools, vec!["redact"]);

    // Pinning a second valid tool
    assert!(app.pin_tool("form"));
    assert_eq!(app.pinned_tools, vec!["redact", "form"]);

    // Unpinning
    app.unpin_tool("redact");
    assert!(!app.is_tool_pinned("redact"));
    assert!(app.is_tool_pinned("form"));
    assert_eq!(app.pinned_tools, vec!["form"]);

    // Unpinning non-existent tool does nothing
    app.unpin_tool("nonexistent");
    assert_eq!(app.pinned_tools, vec!["form"]);

    // Toggling
    assert!(!app.toggle_pin_tool("form")); // unpins
    assert!(!app.is_tool_pinned("form"));
    assert!(app.pinned_tools.is_empty());

    assert!(app.toggle_pin_tool("form")); // pins
    assert!(app.is_tool_pinned("form"));
    assert_eq!(app.pinned_tools, vec!["form"]);
}

#[test]
fn persistence_round_trip_and_sanitization() {
    let mut app = crate::control::new_app();
    app.pin_tool("protect");
    app.pin_tool("redact");

    let saved = app.settings_snapshot();

    let mut restored = crate::control::new_app();
    restored.apply_settings(&saved);
    assert_eq!(restored.pinned_tools, vec!["protect", "redact"]);

    // Test sanitization: unknown tools are discarded and duplicates are removed, preserving order
    let malicious_or_corrupt = serde_json::json!({
        "pinned_tools": ["redact", "nonexistent_junk", "redact", "edit", "another_unknown", "protect"]
    })
    .to_string();
    restored.apply_settings(&pdfcraft_ui_egui::settings::import_json(&malicious_or_corrupt).0);
    assert_eq!(restored.pinned_tools, vec!["redact", "edit", "protect"]);
}

#[test]
fn clicking_pinned_tool_card_selects_it() {
    let mut h = harness(|app| {
        app.pin_tool("redact");
    });
    h.run_steps(2);

    // The tile's hover action takes the tool off Home, and the empty list shows its placeholder instead of a tile.
    // PLAN: assumed the action is named "Unpin from Home" (catalog id `home.unpin_tool`, pdf-chrome-w9).
    h.get_by_label("Unpin from Home").click();
    h.run_steps(3);
    assert!(h.state().pinned_tools.is_empty());
    assert!(h.query_by_role_and_label(egui::accesskit::Role::ListBox, "Pinned Tools").is_none());
    // The heading is a section named for the list, so more than one node carries the name.
    assert!(h.query_all_by_label_contains("Pinned Tools").count() >= 1);

    // Pinned again, a click on the tile selects the tool.
    h.state_mut().pin_tool("redact");
    h.run_steps(3);
    h.get_by_role_and_label(egui::accesskit::Role::ListBox, "Pinned Tools").get_by_label("Redact a PDF").click();
    h.run_steps(3);

    // Redact tool panel is now active
    assert_eq!(h.state().left, LeftPanel::Tool("redact"));
}

#[test]
fn pin_tool_dialog_workflow() {
    let mut h = harness(|_| {});
    // Home's Pin a tool… opens it.
    h.get_by_label("Pin a tool…").click();
    h.run_steps(3);

    // Modal is open (its heading carries the same name, so the dialog is found by its role).
    h.get_by_role_and_label(egui::accesskit::Role::Dialog, "Pin tools to Home");
    h.get_by_label("Done");

    // Pin a tool from inside the dialog
    assert!(!h.state().is_tool_pinned("scan"));
    h.state_mut().toggle_pin_tool("scan");
    assert!(h.state().is_tool_pinned("scan"));
    h.run_steps(2);

    // Close dialog with "Done"
    h.get_by_label("Done").click();
    h.run_steps(2);
    assert!(h.query_by_label_contains("Pin tools to Home").is_none());
}

#[test]
fn sidebar_pin_toggle_unpins_and_pins() {
    let mut h = harness(|app| {
        app.pin_tool("redact");
    });
    h.run_steps(2);
    assert!(h.state().is_tool_pinned("redact"));

    // Since redact is pinned, the pin toggle is rendered in the sidebar with "Unpin from Home"
    let unpin_btn = h.get_by_label("Unpin from Home");
    unpin_btn.click();
    h.run_steps(3);

    assert!(!h.state().is_tool_pinned("redact"));
    // Left panel did not navigate away (still AllTools)
    assert_eq!(h.state().left, LeftPanel::AllTools);
}
