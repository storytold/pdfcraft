//! The Windows WIA 2.0 backend for `craft-scan`. [`Wia`] implements [`craft_scan::Backend`] under
//! the name `"wia"`: `scanner_list` is cheap (no open); `scanners` connects to every device and
//! reads properties, and a device that fails to open is omitted; `acquire` transfers, which feeds
//! paper. The app registers it once ([`register`]); the scan functions of `craft-scan` then
//! answer for it, with device ids of the form `wia:<WIA id>`.
//!
//! WIA is COM, and the `windows` crate's COM calls are `unsafe`. All of that is confined to two
//! modules of this crate: `ffi` (small wrappers around single COM calls and `PROPVARIANT` reads)
//! and `sinks` (the COM callback objects WIA calls back into). Everything else here, and the whole
//! of `craft-scan`, is safe code. `docs/unsafe-audit.md` lists every category and how to review it.
//!
//! [`length`] (what an item publishes about sheet-length detection and what to do about a page-length request) and
//! [`rules`] (the HRESULT tables, the sentences and the status-bit rules) has no COM in it and
//! builds on every platform. The backend itself is empty on other platforms.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod length;
pub mod rules;

#[cfg(windows)]
mod backend;
#[cfg(windows)]
mod ffi;
#[cfg(windows)]
mod sinks;

/// The backend's registry name, and the prefix of its device ids.
pub const NAME: &str = "wia";

#[cfg(windows)]
pub use backend::Wia;

/// Register the WIA backend with the process-wide `craft-scan` registry. Safe to call more than
/// once (and from every entry point): a second registration is ignored. Returns whether a WIA
/// backend is registered afterwards, which is `false` on platforms without WIA.
pub fn register() -> bool {
    #[cfg(windows)]
    {
        use std::sync::Once;
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            // A duplicate (the app registered it itself) is not an error here.
            if let Err(error) = craft_scan::register(Box::new(Wia)) {
                log::debug!("scan: {error}");
            }
        });
        craft_scan::backend_names().contains(&NAME)
    }
    #[cfg(not(windows))]
    {
        false
    }
}
