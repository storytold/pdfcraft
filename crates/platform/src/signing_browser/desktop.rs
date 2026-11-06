use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

use base64::{Engine as _, prelude::BASE64_STANDARD};
use serde_json::{Value, json};

use super::{AUSTRIA_ENDPOINT, is_provider_url};

mod process;

const MAX_PACKET: u64 = 4 * 1024 * 1024;
const MAX_XML: usize = 2 * 1024 * 1024;

pub(super) fn find_browser() -> Result<PathBuf, String> {
    if let Some(value) = std::env::var_os("PDFCRAFT_SIGNING_BROWSER") {
        let path = PathBuf::from(value);
        if path.is_absolute() && path.is_file() {
            return Ok(path);
        }
        return Err("PDFCRAFT_SIGNING_BROWSER must name an existing absolute Chromium, Chrome or Edge executable".into());
    }
    let mut candidates = Vec::new();
    #[cfg(target_os = "windows")]
    for name in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
        if let Some(base) = std::env::var_os(name) {
            for relative in ["Microsoft/Edge/Application/msedge.exe", "Google/Chrome/Application/chrome.exe", "Chromium/Application/chrome.exe"] {
                candidates.push(PathBuf::from(&base).join(relative));
            }
        }
    }
    #[cfg(target_os = "macos")]
    for path in [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
    ] {
        candidates.push(PathBuf::from(path));
    }
    #[cfg(unix)]
    for path in ["/usr/bin/chromium", "/usr/bin/chromium-browser", "/usr/bin/google-chrome", "/usr/local/bin/chromium", "/usr/local/bin/chrome"] {
        candidates.push(PathBuf::from(path));
    }
    candidates.into_iter().find(|p| p.is_absolute() && p.is_file()).ok_or_else(|| {
        "ID Austria needs an installed Chromium, Chrome or Edge browser; set PDFCRAFT_SIGNING_BROWSER to its absolute executable path".into()
    })
}

/// Never format protocol replies or browser stderr into an error: either can hold private
/// session information. CDP traffic remains in memory and is not connected to logging.
struct Protocol {
    writer: mpsc::SyncSender<Value>,
    reader: mpsc::Receiver<Result<Value, String>>,
    next: u64,
    cancel: Arc<AtomicBool>,
    deadline: Instant,
}

impl Protocol {
    fn send(&mut self, method: &str, params: Value, session: Option<&str>) -> Result<u64, String> {
        self.next = self.next.checked_add(1).ok_or("the browser request limit was reached")?;
        let mut message = json!({"id":self.next,"method":method,"params":params});
        if let Some(session) = session {
            message["sessionId"] = json!(session);
        }
        self.writer.try_send(message).map_err(|_| "the signing browser could not accept a protocol request")?;
        Ok(self.next)
    }

    fn receive(&self) -> Result<Value, String> {
        self.receive_before(self.deadline)
    }

    fn receive_before(&self, until: Instant) -> Result<Value, String> {
        loop {
            if self.cancel.load(Ordering::Acquire) {
                return Err("signing was cancelled; no file was saved".into());
            }
            if Instant::now() >= self.deadline.min(until) {
                return Err("the signing browser session expired; start again after checking the document".into());
            }
            match self.reader.recv_timeout(Duration::from_millis(100)) {
                Ok(value) => return value,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err("the signing browser was closed".into()),
            }
        }
    }

    fn call(&mut self, method: &str, params: Value, session: Option<&str>) -> Result<Value, String> {
        let id = self.send(method, params, session)?;
        let started = Instant::now();
        loop {
            if started.elapsed() > Duration::from_secs(30) {
                return Err("the signing browser did not answer its setup request".into());
            }
            let message = self.receive_before(started + Duration::from_secs(30))?;
            if message.get("id").and_then(Value::as_u64) == Some(id) {
                if message.get("error").is_some() {
                    return Err("the signing browser does not support the required protocol operation".into());
                }
                return Ok(message.get("result").cloned().unwrap_or(Value::Null));
            }
        }
    }
}

fn read_packets(reader: Box<dyn Read + Send>, sender: mpsc::SyncSender<Result<Value, String>>) {
    let mut reader = BufReader::new(reader);
    loop {
        let mut packet = Vec::new();
        let result = reader.by_ref().take(MAX_PACKET + 1).read_until(0, &mut packet);
        if result.is_err() || packet.is_empty() {
            let _ = sender.send(Err("the signing browser was closed".into()));
            break;
        }
        if packet.len() as u64 > MAX_PACKET || packet.last() != Some(&0) {
            let _ = sender.send(Err("the signing browser protocol response exceeded its limit".into()));
            break;
        }
        packet.pop();
        let parsed = serde_json::from_slice(&packet).map_err(|_| "the signing browser returned invalid protocol data".to_string());
        if sender.send(parsed).is_err() {
            break;
        }
    }
}

