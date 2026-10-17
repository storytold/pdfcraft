//! RFC 3161 time servers reached over HTTP (RFC 3161 §3.4): the transport `pdfcraft-sign` leaves
//! to the application, which never opens a socket itself.
//!
//! **Opt-in.** Nothing here runs unless a person (or a script or agent they run) names a server:
//! PdfCraft has no default server. When one is named, the request goes to that server only, while
//! signing, and carries what RFC 3161 sends: a SHA-256 fingerprint of the signature value (a
//! document timestamp: of the file), a random nonce and a request for the server's certificate.
//! The document itself is never sent.
//!
//! **Never fails a signing.** [`sign_with_fallback`] signs without the time server's token when
//! the server can't be reached or its answer isn't a valid token for the request, and says why,
//! so the caller can tell the person the document was signed with this computer's clock.
//!
//! **Trust.** Plain `http://` is the norm for time servers: the token is signed by the server and
//! checked here and again when the signature is validated, so the transport adds nothing to its
//! trust. `https://` uses rustls with the operating system's root certificates, as the desktop
//! app's update check does; no root certificate list is bundled.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::cell::{Cell, RefCell};
use std::fmt;
use std::time::Duration;

use pdfcraft_sign::{SignError, TimestampAuthority};

#[cfg(feature = "test-server")]
pub mod test_server;

/// Well-known public RFC 3161 time servers that need no account, offered as suggestions:
/// (name, address). None of them is chosen unless a person picks it.
pub const SUGGESTED: &[(&str, &str)] = &[
    ("DigiCert", "http://timestamp.digicert.com"),
    ("Sectigo", "http://timestamp.sectigo.com"),
    ("GlobalSign", "http://timestamp.globalsign.com/tsa/r6advanced1"),
    ("Entrust", "http://timestamp.entrust.net/TSS/RFC3161sha2TS"),
];

/// The longest server address accepted (characters).
pub const MAX_URL_CHARS: usize = 2048;

/// How long one request may take, connecting included. Signing waits for it.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// The largest answer read (bytes), the same cap the sign crate puts on a `TimeStampResp`.
#[cfg(not(target_arch = "wasm32"))]
const MAX_ANSWER: u64 = 4 * 1024 * 1024;

/// Why a time server's address was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UrlError {
    Empty,
    TooLong,
    /// Not `http://` or `https://`.
    Scheme,
    /// A user name or password in the address.
    Credentials,
    Host,
    Port,
    /// Spaces, control or non-ASCII characters, or a `#fragment`.
    Characters,
}

impl fmt::Display for UrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            UrlError::Empty => "the time server's address is empty",
            UrlError::TooLong => "the time server's address is longer than 2048 characters",
            UrlError::Scheme => "a time server's address starts with http:// or https://",
            UrlError::Credentials => "a time server's address can't carry a user name or password",
            UrlError::Host => "the time server's address has no valid server name",
            UrlError::Port => "the time server's address has an invalid port (1 to 65535)",
            UrlError::Characters => "a time server's address can't contain spaces, control or non-ASCII characters, or a #fragment",
        })
    }
}

impl std::error::Error for UrlError {}

/// Check a time server's address (`http://` or `https://`, a server name or IP address, an
/// optional port and path) and return it trimmed. Every input is untrusted: a setting read back,
/// a typed address, an agent's argument.
pub fn check_url(input: &str) -> Result<String, UrlError> {
    let url = input.trim();
    if url.is_empty() {
        return Err(UrlError::Empty);
    }
    if url.chars().count() > MAX_URL_CHARS {
        return Err(UrlError::TooLong);
    }
    if url.chars().any(|c| !c.is_ascii() || c.is_ascii_whitespace() || c.is_ascii_control() || c == '#') {
        return Err(UrlError::Characters);
    }
    let (scheme, rest) = url.split_once("://").ok_or(UrlError::Scheme)?;
    if !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")) {
        return Err(UrlError::Scheme);
    }
    // The address is ASCII (checked above), so every byte offset is a character boundary.
    let authority = rest.split(['/', '?']).next().unwrap_or_default();
    if authority.contains('@') {
        return Err(UrlError::Credentials);
    }
    let (host, port) = match authority.strip_prefix('[') {
        // An IPv6 literal: `[2001:db8::1]` or `[2001:db8::1]:8080`.
        Some(bracketed) => {
            let (ip, after) = bracketed.split_once(']').ok_or(UrlError::Host)?;
            if ip.is_empty() || !ip.chars().all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.') {
                return Err(UrlError::Host);
            }
            let port = if after.is_empty() { None } else { Some(after.strip_prefix(':').ok_or(UrlError::Host)?) };
            (None, port)
        }
        None => match authority.split_once(':') {
            Some((host, port)) => (Some(host), Some(port)),
            None => (Some(authority), None),
        },
    };
    if let Some(host) = host {
        let label_ok =
            |l: &str| !l.is_empty() && !l.starts_with('-') && !l.ends_with('-') && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        // A fully qualified name may end with a dot.
        let name = host.strip_suffix('.').unwrap_or(host);
        if name.is_empty() || !name.split('.').all(label_ok) {
            return Err(UrlError::Host);
        }
    }
    if let Some(port) = port
        && !(port.len() <= 5 && port.chars().all(|c| c.is_ascii_digit()) && port.parse::<u16>().is_ok_and(|p| p != 0))
    {
        return Err(UrlError::Port);
    }
    Ok(url.to_string())
}

