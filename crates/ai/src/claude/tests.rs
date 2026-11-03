use super::*;
use crate::model::{HAIKU, OPUS};
use crate::source::Sha256;

fn key() -> Secret {
    Secret::new("sk-ant-test")
}

fn passage(page: u32, text: &str) -> Passage {
    Passage { id: SourceId::ThisDocument { page }, title: format!("report.pdf, page {}", page + 1), text: text.into() }
}

fn body(req: &HttpRequest) -> Value {
    serde_json::from_slice(&req.body).unwrap_or(Value::Null)
}

#[test]
fn golden_request() -> Result<(), RequestError> {
    let lib = Sha256::of(b"library doc");
    let passages = [
        passage(3, "Rates rose 3%. Costs fell.  "),
        Passage { id: SourceId::Library { sha256: lib, page: 0 }, title: "  ".into(), text: "Earlier rates were 2%.".into() },
        passage(4, " \n "),
    ];
    let image = Image { kind: ImageType::Png, bytes: vec![1, 2, 3] };
    let turn = user_turn(&passages, Some(&image), "  Why did rates rise?\n")?;
    let req = request(&Options::new(&OPUS), &key(), &[], &turn)?;
    assert_eq!(req.url, "https://api.anthropic.com/v1/messages");
    assert_eq!(
        req.preview().headers,
        vec![
            ("content-type".to_owned(), "application/json".to_owned()),
            ("x-api-key".to_owned(), "••••".to_owned()),
            ("anthropic-version".to_owned(), "2026-10-21".to_owned()),
            ("anthropic-beta".to_owned(), "server-side-fallback-2026-07-01".to_owned()),
        ]
    );
    let expected = json!({
        "model": "claude-opus-5-5",
        "max_tokens": 16000,
        "stream": true,
        "thinking": {"type": "adaptive", "display": "omitted"},
        "output_config": {"effort": "medium"},
        "system": [{"type": "text", "text": SYSTEM_PROMPT, "cache_control": {"type": "ephemeral"}}],
        "fallbacks": "default",
        "messages": [{"role": "user", "content": [
            {"type": "search_result", "source": "pdfcraft-doc:i3", "title": "report.pdf, page 4",
             "content": [{"type": "text", "text": "Rates rose 3%. "}, {"type": "text", "text": "Costs fell.  "}],
             "citations": {"enabled": true}},
            {"type": "search_result", "source": format!("pdfcraft-lib:{lib}:i0"), "title": "Passage",
             "content": [{"type": "text", "text": "Earlier rates were 2%."}],
             "citations": {"enabled": true}},
            {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AQID"}},
            {"type": "text", "text": "Why did rates rise?", "cache_control": {"type": "ephemeral"}},
        ]}],
    });
    assert_eq!(body(&req), expected);
    Ok(())
}

#[test]
fn haiku_gets_no_fallbacks_and_web_search_is_direct() -> Result<(), RequestError> {
    let turn = user_turn(&[], None, "Hi")?;
    let search = WebSearch { max_uses: 0, allowed_domains: vec![" iso.org ".into(), String::new()], blocked_domains: vec!["spam.example".into()] };
    let opts = Options { model: &HAIKU, effort: crate::model::Effort::Low, max_tokens: 0, fallbacks: true, web_search: Some(&search) };
    let req = request(&opts, &key(), &[], &turn)?;
    let b = body(&req);
    assert_eq!(b.get("fallbacks"), None);
    assert!(req.headers.iter().all(|(name, _)| name != "anthropic-beta"));
    assert_eq!(b["max_tokens"], 1);
    assert_eq!(b["output_config"], json!({"effort": "low"}));
    assert_eq!(
        b["tools"],
        json!([{"type": "web_search_20260209", "name": "web_search", "max_uses": 1, "allowed_callers": ["direct"], "allowed_domains": ["iso.org"]}])
    );
    let blocked_only = WebSearch { max_uses: 3, allowed_domains: vec![], blocked_domains: vec!["spam.example".into()] };
    let opts = Options { web_search: Some(&blocked_only), ..Options::new(&OPUS) };
    assert_eq!(body(&request(&opts, &key(), &[], &turn)?)["tools"][0]["blocked_domains"], json!(["spam.example"]));
    Ok(())
}

#[test]
fn history_is_replayed_with_one_breakpoint_at_the_end() -> Result<(), RequestError> {
    let first = user_turn(&[passage(0, "A.")], None, "Q1")?;
    let mut tampered = first.clone();
    for block in &mut tampered.content {
        if let Some(obj) = block.as_object_mut() {
            obj.insert("cache_control".into(), json!({"type": "ephemeral"}));
        }
    }
    let answer = Message {
        role: Role::Assistant,
        content: vec![json!({"type": "thinking", "thinking": "", "signature": "sig"}), json!({"type": "text", "text": "A1"})],
    };
    let second = user_turn(&[], None, "Q2")?;
    let req = request(&Options::new(&OPUS), &key(), &[tampered, answer.clone()], &second)?;
    let b = body(&req);
    let text = b.to_string();
    // The system prompt's and the new turn's: two breakpoints, never one per replayed block.
    assert_eq!(text.matches("cache_control").count(), 2, "{text}");
    assert_eq!(b["messages"][1], json!({"role": "assistant", "content": answer.content}));
    assert_eq!(b["messages"][2]["content"][0]["cache_control"], json!({"type": "ephemeral"}));
    assert_eq!(sent_sources(&[first, answer, second]), vec![Some(SourceId::ThisDocument { page: 0 })]);
    Ok(())
}

#[test]
fn turn_limits() {
    assert_eq!(user_turn(&[], None, " \n "), Err(RequestError::EmptyQuestion));
    assert_eq!(user_turn(&[], None, &"x".repeat(MAX_QUESTION_BYTES + 1)), Err(RequestError::QuestionTooLong));
    let many: Vec<Passage> = (0..=MAX_PASSAGES as u32).map(|i| passage(i, "x")).collect();
    assert_eq!(user_turn(&many, None, "q"), Err(RequestError::TooManyPassages));
    let big = Image { kind: ImageType::Jpeg, bytes: vec![0; MAX_IMAGE_BASE64_BYTES / 4 * 3 + 1] };
    assert_eq!(user_turn(&[], Some(&big), "q"), Err(RequestError::ImageTooLarge));
    // A passage longer than the cap is cut on a character boundary, never refused. (Sentences
    // start upper case: UAX #29 doesn't end a sentence at a period before a lower-case letter.)
    let long = passage(0, &"Él. ".repeat(MAX_PASSAGE_BYTES));
    let turn = user_turn(&[long], None, "q").expect("a long passage is cut, not refused");
    let blocks = turn.content[0]["content"].as_array().map(Vec::len);
    assert_eq!(blocks, Some(MAX_BLOCKS_PER_PASSAGE));
    assert!(turn.content[0].to_string().len() < MAX_PASSAGE_BYTES * 2);
}

#[test]
fn a_huge_history_is_refused() -> Result<(), RequestError> {
    let filler = Message { role: Role::User, content: vec![json!({"type": "text", "text": "x".repeat(MAX_HISTORY_BYTES + MAX_IMAGE_BASE64_BYTES)})] };
    let turn = user_turn(&[], None, "q")?;
    assert_eq!(request(&Options::new(&OPUS), &key(), &[filler], &turn), Err(RequestError::HistoryTooLong));
    Ok(())
}

const ANSWER: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","content":[],"model":"claude-opus-5-5","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":25,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":1200}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"EqQBsig"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: ping
data: {"type": "ping"}

