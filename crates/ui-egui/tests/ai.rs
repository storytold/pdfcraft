//! The AI assistant (M13) in the real shell (egui_kittest), with stand-in providers (no network):
//! setup, Summarize, Ask, Translate, errors, page citations and saved settings.

use std::sync::{Arc, Mutex};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::Session;
use pdfcraft_engine::ai::AiSettings;
use pdfcraft_ui_egui::ai_ui::AiProvider;
use pdfcraft_ui_egui::{PdfCraftApp, RightPanel};

const TEXT: &str = "Invoice 2026-114. Total due: 4,217.50 EUR. Payment within thirty days.";

fn settings() -> AiSettings {
    AiSettings { enabled: true, base_url: "http://127.0.0.1:9/v1".into(), model: "test-model".into(), api_key: "k".into() }
}

/// A provider that records each request and answers `reply` (a chat-completions body or an error).
fn provider(reply: Result<&'static str, &'static str>, seen: Arc<Mutex<Vec<(String, String, String)>>>) -> AiProvider {
    Arc::new(move |endpoint: &str, key: &str, body: &serde_json::Value| {
        seen.lock().unwrap().push((endpoint.to_string(), key.to_string(), body.to_string()));
        reply.map(str::to_string).map_err(str::to_string)
    })
}

fn harness(
    configured: bool,
    reply: Result<&'static str, &'static str>,
) -> (Harness<'static, PdfCraftApp>, Arc<Mutex<Vec<(String, String, String)>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let p = provider(reply, seen.clone());
    let doc = Session::new().create_from_text("Invoice", TEXT).unwrap().to_vec();
    let h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.ai_provider = Some(p.clone());
        if configured {
            app.ai_settings = settings();
        }
        app.open_bytes("invoice.pdf", None, doc.clone()).unwrap();
        app
    });
    (h, seen)
}

/// Run frames until the request has been answered (or give up).
fn settle(h: &mut Harness<'static, PdfCraftApp>) {
    for _ in 0..400 {
        h.run_steps(2);
        if h.query_by_label_contains("waiting for the model").is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.run_steps(2);
}

const ANSWER: &str = r#"{"choices":[{"message":{"content":"<think>checking</think>The total due is 4,217.50 EUR (p. 1)."}}]}"#;

#[test]
fn without_a_provider_set_up_the_commands_lead_to_preferences() {
    let (mut h, seen) = harness(false, Ok(ANSWER));
    h.run_steps(4);
    assert!(h.state_mut().execute("ai.summary"));
    h.run_steps(3);
    assert_eq!(h.state().right, Some(RightPanel::Assistant));
    h.get_by_label("Use an AI provider for Summarize, Ask and Translate");
    assert!(seen.lock().unwrap().is_empty(), "nothing is sent until a provider is set up");
}

#[test]
fn summarize_sends_the_document_and_shows_the_answer_with_its_pages() {
    let (mut h, seen) = harness(true, Ok(ANSWER));
    h.run_steps(4);
    assert!(h.state_mut().execute("ai.summary"));
    settle(&mut h);
    h.get_by_label("The total due is 4,217.50 EUR (p. 1).");
    let sent = seen.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    let (endpoint, key, body) = &sent[0];
    assert_eq!(endpoint, "http://127.0.0.1:9/v1/chat/completions");
    assert_eq!(key, "k");
    assert!(body.contains("[Page 1]") && body.contains("4,217.50 EUR"), "{body}");
    assert!(body.contains(r#""model":"test-model""#));
    // The cited page is a link.
    h.get_by_label("p. 1").click();
    h.run_steps(2);
    assert_eq!(h.state().views[0].current, 0);
}

#[test]
fn ask_sends_the_typed_question() {
    let (mut h, seen) = harness(true, Ok(ANSWER));
    h.run_steps(4);
    assert!(h.state_mut().execute("ai.ask"));
    h.run_steps(3);
    let field = h.get_by_role(egui::accesskit::Role::MultilineTextInput);
    field.focus();
    field.type_text("When is payment due?");
    h.run_steps(2);
    h.get_by_label("Ask").click();
    settle(&mut h);
    let sent = seen.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].2.contains("When is payment due?"));
    h.get_by_label("When is payment due?");
}

#[test]
fn translate_sends_the_current_page_and_the_language() {
    let (mut h, seen) = harness(true, Ok(r#"{"choices":[{"message":{"content":"Rechnung 2026-114."}}]}"#));
    h.run_steps(4);
    assert!(h.state_mut().execute("ai.translate"));
    settle(&mut h);
    h.get_by_label("Rechnung 2026-114.");
    h.get_by_label("Translate into English (page 1)");
    assert!(seen.lock().unwrap()[0].2.contains("into English"));
}

#[test]
fn failures_are_shown_and_the_app_keeps_running() {
    let (mut h, _) = harness(true, Err("couldn't reach the AI endpoint (connection refused)"));
    h.run_steps(4);
    assert!(h.state_mut().execute("ai.summary"));
    settle(&mut h);
    h.get_by_label_contains("connection refused");
    // A malformed answer and a provider that panics are reported, not fatal.
    for reply in [Ok("<html>502 Bad Gateway</html>"), Ok(r#"{"choices":[{"message":{"content":null}}]}"#)] {
        let (mut h, _) = harness(true, reply);
        h.run_steps(4);
        assert!(h.state_mut().execute("ai.summary"));
        settle(&mut h);
        h.get_by_label_contains("Couldn't get an answer");
    }
    let doc = Session::new().create_from_text("x", TEXT).unwrap().to_vec();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.ai_provider = Some(Arc::new(|_: &str, _: &str, _: &serde_json::Value| -> Result<String, String> { panic!("provider bug") }));
        app.ai_settings = settings();
        app.open_bytes("x.pdf", None, doc.clone()).unwrap();
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("ai.summary"));
    settle(&mut h);
    h.get_by_label_contains("provider bug");
}

#[test]
fn settings_are_saved_and_hostile_ones_are_tamed() {
    let mut app = PdfCraftApp::new();
    app.ai_settings = settings();
    let saved = app.persist();
    let mut back = PdfCraftApp::new();
    back.restore(&saved);
    assert_eq!(back.ai_settings, settings());
    // Off by default, and malformed settings change nothing.
    for bad in [r#"{"ai": 5}"#, r#"{"ai": {"enabled": "yes"}}"#, r#"{"ai": null}"#, "{}"] {
        let mut a = PdfCraftApp::new();
        a.restore(bad);
        assert!(!a.ai_settings.enabled, "{bad}");
    }
    let mut a = PdfCraftApp::new();
    a.restore(&format!(r#"{{"ai": {{"enabled": true, "base_url": "{}", "model": "m"}}}}"#, "x".repeat(100_000)));
    assert!(a.ai_settings.base_url.len() <= 2_048);
}
