//! PdfCraft Preflight Engine: Extensible rule validation, automated fixups, and reports.
//!
//! Subsystem covering Milestone M11:
//! - [P1] Preflight analysis
//! - [P1] Preflight analyze and fix
//! - [P1] Built-in preflight profiles and reports

pub mod rule;
pub mod report;
pub mod runner;

pub use rule::{PreflightRule, PreflightSeverity, PreflightViolation};
pub use report::PreflightAuditReport;
pub use runner::PreflightRunner;