event: content_block_start
data: {"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"citations_delta","citation":{"type":"search_result_location","cited_text":"Rates rose 3%. ","source":"pdfcraft-doc:i3","title":"report.pdf, page 4","search_result_index":0,"start_block_index":0,"end_block_index":1}}}

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Rates rose "}}

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"3%."}}

event: content_block_stop
data: {"type":"content_block_stop","index":1}

event: future_event
data: {"type":"future_event","anything":[1,2,3]}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":15}}

event: message_stop
data: {"type":"message_stop"}

"#;

fn decode_with(sources: Vec<Option<SourceId>>, chunks: &[&[u8]]) -> (Vec<Event>, Decoder) {
    let mut d = Decoder::new(sources);
    let mut out = Vec::new();
    for c in chunks {
        d.push(c, &mut out);
    }
    d.finish(&mut out);
    (out, d)
}

fn doc_sources() -> Vec<Option<SourceId>> {
    vec![Some(SourceId::ThisDocument { page: 3 })]
}

#[test]
fn decodes_a_cited_answer() {
    let (events, d) = decode_with(doc_sources(), &[ANSWER.as_bytes()]);
    let usage = Usage { input_tokens: 25, output_tokens: 15, cache_creation_input_tokens: 1200, cache_read_input_tokens: 0, web_search_requests: 0 };
    assert_eq!(
        events,
        vec![
            Event::Started { id: "msg_1".into(), model: "claude-opus-5-5".into() },
            Event::Citation { block: 1, source: Source::ThisDocument { page: 3, quote: "Rates rose 3%. ".into() } },
            Event::Text { block: 1, text: "Rates rose ".into() },
            Event::Text { block: 1, text: "3%.".into() },
            Event::Usage { usage },
            Event::Stop { reason: StopReason::EndTurn },
        ]
    );
    assert_eq!(d.usage(), usage);
    // The assembled message is what the API would have returned, ready to replay.
    let m = d.message();
    assert_eq!(m.role, Role::Assistant);
    assert_eq!(m.content[0], json!({"type": "thinking", "thinking": "", "signature": "EqQBsig"}));
    assert_eq!(m.content[1]["text"], "Rates rose 3%.");
    assert_eq!(m.content[1]["citations"][0]["source"], "pdfcraft-doc:i3");
}

