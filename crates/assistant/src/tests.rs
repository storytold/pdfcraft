use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use super::*;

/// A synthetic PDF: one page per content stream (`(content, ocr_mark, with_image)`), Helvetica as
/// /F1 and a 2×2 grey image as /Im1.
pub(crate) fn pdf(pages: &[(&str, bool, bool)]) -> Vec<u8> {
    let mut objs: Vec<String> = Vec::new();
    let n = pages.len();
    // 1 catalog, 2 pages, 3 font, 4 image, then (page, content) pairs.
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 5 + 2 * i)).collect();
    objs.push("<< /Type /Catalog /Pages 2 0 R >>".into());
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" ")));
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into());
    objs.push("<< /Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 4 >>\nstream\n\u{80}\u{80}\u{80}\u{80}\nendstream".into());
    for (i, (content, ocr, image)) in pages.iter().enumerate() {
        let draw = if *image { format!("q 500 0 0 700 50 50 cm /Im1 Do Q\n{content}") } else { content.to_string() };
        objs.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << /Font << /F1 3 0 R >> /XObject << /Im1 4 0 R >> >> /Contents {} 0 R >>",
            6 + 2 * i
        ));
        let mark = if *ocr { " /PCMark /OCR" } else { "" };
        objs.push(format!("<< /Length {}{mark} >>\nstream\n{draw}\nendstream", draw.len()));
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).bytes());
    for off in offsets {
        out.extend(format!("{off:010} 00000 n \n").bytes());
    }
    out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).bytes());
    out
}

fn text(s: &str) -> String {
    format!("BT /F1 12 Tf 72 700 Td ({s}) Tj ET")
}

fn invisible(s: &str) -> String {
    format!("BT 3 Tr /F1 12 Tf 72 700 Td ({s}) Tj ET")
}

fn read(bytes: Vec<u8>) -> DocText {
    pages::read_document(Arc::new(bytes), None, &|| false).unwrap()
}

#[test]
fn pages_say_where_their_text_comes_from() {
    let doc = read(pdf(&[
        (&text("El contrato vence el treinta de junio."), false, false),
        (&invisible("Factura numero 4521 del proveedor."), false, true),
        (&text("Texto escrito por el OCR de PdfCraft."), true, true),
        ("", false, true),
        ("", false, false),
        (&format!("{}\n{}", text("Parte visible"), invisible("Parte reconocida")), false, true),
    ]));
    let origins: Vec<_> = doc.pages.iter().map(|p| p.origin.clone()).collect();
    assert_eq!(
        origins,
        vec![
            Origin::Extracted,
            Origin::OcrLayer,
            Origin::OcrLayer,
            Origin::NoText { pictures: true },
            Origin::NoText { pictures: false },
            Origin::Mixed
        ]
    );
    assert!(doc.pages[0].text.contains("treinta de junio"));
    assert_eq!(doc.scanned_without_text(), vec![3]);
}

#[test]
fn recognised_text_below_the_threshold_is_illegible() {
    let mut doc = read(pdf(&[("", false, true), ("", false, true)]));
    doc.apply_ocr(0, "Acta de la asamblea".into(), Some(88.0), 50.0);
    doc.apply_ocr(1, "x~ ;; l1".into(), Some(21.0), 50.0);
    assert_eq!(doc.pages[0].origin, Origin::OcrNow { confidence: Some(88.0) });
    assert_eq!(doc.pages[1].origin, Origin::Illegible { confidence: Some(21.0) });
    assert!(doc.pages[1].text.is_empty());
    let p = context::build(&doc, &Task::Summary, 10_000, true);
    assert_eq!(p.included, vec![0]);
    assert_eq!(p.unreadable, vec![1]);
}

fn two_pages() -> DocText {
    DocText {
        pages: vec![
            PageSource {
                page: 0,
                origin: Origin::Extracted,
                text: "El arrendatario pagará un canon mensual de\nmil doscientos pesos por adelan-\ntado.".into(),
                truncated: false,
            },
            PageSource {
                page: 1,
                origin: Origin::OcrLayer,
                text: "La rescisión anticipada exige un preaviso de “sesenta días”.".into(),
                truncated: false,
            },
        ],
        unread_pages: 0,
    }
}

