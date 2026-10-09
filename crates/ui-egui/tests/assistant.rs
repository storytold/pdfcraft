//! The optional AI assistant in the shell (egui_kittest), with a stand-in provider (no network):
//! off by default, Preferences ▸ AI assistant, and Summarize / Ask about this document / Translate.

use std::sync::{Arc, Mutex};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::ai::{AiError, Api, Request, Response, Transport};
use pdfcraft_ui_egui::{AssistantKind, Dialog, PdfCraftApp};

/// A two-page PDF reading "Page 1" and "Page 2".
fn fixture() -> Vec<u8> {
    let mut objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [4 0 R 6 0 R] /Count 2 /MediaBox [0 0 200 300] >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
    ];
    for i in 0..2 {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * i));
        let body = format!("BT /F1 24 Tf 20 150 Td (Page {}) Tj ET", i + 1);
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()));
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// A stand-in provider: records each request and answers with a fixed reply.
struct Fake {
    reply: Result<Response, AiError>,
    seen: Mutex<Vec<Request>>,
}

impl Fake {
    fn answering(text: &str) -> Arc<Self> {
        let body = serde_json::json!({ "choices": [{ "message": { "content": text } }] }).to_string();
        Arc::new(Fake { reply: Ok(Response { status: 200, body }), seen: Mutex::new(Vec::new()) })
    }

    fn sent(&self, i: usize) -> String {
        let body: serde_json::Value = serde_json::from_str(&self.seen.lock().unwrap()[i].body).unwrap();
        body["messages"][1]["content"].as_str().unwrap().to_string()
    }
}

impl Transport for Fake {
    fn post(&self, request: &Request) -> Result<Response, AiError> {
        self.seen.lock().unwrap().push(request.clone());
        self.reply.clone()
    }
}

fn harness(fake: Option<Arc<Fake>>, configured: bool) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1000.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.run_inline = true;
        app.ai_transport = fake.map(|f| f as pdfcraft_ui_egui::AiTransport);
        if configured {
            app.ai_prefs.enabled = true;
            app.ai_prefs.endpoint = "http://localhost:11434/v1".into();
            app.ai_prefs.model = "test-model".into();
        }
        app.open_bytes("two.pdf", None, fixture()).unwrap();
        app
    });
    h.run_steps(4);
    h
}

#[test]
fn the_assistant_is_off_by_default_and_points_to_preferences() {
    let fake = Fake::answering("unused");
    let mut h = harness(Some(fake.clone()), false);
    assert!(!h.state().ai_prefs.enabled);
    for command in ["ai.summary", "ai.ask", "ai.translate"] {
        h.state_mut().dialog = None;
        assert!(h.state_mut().execute(command), "{command} is registered");
        assert_eq!(h.state().dialog, Some(Dialog::Preferences), "{command}");
    }
    h.run_steps(3);
    h.get_by_label("Enable the AI assistant");
    assert!(h.query_by_label("Endpoint URL").is_none(), "the provider fields appear once it is on");
    assert!(fake.seen.lock().unwrap().is_empty(), "nothing is sent while it is off");
    // Turning it on shows the provider fields.
    h.get_by_label("Enable the AI assistant").click();
    h.run_steps(3);
    assert!(h.state().ai_prefs.enabled);
    h.get_by_label("Endpoint URL");
    h.get_by_label("Model");
}

#[test]
fn summarize_sends_the_document_text_and_shows_the_answer() {
    let fake = Fake::answering("Two pages, numbered (p. 1).");
    let mut h = harness(Some(fake.clone()), true);
    assert!(h.state_mut().execute("ai.summary"));
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::Assistant));
    h.get_by_label_contains("Sends this document's text to http://localhost:11434/v1 (model test-model).");
    h.get_by_label("The answer appears here.");
    assert!(fake.seen.lock().unwrap().is_empty(), "opening the dialog sends nothing");
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Summarize").click();
    h.run_steps(4);
    h.get_by_label("Two pages, numbered (p. 1).");
    let sent = fake.sent(0);
    assert!(sent.contains("[Page 1]\nPage 1") && sent.contains("[Page 2]\nPage 2"), "{sent}");
    let seen = fake.seen.lock().unwrap();
    assert_eq!(seen[0].url, "http://localhost:11434/v1/chat/completions");
    assert!(seen[0].headers.iter().all(|(n, _)| *n != "Authorization"));
    drop(seen);
    h.get_by_label("Close").click();
    h.run_steps(3);
    assert_eq!(h.state().dialog, None);
}

