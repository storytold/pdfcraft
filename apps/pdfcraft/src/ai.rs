//! Sends AI assistant requests (M13) to the endpoint set in Preferences ▸ AI assistant: one
//! OpenAI-compatible chat-completions POST per question. Nothing is sent until the user asks.

use std::time::Duration;

use pdfcraft_engine::ai::MAX_RESPONSE_BYTES;

/// Local models can take minutes over a long document.
const TIMEOUT: Duration = Duration::from_secs(600);

/// POST `body` to `endpoint` and return the response body. A non-success status becomes an
/// error, with the server's own message when it gives one.
pub fn send(endpoint: &str, api_key: &str, body: &serde_json::Value) -> Result<String, String> {
    let mut tls = ureq::tls::TlsConfig::builder();
    // Plain http:// (a model on this computer or the local network) needs no certificates.
    if endpoint.starts_with("https://") {
        tls = tls.root_certs(crate::updates::os_roots()?);
    }
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .tls_config(tls.build())
        // Read error answers ourselves, and never resend the document (or the key) elsewhere.
        .http_status_as_error(false)
        .max_redirects(0)
        .build()
        .new_agent();
    let mut request =
        agent.post(endpoint).header("Content-Type", "application/json").header("User-Agent", concat!("PdfCraft/", env!("CARGO_PKG_VERSION")));
    if !api_key.is_empty() {
        request = request.header("Authorization", format!("Bearer {api_key}"));
    }
    let mut response = request.send(body.to_string()).map_err(|e| format!("couldn't reach the AI endpoint ({e})"))?;
    let status = response.status();
    let text = response.body_mut().with_config().limit(MAX_RESPONSE_BYTES).read_to_string().map_err(|e| format!("unreadable answer ({e})"))?;
    if status.is_success() {
        return Ok(text);
    }
    // OpenAI-style servers explain errors as {"error": {"message": …}}.
    let detail = pdfcraft_engine::ai::parse_response(&text).err().filter(|e| e.contains("returned an error"));
    Err(detail.unwrap_or_else(|| match status.as_u16() {
        401 | 403 => format!("the endpoint refused the request (HTTP {status}); check the API key"),
        404 => format!("nothing answers at {endpoint} (HTTP 404); check the endpoint URL"),
        _ => format!("the endpoint answered HTTP {status}"),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// A one-shot local HTTP server answering `status` with `reply`; returns its URL and the
    /// request it received.
    fn serve(status: &'static str, reply: &'static str) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1/chat/completions", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut got = Vec::new();
            let mut buf = [0u8; 4096];
            // Read the headers, then the body by Content-Length.
            loop {
                let n = s.read(&mut buf).unwrap_or(0);
                got.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&got).to_string();
                if let Some(end) = text.find("\r\n\r\n") {
                    let len = text
                        .lines()
                        .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                        .unwrap_or(0);
                    if got.len() >= end + 4 + len {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            let answer =
                format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len());
            s.write_all(answer.as_bytes()).unwrap();
            String::from_utf8_lossy(&got).to_string()
        });
        (url, handle)
    }

    #[test]
    fn a_request_reaches_the_endpoint_with_the_key_and_the_answer_comes_back() {
        let (url, server) = serve("200 OK", r#"{"choices":[{"message":{"content":"Total: 5 (p. 1)"}}]}"#);
        let body = serde_json::json!({"model": "m", "messages": [{"role": "user", "content": "hi"}]});
        let reply = send(&url, "secret", &body).unwrap();
        assert_eq!(pdfcraft_engine::ai::parse_response(&reply).unwrap(), "Total: 5 (p. 1)");
        let request = server.join().unwrap();
        assert!(request.starts_with("POST /v1/chat/completions"), "{request}");
        assert!(request.contains("Bearer secret"));
        assert!(request.contains(r#""model":"m""#));
    }

    #[test]
    fn errors_say_what_went_wrong() {
        let (url, server) = serve("404 Not Found", r#"{"error":{"message":"The model `x` does not exist."}}"#);
        let e = send(&url, "", &serde_json::json!({})).unwrap_err();
        assert!(e.contains("does not exist"), "{e}");
        assert!(!server.join().unwrap().contains("Authorization"));
        let (url, server) = serve("401 Unauthorized", "denied");
        assert!(send(&url, "k", &serde_json::json!({})).unwrap_err().contains("API key"));
        server.join().unwrap();
        // Nothing listening.
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert!(send(&format!("http://127.0.0.1:{port}/v1/chat/completions"), "", &serde_json::json!({})).unwrap_err().contains("couldn't reach"));
    }
}
