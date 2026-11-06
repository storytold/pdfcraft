//! Behavioral regressions using only the public interfaces present before this repair.
#[allow(dead_code)]
#[path = "../../../crates/automation/tests/connected_support/mod.rs"]
mod connected_support;

use egui_kittest::Harness;
use pdfcraft_ui_egui::{PdfCraftApp, control::ControlClient};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    net::TcpStream,
    sync::{Arc, Mutex},
};

struct Owner {
    app: Harness<'static, PdfCraftApp>,
    client: ControlClient,
    writer: TcpStream,
    reader: BufReader<TcpStream>,
}

impl Owner {
    fn new() -> Self {
        let slot = Arc::new(Mutex::new(None));
        let out = slot.clone();
        let mut app = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_eframe(move |cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.open_bytes("first.pdf", None, connected_support::pdf()).unwrap();
            app.open_bytes("second.pdf", None, connected_support::pdf()).unwrap();
            app.active = Some(0);
            app.tool_runner = Some(Box::new(|_, _, name, _, _| Ok(json!({"content":[{"type":"json","value":{"ran":name}}]}))));
            *out.lock().unwrap() = Some(app.attach_control(&cc.egui_ctx));
            app
        });
        app.run_steps(5);
        let client = slot.lock().unwrap().take().unwrap();
        let endpoint = pdfcraft_ui_egui::control::serve(client.clone()).unwrap();
        let mut writer = TcpStream::connect(("127.0.0.1", endpoint.port)).unwrap();
        writer.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
        let mut reader = BufReader::new(writer.try_clone().unwrap());
        writeln!(writer, "{}", json!({"id":0,"method":"auth","params":{"token":endpoint.token}})).unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&line).unwrap()["result"]["ok"], true);
        Self { app, client, writer, reader }
    }
    fn local(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let rx = self.client.send(method, params);
        for _ in 0..100 {
            self.app.step();
            if let Ok(reply) = rx.try_recv() {
                return reply;
            }
        }
        panic!("owner did not answer {method}");
    }
    fn wire_before(&mut self, method: &str, params: Value, before: impl FnOnce(&mut PdfCraftApp)) -> Value {
        let (sent, ready) = std::sync::mpsc::channel();
        let (tx, rx) = std::sync::mpsc::channel();
        let w = &mut self.writer;
        let r = &mut self.reader;
        std::thread::scope(|scope| {
            scope.spawn(move || {
                writeln!(w, "{}", json!({"jsonrpc":"2.0","id":1,"method":method,"params":params})).unwrap();
                w.flush().unwrap();
                sent.send(()).unwrap();
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                tx.send(serde_json::from_str(&line).unwrap()).unwrap();
            });
            ready.recv().unwrap();
            before(self.app.state_mut());
            for _ in 0..2000 {
                self.app.step();
                if let Ok(reply) = rx.try_recv() {
                    return reply;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            panic!("TCP owner did not answer {method}");
        })
    }
    fn wire(&mut self, method: &str, params: Value) -> Value {
        self.wire_before(method, params, |_| {})
    }
    fn person_close(&mut self) {
        for pressed in [true, false] {
            self.app.event(egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE });
            self.app.step();
        }
        self.app.run_steps(2);
    }
    fn ask(&mut self) -> u64 {
        self.wire("agent.listen", json!({"timeoutMs":0,"agent":"synthetic-editor"}));
        let sent = self.local("ui.askAgent", json!({"instruction":"Edit the captured selection"})).unwrap();
        let id = sent["id"].as_u64().unwrap();
        let request = self.wire("agent.listen", json!({"timeoutMs":0}));
        assert_eq!(request["result"]["id"], id, "{request}");
        id
    }
    fn tool(&mut self, name: &str, args: Value) -> Value {
        self.wire("tool.call", json!({"name":name,"arguments":args}))
    }
}

#[test]
fn legacy_send_cannot_change_the_original_documents_identity() {
    let mut owner = Owner::new();
    owner.local("ui.askAgent", json!({})).unwrap();
    owner.app.state_mut().active = Some(1);
    assert!(owner.local("ui.askAgent", json!({"instruction":"wrong tab"})).unwrap_err().contains("STALE_TARGET"));
}

#[test]
fn legacy_successful_read_creates_no_undo_step_and_empty_listen_drops_claim() {
    let mut owner = Owner::new();
    let id = owner.ask();
    let listed = owner.tool("text_paragraphs", json!({"doc":1,"page":1}));
    assert!(listed.get("error").is_none(), "{listed}");
    assert_eq!(
        owner.local("ui.state", json!({})).unwrap()["askAgent"]["edits"],
        0,
        "a successful call without history must not fabricate an Undo step"
    );
    let reply = owner.wire("agent.reply", json!({"id":id,"text":"answer"}));
    assert!(reply.get("error").is_none(), "{reply}");
    assert_eq!(owner.wire("agent.listen", json!({"timeoutMs":0}))["result"], json!({"none":true}));
    let ordinary = owner.tool("page_rotate", json!({"doc":1,"pages":[1],"degrees":90}));
    assert!(ordinary.get("error").is_none(), "{ordinary}");
}

