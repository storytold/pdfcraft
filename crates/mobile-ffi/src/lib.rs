//! FFI boundary for native mobile apps (iOS, Android): a thin UniFFI wrapper around
//! `pdfcraft-render`'s read path (architecture §4, L7 frontends; this crate is `Class::Exempt` in
//! `xtask/src/layers.rs`, the same bucket as the other app entry points).
//!
//! This crate is a reader: open a document, read its page count/title/outline, render pages on
//! demand, and search for text across the document. Editing goes through `pdfcraft-engine` and
//! is out of scope here until a later milestone extends this crate's surface.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::sync::{Arc, Mutex, MutexGuard};

use pdfcraft_render::{OpenError, OutlineItem, PageRenderer, PageText, RenderConfig, RenderRequest, RequestKind, inspect};

uniffi::setup_scaffolding!();

/// Why a document could not be opened or a page could not be rendered.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum MobileError {
    #[error("the file is not a readable PDF")]
    Invalid,
    #[error("the document is protected by a password")]
    NeedsPassword,
    #[error("the password is incorrect")]
    WrongPassword,
    #[error("this document isn't supported yet: {reason}")]
    Unsupported { reason: String },
    #[error("there is no page {index}")]
    NoSuchPage { index: u32 },
    #[error("page {index} could not be rendered: {reason}")]
    RenderFailed { index: u32, reason: String },
}

impl From<OpenError> for MobileError {
    fn from(err: OpenError) -> Self {
        match err {
            OpenError::Invalid(_) => Self::Invalid,
            OpenError::NeedsPassword => Self::NeedsPassword,
            OpenError::WrongPassword => Self::WrongPassword,
            OpenError::Unsupported(reason) => Self::Unsupported { reason },
        }
    }
}

/// A single rasterized page, ready for a native `UIImage` (iOS) or `Bitmap` (Android).
#[derive(uniffi::Record)]
pub struct MobilePage {
    pub width: u32,
    pub height: u32,
    /// Premultiplied RGBA8, row-major, 4 bytes per pixel.
    pub rgba: Vec<u8>,
}

/// One flattened table-of-contents entry (`DocInfo::outline`, depth-first).
#[derive(Clone, uniffi::Record)]
pub struct OutlineEntry {
    pub title: String,
    pub page: u32,
    pub depth: u32,
}

/// A rectangle in the same page-view-space pixel coordinates a [`MobilePage`] rendered at the
/// same `scale` uses, so it can be drawn directly over that bitmap.
#[derive(Clone, Copy, Debug, uniffi::Record)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

/// One match from [`MobileDocument::search`].
#[derive(Clone, Debug, uniffi::Record)]
pub struct SearchMatch {
    pub page: u32,
    /// A line of context around the match, not just the matched characters.
    pub snippet: String,
    /// One rect per visual line the match touches (a match can wrap a line).
    pub rects: Vec<Rect>,
}

/// A document opened for reading: validated once at [`MobileDocument::open`], then rendered on
/// demand through [`MobileDocument::render_page`].
#[derive(uniffi::Object)]
pub struct MobileDocument {
    page_count: usize,
    title: Option<String>,
    outline: Vec<OutlineEntry>,
    renderer: Mutex<PageRenderer>,
}

#[uniffi::export]
impl MobileDocument {
    /// Opens `bytes` for reading. Pass `password` when the document needs one;
    /// `MobileError::NeedsPassword` or `MobileError::WrongPassword` means try again with one.
    #[uniffi::constructor]
    pub fn open(bytes: Vec<u8>, password: Option<String>) -> Result<Arc<Self>, MobileError> {
        let bytes = Arc::new(bytes);
        let info = inspect(bytes.clone(), password.as_deref())?;
        let config = RenderConfig { password: password.map(|p| Arc::from(p.as_str())), ..Default::default() };
        Ok(Arc::new(Self {
            page_count: info.pages.len(),
            title: info.title,
            outline: flatten_outline(&info.outline, 0),
            renderer: Mutex::new(PageRenderer::new(bytes, config)),
        }))
    }

    pub fn page_count(&self) -> u32 {
        u32::try_from(self.page_count).unwrap_or(u32::MAX)
    }

    pub fn title(&self) -> Option<String> {
        self.title.clone()
    }

    pub fn outline(&self) -> Vec<OutlineEntry> {
        self.outline.clone()
    }

