//! Advanced PDF inspection & operations: Form JavaScript, XFA, PDF/A Preflight, Document Compare, Accessibility.

use crate::document::FfiRect;

// MARK: - Form JavaScript

/// Execution result of a JavaScript snippet.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiJsResult {
    pub success: bool,
    pub output: String,
    pub error: Option<String>,
}

/// Pending alerts, console entries, and errors emitted by JavaScript in the document.
#[derive(Debug, Clone, PartialEq, Eq, Default, uniffi::Record)]
pub struct FfiJsOutput {
    pub alerts: Vec<String>,
    pub console: Vec<String>,
    pub errors: Vec<String>,
}

// MARK: - XFA (XML Forms Architecture)

/// Status and metadata of an XFA form.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiXfaStatus {
    /// Form architecture: "none", "static", or "dynamic".
    pub kind: String,
    /// Number of synthesized pages in layout (for dynamic XFA), or 0.
    pub layout_pages: u32,
    /// Number of form fields bound to data.
    pub layout_fields: u32,
    /// Any warnings or unhandled packet notices encountered.
    pub warnings: Vec<String>,
}

// MARK: - Standards & PDF/A Preflight

/// A rule broken by the document under a target PDF/A specification level.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiPreflightIssue {
    pub clause: String,
    pub message: String,
    pub page: Option<u32>,
    pub fixable: bool,
}

/// Standards declared in document XMP metadata.
#[derive(Debug, Clone, PartialEq, Eq, Default, uniffi::Record)]
pub struct FfiPreflightDeclared {
    pub pdfa_part: Option<u32>,
    pub pdfa_conformance: Option<String>,
    pub pdfua_part: Option<u32>,
    pub output_intents: Vec<String>,
}

// MARK: - Compare Files

/// Classification of a difference between two document revisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FfiChangeKind {
    Inserted,
    Deleted,
    Replaced,
}

/// One side of a text difference (words, page, and bounding boxes).
#[derive(Debug, Clone, PartialEq, Default, uniffi::Record)]
pub struct FfiChangeSide {
    pub text: String,
    pub page: u32,
    pub rects: Vec<FfiRect>,
}

/// A text difference found between an older and newer document.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FfiDocumentChange {
    pub kind: FfiChangeKind,
    pub old_side: FfiChangeSide,
    pub new_side: FfiChangeSide,
}

/// Bounding box of a visual rendering difference on a specific page.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FfiVisualDiff {
    pub page: u32,
    pub rect: FfiRect,
}

// MARK: - Accessibility (a11y)

/// Status of an accessibility rule evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FfiA11yStatus {
    Passed,
    Failed,
    NeedsManualCheck,
    Skipped,
}

/// Result of evaluating a single accessibility rule.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiA11yRuleResult {
    pub rule_id: String,
    pub rule_name: String,
    pub category: String,
    pub status: FfiA11yStatus,
    pub description: String,
    pub explanation: String,
    pub findings: Vec<String>,
}

/// Summary report of an Accessibility Checker run across all 32 accessibility rules.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiA11yReport {
    pub passed_count: u32,
    pub failed_count: u32,
    pub manual_check_count: u32,
    pub skipped_count: u32,
    pub results: Vec<FfiA11yRuleResult>,
}

// MARK: - CJK Fallback Fonts (MOB-6.6 / O6)

/// Availability of CJK fallback fonts in the current engine build.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiCjkFontStatus {
    /// Whether a Japanese document fallback face (Shippori Mincho / BIZ UDMincho) is embedded.
    pub has_japanese_font: bool,
    /// Whether Chinese fallback faces are embedded.
    pub has_chinese_font: bool,
    /// Number of embedded craft-fonts available in this build.
    pub craft_fonts_count: u32,
}
