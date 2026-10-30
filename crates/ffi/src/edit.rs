//! Typed hot-path editing primitives (annotations, ink, forms).

/// A single 2D point on a PDF page in user points (y up or down depending on coordinate context).
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct FfiPoint {
    pub x: f32,
    pub y: f32,
}

/// Text markup kind for text selection annotations (§12.5.6.10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FfiTextMarkupKind {
    Highlight,
    Underline,
    StrikeOut,
    Squiggly,
}

impl From<FfiTextMarkupKind> for pdfcraft_engine::Markup {
    fn from(kind: FfiTextMarkupKind) -> Self {
        match kind {
            FfiTextMarkupKind::Highlight => pdfcraft_engine::Markup::Highlight,
            FfiTextMarkupKind::Underline => pdfcraft_engine::Markup::Underline,
            FfiTextMarkupKind::StrikeOut => pdfcraft_engine::Markup::StrikeOut,
            FfiTextMarkupKind::Squiggly => pdfcraft_engine::Markup::Squiggly,
        }
    }
}

/// Converts a packed 0xRRGGBB integer into normalized [r, g, b] float values in 0.0..=1.0.
pub fn rgb_from_u32(rgb: u32) -> [f64; 3] {
    let r = f64::from((rgb >> 16) & 0xFF) / 255.0;
    let g = f64::from((rgb >> 8) & 0xFF) / 255.0;
    let b = f64::from(rgb & 0xFF) / 255.0;
    [r, g, b]
}

/// Converts normalized [r, g, b] float values into a packed 0xRRGGBB integer.
pub fn u32_from_rgb(rgb: [f64; 3]) -> u32 {
    let r = (rgb[0].clamp(0.0, 1.0) * 255.0).round() as u32;
    let g = (rgb[1].clamp(0.0, 1.0) * 255.0).round() as u32;
    let b = (rgb[2].clamp(0.0, 1.0) * 255.0).round() as u32;
    (r << 16) | (g << 8) | b
}

/// Geometric shape kinds for vector annotations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FfiShapeKind {
    Rectangle,
    Oval,
    Line,
    Arrow,
}

/// An ink stroke composed of discrete sampled points.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FfiInkStroke {
    /// 0-based page index.
    pub page: u32,
    /// Sampled points in view coordinates (points, y down).
    pub points: Vec<FfiPoint>,
    /// RGB color as packed 0xRRGGBB integer.
    pub color_rgb: u32,
    /// Stroke line width in points.
    pub width: f32,
}

/// Summary representation of an annotation for UI hit-testing, selection, and manipulation.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FfiAnnotationSummary {
    /// 0-based page index.
    pub page: u32,
    /// 0-based index in the page's `/Annots` array.
    pub index: u32,
    /// Subtype string (e.g., "Highlight", "Text", "Square", "Circle", "Line", "FreeText", "Stamp", "Ink").
    pub subtype: String,
    /// Author / creator (/T) if set.
    pub author: Option<String>,
    /// Comment contents (/Contents) if set.
    pub contents: Option<String>,
    /// Bounding rectangle in view coordinates (points, y down).
    pub rect: crate::document::FfiRect,
    /// Stroke color as packed 0xRRGGBB integer.
    pub color_rgb: Option<u32>,
    /// Whether the annotation is locked against editing or deletion.
    pub locked: bool,
}

/// Review state for comment threads (§12.5.6.3 / ISO 32000-2 StateModel).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FfiReviewState {
    None,
    Accepted,
    Rejected,
    Cancelled,
    Completed,
}

impl From<FfiReviewState> for pdfcraft_engine::ReviewState {
    fn from(s: FfiReviewState) -> Self {
        match s {
            FfiReviewState::None => pdfcraft_engine::ReviewState::None,
            FfiReviewState::Accepted => pdfcraft_engine::ReviewState::Accepted,
            FfiReviewState::Rejected => pdfcraft_engine::ReviewState::Rejected,
            FfiReviewState::Cancelled => pdfcraft_engine::ReviewState::Cancelled,
            FfiReviewState::Completed => pdfcraft_engine::ReviewState::Completed,
        }
    }
}

impl From<pdfcraft_engine::ReviewState> for FfiReviewState {
    fn from(s: pdfcraft_engine::ReviewState) -> Self {
        match s {
            pdfcraft_engine::ReviewState::None => FfiReviewState::None,
            pdfcraft_engine::ReviewState::Accepted => FfiReviewState::Accepted,
            pdfcraft_engine::ReviewState::Rejected => FfiReviewState::Rejected,
            pdfcraft_engine::ReviewState::Cancelled => FfiReviewState::Cancelled,
            pdfcraft_engine::ReviewState::Completed => FfiReviewState::Completed,
        }
    }
}

/// A comment / annotation item for mobile comment list views and review threads.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FfiCommentItem {
    pub page: u32,
    pub index: u32,
    pub subtype: String,
    pub author: Option<String>,
    pub contents: Option<String>,
    pub modified: Option<String>,
    pub name: Option<String>,
    pub in_reply_to: Option<String>,
    pub rect: crate::document::FfiRect,
    pub color_rgb: Option<u32>,
    pub review_state: Option<FfiReviewState>,
    pub is_marked: bool,
    pub locked: bool,
}

/// A finding produced by redaction audit (e.g., covered text or unapplied marks).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FfiRedactionFinding {
    /// 1-based page number.
    pub page: u32,
    /// Kind of finding: "covered-text", "unapplied-mark", etc.
    pub kind: String,
    /// Bounding rectangle in displayed points.
    pub rect: crate::document::FfiRect,
    /// Hidden or covered text detected under the redaction mark.
    pub covered_text: String,
}

/// The result of applying redaction marks, verified with the audit crate.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FfiRedactionApplyResult {
    /// Number of redactions successfully applied.
    pub applied_count: u32,
    /// Whether the post-application security audit passed (0 unredacted text items detected).
    pub audit_passed: bool,
    /// Any remaining unredacted findings detected by the security audit.
    pub audit_findings: Vec<FfiRedactionFinding>,
}