/// One signing's time server: a [`TimestampAuthority`] that posts the request to `url` and
/// accepts only an answer that is a valid token for it. It remembers whether it was asked and
/// why it failed, for [`sign_with_fallback`]; make one per signing.
#[derive(Debug)]
pub struct HttpAuthority {
    url: String,
    timeout: Duration,
    asked: Cell<bool>,
    failure: RefCell<Option<String>>,
}

impl HttpAuthority {
    /// A time server at `url`, checked with [`check_url`]. Nothing is sent until signing asks.
    pub fn new(url: &str) -> Result<Self, UrlError> {
        Ok(Self { url: check_url(url)?, timeout: DEFAULT_TIMEOUT, asked: Cell::new(false), failure: RefCell::new(None) })
    }

    /// Wait at most `timeout` for an answer, connecting included.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// Whether signing has asked this server for a token.
    pub fn asked(&self) -> bool {
        self.asked.get()
    }

    /// Why the last request failed: the server couldn't be reached, refused, or answered with
    /// something that isn't a valid token for the request.
    pub fn failure(&self) -> Option<String> {
        self.failure.try_borrow().ok().and_then(|f| f.clone())
    }

    fn forget(&self) {
        self.asked.set(false);
        if let Ok(mut f) = self.failure.try_borrow_mut() {
            *f = None;
        }
    }

    fn ask(&self, request: &[u8]) -> Result<Vec<u8>, String> {
        let query = pdfcraft_sign::timestamp::parse_request(request).map_err(|e| e.to_string())?;
        let answer = post(&self.url, request, self.timeout)?;
        // Checked here as well as by the signer, so a bad answer counts as the server's failure.
        pdfcraft_sign::timestamp::parse_response(&answer, &query).map_err(|e| format!("its answer was refused: {e}"))?;
        Ok(answer)
    }
}

impl TimestampAuthority for HttpAuthority {
    fn timestamp(&self, request: &[u8]) -> Result<Vec<u8>, SignError> {
        self.asked.set(true);
        self.ask(request).map_err(|reason| {
            let message = format!("the time server {} gave no timestamp: {reason}", self.url);
            if let Ok(mut f) = self.failure.try_borrow_mut() {
                *f = Some(message.clone());
            }
            SignError::Pdf(message)
        })
    }
}

/// Sign with `authority`'s time (`stamped`), and if the time server is what failed, sign again
/// without it (`plain`). Returns the result and, when it has no time server token, why.
///
/// Anything that fails once the server has been asked is retried without it: the time server
/// never costs a person their signature. An error from before the server was asked (a wrong
/// password, a document that may not be signed) is returned as it is, and so is the plain
/// signing's own error.
pub fn sign_with_fallback<T, E: fmt::Display>(
    authority: &HttpAuthority,
    stamped: impl FnOnce(&dyn TimestampAuthority) -> Result<T, E>,
    plain: impl FnOnce() -> Result<T, E>,
) -> Result<(T, Option<String>), E> {
    authority.forget();
    match stamped(authority) {
        Ok(value) => Ok((value, None)),
        Err(e) if authority.asked() => {
            let reason = authority.failure().unwrap_or_else(|| format!("the time server {}'s token could not be used: {e}", authority.url));
            plain().map(|value| (value, Some(reason)))
        }
        Err(e) => Err(e),
    }
}

