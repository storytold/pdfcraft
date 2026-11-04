//! The control channel must observe the window whose document it admits a request for.
use std::sync::{Arc, Mutex};

use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;
use serde_json::json;

#[test]
fn a_detached_window_can_be_captured_without_switching_documents() {
    let client = Arc::new(Mutex::new(None));
    let slot = client.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        *slot.lock().unwrap() = Some(app.attach_control(&cc.egui_ctx));
        for name in ["left.pdf", "right.pdf"] {
            app.open_bytes(name, None, include_bytes!("../data/form.pdf").to_vec()).unwrap();
        }
        app
    });
    h.run_steps(4);
    // The embedded fallback shares the root viewport's input. Do not report that viewport as
    // OS-focused: the queued Focus below is the window that owns these control requests.
    h.input_mut().viewports.get_mut(&egui::ViewportId::ROOT).unwrap().focused = Some(false);
    h.input_mut().focused = false;
    let ctx = h.ctx.clone();
    assert!(h.state_mut().execute("window.move_tab_new"));
    h.state_mut().apply_window_ops(&ctx);
    h.run_steps(3);
    assert_eq!(h.state().window_count(), 2);
    assert_ne!(h.state().focused_window(), pdfcraft_ui_egui::WindowId::ROOT);
    let c = client.lock().unwrap().take().unwrap();
    let rx = c.send("ui.screenshot", json!({}));
    let mut reply = None;
    assert!(crate::support::step_until(&mut h, |_| {
        reply = rx.try_recv().ok();
        reply.is_some()
    }));
    let shot = reply.unwrap().expect("the focused document must also own screenshot issuance");
    assert!(shot["png_base64"].as_str().is_some_and(|png| !png.is_empty()));
    for method in ["ui.inspect", "ui.click", "ui.focus", "ui.move", "ui.drag", "ui.type", "ui.key"] {
        let rx = c.send(method, json!({}));
        let mut reply = None;
        assert!(crate::support::step_until(&mut h, |_| {
            reply = rx.try_recv().ok();
            reply.is_some()
        }));
        assert!(reply.unwrap().unwrap_err().contains("main window"), "{method} must not target the root widget tree");
    }
    assert_eq!(h.state().session.docs().len(), 2);
    assert!(h.state().session.docs().iter().all(|doc| !doc.dirty));
}
