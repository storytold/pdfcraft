//! The scan job journal and its read-only views live in `craft_devices::scan_jobs`, shared by every
//! Craft app; this module keeps the old path and fixes the command name the hints print.

pub use craft_devices::scan_jobs::*;

/// The command the journal's next-step hints name.
pub const PROGRAM: &str = "pdfcraft-cli";
