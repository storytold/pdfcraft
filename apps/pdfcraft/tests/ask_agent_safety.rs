//! Real TCP -> owner queue -> stateful automation, without opening a native window.
#[allow(dead_code)]
#[path = "../../../crates/automation/tests/connected_support/mod.rs"]
mod connected_support;

use egui_kittest::{Harness, kittest::Queryable};
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
            let tools = std::rc::Rc::new(std::cell::RefCell::new(pdfcraft_automation::window::WindowTools::default()));
            let resolver = tools.clone();
            app.tool_resolver =
                Some(Box::new(move |session, connection, name, args| resolver.borrow_mut().resolve_target(connection, session, name, args)));
            app.tool_runner =
                Some(Box::new(move |session, connection, name, args, root| tools.borrow_mut().call(connection, session, root, name, args)));
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
    fn select(&mut self, text: &str) {
        let doc = &self.app.state().session.docs()[0];
        let mut renderer = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), Default::default());
        let text_page =
            renderer.render(pdfcraft_render::RenderRequest { page: 0, kind: pdfcraft_render::RequestKind::Text, ..Default::default() }).text.unwrap();
        let span = text_page.find(text).into_iter().next().unwrap();
        self.app.state_mut().views[0].select_text(0, span.start, span.end - 1);
        self.app.run_steps(4);
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
fn scoped_text_edits_rebase_actual_ink_twice_and_refuse_other_targets() {
    let mut owner = Owner::new();
    owner.select("Bravo paragraph, a single row.");
    let id = owner.ask();
    let invalid = owner.wire("agent.listen", json!({"timeoutMs":"invalid"}));
    assert!(invalid.get("error").is_some());
    assert!(!owner.client.agent_hub().held()[0].0["quads"].as_array().unwrap().is_empty());
    let listing = owner.tool("text_paragraphs", json!({"doc":1,"page":1}));
    assert!(listing.get("error").is_none(), "{listing}");
    for text in ["Bravo revised once.", "Bravo revised twice."] {
        let edited = owner.tool("text_edit", json!({"doc":1,"page":1,"paragraph":2,"text":text}));
        if edited.get("error").is_some() {
            let captured = owner.client.agent_hub().held()[0].0.clone();
            let pdfcraft_ui_egui::ask_agent::Status::Working { conn } = owner.client.agent_hub().snapshot(id).unwrap().status else {
                panic!("request stopped")
            };
            let app = owner.app.state_mut();
            let proof =
                app.tool_resolver.as_mut().unwrap()(&app.session, conn, "text_edit", &json!({"doc":1,"page":1,"paragraph":2,"text":text})).unwrap();
            panic!("{edited}; captured={captured}; actual={proof}");
        }
    }
    let before = owner.app.state().session.docs()[0].bytes.clone();
    for (doc, paragraph) in [(1, 1), (2, 2)] {
        let refused = owner.tool("text_edit", json!({"doc":doc,"page":1,"paragraph":paragraph,"text":"outside"}));
        assert!(refused.get("error").is_some(), "{refused}");
    }
    let refused = owner.tool("text_edit", json!({"doc":1,"page":1,"paragraph":2,"text":"moved","dx":100}));
    assert!(refused.get("error").is_some());
    assert_eq!(owner.app.state().session.docs()[0].bytes, before);
    let reply =
        owner.wire("agent.reply", json!({"id":id,"proposal":"<script>literal proposal</script>","format":"javascript","comment":"Review this note"}));
    assert!(reply.get("error").is_none(), "{reply}");
    owner.app.run_steps(3);
    let comments = owner.tool("comment_list", json!({"doc":1}));
    assert!(comments.to_string().contains("Proposal (not applied)") && comments.to_string().contains("literal proposal"), "{comments}");
    assert_eq!(owner.wire("agent.listen", json!({"timeoutMs":0}))["result"], json!({"none":true}));
    let ordinary = owner.tool("page_rotate", json!({"doc":1,"pages":[1],"degrees":90}));
    assert!(ordinary.get("error").is_none(), "an empty listen must drop its old claim: {ordinary}");
}

