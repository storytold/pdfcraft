//! Pinned tools on Home: pin tools, unpin tools, persistence across sessions,
//! selection modal, and the sidebar pin toggle.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{Dialog, LeftPanel, PdfCraftApp};

fn harness(setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1000.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.run_inline = true;
        setup(&mut app);
        app
    });
    h.run_steps(4);
    h
}

#[test]
fn state_operations_and_validation() {
    let mut app = PdfCraftApp::new();
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
    let mut app = PdfCraftApp::new();
    app.pin_tool("protect");
    app.pin_tool("redact");

    let saved = app.persist();
    assert!(saved.contains("\"pinned_tools\""));

    let mut restored = PdfCraftApp::new();
    restored.restore(&saved);
    assert_eq!(restored.pinned_tools, vec!["protect", "redact"]);

    // Test sanitization: unknown tools are discarded and duplicates are removed, preserving order
    let malicious_or_corrupt = serde_json::json!({
        "pinned_tools": ["redact", "nonexistent_junk", "redact", "edit", "another_unknown", "protect"]
    })
    .to_string();
    restored.restore(&malicious_or_corrupt);
    assert_eq!(restored.pinned_tools, vec!["redact", "edit", "protect"]);
}

#[test]
fn home_shows_pinned_tools_section_and_placeholder() {
    let mut h = harness(|_| {});
    // When no tools are pinned, the placeholder message appears
    h.get_by_label_contains("Pinned Tools");
    h.get_by_label_contains("Pin your favorite tools here for quick access.");
    h.get_by_label("Pin a tool…");

    // Pin a tool and step forward
    h.state_mut().pin_tool("redact");
    h.run_steps(3);

    // Placeholder is gone and Redact card is shown
    assert!(h.query_by_label_contains("Pin your favorite tools here for quick access.").is_none());
    assert!(h.query_all_by_label("Redact a PDF").count() >= 1);

    // Unpin tool and verify placeholder returns
    h.state_mut().unpin_tool("redact");
    h.run_steps(3);
    h.get_by_label_contains("Pin your favorite tools here for quick access.");
}

#[test]
fn clicking_pinned_tool_card_selects_it() {
    let mut h = harness(|app| {
        app.pin_tool("redact");
    });
    h.run_steps(2);

    // The Redact card is rendered and can be clicked to select the tool
    let nodes: Vec<_> = h.query_all_by_label("Redact a PDF").collect();
    assert!(!nodes.is_empty());
    // Click the card
    nodes.last().unwrap().click();
    h.run_steps(3);

    // Redact tool panel is now active
    assert_eq!(h.state().left, LeftPanel::Tool("redact"));
}

#[test]
fn pin_tool_dialog_workflow() {
    let mut h = harness(|app| {
        app.dialog = Some(Dialog::PinTool);
    });
    h.run_steps(3);

    // Modal is open
    h.get_by_label_contains("Pin tools to Home");
    h.get_by_label("Done");

    // Pin a tool from inside the dialog
    assert!(!h.state().is_tool_pinned("scan"));
    h.state_mut().toggle_pin_tool("scan");
    assert!(h.state().is_tool_pinned("scan"));
    h.run_steps(2);

    // Close dialog with "Done"
    h.get_by_label("Done").click();
    h.run_steps(2);
    assert!(h.state().dialog.is_none());
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
