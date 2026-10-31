//! Tests for the connected-window parts of the control channel: private views, uncommitted
//! typing and Ask Agent's connection rules.

use super::*;

#[test]
fn stale_private_widget_trees_remain_private_until_new_frames() {
    let ctx = egui::Context::default();
    let (mut control, _client) = attach(&ctx);
    let mut app = crate::PdfCraftApp::new();
    app.dialog = Some(crate::Dialog::Signature);
    control.tick(&ctx, &mut app);
    app.dialog = None;
    control.tick(&ctx, &mut app); // logic without painting must not release private data
    let since = web_time::Instant::now();
    assert!(matches!(control.handle(&ctx, &mut app, "tool.inspect", &json!({}), since), requests::Handled::Now(Err(_))));
    control.shared.lock().unwrap().passes = 2;
    assert!(matches!(control.handle(&ctx, &mut app, "tool.inspect", &json!({}), since), requests::Handled::Now(Ok(_))));
}

#[test]
fn control_tools_preserve_comment_drafts_and_edit_dialogs() {
    let mut app = crate::PdfCraftApp::new();
    let bytes = app.session.create_blank(200.0, 300.0, 1).unwrap();
    app.open_bytes("synthetic.pdf", None, bytes.as_ref().clone()).unwrap();
    app.tool_runner = Some(Box::new(|_, _, _, _| panic!("must not run over a draft")));
    for kind in 0..4 {
        let comments = &mut app.views[0].comments;
        *comments = Default::default();
        match kind {
            0 => {
                comments.composer = Some(crate::comments::Composer {
                    page: 0,
                    at: [10.0, 10.0],
                    kind: crate::comments::ComposerKind::Note,
                    text: "Draft note".into(),
                    focus: false,
                })
            }
            1 => comments.reply = "Draft reply".into(),
            2 => comments.editing = Some((0, 0, "Draft edit".into())),
            _ => comments.add_box = "Draft new comment".into(),
        }
        let before = format!("{:?}", app.views[0].comments);
        let e = app.tool("page_insert_blank", &json!({"doc":1,"at":1}), None).unwrap_err();
        assert!(e.contains("draft has been kept"), "{e}");
        assert_eq!(format!("{:?}", app.views[0].comments), before);
    }
    app.views[0].comments = Default::default();
    for dialog in [crate::Dialog::FieldProps, crate::Dialog::LinkProps] {
        app.dialog = Some(dialog);
        assert!(app.tool("page_insert_blank", &json!({"doc":1,"at":1}), None).unwrap_err().contains("draft has been kept"));
        assert_eq!(app.dialog, Some(dialog));
    }
}

#[test]
fn control_tool_call_keeps_uncommitted_typing() {
    let mut app = crate::PdfCraftApp::new();
    let bytes = app.session.create_blank(200.0, 300.0, 1).unwrap();
    app.open_bytes("synthetic.pdf", None, bytes.as_ref().clone()).unwrap();
    app.tool_runner = Some(Box::new(|_, _, _, _| panic!("must not run over a draft")));
    app.views[0].content.draft = Some(crate::content_ui::TextDraft {
        page: 0,
        index: None,
        rect: [10.0, 10.0, 100.0, 30.0],
        text: "Uncommitted".into(),
        style: crate::content_ui::default_style(),
        focus: false,
        selection: None,
        typing_from: None,
    });
    let e = app.tool("doc_save", &json!({"doc":1}), None).unwrap_err();
    assert!(e.contains("draft has been kept"), "{e}");
    assert_eq!(app.views[0].content.draft.as_ref().unwrap().text, "Uncommitted");
}

#[cfg(not(target_arch = "wasm32"))]
mod agent_transport_tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};

    fn exchange(w: &mut std::net::TcpStream, r: &mut BufReader<std::net::TcpStream>, msg: Value) -> Value {
        writeln!(w, "{msg}").unwrap();
        w.flush().unwrap();
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    #[test]
    fn ask_agent_tcp_auth_owner_late_reply_and_longpoll() {
        let ctx = egui::Context::default();
        let (_control, client) = attach(&ctx);
        let hub = client.agent_hub();
        let endpoint = serve(client).unwrap();
        let connect = || {
            let w = std::net::TcpStream::connect(("127.0.0.1", endpoint.port)).unwrap();
            w.set_read_timeout(Some(std::time::Duration::from_secs(3))).unwrap();
            let r = BufReader::new(w.try_clone().unwrap());
            (w, r)
        };
        let (mut bad, mut br) = connect();
        assert_eq!(exchange(&mut bad, &mut br, json!({"id":1,"method":"auth","params":{"token":"wrong"}}))["ok"], false);
        assert!(hub.agents().is_empty());
        let (mut w, mut r) = connect();
        assert_eq!(exchange(&mut w, &mut r, json!({"id":1,"method":"auth","params":{"token":endpoint.token}}))["result"]["ok"], true);
        assert_eq!(
            exchange(&mut w, &mut r, json!({"id":2,"method":"agent.listen","params":{"timeoutMs":0,"agent":"editor"}}))["result"],
            json!({"none":true})
        );
        let id = hub.ask(crate::ask_agent::Request {
            id: 0,
            instruction: "Explain synthetic page".into(),
            history: Vec::new(),
            selection: json!({"pages":[1]}),
            document: json!({"id":"pdf:1"}),
            agent: None,
        });
        let claim = exchange(&mut w, &mut r, json!({"id":3,"method":"agent.listen","params":{"timeoutMs":0}}));
        assert_eq!(claim["result"]["id"], id);
        let (mut other, mut or) = connect();
        exchange(&mut other, &mut or, json!({"id":1,"method":"auth","params":{"token":endpoint.token}}));
        let denied = exchange(&mut other, &mut or, json!({"id":2,"method":"agent.reply","params":{"id":id,"text":"stolen"}}));
        assert_eq!(denied["errorCode"], "NOT_OWNER");
        assert!(denied["error"].is_string());
        let reply = exchange(&mut w, &mut r, json!({"id":4,"method":"agent.reply","params":{"id":id,"text":"answer"}}));
        assert_eq!(reply["result"], json!({"id": id, "done": true}));
        let before = std::time::Instant::now();
        assert_eq!(exchange(&mut w, &mut r, json!({"id":5,"method":"agent.listen","params":{"timeoutMs":120}}))["result"], json!({"none":true}));
        assert!(before.elapsed() >= std::time::Duration::from_millis(100));
    }
}
