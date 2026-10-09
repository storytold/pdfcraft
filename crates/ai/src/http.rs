//! The HTTP(S) [`Transport`] (the `http` feature): one POST per request, over rustls with the
//! operating system's trusted roots. Redirects are refused, since the body is the user's document.

use std::time::Duration;

use crate::{AiError, MAX_RESPONSE_BYTES, Request, Response, Transport};

/// How long one request may take, connection to last byte. Local models can be slow.
const TIMEOUT: Duration = Duration::from_secs(300);

/// Sends requests with `ureq`.
#[derive(Clone, Copy, Debug, Default)]
pub struct HttpTransport;

impl Transport for HttpTransport {
    fn post(&self, request: &Request) -> Result<Response, AiError> {
        let network = |e: String| AiError::Network(e);
        let mut config = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .max_redirects(0)
            // Error statuses come back as responses, so the provider's own message can be shown.
            .http_status_as_error(false)
            .user_agent(concat!("PdfCraft/", env!("CARGO_PKG_VERSION")));
        if request.url.to_ascii_lowercase().starts_with("https://") {
            config = config.tls_config(ureq::tls::TlsConfig::builder().root_certs(os_roots().map_err(network)?).build());
        }
        let agent = config.build().new_agent();
        let mut call = agent.post(&request.url);
        for (name, value) in &request.headers {
            call = call.header(*name, value.as_str());
        }
        // ureq's errors name the URL at most, never a header, so the key can't leak through them.
        let mut response = call.send(request.body.as_str()).map_err(|e| network(e.to_string()))?;
        let status = response.status().as_u16();
        if (300..400).contains(&status) {
            return Err(network(format!("the endpoint redirects elsewhere (status {status}); set the provider's final URL")));
        }
        let body =
            response.body_mut().with_config().limit(MAX_RESPONSE_BYTES).read_to_string().map_err(|e| network(format!("unreadable answer ({e})")))?;
        Ok(Response { status, body })
    }
}

/// The certificate authorities the operating system trusts.
fn os_roots() -> Result<ureq::tls::RootCerts, String> {
    let found = rustls_native_certs::load_native_certs();
    let certs: Vec<ureq::tls::Certificate<'static>> = found.certs.iter().map(|c| ureq::tls::Certificate::from_der(c.as_ref()).to_owned()).collect();
    if certs.is_empty() {
        return Err("no trusted certificates found on this system".into());
    }
    Ok(ureq::tls::RootCerts::new_with_certs(&certs))
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};

    use super::*;

    /// A one-shot local HTTP server answering with `reply`; returns its base URL and what it received.
    fn serve(status: &str, extra: &str, body: &str) -> (String, std::thread::JoinHandle<String>) {
        let reply = format!(
            "HTTP/1.1 {status}
{extra}Content-Length: {}
Connection: close

{body}",
            body.len()
        );
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut got = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = stream.read(&mut buf).unwrap();
                got.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&got);
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let len =
                        head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap()));
                    if n == 0 || body.len() >= len.unwrap_or(0) {
                        break;
                    }
                }
            }
            stream.write_all(reply.as_bytes()).unwrap();
            String::from_utf8_lossy(&got).into_owned()
        });
        (url, handle)
    }

    fn pages() -> Vec<crate::Page> {
        vec![crate::Page { number: 1, text: "Invoice 42, total due 10 EUR.".into() }]
    }

    #[test]
    fn a_local_server_is_asked_and_its_answer_read() {
        let (url, server) = serve(
            "200 OK",
            "Content-Type: application/json
",
            r#"{"choices":[{"message":{"role":"assistant","content":"Hi."}}]}"#,
        );
        let provider = crate::Provider::new(crate::Api::OpenAi, format!("{url}/v1"), "local-model").with_key(Some("secret-key".into()));
        let answer = crate::run(&provider, &HttpTransport, &crate::Task::Summarize, &pages()).unwrap();
        assert_eq!(answer.text, "Hi.");
        let got = server.join().unwrap();
        assert!(got.starts_with("POST /v1/chat/completions HTTP/1.1"), "{got}");
        assert!(got.to_ascii_lowercase().contains("authorization: bearer secret-key"), "{got}");
        assert!(got.contains("Invoice 42") && got.contains("local-model"), "{got}");
    }

    #[test]
    fn error_statuses_and_redirects_are_reported_not_followed() {
        let (url, server) = serve("401 Unauthorized", "", r#"{"error":{"message":"bad credentials"}}"#);
        let provider = crate::Provider::new(crate::Api::OpenAi, url, "m");
        let e = crate::run(&provider, &HttpTransport, &crate::Task::Summarize, &pages()).unwrap_err();
        assert_eq!(e, AiError::Provider { status: 401, message: "bad credentials".into() });
        server.join().unwrap();

        let (url, server) = serve(
            "307 Temporary Redirect",
            "Location: http://example.invalid/
",
            "",
        );
        let provider = crate::Provider::new(crate::Api::OpenAi, url, "m");
        let e = crate::run(&provider, &HttpTransport, &crate::Task::Summarize, &pages()).unwrap_err();
        assert!(matches!(e, AiError::Network(_)), "{e:?}");
        server.join().unwrap();
    }

    #[test]
    fn an_unreachable_endpoint_is_a_network_error() {
        // A port nothing listens on: bound, then released.
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let provider = crate::Provider::new(crate::Api::OpenAi, format!("http://127.0.0.1:{port}"), "m");
        let e = crate::run(&provider, &HttpTransport, &crate::Task::Summarize, &pages()).unwrap_err();
        assert!(matches!(e, AiError::Network(_)), "{e:?}");
    }
}