#[test]
fn ask_and_translate_carry_what_the_user_typed() {
    let fake = Fake::answering("Answer.");
    let mut h = harness(Some(fake.clone()), true);
    h.state_mut().ai_key = "typed-key".into();
    assert!(h.state_mut().execute("ai.ask"));
    h.run_steps(3);
    // Without a question nothing is sent, and the dialog says why.
    h.get_by_label("Ask").click();
    h.run_steps(3);
    assert!(fake.seen.lock().unwrap().is_empty());
    h.get_by_label_contains("type a question first");
    h.state_mut().assistant.question = "How many pages?".into();
    h.get_by_label("Ask").click();
    h.run_steps(3);
    h.get_by_label("Answer.");
    assert!(fake.sent(0).ends_with("Question: How many pages?"));
    assert!(fake.seen.lock().unwrap()[0].headers.contains(&("Authorization", "Bearer typed-key".to_string())));

    // Another task starts without the previous answer.
    assert!(h.state_mut().execute("ai.translate"));
    h.run_steps(3);
    assert_eq!(h.state().assistant.kind, AssistantKind::Translate);
    assert!(h.state().assistant.answer.is_none());
    h.state_mut().assistant.language = "Italian".into();
    h.state_mut().start_assistant();
    h.run_steps(3);
    assert!(fake.sent(1).contains("into Italian"));
}

#[test]
fn provider_and_settings_problems_are_shown_not_fatal() {
    let failing = Arc::new(Fake { reply: Err(AiError::Network("connection refused".into())), seen: Mutex::new(Vec::new()) });
    let mut h = harness(Some(failing), true);
    h.state_mut().execute("ai.summary");
    h.state_mut().start_assistant();
    h.run_steps(3);
    h.get_by_label_contains("The AI assistant couldn't answer: couldn't reach the AI provider: connection refused");
    assert!(!h.state().assistant.is_running());

    // A key is never sent in the clear to another machine: refused before any request.
    let fake = Fake::answering("unused");
    let mut h = harness(Some(fake.clone()), true);
    h.state_mut().ai_prefs.endpoint = "http://example.com/v1".into();
    h.state_mut().ai_key = "secret".into();
    h.state_mut().execute("ai.summary");
    h.state_mut().start_assistant();
    h.run_steps(3);
    h.get_by_label_contains("only sent over https://");
    assert!(fake.seen.lock().unwrap().is_empty());

    // Without a way to send (the web build), the command says so and opens nothing.
    let mut h = harness(None, true);
    h.state_mut().execute("ai.summary");
    assert_eq!(h.state().dialog, None);
    assert!(h.state().toast.as_ref().is_some_and(|t| t.0.contains("isn't available in this build")));
}

#[test]
fn settings_keep_the_provider_but_never_the_key() {
    let mut app = PdfCraftApp::new();
    app.ai_prefs.enabled = true;
    app.ai_prefs.api = Api::Anthropic;
    app.ai_prefs.endpoint = "https://api.anthropic.com".into();
    app.ai_prefs.model = "some-model".into();
    app.ai_prefs.key_env = "MY_PROVIDER_KEY".into();
    app.ai_key = "sk-very-secret".into();
    let saved = app.persist();
    assert!(!saved.contains("sk-very-secret"), "the key must not be written to the settings");
    let mut restored = PdfCraftApp::new();
    restored.restore(&saved);
    assert_eq!(restored.ai_prefs, app.ai_prefs);
    assert!(restored.ai_key.is_empty());

    // Settings are untrusted: odd values fall back to off and empty, and long text is cut.
    let mut odd = PdfCraftApp::new();
    odd.restore(
        &serde_json::json!({ "ai": { "enabled": "yes", "api": 7, "endpoint": ["x"], "model": "m\u{0}\n".repeat(10_000), "key_env": null } })
            .to_string(),
    );
    assert!(!odd.ai_prefs.enabled && odd.ai_prefs.endpoint.is_empty() && odd.ai_prefs.api == Api::OpenAi);
    assert!(odd.ai_prefs.model.chars().count() <= 256 && !odd.ai_prefs.model.contains('\n'));
    let mut none = PdfCraftApp::new();
    none.restore("{}");
    assert_eq!(none.ai_prefs, Default::default());

    // An environment variable name that isn't one is refused, not looked up.
    let mut app = PdfCraftApp::new();
    app.ai_prefs = pdfcraft_ui_egui::AiPrefs {
        enabled: true,
        endpoint: "https://example.com/v1".into(),
        model: "m".into(),
        key_env: "NOT A NAME".into(),
        ..Default::default()
    };
    assert!(app.ai_provider().unwrap_err().contains("letters, digits and underscores"));
    app.ai_prefs.key_env = "PDFCRAFT_TEST_SURELY_UNSET_VARIABLE".into();
    assert!(app.ai_provider().unwrap_err().contains("is not set"));
}

#[test]
fn the_provider_can_be_set_through_view_options_but_never_the_key() {
    let mut app = PdfCraftApp::new();
    app.set_option("ai", "on").unwrap();
    app.set_option("ai-api", "anthropic").unwrap();
    app.set_option("ai-endpoint", " https://api.anthropic.com ").unwrap();
    app.set_option("ai-model", "some-model").unwrap();
    assert!(app.ai_prefs.enabled && app.ai_prefs.api == Api::Anthropic);
    assert_eq!((app.ai_prefs.endpoint.as_str(), app.ai_prefs.model.as_str()), ("https://api.anthropic.com", "some-model"));
    assert!(app.set_option("ai", "maybe").is_err());
    assert!(app.set_option("ai-api", "gemini").is_err());
    assert!(app.set_option("ai-key", "secret").is_err(), "a key is not a view option");
    app.set_option("ai", "off").unwrap();
    assert!(!app.ai_prefs.enabled);
}
