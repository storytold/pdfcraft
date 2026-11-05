//! Image acquisition. The settings, errors, page handling and the backend registry are the
//! `craft-scan` crate, re-exported here so the app and the tools share one surface. The scanner
//! backend is `pdfcraft-wia` (Windows WIA); this module registers it with the process-wide
//! registry, once, and with no backend registered (any other platform) every request answers
//! [`ScanError::Unsupported`].
//!
//! **Registration is one call, and it happens here.** [`register_backends`] is idempotent, and the
//! functions below that need a backend call it first, so the GUI, the CLI, the automation tools and
//! the language bindings all get the same registry by using `pdfcraft_platform::scan` and need no
//! start-up code of their own. An app that wants the registry ready earlier (to report
//! [`backend_names`] at launch) can call it itself.
//!
//! [`acquire`] and [`ProgressToken`] deliver in-memory [`Page`] buffers. Path helpers such as
//! [`write_pages`] are optional sinks for callers that want files; bindings that stay in memory
//! never need them.

pub use craft_scan::*;

/// Register the scanner backends of this platform with the `craft-scan` registry. Safe to call any
/// number of times and from any thread: a backend is added once. Returns whether any backend is
/// registered afterwards.
pub fn register_backends() -> bool {
    pdfcraft_wia::register()
}

/// Whether a scanner backend is registered. A run-time answer: every platform compiles the same
/// code, and a platform without a backend reports [`ScanError::Unsupported`] when a scan is asked for.
pub fn supported() -> bool {
    register_backends();
    craft_scan::supported()
}

/// The names of the registered backends (`["wia"]` on Windows).
pub fn backend_names() -> Vec<&'static str> {
    register_backends();
    craft_scan::backend_names()
}

/// The scanners the backends list, without opening any. Ids are `"<backend>:<native id>"`.
pub fn scanner_list() -> Result<Listing<ScannerRef>, ScanError> {
    register_backends();
    craft_scan::scanner_list()
}

/// Every scanner that opens, with its capabilities. Expensive: it opens each device.
pub fn scanners() -> Result<Listing<Scanner>, ScanError> {
    register_backends();
    craft_scan::scanners()
}

/// One scanner's capabilities, by namespaced id (`wia:<id>`). Opens that device only.
pub fn scanner_details(id: &str) -> Result<Scanner, ScanError> {
    register_backends();
    craft_scan::scanner_details(id)
}

/// What the paper path is doing before a scan; `Ok(None)` when the backend cannot say.
pub fn status(id: &str) -> Result<Option<HandlingStatus>, ScanError> {
    register_backends();
    craft_scan::status(id)
}

/// Backend diagnostics for the devices matching `filter` (all of them when it is empty).
pub fn diagnostic_dump(filter: &str) -> Result<String, ScanError> {
    register_backends();
    craft_scan::diagnostic_dump(filter)
}

/// Scan. `ScanSettings::device` is `wia:<id>`, empty, or a fragment of the scanner's name.
pub fn acquire(settings: &ScanSettings) -> Result<Acquired, ScanError> {
    register_backends();
    craft_scan::acquire(settings)
}

/// The contract lists the bindings expose, as JSON: [`craft_scan::contract_json`] plus the HRESULT
/// table of the WIA backend under `hresults`.
pub fn contract_json() -> String {
    pdfcraft_wia::rules::contract_json()
}