#[test]
fn only_quotations_found_in_the_page_text_are_shown() {
    let doc = two_pages();
    let raw = r#"{"points":[
        {"statement":"El canon es mensual.","quotes":[{"page":1,"text":"un canon mensual de mil doscientos pesos por adelantado"}]},
        {"statement":"Preaviso.","quotes":[{"page":1,"text":"exige un preaviso de \"sesenta días\""}]},
        {"statement":"Inventado.","quotes":[{"page":2,"text":"el inquilino puede subarrendar libremente"}]},
        {"statement":"Corto.","quotes":[{"page":1,"text":"canon"}]}
    ],"not_found":false}"#;
    let a = verify::check(raw, &doc, &[0, 1]);
    assert_eq!(a.points.len(), 4);
    // Matched across the line break and the hyphenation; shown as the page has it.
    assert_eq!(a.points[0].quotes[0].text, "un canon mensual de mil doscientos pesos por adelan- tado");
    assert_eq!(a.points[0].quotes[0].page, 0);
    // Found on page 2 although the model said page 1: kept, with the right page and a note.
    assert_eq!(a.points[1].quotes[0].page, 1);
    assert_eq!(a.points[1].quotes[0].claimed_page, Some(1));
    assert_eq!(a.points[1].quotes[0].origin, Origin::OcrLayer);
    assert!(!a.points[2].supported());
    assert!(!a.points[3].supported());
    assert_eq!(a.rejected.len(), 2);
}

#[test]
fn a_quotation_on_a_page_not_sent_is_refused() {
    let doc = two_pages();
    let raw = r#"{"points":[{"statement":"x","quotes":[{"page":2,"text":"exige un preaviso de sesenta días"}]}],"not_found":false}"#;
    let a = verify::check(raw, &doc, &[0]);
    assert!(a.points[0].quotes.is_empty());
    assert!(a.rejected[0].reason.contains("no fue enviada"));
}

#[test]
fn unreadable_replies_and_not_found_are_reported() {
    let doc = two_pages();
    assert!(verify::check("no sé", &doc, &[0, 1]).unparsed.is_some());
    let a = verify::check("```json\n{\"points\":[],\"not_found\":true}\n```", &doc, &[0, 1]);
    assert!(a.not_found && a.points.is_empty());
}

#[test]
fn ellipses_join_parts_in_order() {
    let doc = two_pages();
    let raw = r#"{"points":[{"statement":"x","quotes":[{"page":1,"text":"El arrendatario pagará … por adelantado"}]}],"not_found":false}"#;
    let a = verify::check(raw, &doc, &[0, 1]);
    assert_eq!(a.points[0].quotes.len(), 1);
}

#[test]
fn questions_choose_the_matching_pages_when_the_document_does_not_fit() {
    let mut pages = Vec::new();
    for i in 0..30 {
        let body = if i == 17 {
            "La cláusula de jurisdicción fija los tribunales de Rosario. ".repeat(20)
        } else {
            "Texto general sin relación. ".repeat(40)
        };
        pages.push(PageSource { page: i, origin: Origin::Extracted, text: body, truncated: false });
    }
    let doc = DocText { pages, unread_pages: 0 };
    let p = context::build(&doc, &Task::Question("¿Qué tribunales fija la jurisdicción?".into()), 4000, true);
    assert!(p.included.contains(&17), "{:?}", p.included);
    assert!(p.included.len() < 30);
    assert!(!p.omitted.is_empty());
    assert!(p.user.contains("=== Página 18 (texto extraído) ==="));
}

#[test]
fn only_loopback_addresses_are_local() {
    for url in ["http://127.0.0.1:11434", "http://localhost:1234/v1", "http://[::1]:8080", "https://127.5.0.1"] {
        assert!(provider::is_local(url).unwrap(), "{url}");
    }
    for url in ["https://api.example.com/v1", "http://192.168.1.10:11434", "http://10.0.0.2"] {
        assert!(!provider::is_local(url).unwrap(), "{url}");
    }
    // A user name can disguise the host.
    assert!(provider::is_local("http://127.0.0.1@evil.example").is_err());
    assert!(provider::is_local("ftp://127.0.0.1").is_err());
}

#[test]
fn an_external_endpoint_needs_the_persons_choice() {
    let ep = Endpoint { kind: Kind::OpenAi, base_url: "https://api.example.com/v1".into(), model: "m".into(), api_key: None };
    let err = run(&ep, &two_pages(), &Task::Summary, &Settings::default(), &AtomicBool::new(false), |_| {}).unwrap_err();
    assert!(err.contains("no está en esta Mac"), "{err}");
}

