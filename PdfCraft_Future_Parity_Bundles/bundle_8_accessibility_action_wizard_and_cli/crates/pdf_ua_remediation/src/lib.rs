//! PdfCraft PDF/UA Remediation: Matterhorn 32-rule validator, auto-tagger, and table remediator.
//!
//! Subsystem covering Milestone M12:
//! - [P1] PDF/UA-1 & PDF/UA-2 verification
//! - [P1] Automatically tag a document
//! - [P1] Reading order tool and mark content as artifact
//! - [P1] Table /TH scope remediation

pub mod validator;
pub mod autotag;
pub mod table_fix;

pub use validator::{AccessibilitySeverity, PdfUaValidator, UaViolation};
pub use autotag::{AutoTagClassifier, ContentBlock, StandardStructureTag};
pub use table_fix::{TableHeaderScope, TableRemediator};
