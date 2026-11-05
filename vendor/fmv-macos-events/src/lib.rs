//! Main-thread Apple-event adapter. The viewer consumes only safe Rust messages.
//!
//! PdfCraft patch (#6324): odoc items are decoded from their raw descriptor bytes so
//! that a path whose on-disk name is NFC-precomposed is delivered byte-exact. The
//! decoding lives in [`odoc_path`] (pure Rust, tested on every platform); the
//! Objective-C adapter lives in [`macos`] and is upstream `fmv-macos-events` 0.1.0
//! except for the open-documents handler's item decode. The public API is unchanged.
#![deny(unsafe_op_in_unsafe_fn)]

/// Byte-level odoc decoding, independent of Objective-C so its tests run everywhere.
#[cfg(any(target_os = "macos", test))]
mod odoc_path;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::{Event, Inbox, Registration, install_edit_menu};
