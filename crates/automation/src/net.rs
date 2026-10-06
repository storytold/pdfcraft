//! Outbound network for the timestamp and revocation tools.
//!
//! Strictly on demand: a request only happens when a tool is given (or has configured) a
//! server. Every URL must be https, or plain http on the loopback (a local TSA); requests are
//! time-boxed, answers are size-capped, and redirects are not followed. In the browser build
//! every entry point reports the tools as unavailable.

const MAX_BODY: usize = 4 * 1024 * 1024;

/// Only https anywhere, or plain http on the loopback (a local TSA or CRL server).
pub fn check_url(url: &str) -> Result<(), String> {
    let loopback = ["127.0.0.1", "localhost", "[::1]"].iter().any(|h| url["http://".len()..].starts_with(h));
    let ok = url.starts_with("https://") || (url.starts_with("http://") && loopback);
    ok.then_some(()).ok_or_else(|| "timestamp and revocation servers must use https (plain http is only allowed on the loopback)".into())
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::{MAX_BODY, check_url};
    use printcraft_engine::sign::{SignError, TimestampAuthority};
    use std::time::Duration;

    /// A configured RFC 3161 timestamp server.
    pub struct TsaClient {
        url: String,
    }

    impl TsaClient {
        pub fn new(url: String) -> Result<Self, String> {
            check_url(&url)?;
            Ok(Self { url })
        }
    }

    impl TimestampAuthority for TsaClient {
        fn timestamp(&self, request: &[u8]) -> Result<Vec<u8>, SignError> {
            post_bytes(&self.url, "application/timestamp-query", "application/timestamp-reply", request)
        }
    }

    fn agent() -> Result<ureq::Agent, String> {
        let found = rustls_native_certs::load_native_certs();
        let certs: Vec<ureq::tls::Certificate<'static>> =
            found.certs.iter().map(|c| ureq::tls::Certificate::from_der(c.as_ref()).to_owned()).collect();
        if certs.is_empty() {
            return Err("no trusted certificates found on this system".into());
        }
        Ok(ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(15)))
            .max_redirects(0)
            .tls_config(ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::new_with_certs(&certs)).build())
            .build()
            .new_agent())
    }

    fn request(url: &str, method: &'static str, content_type: &str, accept: &str, body: &[u8]) -> Result<Vec<u8>, String> {
        check_url(url)?;
        let agent = agent()?;
        let mut response = match method {
            "POST" => agent
                .post(url)
                .header("Content-Type", content_type)
                .header("Accept", accept)
                .header("User-Agent", concat!("PrintCraft/", env!("CARGO_PKG_VERSION")))
                .send(body),
            _ => agent.get(url).header("Accept", accept).header("User-Agent", concat!("PrintCraft/", env!("CARGO_PKG_VERSION"))).call(),
        }
        .map_err(|e| format!("{url}: {e}"))?;
        let bytes = response.body_mut().with_config().limit(MAX_BODY as u64).read_to_vec().map_err(|e| format!("{url}: {e}"))?;
        Ok(bytes)
    }

    /// POST `body` and return the (capped) answer.
    pub fn post_bytes(url: &str, content_type: &str, accept: &str, body: &[u8]) -> Result<Vec<u8>, SignError> {
        request(url, "POST", content_type, accept, body).map_err(SignError::Network)
    }

    /// GET a DER blob (a CRL from a distribution point).
    pub fn fetch(url: &str) -> Result<Vec<u8>, String> {
        request(url, "GET", "application/pkix-crl", "application/pkix-crl", &[])
    }
}

#[cfg(target_arch = "wasm32")]
mod native {
    const OFF: &str = "network tools are unavailable in the browser build";

    pub struct TsaClient;

    impl TsaClient {
        pub fn new(_url: String) -> Result<Self, String> {
            Err(OFF.into())
        }
    }

    pub fn post_bytes(_url: &str, _content_type: &str, _accept: &str, _body: &[u8]) -> Result<Vec<u8>, String> {
        Err(OFF.into())
    }

    pub fn fetch(_url: &str) -> Result<Vec<u8>, String> {
        Err(OFF.into())
    }
}

pub use native::{TsaClient, fetch, post_bytes};