#[test]
fn citations_of_passages_not_sent_are_skipped() {
    let skipped = |sources: Vec<Option<SourceId>>| {
        let (events, _) = decode_with(sources, &[ANSWER.as_bytes()]);
        events.iter().any(|e| matches!(e, Event::Skipped { .. })) && !events.iter().any(|e| matches!(e, Event::Citation { .. }))
    };
    // Nothing sent at index 0, a different passage there, or one that wasn't ours.
    assert!(skipped(vec![]));
    assert!(skipped(vec![Some(SourceId::ThisDocument { page: 2 })]));
    assert!(skipped(vec![None]));
    assert!(!skipped(doc_sources()));
}

const SEARCH: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_2","model":"claude-opus-5-5","usage":{"input_tokens":100,"output_tokens":1}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"server_tool_use","id":"srvtoolu_1","name":"web_search"}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"query\": \"pdf "}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"2.0 spec\"}"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: content_block_start
data: {"type":"content_block_start","index":1,"content_block":{"type":"web_search_tool_result","tool_use_id":"srvtoolu_1","content":[{"type":"web_search_result","title":"ISO 32000-2","url":"https://www.iso.org/standard/75839.html","encrypted_content":"EqgfCio","page_age":"2020"}]}}

event: content_block_stop
data: {"type":"content_block_stop","index":1}

event: content_block_start
data: {"type":"content_block_start","index":2,"content_block":{"type":"web_search_tool_result","tool_use_id":"srvtoolu_x","content":{"type":"web_search_tool_result_error","error_code":"max_uses_exceeded"}}}

event: content_block_stop
data: {"type":"content_block_stop","index":2}

event: content_block_start
data: {"type":"content_block_start","index":3,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":3,"delta":{"type":"citations_delta","citation":{"type":"web_search_result_location","url":"https://www.iso.org/standard/75839.html","title":"ISO 32000-2","encrypted_index":"Eo8B","cited_text":"PDF 2.0"}}}

event: content_block_delta
data: {"type":"content_block_delta","index":3,"delta":{"type":"citations_delta","citation":{"type":"web_search_result_location","url":"javascript:alert(1)","title":"x","encrypted_index":"E","cited_text":"x"}}}

event: content_block_delta
data: {"type":"content_block_delta","index":3,"delta":{"type":"text_delta","text":"PDF 2.0 is ISO 32000-2."}}

event: content_block_stop
data: {"type":"content_block_stop","index":3}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"pause_turn"},"usage":{"output_tokens":40,"server_tool_use":{"web_search_requests":1}}}

event: message_stop
data: {"type":"message_stop"}

"#;

#[test]
fn decodes_web_search() {
    let (events, d) = decode_with(vec![], &[SEARCH.as_bytes()]);
    let iso = "https://www.iso.org/standard/75839.html";
    assert_eq!(
        events,
        vec![
            Event::Started { id: "msg_2".into(), model: "claude-opus-5-5".into() },
            Event::WebSearch { query: "pdf 2.0 spec".into() },
            Event::WebResults { results: vec![WebResult { url: iso.into(), title: "ISO 32000-2".into() }], error: None },
            Event::WebResults { results: vec![], error: Some("max_uses_exceeded".into()) },
            Event::Citation { block: 3, source: Source::Web { url: iso.into(), title: "ISO 32000-2".into(), quote: "PDF 2.0".into() } },
            Event::Skipped { what: "a citation of a passage that wasn't sent".into() },
            Event::Text { block: 3, text: "PDF 2.0 is ISO 32000-2.".into() },
            Event::Usage { usage: Usage { input_tokens: 100, output_tokens: 40, web_search_requests: 1, ..Default::default() } },
            Event::Stop { reason: StopReason::PauseTurn },
        ]
    );
    // Sent back unchanged to resume the paused turn: the tool input parsed, the encrypted content kept.
    let m = d.message();
    assert_eq!(m.content[0], json!({"type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search", "input": {"query": "pdf 2.0 spec"}}));
    assert_eq!(m.content[1]["content"][0]["encrypted_content"], "EqgfCio");
    assert_eq!(m.content[3]["citations"].as_array().map(Vec::len), Some(2));
}

