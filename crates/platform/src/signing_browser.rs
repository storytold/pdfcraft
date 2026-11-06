//! Opt-in Security Layer browser transport on desktop platforms.
//!
//! A-Trust requires credentials to be entered on its own browser pages. We send the initial
//! XML request and receive only the final top-level XML resource. Login pages, their DOM,
//! requests and credentials are never read or filled. No JavaScript is injected.
//! The installed Chromium/Chrome/Edge runs with a fresh private profile and inherited pipes,
//! never a debugging TCP port. CDP Page and the one initial Fetch request are sufficient;
//! the Network domain (which could expose authentication POST bodies) is not enabled.
//!
//! Protocol: https://chromedevtools.github.io/devtools-protocol/tot/Page/ and Fetch/.
//! Endpoint: https://technology.a-sit.at/faqs/ .

#[cfg(not(target_arch = "wasm32"))]
mod desktop;

/// The provider endpoint; callers cannot substitute an endpoint obtained from a PDF.
pub const AUSTRIA_ENDPOINT: &str = "https://service.a-trust.at/mobile/https-security-layer-request/default.aspx";

/// A running exchange, injected by the host. Synthetic implementations allow end-to-end
/// tests without contacting an identity provider or bypassing personal authorization.
pub trait SigningExchange: Send {
    /// Pending, or one final XML response/error. A caller stops polling after a final result.
    fn poll(&self) -> Option<Result<Vec<u8>, String>>;
    /// Cancel and release the exchange's own resources.
    fn cancel(&self);
}

/// Explicitly configured browser transport. Nothing calls it while merely listing providers.
pub type SigningTransport = std::sync::Arc<dyn Fn(Vec<u8>) -> Result<Box<dyn SigningExchange>, String> + Send + Sync>;

/// The real desktop transport. Constructing this factory opens no browser and no connection.
pub fn installed_browser_transport() -> SigningTransport {
    std::sync::Arc::new(|xml| {
        #[cfg(not(target_arch = "wasm32"))]
        {
            Ok(Box::new(SigningBrowser::start(xml)?) as Box<dyn SigningExchange>)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = xml;
            Err("ID Austria browser signing is available in the desktop application".into())
        }
    })
}

/// Whether a URL is an original provider page. User info, ports, other schemes and lookalike
/// hostnames never qualify for receiving a signature response.
pub fn is_provider_url(value: &str) -> bool {
    let Ok(url) = url::Url::parse(value) else { return false };
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port_or_known_default() == Some(443)
        && matches!(url.host_str(), Some("service.a-trust.at" | "www.a-trust.at"))
        && url.path().starts_with("/mobile/https-security-layer-request/")
}

/// A browser operation. Dropping or cancelling it closes its isolated browser, not any
/// browser already open on the user's machine. Results contain XML, never login credentials.
#[cfg(not(target_arch = "wasm32"))]
pub struct SigningBrowser {
    receiver: std::sync::mpsc::Receiver<Result<Vec<u8>, String>>,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl std::fmt::Debug for SigningBrowser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SigningBrowser").finish_non_exhaustive()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl SigningBrowser {
    /// Start only after a user explicitly accepted sending this prepared document to A-Trust.
    /// No signing account, credentials, browser download or background service is created.
    pub fn start(xml: Vec<u8>) -> Result<Self, String> {
        if xml.is_empty() || xml.len() > 140 * 1024 * 1024 {
            return Err("the Security Layer request is empty or exceeds 140 MiB".into());
        }
        let executable = desktop::find_browser()?;
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::Builder::new()
            .name("signature-browser".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(|| desktop::exchange(&executable, &xml, &worker_cancel))
                    .unwrap_or_else(|_| Err("the signing browser stopped unexpectedly; your document is unchanged".into()));
                let _ = sender.send(result);
            })
            .map_err(|_| "could not start the signing browser worker".to_string())?;
        Ok(Self { receiver, cancel, worker: Some(worker) })
    }

    /// Receive without blocking the application. Call again while `None` is returned.
    pub fn poll(&self) -> Option<Result<Vec<u8>, String>> {
        match self.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Err("the signing browser was closed".into())),
        }
    }

    /// Cancel locally; a response arriving after cancellation must not be applied.
    pub fn cancel(&self) {
        self.cancel.store(true, std::sync::atomic::Ordering::Release);
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for SigningBrowser {
    fn drop(&mut self) {
        self.cancel();
        // Complete our isolated process/profile cleanup before the host exits. The
        // coordinator observes cancellation every 100 ms; pipe writes run separately.
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl SigningExchange for SigningBrowser {
    fn poll(&self) -> Option<Result<Vec<u8>, String>> {
        SigningBrowser::poll(self)
    }
    fn cancel(&self) {
        SigningBrowser::cancel(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_responses_only_come_from_original_https_provider_pages() {
        assert!(is_provider_url(AUSTRIA_ENDPOINT));
        assert!(is_provider_url("https://www.a-trust.at/mobile/https-security-layer-request/main.aspx?sid=synthetic"));
        for url in [
            "http://service.a-trust.at/mobile/https-security-layer-request/x",
            "https://service.a-trust.at.evil.test/mobile/https-security-layer-request/x",
            "https://service.a-trust.at:444/mobile/https-security-layer-request/x",
            "https://user:password@service.a-trust.at/mobile/https-security-layer-request/x",
            "https://service.a-trust.at/elsewhere",
            "file:///private",
            "javascript:alert(1)",
        ] {
            assert!(!is_provider_url(url), "{url}");
        }
    }
}