#[test]
fn recognised_text_keeps_the_originals_ocr_limits() {
    let mut doc = read(pdf(&[("", false, true), ("", false, true)]));
    doc.apply_ocr(0, "palabra ".repeat(pages::MAX_OCR_WORDS - 2), None, pages::LOW_CONFIDENCE);
    assert!(matches!(doc.pages[0].origin, Origin::OcrNow { .. }));
    // Together the two pages pass 65 536 words: the second is refused, and says why.
    doc.apply_ocr(1, "otra palabra más".into(), None, pages::LOW_CONFIDENCE);
    assert!(matches!(&doc.pages[1].origin, Origin::Failed(e) if e.contains("límite")));
    // A reported confidence in the original's low band (< 70) is illegible.
    let mut doc = read(pdf(&[("", false, true)]));
    doc.apply_ocr(0, "Acta de la asamblea ordinaria".into(), Some(69.0), pages::LOW_CONFIDENCE);
    assert!(matches!(doc.pages[0].origin, Origin::Illegible { .. }));
}

#[test]
fn cancelling_closes_a_silent_local_request_promptly() {
    // A server that accepts and never answers, like Ollama while a large model loads.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut conn, _) = listener.accept().unwrap();
        let mut buf = [0u8; 4096];
        // Read until the client closes the connection: that is what makes a server stop.
        while let Ok(n) = std::io::Read::read(&mut conn, &mut buf) {
            if n == 0 {
                break;
            }
        }
    });
    let cancel = Arc::new(AtomicBool::new(false));
    let c = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        c.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    let started = std::time::Instant::now();
    let url = format!("http://127.0.0.1:{port}/api/chat");
    let r = local_http::post_lines(&url, "{}", None, Duration::from_secs(60), 1 << 20, &cancel, |_| Ok(true));
    assert_eq!(r, Err(local_http::CANCELLED.to_owned()));
    // Before the fix this spun forever (an `Interrupted` error is retried by the line reader).
    assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
    // The server saw the connection closed.
    server.join().unwrap();
}

#[test]
fn subscription_cli_requires_session_consent_before_starting() {
    let doc =
        pages::read_document(Arc::from(pdf(&[("BT /F1 12 Tf 50 700 Td (Synthetic document text only.) Tj ET", false, false)])), None, &|| false)
            .unwrap();
    for kind in [Kind::ClaudeCode, Kind::Codex] {
        let ep = Endpoint { kind, base_url: "/nonexistent/program-must-not-run".into(), model: "default".into(), api_key: None };
        let result = run(&ep, &doc, &Task::Summary, &Settings::default(), &AtomicBool::new(false), |_| {});
        assert!(result.unwrap_err().contains("expresamente"));
    }
}

#[test]
fn openai_without_a_key_is_rejected_before_network_access() {
    let mut ep = Endpoint { kind: Kind::OpenAi, base_url: "https://api.openai.com/v1".into(), model: "example".into(), api_key: None };
    assert!(provider::missing_api_key(&ep));
    assert!(provider::list_models(&ep).unwrap_err().contains("No se realizó ninguna solicitud"));
    ep.api_key = Some("   ".into());
    assert!(provider::missing_api_key(&ep));
    ep.kind = Kind::Codex;
    assert!(!provider::missing_api_key(&ep));
    ep.kind = Kind::OpenAi;
    ep.base_url = "http://127.0.0.1:1234/v1".into();
    assert!(!provider::missing_api_key(&ep));
}

#[test]
fn user_selected_answer_language_controls_questions_and_summaries() {
    let doc = pages::read_document(Arc::new(pdf(&[("BT /F1 12 Tf 50 700 Td (The workshop starts at ten.) Tj ET", false, false)])), None, &|| false)
        .unwrap();
    let question = Task::Question("Que horario tiene?".into());
    let es = context::build(&doc, &question, 36_000, true);
    let en = context::build(&doc, &question, 36_000, false);
    assert!(es.user.contains("responda en español"));
    assert!(en.user.contains("answer in English"));
    assert!(es.user.contains("The workshop starts at ten."));
    assert!(en.user.contains("The workshop starts at ten."));
    assert!(context::build(&doc, &Task::Summary, 36_000, false).user.contains("in English"));
    assert!(context::build(&doc, &Task::Summary, 36_000, true).user.contains("en español"));
}