#[test]
fn errors_and_truncation() {
    let mid = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{}}\n\nevent: error\ndata: {\"type\": \"error\", \"error\": {\"type\": \"overloaded_error\", \"message\": \"Overloaded\"}}\n\ndata: {\"type\":\"message_stop\"}\n\n";
    let (events, _) = decode_with(vec![], &[mid.as_bytes()]);
    assert_eq!(
        events,
        vec![
            Event::Started { id: String::new(), model: String::new() },
            Event::ProviderError { kind: "overloaded_error".into(), message: "Overloaded".into() },
        ]
    );

    let cut = ANSWER.split("event: message_delta").next().unwrap_or_default();
    let (events, d) = decode_with(doc_sources(), &[cut.as_bytes()]);
    assert_eq!(events.last(), Some(&Event::Stop { reason: StopReason::Truncated }));
    assert_eq!(d.stop_reason(), Some(&StopReason::Truncated));

    let junk = "data: {not json\n\ndata: {\"type\":\"content_block_delta\",\"index\":9,\"delta\":{\"type\":\"text_delta\",\"text\":\"x\"}}\n\ndata: {\"type\":\"content_block_start\",\"index\":\"0\"}\n\n";
    let (events, _) = decode_with(vec![], &[junk.as_bytes()]);
    assert_eq!(events.iter().filter(|e| matches!(e, Event::Skipped { .. })).count(), 3);

    assert_eq!(
        error_event(401, br#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"},"request_id":"req_1"}"#),
        Event::ProviderError { kind: "authentication_error".into(), message: "invalid x-api-key".into() }
    );
    assert_eq!(
        error_event(502, b"<html>Bad gateway</html>"),
        Event::ProviderError { kind: "http_502".into(), message: "<html>Bad gateway</html>".into() }
    );
}

#[test]
fn an_endless_answer_stops_at_the_cap() {
    let mut d = Decoder::new(vec![]);
    let mut out = Vec::new();
    d.push(b"data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n", &mut out);
    let chunk = format!(
        "data: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"{}\"}}}}\n\n",
        "x".repeat(64 * 1024)
    );
    for _ in 0..(MAX_ANSWER_BYTES / (64 * 1024) + 2) {
        d.push(chunk.as_bytes(), &mut out);
    }
    d.finish(&mut out);
    assert_eq!(out.last(), Some(&Event::Stop { reason: StopReason::TooLong }));
    assert_eq!(out.iter().filter(|e| matches!(e, Event::Stop { .. })).count(), 1);
    let kept = d.message().content[0]["text"].as_str().map(str::len).unwrap_or(0);
    assert!(kept <= MAX_ANSWER_BYTES, "{kept}");
}

proptest::proptest! {
    /// Where the transport splits the body never changes what is decoded.
    #[test]
    fn decoder_split_invariance(a in 0usize..4000, b in 0usize..4000) {
        let bytes = SEARCH.as_bytes();
        let (a, b) = (a.min(bytes.len()), b.min(bytes.len()));
        let (lo, hi) = (a.min(b), a.max(b));
        let (whole, _) = decode_with(vec![], &[bytes]);
        let (split, _) = decode_with(vec![], &[&bytes[..lo], &bytes[lo..hi], &bytes[hi..]]);
        proptest::prop_assert_eq!(whole, split);
    }

    #[test]
    fn decoder_never_panics(body in proptest::collection::vec(proptest::num::u8::ANY, 0..3000)) {
        let _ = decode_with(doc_sources(), &[&body]);
    }

    /// Valid events in any order, with any indices, never panic either.
    #[test]
    fn decoder_survives_shuffled_events(order in proptest::collection::vec(0usize..40, 0..60)) {
        let events: Vec<&str> = ANSWER.split("\n\n").chain(SEARCH.split("\n\n")).collect();
        let mut body = String::new();
        for i in order {
            if let Some(e) = events.get(i % events.len().max(1)) {
                body.push_str(e);
                body.push_str("\n\n");
            }
        }
        let _ = decode_with(doc_sources(), &[body.as_bytes()]);
    }
}
