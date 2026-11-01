//! MCP -> control queue -> real window session -> form edit, entirely without a native window.
use std::sync::{Arc, Mutex};

use egui_kittest::Harness;
use pdfcraft_automation::mcp::McpServer;
use pdfcraft_automation::window::Window;
use pdfcraft_ui_egui::PdfCraftApp;
use pdfcraft_ui_egui::control::ControlClient;
use serde_json::{Value, json};

struct WindowHarness {
    app: Harness<'static, PdfCraftApp>,
    client: ControlClient,
}

impl Window for WindowHarness {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let rx = self.client.send(method, params);
        for _ in 0..30 {
            self.app.step();
            if let Ok(reply) = rx.try_recv() {
                return reply;
            }
        }
        Err("headless window did not answer".into())
    }
    fn describe(&self) -> String {
        "headless test window".into()
    }
}

fn server() -> McpServer {
    let slot = Arc::new(Mutex::new(None));
    let out = Arc::clone(&slot);
    let mut app = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_eframe(move |cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        // Blank PDF made by the engine, then synthetic fields made by the normal automation API.
        let pdf = app.session.create_blank(200.0, 300.0, 1).unwrap();
        app.open_bytes("synthetic.pdf", None, pdf.as_ref().clone()).unwrap();
        let doc = app.views[0].id.0;
        for (name, y) in [("first", 10), ("last", 40)] {
            pdfcraft_automation::window::call_for_window(
                &mut app.session,
                None,
                "form_add_field",
                &json!({"doc":doc,"page":1,"type":"text","name":name,"rect":[10,y,180,y+20]}),
            )
            .unwrap();
        }
        app.tool_runner = Some(Box::new(|session, name, args, root| pdfcraft_automation::window::call_for_window(session, root, name, args)));
        *out.lock().unwrap() = Some(app.attach_control(&cc.egui_ctx));
        app
    });
    app.run_steps(4);
    let client = slot.lock().unwrap().take().unwrap();
    McpServer::new(pdfcraft_automation::Automation::new()).with_window(Box::new(WindowHarness { app, client }))
}

fn call(s: &mut McpServer, name: &str, args: Value) -> Value {
    let r = s.handle(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}})).unwrap();
    assert!(r.get("error").is_none(), "{r}");
    r["result"].clone()
}

#[test]
fn connected_form_fill_uses_the_windows_fields_and_one_undo_step() {
    let mut s = server();
    let state = call(&mut s, "ui_state", json!({}));
    let doc = state["structuredContent"]["documents"][0]["doc"].as_u64().unwrap();
    let fill = call(&mut s, "form_fill", json!({"doc":doc,"values":{"first":"Synthetic","last":"Example"}}));
    assert_ne!(fill["isError"], true, "{fill}");
    let fields = call(&mut s, "form_fields", json!({"doc":doc}));
    let text = fields.to_string();
    assert!(text.contains("Synthetic") && text.contains("Example"), "{fields}");
    let undo = call(&mut s, "edit_undo", json!({"doc":doc}));
    assert_ne!(undo["isError"], true, "{undo}");
    let fields = call(&mut s, "form_fields", json!({"doc":doc}));
    let text = fields.to_string();
    assert!(!text.contains("Synthetic") && !text.contains("Example"), "one Undo removes both values: {fields}");
    for (name, args) in [
        ("sign_document", json!({"doc":doc})),
        ("fill_sign_add", json!({"doc":doc,"page":1,"type":"signature","text":"Synthetic","at":[10,10]})),
        ("command_run", json!({"id":"sign.fill.text","params":{"doc":doc,"page":1,"type":"initials","text":"SE","at":[10,10]}})),
    ] {
        let r = call(&mut s, name, args);
        assert_eq!(r["isError"], true, "{r}");
        assert!(r.to_string().contains("person"), "{r}");
    }
    // Preparing the signature field remains possible; signing stays in the person's UI.
    let r = call(&mut s, "form_add_field", json!({"doc":doc,"page":1,"type":"signature","name":"Approval","rect":[10,80,180,110]}));
    assert_ne!(r["isError"], true, "{r}");
}
