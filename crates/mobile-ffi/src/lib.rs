//! FFI boundary for native mobile apps (iOS, Android): a thin UniFFI wrapper around
//! `pdfcraft-render`'s read path (architecture §4, L7 frontends; this crate is `Class::Exempt` in
//! `xtask/src/layers.rs`, the same bucket as the other app entry points).
//!
//! Milestone 1 is a reader: open a document, read its page count, title and outline, and render
//! pages on demand. Editing goes through `pdfcraft-engine` and is out of scope here until a later
//! milestone extends this crate's surface.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::sync::{Arc, Mutex, MutexGuard};

use pdfcraft_render::{OpenError, OutlineItem, PageRenderer, RenderConfig, RenderRequest, inspect};

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
}

/// Recovers the guard from a poisoned mutex instead of panicking (CLAUDE.md §Never panic): a
/// render call that failed midway leaves no invariant for `PageRenderer` itself to violate.
fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
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
