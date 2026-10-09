//! The optional AI assistant tools, end to end with a stand-in provider (no network).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use pdfcraft_automation::{Automation, Content, ToolError};
use pdfcraft_engine::ai::{AiError, Api, Provider, Request, Response, Transport};
use serde_json::{Value, json};

/// A PDF with `n` 200×300 pt pages reading "Page 1", "Page 2", …
fn fixture(n: usize) -> Vec<u8> {
    let mut objs: Vec<Vec<u8>> = vec![b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()];
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 200 300] >>", kids.join(" ")).into_bytes());
    objs.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
    for i in 0..n {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * i).into_bytes());
        let body = format!("BT /F1 24 Tf 20 150 Td (Page {}) Tj ET", i + 1);
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()).into_bytes());
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn workdir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pdfcraft-automation-ai-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.pdf"), fixture(3)).unwrap();
    dir
}

/// A stand-in provider: records each request and answers with a fixed reply.
struct Fake {
    status: u16,
    body: String,
    seen: Mutex<Vec<Request>>,
}

impl Fake {
    fn answering(text: &str) -> Arc<Self> {
        let body = json!({ "choices": [{ "message": { "role": "assistant", "content": text }, "finish_reason": "stop" }] }).to_string();
        Arc::new(Fake { status: 200, body, seen: Mutex::new(Vec::new()) })
    }

    /// The user message of the `i`-th request.
    fn sent(&self, i: usize) -> String {
        let body: Value = serde_json::from_str(&self.seen.lock().unwrap()[i].body).unwrap();
        body["messages"][1]["content"].as_str().unwrap().to_string()
    }
}

impl Transport for Fake {
    fn post(&self, request: &Request) -> Result<Response, AiError> {
        self.seen.lock().unwrap().push(request.clone());
        Ok(Response { status: self.status, body: self.body.clone() })
    }
}

fn provider() -> Provider {
    Provider::new(Api::OpenAi, "http://localhost:11434/v1", "test-model")
}

fn ok(a: &mut Automation, tool: &str, args: Value) -> Value {
    match a.call(tool, &args) {
        Ok(mut c) => match c.remove(0) {
            Content::Json(v) => v,
            other => panic!("{tool}: expected JSON, got {other:?}"),
        },
        Err(e) => panic!("{tool} {args}: {e}"),
    }
}

#[test]
fn ai_tools_are_off_until_a_provider_is_configured() {
    let dir = workdir("off");
    let mut a = Automation::new().with_root(&dir).unwrap();
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].clone();
    for (tool, args) in [
        ("ai_summarize", json!({ "doc": doc })),
        ("ai_ask", json!({ "doc": doc, "question": "What is this?" })),
        ("ai_translate", json!({ "doc": doc, "language": "Italian" })),
    ] {
        match a.call(tool, &args) {
            Err(ToolError::Failed(m)) => assert!(m.contains("off by default") && m.contains("PDFCRAFT_AI_ENDPOINT"), "{tool}: {m}"),
            other => panic!("{tool}: expected a refusal, got {other:?}"),
        }
    }
}

#[test]
fn ai_tools_send_the_page_text_and_return_the_answer() {
    let dir = workdir("on");
    let fake = Fake::answering("It lists three pages (p. 1).");
    let mut a = Automation::new().with_root(&dir).unwrap().with_ai(provider(), fake.clone());
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].clone();

    let r = ok(&mut a, "ai_summarize", json!({ "doc": doc }));
    assert_eq!(r["answer"], "It lists three pages (p. 1).");
    assert_eq!((r["pages_sent"].as_u64(), r["pages_with_text"].as_u64(), r["truncated"].as_bool()), (Some(3), Some(3), Some(false)));
    assert_eq!((r["model"].as_str(), r["endpoint"].as_str()), (Some("test-model"), Some("http://localhost:11434/v1")));
    let sent = fake.sent(0);
    for page in 1..=3 {
        assert!(sent.contains(&format!("[Page {page}]\nPage {page}")), "{sent}");
    }
    assert!(sent.contains("Summarize the document above"));

    // Only the pages asked for are sent, under their own numbers.
    let r = ok(&mut a, "ai_ask", json!({ "doc": doc, "question": "What does page 2 say?", "pages": [2] }));
    assert_eq!(r["pages_sent"], 1);
    let sent = fake.sent(1);
    assert!(sent.contains("[Page 2]\nPage 2") && !sent.contains("[Page 1]") && !sent.contains("[Page 3]"), "{sent}");
    assert!(sent.ends_with("Question: What does page 2 say?"), "{sent}");

    ok(&mut a, "ai_translate", json!({ "doc": doc, "language": "Italian" }));
    assert!(fake.sent(2).contains("Translate the document above into Italian"));

    // The tools read: nothing about the document changed.
    let listed = ok(&mut a, "doc_list", json!({}));
    assert_eq!(listed["documents"][0]["dirty"], false);
    assert_eq!(fake.seen.lock().unwrap().len(), 3);
}

#[test]
fn ai_tool_failures_are_errors_an_agent_can_act_on() {
    let dir = workdir("errors");
    let fake = Arc::new(Fake { status: 404, body: r#"{"error":{"message":"model 'test-model' not found"}}"#.into(), seen: Mutex::new(Vec::new()) });
    let mut a = Automation::new().with_root(&dir).unwrap().with_ai(provider(), fake.clone());
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].clone();
    match a.call("ai_summarize", &json!({ "doc": doc })) {
        Err(ToolError::Failed(m)) => assert!(m.contains("404") && m.contains("model 'test-model' not found"), "{m}"),
        other => panic!("{other:?}"),
    }
    // Bad arguments never reach the provider.
    let before = fake.seen.lock().unwrap().len();
    assert!(matches!(a.call("ai_ask", &json!({ "doc": doc, "question": "   " })), Err(ToolError::InvalidArgs(_))));
    assert!(matches!(a.call("ai_ask", &json!({ "doc": doc })), Err(ToolError::InvalidArgs(_))));
    assert!(matches!(a.call("ai_summarize", &json!({ "doc": doc, "pages": [9] })), Err(ToolError::InvalidArgs(_))));
    assert!(matches!(a.call("ai_translate", &json!({ "doc": doc, "language": "Italian", "model": "x" })), Err(ToolError::InvalidArgs(_))));
    assert!(matches!(a.call("ai_summarize", &json!({ "doc": 99 })), Err(ToolError::Failed(_))));
    assert_eq!(fake.seen.lock().unwrap().len(), before);
}