/// Preserve the real browser's public navigation headers when changing its initial GET to
/// a POST. Fetch.continueRequest replaces the header set; login cookies/credentials are
/// never copied into a program-built request, and later requests are not intercepted.
fn initial_headers(params: &Value) -> Vec<Value> {
    let mut headers = Vec::new();
    if let Some(original) = params.pointer("/request/headers").and_then(Value::as_object) {
        for (name, value) in original {
            if matches!(
                name.to_ascii_lowercase().as_str(),
                "user-agent"
                    | "accept"
                    | "accept-language"
                    | "sec-fetch-site"
                    | "sec-fetch-mode"
                    | "sec-fetch-dest"
                    | "sec-fetch-user"
                    | "sec-ch-ua"
                    | "sec-ch-ua-mobile"
                    | "sec-ch-ua-platform"
            ) && let Some(value) = value.as_str().filter(|v| v.len() <= 8192 && !v.contains(['\r', '\n']))
            {
                headers.push(json!({"name":name,"value":value}));
            }
        }
    }
    headers.push(json!({"name":"Content-Type","value":"application/x-www-form-urlencoded"}));
    headers
}

fn xml_body(result: &Value) -> Result<Vec<u8>, String> {
    let text = result.get("content").and_then(Value::as_str).ok_or("the browser returned no XML resource")?;
    if text.len() > MAX_XML * 2 {
        return Err("the provider response exceeded 2 MiB".into());
    }
    let bytes = if result.get("base64Encoded").and_then(Value::as_bool) == Some(true) {
        BASE64_STANDARD.decode(text).map_err(|_| "the browser returned invalid resource encoding")?
    } else {
        text.as_bytes().to_vec()
    };
    if bytes.len() > MAX_XML {
        return Err("the provider response exceeded 2 MiB".into());
    }
    Ok(bytes)
}

pub(super) fn exchange(executable: &Path, xml: &[u8], cancel: &Arc<AtomicBool>) -> Result<Vec<u8>, String> {
    exchange_inner(executable, xml, cancel, false)
}

fn exchange_inner(executable: &Path, xml: &[u8], cancel: &Arc<AtomicBool>, headless_test: bool) -> Result<Vec<u8>, String> {
    let profile = tempfile::Builder::new().prefix("pdfcraft-sign-").tempdir().map_err(|_| "could not create a private signing browser profile")?;
    let (child, writer, reader) = process::launch(executable, profile.path(), headless_test)?;
    let (sender, receiver) = mpsc::sync_channel(16);
    let write_error = sender.clone();
    let read_thread = std::thread::Builder::new()
        .name("signature-browser-pipe".into())
        .spawn(move || read_packets(reader, sender))
        .map_err(|_| "could not read the signing browser pipe")?;
    let (write_tx, write_rx) = mpsc::sync_channel(4);
    let write_thread = std::thread::Builder::new()
        .name("signature-browser-write".into())
        .spawn(move || {
            let mut writer = writer;
            while let Ok(message) = write_rx.recv() {
                if serde_json::to_writer(&mut writer, &message).is_err() || writer.write_all(&[0]).is_err() || writer.flush().is_err() {
                    let _ = write_error.send(Err("could not write the signing browser pipe".into()));
                    break;
                }
            }
        })
        .map_err(|_| "could not write the signing browser pipe")?;
    let mut protocol =
        Protocol { writer: write_tx, reader: receiver, next: 0, cancel: cancel.clone(), deadline: Instant::now() + Duration::from_secs(600) };
    let result = run_exchange(&mut protocol, xml, headless_test);
    // Kill our isolated browser and its helpers before releasing the private profile. Never
    // terminate another browser instance or leave a blocked reader holding the document.
    drop(protocol);
    drop(child);
    let _ = read_thread.join();
    let _ = write_thread.join();
    // Some Windows browser helpers take a moment to release their profile files. TempDir
    // makes a best-effort cleanup; no reusable credential profile is opened on a later run.
    drop(profile);
    result
}