#[test]
fn partial_paragraphs_and_ui_bypasses_are_refused_and_reply_uses_current_owner_state() {
    let mut owner = Owner::new();
    owner.select("Bravo");
    let id = owner.ask();
    owner.tool("text_paragraphs", json!({"doc":1,"page":1}));
    let before = owner.app.state().session.docs()[0].bytes.clone();
    let refused = owner.tool("text_edit", json!({"doc":1,"page":1,"paragraph":2,"text":"whole paragraph"}));
    assert!(refused.to_string().contains("OUTSIDE_SELECTION"), "{refused}");
    assert_eq!(owner.app.state().session.docs()[0].bytes, before);
    for method in ["ui.command", "ui.type", "ui.click", "ui.askAgent"] {
        let refused = owner.wire(method, json!({"id":"edit.undo","text":"bypass","close":true}));
        let expected = if method == "ui.askAgent" { "HUMAN_REVIEW_REQUIRED" } else { "ASK_AGENT_TOOLS_REQUIRED" };
        assert!(refused.to_string().contains(expected), "{refused}");
    }
    assert_eq!(owner.wire("agent.listen", json!({"timeoutMs":0}))["result"], json!({"none":true}));
    let bypass = owner.wire("ui.command", json!({"id":"edit.undo"}));
    assert!(bypass.to_string().contains("ASK_AGENT_TOOLS_REQUIRED"), "a cleared tag cannot bypass a still-owned request: {bypass}");
    // The network has received the response, but validation must still see an edit applied
    // before the owner services it. A captured target token cannot close this race.
    let stale = owner.wire_before("agent.reply", json!({"id":id,"text":"obsolete answer"}), |app| {
        let doc = app.session.docs()[0].id;
        app.session.apply(doc, pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    });
    assert!(matches!(stale["errorCode"].as_str(), Some("STALE_TARGET" | "READ_ONLY_REQUEST")), "{stale}");
    assert!(owner.client.agent_hub().snapshot(id).unwrap().answer.is_none());
}

#[test]
fn send_stays_on_its_original_document_and_save_adds_no_undo_step() {
    let mut owner = Owner::new();
    owner.local("ui.askAgent", json!({})).unwrap();
    owner.app.state_mut().active = Some(1);
    assert!(owner.local("ui.askAgent", json!({"instruction":"wrong tab"})).unwrap_err().contains("STALE_TARGET"));
    owner.app.state_mut().active = Some(0);
    owner.app.state_mut().set_option("select", "1").unwrap();
    owner.ask();
    for _ in 0..2 {
        assert!(owner.tool("page_rotate", json!({"doc":1,"pages":[1],"degrees":90})).get("error").is_none());
    }
    let path = std::env::temp_dir().join(format!("pdfcraft-ask-agent-save-{}.pdf", std::process::id()));
    let saved = owner.tool("doc_save", json!({"doc":1,"path":path}));
    assert!(saved.get("error").is_none(), "{saved}");
    assert_eq!(owner.local("ui.state", json!({})).unwrap()["askAgent"]["edits"], 2);
    let refused = owner.wire("ui.askAgent", json!({"close":true}));
    assert_eq!(refused["errorCode"], "HUMAN_REVIEW_REQUIRED", "{refused}");
    owner.person_close();
    let app = owner.app.state_mut();
    let doc = app.session.docs()[0].id;
    assert_eq!(app.session.get(doc).unwrap().undo_depth(), 1);
    app.session.undo(doc).unwrap();
    assert_eq!(app.session.get(doc).unwrap().info.pages[0].rotation, 0);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn failed_tool_work_keeps_a_literal_error_note_and_one_person_owned_undo_group() {
    let mut owner = Owner::new();
    owner.app.state_mut().set_option("select", "1").unwrap();
    let id = owner.ask();
    let edit = owner.tool("page_rotate", json!({"doc":1,"pages":[1],"degrees":90}));
    assert!(edit.get("error").is_none(), "{edit}");
    let reply = owner.wire("agent.reply", json!({"id":id,"error":"Failed after the first tool edit: <literal>"}));
    assert!(reply.get("error").is_none(), "{reply}");
    owner.app.run_steps(3);
    let notes = owner.tool("comment_list", json!({"doc":1}));
    assert!(notes.to_string().contains("Error: Failed after the first tool edit: <literal>"), "{notes}");
    assert_eq!(owner.local("ui.state", json!({})).unwrap()["askAgent"]["edits"], 2);
    let refused = owner.wire("ui.askAgent", json!({"close":true}));
    assert_eq!(refused["errorCode"], "HUMAN_REVIEW_REQUIRED", "{refused}");
    owner.person_close();
    let app = owner.app.state_mut();
    let doc = app.session.docs()[0].id;
    assert_eq!(app.session.get(doc).unwrap().undo_depth(), 1);
    app.session.undo(doc).unwrap();
    assert_eq!(app.session.get(doc).unwrap().info.pages[0].rotation, 0);
    assert_eq!(app.session.get(doc).unwrap().undo_depth(), 0);
}

/// Real edits reach the engine's bounded history without manufacturing a large document.
fn fill_history(owner: &mut Owner, count: usize) {
    for _ in 0..count {
        let result = owner.tool("page_rotate", json!({"doc":1,"pages":[1],"degrees":90}));
        assert!(result.get("error").is_none(), "{result}");
    }
    assert_eq!(owner.app.state().session.docs()[0].undo_depth(), count);
    owner.app.state_mut().set_option("select", "1").unwrap();
}

/// Adding the response note at the history cap must never undo only that note.
#[test]
fn bounded_history_response_note_refuses_partial_agent_undo() {
    let mut owner = Owner::new();
    fill_history(&mut owner, 99);
    let id = owner.ask();
    let result = owner.tool("page_rotate", json!({"doc":1,"pages":[1],"degrees":90}));
    assert!(result.get("error").is_none(), "{result}");
    assert_eq!(owner.app.state().session.docs()[0].undo_depth(), 100);
    let reply = owner.wire("agent.reply", json!({"id":id,"text":"The rotation is complete."}));
    assert!(reply.get("error").is_none(), "{reply}");
    owner.app.run_steps(3);
    let notes = owner.tool("comment_list", json!({"doc":1}));
    assert!(notes.to_string().contains("The rotation is complete."), "{notes}");
    let before = owner.app.state().session.docs()[0].bytes.clone();
    let rotation = owner.app.state().session.docs()[0].info.pages[0].rotation;
    owner.app.get_by_label("Undo").click();
    owner.app.run_steps(3);
    assert!(owner.app.state().session.docs()[0].bytes == before, "an unprovable checkpoint must refuse the whole Undo");
    assert_eq!(owner.app.state().session.docs()[0].info.pages[0].rotation, rotation);
    assert_eq!(owner.app.state().session.docs()[0].undo_depth(), 100);
    assert!(owner.client.agent_hub().snapshot(id).is_none(), "the person's choice was actually handled");
}

/// A batch beginning with Save is not a standalone, history-free Save.
#[test]
fn bounded_history_save_then_rotation_batch_cannot_borrow_the_save_exception() {
    let mut owner = Owner::new();
    fill_history(&mut owner, 100);
    let initial_rotation = owner.app.state().session.docs()[0].info.pages[0].rotation;
    let id = owner.ask();
    let path = std::env::temp_dir().join(format!("pdfcraft-ask-bounded-batch-{}.pdf", std::process::id()));
    let result = owner.tool(
        "command_batch",
        json!({"steps":[
            {"id":"file.save","params":{"doc":1,"path":path}},
            {"id":"page.rotate","params":{"doc":1,"pages":[1],"degrees":90}}
        ]}),
    );
    assert!(result.get("error").is_none(), "the actual captured page batch must still run: {result}");
    assert!(path.exists(), "the first step really saved the document");
    std::fs::remove_file(path).unwrap();
    assert_eq!(owner.app.state().session.docs()[0].info.pages[0].rotation, (initial_rotation + 90) % 360);
    assert_eq!(owner.app.state().session.docs()[0].undo_depth(), 100);
    let reply = owner.wire("agent.reply", json!({"id":id,"text":"The save and rotation completed."}));
    assert!(reply.get("error").is_none(), "{reply}");
    owner.app.run_steps(3);
    let notes = owner.tool("comment_list", json!({"doc":1}));
    assert!(!notes.to_string().contains("The save and rotation completed."), "do not append a note to an unprovable transaction: {notes}");
    assert!(owner.local("ui.askAgent", json!({"instruction":"continue"})).unwrap_err().contains("STALE_TARGET"));
    let before = owner.app.state().session.docs()[0].bytes.clone();
    owner.app.get_by_label("Undo").click();
    owner.app.run_steps(3);
    assert!(owner.app.state().session.docs()[0].bytes == before, "refusal must preserve the entire edited state");
    assert_eq!(owner.app.state().session.docs()[0].undo_depth(), 100);
    assert!(owner.client.agent_hub().snapshot(id).is_none());
}
