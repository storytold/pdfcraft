//! Preferences ▸ Tools shown in All tools, end to end through the control channel: a preset or a
//! hidden list tidies All tools, the ⌘K palette and Home's recommended tools, while `ui.set tool`
//! (like the menus and mode tabs) still reaches every tool.

use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;
use serde_json::json;

use crate::control::{ControlClient, call, ok};

/// One empty 200×300 pt page.
const PAGE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R >> endobj
trailer << /Root 1 0 R >>
%%EOF";

fn harness() -> (Harness<'static, PdfCraftApp>, ControlClient) {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        // Keeps the presented frame's access tree for `ui.inspect` and `ui.click {label}`.
        craft::ui::runtime::windows::WindowHost::of(&cc.egui_ctx).keep_access_tree();
        let mut app = crate::control::new_app();
        app.set_option(&egui::Context::default(), "language", "en").unwrap();
        app.attach_control(craft::automation::ControlChannel::new(rx), None);
        app.open_bytes(&egui::Context::default(), "page.pdf", None, PAGE.to_vec()).unwrap();
        app
    });
    h.run_steps(4);
    (h, tx)
}

fn set(h: &mut Harness<'static, PdfCraftApp>, c: &ControlClient, key: &str, value: &str) {
    ok(h, c, "ui.set", json!({"key": key, "value": value}));
    h.run_steps(2);
}

/// How many widgets on screen are labelled with `text` (a substring, any case).
fn shown(h: &mut Harness<'static, PdfCraftApp>, c: &ControlClient, text: &str) -> u64 {
    ok(h, c, "ui.inspect", json!({"query": text}))["count"].as_u64().unwrap()
}

/// How many heading texts read `text`; a region or list named after a heading doesn't count.
fn headings(h: &mut Harness<'static, PdfCraftApp>, c: &ControlClient, text: &str) -> u64 {
    ok(h, c, "ui.inspect", json!({"query": text, "role": "Label"}))["count"].as_u64().unwrap()
}

/// Opens the palette, types `query` and counts its rows labelled `row`; Escape closes it again.
fn palette_rows(h: &mut Harness<'static, PdfCraftApp>, c: &ControlClient, query: &str, row: &str) -> usize {
    use egui_kittest::kittest::Queryable;
    set(h, c, "palette", "");
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Search commands").type_text(query);
    h.run_steps(4);
    let rows = h.query_all_by(|n| n.role() == egui::accesskit::Role::ListBoxOption && n.label().is_some_and(|l| l.starts_with(row))).count();
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    rows
}

#[test]
fn a_preset_tidies_all_tools_and_the_palette_but_ui_set_still_opens_a_hidden_tool() {
    let (mut h, c) = harness();
    set(&mut h, &c, "tools", "expanded");
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["tool_preset"], "all");
    assert!(shown(&mut h, &c, "Compress a PDF") > 0);
    assert!(shown(&mut h, &c, "View less") > 0);

    set(&mut h, &c, "tool-preset", "read-comment");
    let state = ok(&mut h, &c, "ui.state", json!({}));
    assert_eq!(state["tool_preset"], "read-comment");
    assert!(state["hidden_tools"].as_array().unwrap().contains(&json!("compress")), "{state}");
    assert_eq!(shown(&mut h, &c, "Compress a PDF"), 0);
    assert!(shown(&mut h, &c, "Add comments") > 0);
    assert!(shown(&mut h, &c, "Compare files") > 0);
    // Four tools fit without folding, so there's nothing to expand or collapse.
    assert_eq!(shown(&mut h, &c, "View less") + shown(&mut h, &c, "View more"), 0);

    // ⌘K no longer lists the hidden tool, but still finds other entries by name.
    assert_eq!(palette_rows(&mut h, &c, "compress", "Compress a PDF"), 0);
    assert!(palette_rows(&mut h, &c, "add comments", "Add comments") > 0);

    // Hiding only tidies the lists: the control channel still opens the tool.
    set(&mut h, &c, "tool", "compress");
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["left_panel"], "Tool(\"compress\")");

    // One more tool shown is no longer the preset.
    set(&mut h, &c, "hidden-tools", "compress");
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["tool_preset"], "custom");
    set(&mut h, &c, "hidden-tools", "none");
    let state = ok(&mut h, &c, "ui.state", json!({}));
    assert_eq!(state["tool_preset"], "all");
    assert_eq!(state["hidden_tools"], json!([]));
    // Shown again, the tool is back in the palette.
    assert!(palette_rows(&mut h, &c, "compress", "Compress a PDF") > 0);
}

