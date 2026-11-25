//! The pinned-tools setting: pin and unpin, validation and persistence across sessions. PdfChocolate
//! shows no pinned tools on Home, but keeps the setting so settings files stay compatible.

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
