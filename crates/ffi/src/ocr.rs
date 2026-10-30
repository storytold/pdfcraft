//! Scan & OCR bridge: Recognize text, inspect model availability, and add searchable text layers.

use crate::document::FfiRect;

/// Status and configuration of the OCR subsystem.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiOcrStatus {
    pub available: bool,
    pub search_dirs: Vec<String>,
    pub languages: Vec<String>,
}

/// A recognized word with its bounding box in view coordinates.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FfiOcrWord {
    pub text: String,
    pub rect: FfiRect,
}

/// Result of running text recognition on a document page.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FfiOcrPageResult {
    pub page: u32,
    pub words: Vec<FfiOcrWord>,
    pub text: String,
    pub word_count: u32,
    pub already_on_page: u32,
    pub skipped: Option<String>,
}

/// Helper to convert a PlacedWord from user coordinates to view-space FfiOcrWord.
#[allow(dead_code)]
pub(crate) fn placed_word_to_ffi(w: &pdfcraft_engine::ocr::PlacedWord, page: &pdfcraft_render::PageInfo) -> FfiOcrWord {
    // w.origin is bottom-left in user coords, w.across is width vector, w.up is height vector
    let (ux0, uy0) = (w.origin[0] as f32, w.origin[1] as f32);
    let (ux1, uy1) = ((w.origin[0] + w.across[0]) as f32, (w.origin[1] + w.up[1]) as f32);
    let [vx0, vy0] = page.user_to_view(ux0, uy0);
    let [vx1, vy1] = page.user_to_view(ux1, uy1);
    let min_x = vx0.min(vx1);
    let min_y = vy0.min(vy1);
    let width = (vx1 - vx0).abs();
    let height = (vy1 - vy0).abs();
    FfiOcrWord { text: w.text.clone(), rect: FfiRect { x: min_x, y: min_y, width, height } }
}