#[test]
fn unknown_tools_and_presets_are_refused_without_changing_anything() {
    let (mut h, c) = harness();
    set(&mut h, &c, "hidden-tools", "redact");
    for (key, value) in [("hidden-tools", "compress,no-such-tool"), ("hidden-tools", ""), ("tool-preset", "everything"), ("tool-preset", "custom")] {
        assert!(call(&mut h, &c, "ui.set", json!({"key": key, "value": value})).is_err(), "{key}={value}");
        assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["hidden_tools"], json!(["redact"]), "{key}={value}");
    }
}

#[test]
fn home_recommends_only_shown_tools_and_keeps_open_file() {
    let (mut h, c) = harness();
    set(&mut h, &c, "left", "closed");
    set(&mut h, &c, "home", "");
    assert!(shown(&mut h, &c, "Protect a PDF") > 0);
    set(&mut h, &c, "hidden-tools", "protect");
    assert_eq!(shown(&mut h, &c, "Protect a PDF"), 0);
    assert!(shown(&mut h, &c, "Organize pages") > 0);
    assert!(headings(&mut h, &c, "Recommended tools") > 0);
    // With nothing left to recommend, the heading goes too and Open file stays.
    set(&mut h, &c, "hidden-tools", "organize,comment,form,edit,protect");
    assert_eq!(shown(&mut h, &c, "Use now"), 0);
    assert_eq!(headings(&mut h, &c, "Recommended tools"), 0);
    assert!(shown(&mut h, &c, "Open file") > 0);
}

#[test]
fn all_tools_points_to_preferences_when_every_tool_is_hidden() {
    let (mut h, c) = harness();
    let every: Vec<&str> = pdfcraft_engine::catalog::TOOL_GROUPS.iter().map(|g| g.id).collect();
    set(&mut h, &c, "hidden-tools", &every.join(","));
    assert!(shown(&mut h, &c, "No tools are shown") > 0);
    assert_eq!(shown(&mut h, &c, "Add comments"), 0);

    // The expanded view says the same, not that a search found nothing.
    set(&mut h, &c, "tools", "full");
    assert!(shown(&mut h, &c, "No tools are shown") > 0);
    assert_eq!(shown(&mut h, &c, "No tools match your search"), 0);
}

#[test]
fn the_expanded_all_tools_view_leaves_out_hidden_tools_and_their_empty_categories() {
    let (mut h, c) = harness();
    set(&mut h, &c, "tools", "full");
    assert!(shown(&mut h, &c, "Compress a PDF") > 0);
    assert!(shown(&mut h, &c, "Create & Edit") > 0);

    set(&mut h, &c, "tool-preset", "read-comment");
    assert_eq!(shown(&mut h, &c, "Compress a PDF"), 0);
    assert!(shown(&mut h, &c, "Add comments") > 0);
    assert!(shown(&mut h, &c, "Compare files") > 0);
    // Every tool under Create & Edit is hidden, so its heading goes too.
    assert_eq!(shown(&mut h, &c, "Create & Edit"), 0);
    assert!(shown(&mut h, &c, "Review & Compare") > 0);
}

#[test]
fn preferences_checkboxes_change_the_list_and_the_preset() {
    use egui_kittest::kittest::Queryable;
    let (mut h, c) = harness();
    set(&mut h, &c, "left", "closed");
    h.state_mut().open_preferences();
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::CheckBox, "Compress a PDF").scroll_to_me();
    h.run_steps(4);
    h.get_by_role_and_label(egui::accesskit::Role::CheckBox, "Compress a PDF").click();
    h.run_steps(2);
    let state = ok(&mut h, &c, "ui.state", json!({}));
    assert_eq!(state["hidden_tools"], json!(["compress"]));
    assert_eq!(state["tool_preset"], "custom");
    h.get_by_role_and_label(egui::accesskit::Role::CheckBox, "Compress a PDF").click();
    h.run_steps(2);
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["tool_preset"], "all");
}
