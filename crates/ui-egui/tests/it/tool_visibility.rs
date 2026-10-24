//! Preferences ▸ Tools shown in All tools, end to end through the control channel: a preset or a
//! hidden list tidies All tools, the ⌘K palette and Home's recommended tools, while `ui.set tool`
//! (like the menus and mode tabs) still reaches every tool.

use std::sync::{Arc, Mutex};

use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;
use pdfcraft_ui_egui::control::{ControlClient, Reply};
use serde_json::{Value, json};

/// One empty 200×300 pt page.
const PAGE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R >> endobj
trailer << /Root 1 0 R >>
%%EOF";

fn harness() -> (Harness<'static, PdfCraftApp>, ControlClient) {
    let slot: Arc<Mutex<Option<ControlClient>>> = Arc::default();
    let s = slot.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        *s.lock().unwrap() = Some(app.attach_control(&cc.egui_ctx));
        app.open_bytes("page.pdf", None, PAGE.to_vec()).unwrap();
        app
    });
    h.run_steps(4);
    let client = slot.lock().unwrap().take().unwrap();
    (h, client)
}

/// Send a request and run frames until it is answered.
fn call(h: &mut Harness<'static, PdfCraftApp>, c: &ControlClient, method: &str, params: Value) -> Reply {
    let rx = c.send(method, params);
    for _ in 0..30 {
        h.step();
        if let Ok(r) = rx.try_recv() {
            return r;
        }
    }
    panic!("{method}: no reply after 30 frames");
}

fn ok(h: &mut Harness<'static, PdfCraftApp>, c: &ControlClient, method: &str, params: Value) -> Value {
    call(h, c, method, params).unwrap_or_else(|e| panic!("{method}: {e}"))
}

fn set(h: &mut Harness<'static, PdfCraftApp>, c: &ControlClient, key: &str, value: &str) {
    ok(h, c, "ui.set", json!({"key": key, "value": value}));
    h.run_steps(2);
}

/// How many widgets on screen are labelled with `text` (a substring, any case).
fn shown(h: &mut Harness<'static, PdfCraftApp>, c: &ControlClient, text: &str) -> u64 {
    ok(h, c, "ui.inspect", json!({"query": text}))["count"].as_u64().unwrap()
}

/// How many buttons (palette rows, tool rows, cards) are labelled with `text`. Unlike [`shown`],
/// this leaves out the palette's search field, which holds the query itself.
fn buttons(h: &mut Harness<'static, PdfCraftApp>, c: &ControlClient, text: &str) -> u64 {
    ok(h, c, "ui.inspect", json!({"query": text, "role": "Button"}))["count"].as_u64().unwrap()
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

    // ⌘K no longer lists the hidden tool, but still finds its commands by name.
    set(&mut h, &c, "palette", "compress");
    assert_eq!(buttons(&mut h, &c, "Compress a PDF"), 0);
    set(&mut h, &c, "palette", "reduce file size");
    assert!(buttons(&mut h, &c, "Reduce file size") > 0);
    set(&mut h, &c, "palette", "open");
    assert!(buttons(&mut h, &c, "Open…") > 0);
    ok(&mut h, &c, "ui.key", json!({"key": "Escape"}));

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
    assert!(shown(&mut h, &c, "Recommended tools") > 0);
    // With nothing left to recommend, the heading goes too and Open file stays.
    set(&mut h, &c, "hidden-tools", "organize,comment,form,edit,protect");
    assert_eq!(shown(&mut h, &c, "Use now"), 0);
    assert_eq!(shown(&mut h, &c, "Recommended tools"), 0);
    assert!(shown(&mut h, &c, "Open file") > 0);
}

#[test]
fn all_tools_points_to_preferences_when_every_tool_is_hidden() {
    let (mut h, c) = harness();
    let every: Vec<&str> = pdfcraft_engine::catalog::TOOL_GROUPS.iter().map(|g| g.id).collect();
    set(&mut h, &c, "hidden-tools", &every.join(","));
    assert!(shown(&mut h, &c, "No tools are shown") > 0);
    assert_eq!(shown(&mut h, &c, "Add comments"), 0);
}

#[test]
fn preferences_checkboxes_and_preset_change_the_list() {
    let (mut h, c) = harness();
    set(&mut h, &c, "left", "closed");
    set(&mut h, &c, "dialog", "preferences");
    assert!(shown(&mut h, &c, "Tools shown in All tools") > 0);
    ok(&mut h, &c, "ui.click", json!({"label": "Compress a PDF"}));
    h.run_steps(2);
    let state = ok(&mut h, &c, "ui.state", json!({}));
    assert_eq!(state["hidden_tools"], json!(["compress"]));
    assert_eq!(state["tool_preset"], "custom");
    assert!(shown(&mut h, &c, "Custom selection") > 0);
    ok(&mut h, &c, "ui.click", json!({"label": "Compress a PDF"}));
    h.run_steps(2);
    assert_eq!(ok(&mut h, &c, "ui.state", json!({}))["tool_preset"], "all");
}