fn run_exchange(protocol: &mut Protocol, xml: &[u8], _probe: bool) -> Result<Vec<u8>, String> {
    let started = Instant::now();
    let target = loop {
        let targets = protocol.call("Target.getTargets", json!({}), None)?;
        if let Some(target) = targets
            .get("targetInfos")
            .and_then(Value::as_array)
            .and_then(|ts| ts.iter().find(|t| t.get("type").and_then(Value::as_str) == Some("page")))
            .and_then(|t| t.get("targetId"))
            .and_then(Value::as_str)
        {
            break target.to_string();
        }
        if started.elapsed() >= Duration::from_secs(30) {
            return Err("the signing browser did not create a page".into());
        }
        if protocol.cancel.load(Ordering::Acquire) {
            return Err("signing was cancelled".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let attached = protocol.call("Target.attachToTarget", json!({"targetId":target,"flatten":true}), None)?;
    let session = attached.get("sessionId").and_then(Value::as_str).ok_or("could not attach to the signing browser page")?;
    protocol.call("Page.enable", json!({}), Some(session))?;
    // Intercept only the initial known request. No subsequent login request or Network
    // event stream is observed. The user interacts with the provider's original pages.
    protocol.call(
        "Fetch.enable",
        json!({"patterns":[{"urlPattern":AUSTRIA_ENDPOINT,"requestStage":"Request","resourceType":"Document"}]}),
        Some(session),
    )?;
    let navigate = protocol.send("Page.navigate", json!({"url":AUSTRIA_ENDPOINT}), Some(session))?;
    let mut pending_calls = vec![navigate];
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("XMLRequest", std::str::from_utf8(xml).map_err(|_| "the signing request must be UTF-8 XML")?)
        .finish();
    let mut posted = false;
    let mut resource = None::<(String, String)>;
    loop {
        let message = protocol.receive()?;
        if let Some(id) = message.get("id").and_then(Value::as_u64)
            && pending_calls.contains(&id)
        {
            pending_calls.retain(|pending| *pending != id);
            if message.get("error").is_some() || message.pointer("/result/errorText").is_some() {
                return Err("the signing browser could not submit or load the provider request; close it and start again".into());
            }
            continue;
        }
        if message.get("sessionId").and_then(Value::as_str) != Some(session) {
            continue;
        }
        let params = message.get("params").unwrap_or(&Value::Null);
        match message.get("method").and_then(Value::as_str) {
            Some("Fetch.requestPaused") if !posted => {
                let request = params.get("requestId").and_then(Value::as_str).ok_or("invalid initial browser request")?;
                let url = params.pointer("/request/url").and_then(Value::as_str);
                if url != Some(AUSTRIA_ENDPOINT) {
                    return Err("unexpected initial signing browser destination".into());
                }
                let continued = protocol.send(
                    "Fetch.continueRequest",
                    json!({"requestId":request,"method":"POST","postData":BASE64_STANDARD.encode(body.as_bytes()),"headers":initial_headers(params)}),
                    Some(session),
                )?;
                pending_calls.push(continued);
                pending_calls.push(protocol.send("Fetch.disable", json!({}), Some(session))?);
                posted = true;
            }
            Some("Page.frameNavigated") => {
                let Some(frame) = params.get("frame") else { continue };
                if frame.get("parentId").is_some() {
                    continue;
                }
                let mime = frame.get("mimeType").and_then(Value::as_str);
                let url = frame.get("url").and_then(Value::as_str).unwrap_or("");
                #[cfg(test)]
                if _probe && posted && mime == Some("text/html") && is_provider_url(url) {
                    return Err("anonymous probe reached an original provider HTML page".into());
                }
                if matches!(mime, Some("text/xml" | "application/xml")) && is_provider_url(url) {
                    let frame_id = frame.get("id").and_then(Value::as_str).ok_or("invalid signing browser frame")?;
                    resource = Some((frame_id.to_string(), url.to_string()));
                } else {
                    resource = None;
                }
                if url.starts_with("chrome-error:") {
                    return Err("the signing browser could not connect securely to the provider".into());
                }
            }
            Some("Page.frameStoppedLoading") if posted => {
                if let Some((frame, url)) = resource.take() {
                    if params.get("frameId").and_then(Value::as_str) != Some(frame.as_str()) {
                        resource = Some((frame, url));
                        continue;
                    }
                    let content = protocol.call("Page.getResourceContent", json!({"frameId":frame,"url":url}), Some(session))?;
                    return xml_body(&content);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_post_preserves_browser_identity_without_copying_credentials() {
        let headers = initial_headers(
            &json!({"request":{"headers":{"User-Agent":"Synthetic Chromium","Accept":"text/html","Cookie":"private-cookie","Authorization":"private-credential","Content-Type":"text/plain"}}}),
        );
        let encoded = serde_json::to_string(&headers).unwrap();
        assert!(encoded.contains("Synthetic Chromium"));
        assert!(encoded.contains("application/x-www-form-urlencoded"));
        assert!(!encoded.contains("private"));
        assert!(!encoded.contains("text/plain"));
    }

    #[test]
    fn xml_resource_decoding_is_bounded_and_never_echoes_input() {
        assert_eq!(xml_body(&json!({"content":"<ok/>","base64Encoded":false})).unwrap(), b"<ok/>");
        assert_eq!(xml_body(&json!({"content":"PG9rLz4=","base64Encoded":true})).unwrap(), b"<ok/>");
        assert!(!xml_body(&json!({"content":"secret-token!","base64Encoded":true})).unwrap_err().contains("secret"));
        assert!(xml_body(&json!({"content":"x".repeat(MAX_XML+1)})).is_err());
    }

    #[test]
    fn closed_browser_pipe_is_reported_even_while_the_writer_owns_a_sender() {
        let (tx, rx) = mpsc::sync_channel(16);
        let _writer_error = tx.clone();
        read_packets(Box::new(std::io::Cursor::new(Vec::<u8>::new())), tx);
        assert!(rx.recv_timeout(Duration::from_millis(100)).unwrap().unwrap_err().contains("closed"));
    }

    #[test]
    fn cancellation_wins_over_a_queued_reply_and_writes_never_block_the_coordinator() {
        let (write, _write_rx) = mpsc::sync_channel(1);
        let (read_tx, read) = mpsc::sync_channel(1);
        read_tx.send(Ok(json!({"id":1,"result":{}}))).unwrap();
        let mut protocol = Protocol {
            writer: write,
            reader: read,
            next: 0,
            cancel: Arc::new(AtomicBool::new(true)),
            deadline: Instant::now() + Duration::from_secs(600),
        };
        assert!(protocol.receive().unwrap_err().contains("cancelled"));
        protocol.send("Synthetic.test", json!({}), None).unwrap();
        assert!(protocol.send("Synthetic.test", json!({}), None).is_err());
    }

    #[test]
    fn pipe_framing_rejects_an_unterminated_packet() {
        let (tx, rx) = mpsc::sync_channel(16);
        read_packets(Box::new(std::io::Cursor::new(b"{\"secret\":\"test\"}".to_vec())), tx);
        assert!(rx.recv().unwrap().unwrap_err().contains("limit"));
    }

    #[test]
    #[ignore = "requires installed browser and explicit provider access; no personal approval"]
    fn live_cms_request_reaches_original_provider_html_without_a_client_identifier() {
        let browser = find_browser().unwrap();
        // Contributor-original synthetic PDF bytes, never a personal document. The probe
        // observes only the navigation MIME type and origin; no HTML or login DOM is read.
        let pdf = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [] /Count 0 >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF";
        let xml = format!(
            "<sl:CreateCMSSignatureRequest xmlns:sl=\"http://www.buergerkarte.at/namespaces/securitylayer/1.2#\" Structure=\"detached\"><sl:KeyboxIdentifier>SecureSignatureKeypair</sl:KeyboxIdentifier><sl:DataObject><sl:MetaInfo><sl:MimeType>application/pdf</sl:MimeType></sl:MetaInfo><sl:Content><sl:Base64Content>{}</sl:Base64Content></sl:Content></sl:DataObject></sl:CreateCMSSignatureRequest>",
            BASE64_STANDARD.encode(pdf)
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let result = exchange_inner(&browser, xml.as_bytes(), &cancel, true);
        match result {
            Err(message) => assert_eq!(message, "anonymous probe reached an original provider HTML page"),
            Ok(_) => panic!("the anonymous provider probe returned XML instead of an HTML navigation; response omitted"),
        }
    }

    /// Explicitly opt-in, unauthenticated live protocol probe; never part of ordinary tests.
    #[test]
    #[ignore = "requires PDFCRAFT_SIGNING_BROWSER and explicit provider access"]
    fn live_null_operation_receives_xml_without_reading_login_pages() {
        let browser = find_browser().unwrap();
        let xml = b"<sl:NullOperationRequest xmlns:sl=\"http://www.buergerkarte.at/namespaces/securitylayer/1.2#\"/>";
        let cancel = Arc::new(AtomicBool::new(false));
        let result = exchange_inner(&browser, xml, &cancel, true).unwrap();
        assert!(result.starts_with(b"<?xml") || result.starts_with(b"<"));
    }
}