#[test]
fn legacy_reply_must_resolve_the_current_document_on_its_owner() {
    let mut owner = Owner::new();
    let id = owner.ask();
    let reply = owner.wire_before("agent.reply", json!({"id":id,"text":"stale answer"}), |app| {
        let doc = app.session.docs()[0].id;
        app.session.apply(doc, pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    });
    assert!(matches!(reply["errorCode"].as_str(), Some("STALE_TARGET" | "READ_ONLY_REQUEST")), "{reply}");
    assert!(owner.client.agent_hub().snapshot(id).unwrap().answer.is_none());
}

#[test]
fn legacy_claim_cannot_mutate_another_document_without_a_target_proof() {
    let mut owner = Owner::new();
    owner.ask();
    owner.app.state_mut().tool_runner = Some(Box::new(|session, _, _, args, _| {
        let doc = pdfcraft_engine::DocId(args["doc"].as_u64().unwrap());
        session.apply(doc, pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
        Ok(json!({"rotated":true}))
    }));
    let original = owner.app.state().session.docs()[1].bytes.clone();
    let refused = owner.tool("page_rotate", json!({"doc":2,"pages":[1],"degrees":90}));
    assert!(refused.get("error").is_some(), "an unprovable mutation must never run: {refused}");
    assert_eq!(owner.app.state().session.docs()[1].bytes, original);
}

#[test]
fn legacy_field_detection_is_read_only_only_when_add_is_explicitly_false() {
    let mut owner = Owner::new();
    let form = {
        let app = owner.app.state_mut();
        let bytes =
            app.session.create_from_text("Synthetic printed form", "Name: ______________________\n\nEmail address: ____________________").unwrap();
        app.open_bytes("printed-form.pdf", None, bytes.as_ref().clone()).unwrap();
        let id = app.session.docs().last().unwrap().id;
        app.active = Some(0);
        id
    };
    let mut tools = pdfcraft_automation::window::WindowTools::default();
    owner.app.state_mut().tool_runner =
        Some(Box::new(move |session, connection, name, args, root| tools.call(connection, session, root, name, args)));
    owner.ask();
    let inspected = owner.tool("form_detect_fields", json!({"doc":form.0,"add":false}));
    assert!(inspected.get("error").is_none(), "{inspected}");
    assert_eq!(inspected["result"]["content"][0]["value"]["fields"].as_array().unwrap().len(), 2);
    assert!(owner.app.state().session.get(form).unwrap().form.is_empty());
    let refused = owner.tool("form_detect_fields", json!({"doc":form.0}));
    let after = owner.app.state().session.get(form).unwrap().form.len();
    assert!(refused.get("error").is_some(), "default detection must not add fields outside the request: {refused}; fields_after={after}");
    assert_eq!(after, 0);
    owner.person_close();
    owner.wire("agent.listen", json!({"timeoutMs":0}));
    let ordinary = owner.tool("form_detect_fields", json!({"doc":form.0}));
    assert!(ordinary.get("error").is_none(), "ordinary detection must retain its existing ability to add fields: {ordinary}");
    assert_eq!(owner.app.state().session.get(form).unwrap().form.len(), 2);
}

#[test]
fn legacy_remote_close_cannot_keep_a_failed_request_after_its_claim_is_cleared() {
    let mut owner = Owner::new();
    owner.local("ui.askAgent", json!({})).unwrap();
    owner.local("ui.askAgent", json!({"close":true})).unwrap();
    let id = owner.ask();
    let failed = owner.wire("agent.reply", json!({"id":id,"error":"synthetic failure"}));
    assert!(failed.get("error").is_none(), "{failed}");
    assert_eq!(owner.wire("agent.listen", json!({"timeoutMs":0}))["result"], json!({"none":true}));
    let refused = owner.wire("ui.askAgent", json!({"close":true}));
    assert_eq!(refused["errorCode"], "HUMAN_REVIEW_REQUIRED", "{refused}");
    assert!(owner.client.agent_hub().snapshot(id).is_some());
    assert_eq!(owner.local("ui.state", json!({})).unwrap()["askAgent"]["bubbleOpen"], true);
    owner.person_close();
    assert!(owner.client.agent_hub().snapshot(id).is_none());
}

#[test]
fn legacy_malformed_request_tags_are_rejected_before_any_action() {
    let mut owner = Owner::new();
    owner.ask();
    let before = owner.app.state().session.docs()[0].bytes.clone();
    for tag in [Value::Null, json!("invalid"), json!(-1), json!(1.5)] {
        let refused = owner.wire("tool.call", json!({"name":"page_rotate","arguments":{"doc":1,"pages":[1],"degrees":90},"requestId":tag}));
        assert_eq!(refused["errorCode"], "NOT_OWNER", "{refused}");
        assert_eq!(owner.app.state().session.docs()[0].bytes, before);
    }
}