#[test]
fn endpoint_debug_never_prints_credentials() {
    let ep = Endpoint { api_key: Some("synthetic-do-not-log".into()), ..Endpoint::default() };
    assert!(!format!("{ep:?}").contains("synthetic-do-not-log"));
}

#[test]
fn ambiguous_provider_urls_are_refused() {
    for url in [
        "http://localhost\\example.com",
        "http://localhost/a\r\nHost: example.com",
        "http://localhost?next=example.com",
        "https://user:pass@example.com",
    ] {
        assert!(provider::is_local(url).is_err(), "{url}");
    }
}

#[test]
fn unapplied_redactions_block_extraction() {
    let bytes = pdf(&[(&text("Synthetic confidential passage"), false, false)]);
    let mut doc = pdfcraft_cos::Document::open(Arc::new(bytes)).unwrap();
    let page = pdfcraft_model::pages(&doc).first().unwrap().clone();
    let mut dict = page.dict;
    let mut mark = pdfcraft_cos::Dict::new();
    mark.set(b"Subtype".to_vec(), pdfcraft_cos::Object::Name(b"Redact".to_vec()));
    dict.set(b"Annots".to_vec(), pdfcraft_cos::Object::Array(vec![pdfcraft_cos::Object::Dict(mark)]));
    doc.set(page.obj, pdfcraft_cos::Object::Dict(dict));
    let bytes = pdfcraft_cos::write_full(&doc, &pdfcraft_cos::SaveOptions::default()).unwrap();
    assert!(pages::read_document(Arc::new(bytes), None, &|| false).unwrap_err().contains("redaction"));
}

#[test]
fn copy_restrictions_block_extraction() {
    use pdfcraft_cos::{Algorithm, NewEncryption, SaveOptions, write_full};
    let mut doc = pdfcraft_cos::Document::open(Arc::new(pdf(&[(&text("Restricted synthetic text"), false, false)]))).unwrap();
    doc.set_encryption_legacy(&NewEncryption {
        algorithm: Algorithm::Aes128,
        user_password: "reader",
        owner_password: "owner",
        permissions: -3904,
        encrypt_metadata: true,
        seed: [9; 32],
    })
    .unwrap();
    let bytes = Arc::new(write_full(&doc, &SaveOptions::default()).unwrap());
    assert!(pages::read_document(bytes.clone(), Some("reader"), &|| false).unwrap_err().contains("copying"));
    assert!(pages::read_document(bytes, Some("owner"), &|| false).is_ok());
}

#[test]
fn ollama_cloud_entries_are_not_presented_as_local_models() {
    for entry in [
        serde_json::json!({"name":"example:cloud"}),
        serde_json::json!({"name":"example:large-cloud"}),
        serde_json::json!({"name":"example:local", "remote_host":"https://example.com"}),
    ] {
        assert!(!provider::local_ollama_model(&entry));
    }
    assert!(provider::local_ollama_model(&serde_json::json!({"name":"example:local"})));
}

#[test]
fn compatible_loopback_provider_receives_key_and_returns_verified_answer() {
    use std::io::{BufRead, BufReader, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let worker = std::thread::spawn(move || {
        for i in 0..2 {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut reader = BufReader::new(socket.try_clone().unwrap());
            let mut request = String::new();
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse::<usize>().unwrap();
                }
                request.push_str(&line);
            }
            assert!(request.to_ascii_lowercase().contains("authorization: bearer synthetic-key"));
            assert!(length < 100_000);
            let mut bytes = vec![0; length];
            reader.read_exact(&mut bytes).unwrap();
            let body = if i == 0 {
                "{\"data\":[{\"id\":\"fixture\"}]}".to_owned()
            } else {
                let prompt: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(prompt["model"], "fixture");
                let answer = serde_json::json!({"points":[],"not_found":true}).to_string();
                format!("data: {}\n\ndata: [DONE]\n\n", serde_json::json!({"choices":[{"delta":{"content":answer}}]}))
            };
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        }
    });
    let ep = Endpoint { kind: Kind::OpenAi, base_url: base, model: "fixture".into(), api_key: Some("synthetic-key".into()) };
    let outcome = run(&ep, &two_pages(), &Task::Summary, &Settings::default(), &AtomicBool::new(false), |_| {}).unwrap();
    assert!(outcome.answer.not_found);
    worker.join().unwrap();
}
