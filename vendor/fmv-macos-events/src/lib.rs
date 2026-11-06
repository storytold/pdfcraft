//! Main-thread Apple-event adapter. The viewer consumes only safe Rust messages.
//!
//! PdfCraft patch (#6324): odoc items are decoded from their raw descriptor bytes so
//! that a path whose on-disk name is NFC-precomposed is delivered byte-exact. The
//! decoding lives in [`odoc_path`] (pure Rust; its decoding tests run on every
//! platform, the four that create real files on Unix only); the Objective-C adapter
//! lives in [`macos`] and is upstream `fmv-macos-events` 0.1.0 except for the
//! open-documents handler's item decode. The public API is unchanged.
//! Delivery is best-effort and never silent: an item nothing could verify is still
//! delivered as an unverified path, and verification failures surface through the
//! app's own open error instead of the open doing nothing.
#![deny(unsafe_op_in_unsafe_fn)]

/// Byte-level odoc decoding, independent of Objective-C so it is testable off macOS.
#[cfg(any(target_os = "macos", test))]
mod odoc_path;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::{Event, Inbox, Registration, install_edit_menu};