    /// Renders `page` (0-based) at `scale` device pixels per PDF point — the screen's device
    /// scale factor times the desired zoom level.
    pub fn render_page(&self, page: u32, scale: f32) -> Result<MobilePage, MobileError> {
        if page as usize >= self.page_count {
            return Err(MobileError::NoSuchPage { index: page });
        }
        let mut renderer = lock_or_recover(&self.renderer);
        let shot = renderer.render(RenderRequest { page: page as usize, scale, ..Default::default() });
        if let Some(reason) = shot.error {
            return Err(MobileError::RenderFailed { index: page, reason });
        }
        Ok(MobilePage { width: shot.width, height: shot.height, rgba: shot.rgba.to_vec() })
    }

    /// Searches every page for `query` at `scale` (the same scale the caller renders pages at,
    /// so each match's `rects` line up with that page's bitmap). A page whose text can't be
    /// extracted is skipped, not an error for the whole search.
    ///
    /// Extracts text for every page in one call, so cost scales with document length; there is
    /// no incremental/streaming search yet (unlike the desktop app's find bar).
    pub fn search(&self, query: String, case_sensitive: bool, whole_words: bool, scale: f32) -> Vec<SearchMatch> {
        let mut renderer = lock_or_recover(&self.renderer);
        let mut matches = Vec::new();
        for page in 0..self.page_count {
            let shot = renderer.render(RenderRequest { page, kind: RequestKind::Text, scale, ..Default::default() });
            let Some(text) = shot.text else { continue };
            let Ok(page) = u32::try_from(page) else { continue };
            for range in text.find_opts(&query, case_sensitive, whole_words) {
                matches.push(SearchMatch { page, snippet: text.text_of(line_range(&text, &range)), rects: match_rects(&text, &range) });
            }
        }
        matches
    }
}

/// Recovers the guard from a poisoned mutex instead of panicking (CLAUDE.md §Never panic): a
/// render call that failed midway leaves no invariant for `PageRenderer` itself to violate.
fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Widens `range` to the full line(s) its first and last glyph belong to, for a snippet with a
/// line of context rather than just the bare matched characters. Falls back to `range` itself
/// when a line's glyphs can't be found (defensive only; `find_opts` always returns valid ranges).
fn line_range(text: &PageText, range: &std::ops::Range<usize>) -> std::ops::Range<usize> {
    let first_line = text.line_of.get(range.start).copied();
    let last_line = range.end.checked_sub(1).and_then(|i| text.line_of.get(i)).copied();
    let start = first_line.and_then(|line| text.line_of.iter().position(|&l| l == line));
    let end = last_line.and_then(|line| text.line_of.iter().rposition(|&l| l == line));
    match (start, end) {
        (Some(start), Some(end)) => start..(end + 1),
        _ => range.clone(),
    }
}

/// One bounding rect per visual line `range`'s glyphs touch (a match can wrap a line); glyphs of
/// the same line are contiguous in `text.glyphs`, so adjacent same-line glyphs merge in place.
fn match_rects(text: &PageText, range: &std::ops::Range<usize>) -> Vec<Rect> {
    let mut rects: Vec<(u32, Rect)> = Vec::new();
    for i in range.clone() {
        let (Some(glyph), Some(&line)) = (text.glyphs.get(i), text.line_of.get(i)) else { continue };
        let [x0, y0, x1, y1] = glyph.rect;
        match rects.last_mut() {
            Some((last_line, rect)) if *last_line == line => {
                rect.x0 = rect.x0.min(x0);
                rect.y0 = rect.y0.min(y0);
                rect.x1 = rect.x1.max(x1);
                rect.y1 = rect.y1.max(y1);
            }
            _ => rects.push((line, Rect { x0, y0, x1, y1 })),
        }
    }
    rects.into_iter().map(|(_, rect)| rect).collect()
}

fn flatten_outline(items: &[OutlineItem], depth: u32) -> Vec<OutlineEntry> {
    let mut out = Vec::new();
    for item in items {
        if let Some(page) = item.page.and_then(|p| u32::try_from(p).ok()) {
            out.push(OutlineEntry { title: item.title.clone(), page, depth });
        }
        out.extend(flatten_outline(&item.children, depth + 1));
    }
    out
}

#[cfg(test)]
mod tests;
