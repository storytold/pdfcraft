use pdfcraft_render::text::PageText;
use pdfcraft_render::{DocInfo, OutlineItem};

/// Geometry of a single page in PDF points.
#[derive(Debug, Clone, uniffi::Record)]
pub struct FfiPageGeometry {
    /// Width in points.
    pub width: f32,
    /// Height in points.
    pub height: f32,
    /// Clockwise rotation in degrees (0, 90, 180, 270).
    pub rotation: u32,
    /// Page label (e.g., "1", "iv", "A-1").
    pub label: String,
}

/// Document metadata and structural summary.
#[derive(Debug, Clone, uniffi::Record)]
pub struct FfiDocInfo {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Option<String>,
    pub creator: Option<String>,
    pub producer: Option<String>,
    pub page_count: u32,
    pub encrypted: bool,
    pub has_javascript: bool,
}

/// A rectangular box in view space (points, y down).
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct FfiRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// An outline / bookmark item.
#[derive(Debug, Clone, uniffi::Record)]
pub struct FfiOutlineItem {
    pub title: String,
    /// 0-based destination page, if set.
    pub page: Option<u32>,
    pub children: Vec<FfiOutlineItem>,
}

/// A search match result on a page.
#[derive(Debug, Clone, uniffi::Record)]
pub struct FfiFindMatch {
    pub page: u32,
    pub start_glyph: u32,
    pub end_glyph: u32,
    pub text: String,
    pub rect: FfiRect,
}

/// An interactive hyperlink on a page.
#[derive(Debug, Clone, uniffi::Record)]
pub struct FfiLink {
    pub page: u32,
    pub rect: FfiRect,
    pub target_page: Option<u32>,
    pub target_uri: Option<String>,
}

/// A crash-recovery or autosave snapshot of a document with unsaved modifications.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiRecoverySnapshot {
    /// Document identifier.
    pub doc_id: u64,
    /// Document display name.
    pub name: String,
    /// Original file path if opened from disk.
    pub path: Option<String>,
    /// Incremental PDF bytes of the current state.
    pub bytes: Vec<u8>,
    /// Whether the snapshot is encrypted (requires password to reopen).
    pub encrypted: bool,
}

pub(crate) fn map_doc_info(info: &DocInfo) -> FfiDocInfo {
    FfiDocInfo {
        title: info.title.clone(),
        author: info.author.clone(),
        subject: info.subject.clone(),
        keywords: info.keywords.clone(),
        creator: info.creator.clone(),
        producer: info.producer.clone(),
        page_count: info.pages.len() as u32,
        encrypted: info.encrypted,
        has_javascript: info.has_javascript,
    }
}

pub(crate) fn map_outline(items: &[OutlineItem]) -> Vec<FfiOutlineItem> {
    items
        .iter()
        .map(|item| FfiOutlineItem { title: item.title.clone(), page: item.page.map(|p| p as u32), children: map_outline(&item.children) })
        .collect()
}

pub(crate) fn find_in_page_text(page_idx: u32, text: &PageText, query: &str, case_sensitive: bool) -> Vec<FfiFindMatch> {
    let ranges = text.find_opts(query, case_sensitive, false);
    ranges
        .into_iter()
        .map(|range| {
            let matched_text = text.text_of(range.clone());
            let mut min_x = f32::MAX;
            let mut min_y = f32::MAX;
            let mut max_x = f32::MIN;
            let mut max_y = f32::MIN;

            for i in range.clone() {
                if let Some(g) = text.glyphs.get(i) {
                    min_x = min_x.min(g.rect[0]);
                    min_y = min_y.min(g.rect[1]);
                    max_x = max_x.max(g.rect[2]);
                    max_y = max_y.max(g.rect[3]);
                }
            }

            let rect = if min_x <= max_x && min_y <= max_y {
                FfiRect { x: min_x, y: min_y, width: max_x - min_x, height: max_y - min_y }
            } else {
                FfiRect { x: 0.0, y: 0.0, width: 0.0, height: 0.0 }
            };

            FfiFindMatch { page: page_idx, start_glyph: range.start as u32, end_glyph: range.end as u32, text: matched_text, rect }
        })
        .collect()
}
