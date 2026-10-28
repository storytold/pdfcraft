//! HTTP for signing online: RFC 3161 timestamp servers, OCSP responders, CRLs and CA
//! certificates (`pdfcraft_sign::ltv::Fetch`). Nothing here runs unless the user turned
//! timestamps or LTV on; there is no background traffic.

use std::time::Duration;

use pdfcraft_sign::SignError;

/// The largest answer read (a CRL of a big CA runs to a few MiB).
const LIMIT: u64 = 8 << 20;

/// Blocking requests with a timeout; HTTPS trusts the operating system's certificate authorities.
pub struct Http;

impl Http {
    /// One agent for every request: the OS trust store is read once, not per request.
    fn agent() -> &'static ureq::Agent {
        static AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
        AGENT.get_or_init(Self::build)
    }

    fn build() -> ureq::Agent {
        let roots: Vec<ureq::tls::Certificate<'static>> =
            rustls_native_certs::load_native_certs().certs.iter().map(|c| ureq::tls::Certificate::from_der(c.as_ref()).to_owned()).collect();
        ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(20)))
            .user_agent(concat!("PdfCraft/", env!("CARGO_PKG_VERSION")))
            .tls_config(ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::new_with_certs(&roots)).build())
            .build()
            .new_agent()
    }
}

fn read(url: &str, response: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<Vec<u8>, SignError> {
    let failed = |e: String| SignError::Crypto(format!("{url}: {e}"));
    let mut response = response.map_err(|e| failed(format!("no answer ({e})")))?;
    response.body_mut().with_config().limit(LIMIT).read_to_vec().map_err(|e| failed(format!("unreadable answer ({e})")))
}

impl pdfcraft_sign::ltv::Fetch for Http {
    fn get(&self, url: &str) -> Result<Vec<u8>, SignError> {
        read(url, Self::agent().get(url).call())
    }

    fn post(&self, url: &str, content_type: &str, body: &[u8]) -> Result<Vec<u8>, SignError> {
        read(url, Self::agent().post(url).header("Content-Type", content_type).send(body))
    }
}