/// POST one RFC 3161 request and read the answer.
#[cfg(not(target_arch = "wasm32"))]
fn post(url: &str, request: &[u8], timeout: Duration) -> Result<Vec<u8>, String> {
    let mut config = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        // Answered where it was asked: a redirect is a failure, not a second destination.
        .max_redirects(0)
        .http_status_as_error(false);
    // `check_url` accepted only http and https; only https needs the system's roots.
    if url.get(..8).is_some_and(|s| s.eq_ignore_ascii_case("https://")) {
        config = config.tls_config(ureq::tls::TlsConfig::builder().root_certs(os_roots()?).build());
    }
    let mut response = config
        .build()
        .new_agent()
        .post(url)
        .header("Content-Type", "application/timestamp-query")
        .header("Accept", "application/timestamp-reply")
        .header("User-Agent", concat!("PdfCraft/", env!("CARGO_PKG_VERSION")))
        .send(request)
        .map_err(|e| format!("it could not be reached ({e})"))?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(format!("it answered HTTP {status}"));
    }
    response.body_mut().with_config().limit(MAX_ANSWER).read_to_vec().map_err(|e| format!("its answer could not be read ({e})"))
}

/// The certificate authorities the operating system trusts (for `https://` servers).
#[cfg(not(target_arch = "wasm32"))]
fn os_roots() -> Result<ureq::tls::RootCerts, String> {
    let found = rustls_native_certs::load_native_certs();
    let certs: Vec<ureq::tls::Certificate<'static>> = found.certs.iter().map(|c| ureq::tls::Certificate::from_der(c.as_ref()).to_owned()).collect();
    if certs.is_empty() {
        return Err("no trusted certificates were found on this system for an https:// server".into());
    }
    Ok(ureq::tls::RootCerts::new_with_certs(&certs))
}

/// The browser build has no sockets of its own.
#[cfg(target_arch = "wasm32")]
fn post(_url: &str, _request: &[u8], _timeout: Duration) -> Result<Vec<u8>, String> {
    Err("time servers can't be reached from the browser version of PdfCraft".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_checked() {
        for ok in [
            "http://timestamp.digicert.com",
            "  https://tsa.example.org/tsr?x=1  ",
            "HTTP://Example.COM:8080/a/b",
            "http://127.0.0.1:3180/",
            "http://[::1]:8080/tsa",
            "http://[2001:db8::1]",
            "http://example.com.",
        ] {
            assert_eq!(check_url(ok), Ok(ok.trim().to_string()), "{ok}");
        }
        for (bad, why) in [
            ("", UrlError::Empty),
            ("   ", UrlError::Empty),
            ("timestamp.digicert.com", UrlError::Scheme),
            ("ftp://example.com", UrlError::Scheme),
            ("file:///etc/passwd", UrlError::Scheme),
            ("javascript://x", UrlError::Scheme),
            ("http://", UrlError::Host),
            ("http:///path", UrlError::Host),
            ("http://user:pw@example.com", UrlError::Credentials),
            ("http://localhost@evil.example", UrlError::Credentials),
            ("http://exa mple.com", UrlError::Characters),
            ("http://example.com/\u{7f}", UrlError::Characters),
            ("http://exämple.com", UrlError::Characters),
            ("http://example.com/#frag", UrlError::Characters),
            ("http://ex_ample.com", UrlError::Host),
            ("http://-example.com", UrlError::Host),
            ("http://a..b", UrlError::Host),
            ("http://[::1", UrlError::Host),
            ("http://[zz]", UrlError::Host),
            ("http://[::1]x", UrlError::Host),
            ("http://example.com:", UrlError::Port),
            ("http://example.com:0", UrlError::Port),
            ("http://example.com:65536", UrlError::Port),
            ("http://example.com:+80", UrlError::Port),
            ("http://example.com:1:2", UrlError::Port),
            ("h", UrlError::Scheme),
        ] {
            assert_eq!(check_url(bad), Err(why), "{bad:?}");
            assert!(HttpAuthority::new(bad).is_err(), "{bad:?}");
        }
        assert_eq!(check_url(&format!("http://example.com/{}", "a".repeat(MAX_URL_CHARS))), Err(UrlError::TooLong));
        // Every suggestion passes its own check.
        for (_, url) in SUGGESTED {
            assert_eq!(check_url(url).as_deref(), Ok(*url));
        }
    }

    #[test]
    fn short_and_multibyte_inputs_never_panic() {
        for s in ["", "h", "ht", "http:", "http:/", "http://", "é", "http://é", "😀://x", "http://[", "http://]", "http://:", "http://[]:", "://"]
        {
            let _ = check_url(s);
        }
    }
}
