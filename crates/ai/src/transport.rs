//! The seam between the assistant and the network. This crate builds an [`HttpRequest`] and
//! decodes the bytes that come back; the app supplies the [`Transport`] that sends it (the
//! desktop app's lives in `pdfcraft-platform`, tests use a fake). Without one, nothing is sent.

use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::ops::ControlFlow;

/// A value that must not be shown: the API key. `Debug` prints a mask, and there is no `Display`
/// or `Serialize`, so it can't reach a log, a preview or a settings file by accident.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    /// The value itself, for the transport that sends it to the provider.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(••••)")
    }
}

/// What a masked header value shows in a preview.
pub const MASK: &str = "••••";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HeaderValue {
    Plain(String),
    Secret(Secret),
}

impl HeaderValue {
    pub fn expose(&self) -> &str {
        match self {
            Self::Plain(v) => v,
            Self::Secret(s) => s.expose(),
        }
    }
}

/// A POST with a JSON body, ready for a [`Transport`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpRequest {
    pub url: String,
    pub headers: Vec<(String, HeaderValue)>,
    pub body: Vec<u8>,
}

/// An [`HttpRequest`] as the preview shows it: secret headers masked.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct RequestPreview {
    pub url: String,
    pub headers: Vec<(String, String)>,
    /// The body as sent, parsed so it can be shown or compared; `null` if it isn't JSON.
    pub body: serde_json::Value,
}

impl HttpRequest {
    pub fn preview(&self) -> RequestPreview {
        let headers = self
            .headers
            .iter()
            .map(|(name, value)| {
                let shown = match value {
                    HeaderValue::Plain(v) => v.clone(),
                    HeaderValue::Secret(_) => MASK.to_owned(),
                };
                (name.clone(), shown)
            })
            .collect();
        let body = serde_json::from_slice(&self.body).unwrap_or(serde_json::Value::Null);
        RequestPreview { url: self.url.clone(), headers, body }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    /// No transport here: the web build, or a headless run without the network flag.
    #[error("sending requests needs the desktop app")]
    Unavailable,
    /// The address breaks the policy ([`check_url`]) or the server tried to redirect.
    #[error("refused: {0}")]
    Refused(String),
    #[error("couldn't connect: {0}")]
    Connect(String),
    #[error("the provider took too long to answer")]
    Timeout,
    #[error("the answer was larger than {0} bytes")]
    TooLarge(usize),
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    Io(String),
}

/// Sends requests for the assistant.
pub trait Transport: Send + Sync {
    /// POST `req` and hand the response body to `on_bytes` as it arrives, returning the HTTP
    /// status. `on_bytes` returning `ControlFlow::Break` cancels: the transport stops reading
    /// and returns [`TransportError::Cancelled`].
    fn post_stream(&self, req: &HttpRequest, on_bytes: &mut dyn FnMut(&[u8]) -> ControlFlow<()>) -> Result<u16, TransportError>;
}

/// The policy every transport applies before connecting: `https://` to any host, or `http://`
/// only to the loopback interface (a model running on this machine). No user name or password in
/// the address, no whitespace or control characters. Redirects are a transport's to refuse.
pub fn check_url(url: &str) -> Result<(), TransportError> {
    let refuse = |why: &str| Err(TransportError::Refused(why.to_owned()));
    // A backslash is a path separator to some URL parsers and part of the host to others.
    if url.chars().any(|c| c.is_whitespace() || c.is_control() || c == '\\') {
        return refuse("the address contains spaces, backslashes or control characters");
    }
    let lower = url.to_ascii_lowercase();
    let (secure, rest) = if let Some(rest) = lower.strip_prefix("https://") {
        (true, rest)
    } else if let Some(rest) = lower.strip_prefix("http://") {
        (false, rest)
    } else {
        return refuse("only https:// addresses (or http:// on this computer) are allowed");
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        return refuse("the address contains a user name or password");
    }
    let host = host_of(authority);
    if host.is_empty() {
        return refuse("the address has no host");
    }
    if !secure && !is_loopback(host) {
        return refuse("http:// is allowed only to this computer (localhost); use https://");
    }
    Ok(())
}

/// The host part of `authority` (lowercase already): a bracketed IPv6 address or what comes
/// before the port.
fn host_of(authority: &str) -> &str {
    if let Some(inner) = authority.strip_prefix('[') {
        return inner.split(']').next().unwrap_or_default();
    }
    authority.split(':').next().unwrap_or_default()
}

fn is_loopback(host: &str) -> bool {
    // Only the name itself: a `*.localhost` subdomain is resolved by whatever DNS the system uses.
    if host == "localhost" {
        return true;
    }
    if let Ok(v4) = host.parse::<Ipv4Addr>() {
        return v4.is_loopback();
    }
    host.parse::<Ipv6Addr>().is_ok_and(|v6| v6.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_never_shows() {
        let req = HttpRequest {
            url: "https://api.anthropic.com/v1/messages".into(),
            headers: vec![
                ("x-api-key".into(), HeaderValue::Secret(Secret::new("sk-ant-test-123"))),
                ("anthropic-version".into(), HeaderValue::Plain("2026-10-21".into())),
            ],
            body: br#"{"a":1}"#.to_vec(),
        };
        let shown = format!("{req:?} {:?}", req.preview());
        assert!(!shown.contains("sk-ant-test-123"), "{shown}");
        assert_eq!(req.preview().headers[0].1, MASK);
        assert_eq!(req.preview().body, serde_json::json!({"a": 1}));
    }

    #[test]
    fn url_policy() {
        for ok in [
            "https://api.anthropic.com/v1/messages",
            "HTTPS://Example.com:8443/x",
            "http://localhost:11434/v1/chat/completions",
            "http://127.0.0.1:8080",
            "http://[::1]:1234/v1",
        ] {
            assert_eq!(check_url(ok), Ok(()), "{ok}");
        }
        for bad in [
            "http://api.anthropic.com/v1/messages",
            "http://localhost@evil.example/",
            "https://user:pw@api.anthropic.com/",
            "http://127.0.0.1.evil.example/",
            "http://localhost.evil.example/",
            "http://ollama.localhost/",
            "ftp://localhost/",
            "file:///etc/passwd",
            "https://",
            "https://exa mple.com/",
            "https://example.com/\n",
            "http://localhost\\@evil.example/",
            "http://localhost:80\\.evil.example/",
            "javascript:alert(1)",
            "",
        ] {
            assert!(check_url(bad).is_err(), "{bad}");
        }
    }
}
