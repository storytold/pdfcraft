//! Bounded, read-only snapshots for local assistive reading. No audio or OS dependencies.
use crate::Document;
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub const MAX_PAGES: usize = 64;
pub const MAX_TEXT_BYTES: usize = 1_048_576;
pub const MAX_CHUNKS: usize = 4096;
const CHUNK_BYTES: usize = 2048;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    /// 1-based document page, even when preceding empty pages were skipped.
    pub page: usize,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub chunks: Vec<Chunk>,
    pub empty_pages: Vec<usize>,
}

/// Owns a snapshot so native frontends can extract off their UI thread. It never changes the PDF.
#[derive(Clone)]
pub struct Source {
    bytes: Arc<Vec<u8>>,
    password: Option<Arc<str>>,
    pages: Vec<usize>,
}

impl Document {
    /// Prepare 0-based pages in strictly increasing document order. Accessibility reading is
    /// allowed even when copying is forbidden (ISO 32000-2 §7.6.4.2).
    pub fn read_aloud_source(&self, pages: &[usize]) -> Result<Source, String> {
        if pages.is_empty() || pages.len() > MAX_PAGES {
            return Err(format!("choose 1–{MAX_PAGES} pages for reading aloud"));
        }
        if pages.windows(2).any(|p| p.first() >= p.get(1)) {
            return Err("read-aloud pages must be unique and in document order".into());
        }
        if let Some(page) = pages.iter().find(|p| self.info.pages.get(**p).is_none()) {
            return Err(format!("page {} is outside the document", page.saturating_add(1)));
        }
        Ok(Source { bytes: self.bytes.clone(), password: self.password.as_deref().map(Arc::from), pages: pages.to_vec() })
    }
}

impl Source {
    /// Extract the snapshot into bounded speech chunks without modifying document bytes.
    pub fn extract(&self) -> Result<Plan, String> {
        self.extract_cancellable(&AtomicBool::new(false))
    }

    /// Extract with cooperative cancellation and convert renderer panics into an error.
    pub fn extract_cancellable(&self, cancelled: &AtomicBool) -> Result<Plan, String> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.extract_inner(cancelled)))
            .unwrap_or_else(|_| Err("text extraction stopped unexpectedly; the document is unchanged".into()))
    }

    /// Read requested pages in order, enforcing text/chunk budgets and reporting empty pages.
    fn extract_inner(&self, cancelled: &AtomicBool) -> Result<Plan, String> {
        let mut renderer = PageRenderer::new(self.bytes.clone(), RenderConfig { password: self.password.clone(), ..Default::default() });
        let mut plan = Plan { chunks: Vec::new(), empty_pages: Vec::new() };
        let mut bytes = 0usize;
        for &page in &self.pages {
            if cancelled.load(Ordering::Relaxed) {
                return Err("read-aloud preparation was cancelled".into());
            }
            let result = renderer.render(RenderRequest { page, kind: RequestKind::Text, tile: None, scale: 1.0, tag: 0, preview: false });
            if let Some(e) = result.error {
                return Err(format!("page {}: {e}", page.saturating_add(1)));
            }
            let text = result.text.map(|t| t.plain_text()).unwrap_or_default();
            bytes = bytes.checked_add(text.len()).filter(|n| *n <= MAX_TEXT_BYTES).ok_or("read-aloud text exceeds 1 MiB; choose fewer pages")?;
            if text.trim().is_empty() {
                plan.empty_pages.push(page.saturating_add(1));
                continue;
            }
            split_text(&text, page.saturating_add(1), &mut plan.chunks)?;
        }
        if plan.chunks.is_empty() {
            return Err("these pages have no readable text; run OCR on scanned pages first".into());
        }
        Ok(plan)
    }
}

/// Append UTF-8-safe chunks for a 1-based page, preferring sentence and word boundaries.
/// Return an error instead of silently dropping text when the chunk budget is exhausted.
fn split_text(mut text: &str, page: usize, chunks: &mut Vec<Chunk>) -> Result<(), String> {
    while !text.trim().is_empty() {
        text = text.trim_start();
        let mut end = text.len().min(CHUNK_BYTES);
        while !text.is_char_boundary(end) {
            end = end.saturating_sub(1);
        }
        let prefix = text.get(..end).ok_or("invalid text boundary")?;
        if end < text.len() {
            // Prefer sentence/line boundaries, then a word boundary, without dropping letters.
            if let Some((at, ch)) = prefix.char_indices().rev().find(|(at, ch)| *at >= end / 2 && matches!(ch, '.' | '!' | '?' | '\n')) {
                end = at + ch.len_utf8();
            } else if let Some((at, ch)) = prefix.char_indices().rev().find(|(at, ch)| *at >= end / 2 && ch.is_whitespace()) {
                end = at + ch.len_utf8();
            }
        }
        if chunks.len() >= MAX_CHUNKS {
            return Err("read-aloud text has too many chunks; choose fewer pages".into());
        }
        let chunk = text.get(..end).ok_or("invalid text boundary")?.trim();
        if !chunk.is_empty() {
            chunks.push(Chunk { page, text: chunk.into() });
        }
        text = text.get(end..).ok_or("invalid text boundary")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Verify chunking preserves non-whitespace Unicode content, page numbers and byte limits.
    #[test]
    fn unicode_chunks_preserve_text_and_page_without_splitting_characters() {
        for text in ["hello world. ".repeat(800), "字🌍مرحبا".repeat(800), "x".repeat(5000)] {
            let mut chunks = Vec::new();
            split_text(&text, 7, &mut chunks).unwrap();
            assert!(chunks.iter().all(|c| c.page == 7 && !c.text.is_empty() && c.text.len() <= CHUNK_BYTES));
            assert_eq!(
                chunks.iter().flat_map(|c| c.text.chars()).filter(|c| !c.is_whitespace()).collect::<String>(),
                text.chars().filter(|c| !c.is_whitespace()).collect::<String>()
            );
        }
    }
    /// Verify a full chunk budget rejects additional text instead of truncating the reading plan.
    #[test]
    fn chunk_limit_is_an_error_not_silent_truncation() {
        let mut chunks = vec![Chunk { page: 1, text: "full".into() }; MAX_CHUNKS];
        assert!(split_text("extra", 1, &mut chunks).is_err());
    }
}
