//! Read-only PDF previews on COS clones: inline paragraph drafts, textless editing backgrounds,
//! and the page beneath a dragged added item or the marks an Update replaces.

use std::sync::Arc;

use pdfcraft_render::{PageRenderer, RenderConfig, RenderPool, RenderRequest, RenderedPage};

use crate::{Document, guard};

pub struct TextEditPreview {
    page: usize,
    bytes: Arc<Vec<u8>>,
    config: RenderConfig,
}

/// A read-only snapshot that can be sent to the draft worker without parsing the page on
/// the UI thread. Its session is opened on the worker and reused across keystrokes.
pub struct TextDraftSource {
    cos: pdfcraft_cos::Document,
    page: usize,
    block: usize,
    config: RenderConfig,
    /// The page's size and turn, to sample an OCR paragraph's scan colours on the worker.
    info: Option<pdfcraft_render::PageInfo>,
}

pub struct TextDraftSession {
    session: pdfcraft_edit::TextSession,
    config: RenderConfig,
    /// The paper and ink colours of an OCR paragraph's scan, which Apply covers it with.
    ocr: Option<([f64; 3], [f64; 3])>,
}

pub struct PreparedTextDraft {
    pub layout: crate::TextEditLayout,
    pub preview: TextEditPreview,
}

impl TextDraftSource {
    pub fn open(&self) -> Result<TextDraftSession, String> {
        guard(|| {
            let session = pdfcraft_edit::TextSession::open(&self.cos, self.page, self.block).map_err(|e| e.to_string())?;
            let ocr = self.info.as_ref().and_then(|info| crate::scan_colours::sample(&self.cos, info, self.page, session.source()));
            Ok(TextDraftSession { session, config: self.config.clone(), ocr })
        })?
    }
}

impl TextDraftSession {
    pub fn prepare(&self, text: &str, style: &pdfcraft_edit::BlockStyle) -> Result<PreparedTextDraft, String> {
        guard(|| {
            // An OCR paragraph is drafted as Apply writes it: over a cover of the scan's paper,
            // in its ink, and only the changed words (`ocr_text_style`).
            let source = self.session.source();
            let (cos, layout) = match self.ocr.filter(|_| style.cover.is_none()) {
                Some(colours) => match crate::session::ocr_text_style(colours, source, text, style.clone()) {
                    Some(style) => self.session.prepare(text, &style),
                    // The same words: Apply leaves the page as it is.
                    None => {
                        let plain = pdfcraft_edit::BlockStyle::default();
                        let (cos, unchanged) = self.session.prepare(&source.editor_text, &plain).map_err(|e| e.to_string())?;
                        let layout = if text == source.editor_text { Ok(unchanged) } else { self.session.layout(text, &plain) };
                        layout.map(|layout| (cos, layout))
                    }
                },
                None => self.session.prepare(text, style),
            }
            .map_err(|e| e.to_string())?;
            let bytes = Arc::new(pdfcraft_cos::write_incremental(&cos, &pdfcraft_cos::SaveOptions::default()).map_err(|e| e.to_string())?);
            Ok(PreparedTextDraft { layout, preview: TextEditPreview { page: self.session.page(), bytes, config: self.config.clone() } })
        })?
    }
}

impl TextEditPreview {
    pub fn renderer(&self) -> RenderPool {
        RenderPool::new(self.bytes.clone(), 1, self.config.clone())
    }

    pub fn render_background(&self, scale: f32) -> Result<RenderedPage, String> {
        render_one(&self.bytes, &self.config, self.page, scale)
    }
}

/// A document's pages without its marks of one kind ([`Document::without_marks`]).
pub struct UnmarkedPages {
    bytes: Arc<Vec<u8>>,
    config: RenderConfig,
    /// Parsed on the first [`Self::render_page`], once (a copy that can't be parsed isn't tried
    /// again), and kept: parsing costs about as much as reading the document.
    parsed: Option<PageRenderer>,
    /// How many times it was parsed.
    #[cfg(test)]
    builds: u64,
}

impl UnmarkedPages {
    pub fn renderer(&self) -> RenderPool {
        RenderPool::new(self.bytes.clone(), 1, self.config.clone())
    }

    /// `page` at `scale`, on this thread.
    pub fn render_page(&mut self, page: usize, scale: f32) -> Result<RenderedPage, String> {
        if !scale.is_finite() || scale <= 0.0 {
            return Err("preview scale must be finite and positive".into());
        }
        if self.parsed.is_none() {
            self.parsed = Some(PageRenderer::new(self.bytes.clone(), self.config.clone()));
            #[cfg(test)]
            {
                self.builds += 1;
            }
        }
        let Some(renderer) = self.parsed.as_mut() else { return Err("the document could not be parsed".into()) };
        let out = guard(|| renderer.render(RenderRequest { page, scale, ..Default::default() }));
        let out = match out {
            Ok(out) => out,
            Err(e) => {
                // A panic may have left it half-way: parse afresh next time.
                self.parsed = None;
                return Err(e);
            }
        };
        if let Some(e) = &out.error {
            return Err(e.clone());
        }
        Ok(out)
    }

    /// How many times [`Self::render_page`] has parsed the document.
    #[cfg(test)]
    pub(crate) fn parser_builds(&self) -> u64 {
        self.builds
    }
}

/// `page` of `bytes` at `scale`, on this thread.
fn render_one(bytes: &Arc<Vec<u8>>, config: &RenderConfig, page: usize, scale: f32) -> Result<RenderedPage, String> {
    if !scale.is_finite() || scale <= 0.0 {
        return Err("preview scale must be finite and positive".into());
    }
    guard(|| {
        let mut renderer = PageRenderer::new(bytes.clone(), config.clone());
        let out = renderer.render(RenderRequest { page, scale, ..Default::default() });
        if let Some(e) = &out.error {
            return Err(e.clone());
        }
        Ok(out)
    })?
}

impl Document {
    /// Background for retyping an added text item, without changing its saved content.
    pub fn added_text_preview(&self, page: usize, index: usize) -> Result<TextEditPreview, String> {
        guard(|| {
            if !self.added().iter().filter(|a| a.page == page).nth(index).is_some_and(|a| matches!(a.content, crate::AddedContent::Text(_))) {
                return Err("the selected added item is not text".into());
            }
            let editor = self.editor.as_ref().ok_or("the document can't be read")?;
            let mut cos = editor.cos.clone();
            pdfcraft_edit::delete_content(&mut cos, page, index).map_err(|e| e.to_string())?;
            let bytes = Arc::new(pdfcraft_cos::write_incremental(&cos, &pdfcraft_cos::SaveOptions::default()).map_err(|e| e.to_string())?);
            Ok(TextEditPreview { page, bytes, config: self.config.clone() })
        })?
    }

    /// Measure a draft without changing bytes, dirty state, or undo history.
    pub fn text_edit_layout(
        &self,
        page: usize,
        block: usize,
        text: &str,
        style: &pdfcraft_edit::BlockStyle,
    ) -> Result<pdfcraft_edit::text::TextEditLayout, String> {
        guard(|| {
            let editor = self.editor.as_ref().ok_or("the document can't be read")?;
            // Measure the rewrite Apply makes, as the draft preview draws it: an OCR paragraph's
            // scan cover, ink and changed words only. A draft Apply leaves as it is keeps its input.
            let edit = crate::Edit::EditTextBlock { page, block, text: text.to_owned(), style: style.clone() };
            let prepared = crate::session::prepare_ocr_text_edit(editor, &self.info, edit);
            let style = match &prepared {
                Some(crate::Edit::EditTextBlock { style, .. }) => style,
                _ => style,
            };
            pdfcraft_edit::text::text_edit_layout(&editor.cos, page, block, text, style).map_err(|e| e.to_string())
        })?
    }

    /// Embedded source faces for a text editor, bounded and validated as sfnt programs.
    /// This is read-only; no source font is installed or exported.
    pub fn text_editor_fonts(&self, page: usize) -> Vec<pdfcraft_edit::text::TextEditorFont> {
        self.editor.as_ref().and_then(|e| pdfcraft_edit::text::text_editor_fonts(&e.cos, page).ok()).unwrap_or_default()
    }

    /// Capture the current editing state without extracting or rewriting page text.
    pub fn text_draft_source(&self, page: usize, block: usize) -> Result<TextDraftSource, String> {
        guard(|| {
            let editor = self.editor.as_ref().ok_or("the document can't be read")?;
            Ok(TextDraftSource { cos: editor.cos.clone(), page, block, config: self.config.clone(), info: self.info.pages.get(page).cloned() })
        })?
    }

    /// Render a paragraph draft with the same PDF fonts and rewrite as Apply, on a copy.
    /// The live document, its dirty state and undo history are untouched.
    pub fn text_edit_draft_preview(
        &self,
        page: usize,
        block: usize,
        text: &str,
        style: &pdfcraft_edit::BlockStyle,
    ) -> Result<TextEditPreview, String> {
        Ok(self.text_draft_source(page, block)?.open()?.prepare(text, style)?.preview)
    }

    /// Preparing the background never changes working bytes, dirty state or undo history.
    pub fn text_edit_preview(&self, page: usize, block: usize) -> Result<TextEditPreview, String> {
        guard(|| {
            let editor = self.editor.as_ref().ok_or("the document can't be read")?;
            let mut cos = editor.cos.clone();
            pdfcraft_edit::clear_block(&mut cos, page, block).map_err(|e| e.to_string())?;
            let bytes = Arc::new(pdfcraft_cos::write_incremental(&cos, &pdfcraft_cos::SaveOptions::default()).map_err(|e| e.to_string())?);
            Ok(TextEditPreview { page, bytes, config: self.config.clone() })
        })?
    }

    /// The page beneath an added item while it is dragged: `index` counts the page's added
    /// items, as `Edit::DeleteContent` does. Read-only, like [`Self::text_edit_preview`].
    pub fn added_content_preview(&self, page: usize, index: usize) -> Result<TextEditPreview, String> {
        guard(|| {
            let editor = self.editor.as_ref().ok_or("the document can't be read")?;
            let mut cos = editor.cos.clone();
            pdfcraft_edit::delete_content(&mut cos, page, index).map_err(|e| e.to_string())?;
            let bytes = Arc::new(pdfcraft_cos::write_incremental(&cos, &pdfcraft_cos::SaveOptions::default()).map_err(|e| e.to_string())?);
            Ok(TextEditPreview { page, bytes, config: self.config.clone() })
        })?
    }

    /// The document without its marks of `kind`, on every page: what Update replaces them on, so
    /// its preview doesn't draw the new marks over the old ones (or tint the old background).
    /// Made once for all the pages a dialog steps through, as cloning and writing the document
    /// costs about a save. `None` when the document has none, and the thumbnails already are
    /// those pages. Read-only, like [`Self::text_edit_preview`].
    pub fn without_marks(&self, kind: crate::MarkKind) -> Result<Option<UnmarkedPages>, String> {
        if !self.marks().contains(&kind) {
            return Ok(None);
        }
        guard(|| {
            let editor = self.editor.as_ref().ok_or("the document can't be read")?;
            let mut cos = editor.cos.clone();
            let n = pdfcraft_model::pages(&cos).len();
            if pdfcraft_edit::remove_marks(&mut cos, &(0..n).collect::<Vec<_>>(), kind).map_err(|e| e.to_string())? == 0 {
                return Ok(None);
            }
            let bytes = Arc::new(pdfcraft_cos::write_incremental(&cos, &pdfcraft_cos::SaveOptions::default()).map_err(|e| e.to_string())?);
            Ok(Some(UnmarkedPages {
                bytes,
                config: self.config.clone(),
                parsed: None,
                #[cfg(test)]
                builds: 0,
            }))
        })?
    }

    /// An added item alone, on a transparent page: what follows the pointer while it is dragged,
    /// so the page under its first place doesn't come along. Read-only, like
    /// [`Self::added_content_preview`].
    pub fn added_content_alone(&self, page: usize, index: usize) -> Result<TextEditPreview, String> {
        guard(|| {
            let editor = self.editor.as_ref().ok_or("the document can't be read")?;
            let mut cos = editor.cos.clone();
            pdfcraft_edit::isolate_content(&mut cos, page, index).map_err(|e| e.to_string())?;
            let bytes = Arc::new(pdfcraft_cos::write_incremental(&cos, &pdfcraft_cos::SaveOptions::default()).map_err(|e| e.to_string())?);
            Ok(TextEditPreview { page, bytes, config: RenderConfig { transparent: true, ..self.config.clone() } })
        })?
    }
}

#[cfg(test)]
mod draft_tests {
    use super::*;

    fn document() -> (crate::Session, crate::DocId) {
        let mut session = crate::Session::new();
        let text = (0..30).map(|n| format!("Paragraph {n}: wide W and narrow i characters share the same PDF fonts.\n\n")).collect::<String>();
        let bytes = session.create_from_text("Draft", &text).unwrap();
        let id = session.open("draft.pdf", None, bytes, None).unwrap();
        (session, id)
    }

    #[test]
    fn prepared_draft_layout_and_pixels_match_apply_without_changing_the_document() {
        let (mut session, id) = document();
        let doc = session.get(id).unwrap();
        let bytes = doc.bytes.clone();
        let source = doc.text_draft_source(0, 0).unwrap();
        let worker_session = source.open().unwrap();
        let text = "A changed heading with WiWi";
        let style = pdfcraft_edit::BlockStyle::default();
        let draft = worker_session.prepare(text, &style).unwrap();
        assert_eq!(draft.layout, doc.text_edit_layout(0, 0, text, &style).unwrap());
        let preview = draft.preview.render_background(1.0).unwrap();
        assert_eq!(doc.bytes, bytes);
        assert!(!doc.dirty && doc.can_undo().is_none());
        session.apply(id, crate::Edit::EditTextBlock { page: 0, block: 0, text: text.into(), style }).unwrap();
        let applied = session.get(id).unwrap();
        let mut renderer = PageRenderer::new(applied.display.clone(), applied.config.clone());
        let applied = renderer.render(RenderRequest { page: 0, scale: 1.0, ..Default::default() });
        assert!(applied.error.is_none());
        assert_eq!(preview.rgba, applied.rgba);
        let second = worker_session.prepare("Another heading", &Default::default()).unwrap();
        assert_eq!(second.layout, source.open().unwrap().prepare("Another heading", &Default::default()).unwrap().layout);
    }

    #[test]
    #[ignore = "manual timing comparison, no machine-dependent speed assertion"]
    fn compare_separate_and_combined_draft_preparation() {
        let (session, id) = document();
        let doc = session.get(id).unwrap();
        let base = &doc.editor.as_ref().unwrap().cos;
        let style = pdfcraft_edit::BlockStyle::default();
        let drafts: Vec<_> = (0..30).map(|n| format!("Typing a changed heading {n}")).collect();
        let start = std::time::Instant::now();
        for text in &drafts {
            std::hint::black_box(doc.text_edit_layout(0, 0, text, &style).unwrap());
            // The previous UI prepared layout, then independently rewrote/serialized a preview.
            let mut cos = base.clone();
            pdfcraft_edit::rewrite_block(&mut cos, 0, 0, Some(text), &style).unwrap();
            std::hint::black_box(pdfcraft_cos::write_incremental(&cos, &Default::default()).unwrap());
        }
        let separate = start.elapsed();
        let start = std::time::Instant::now();
        let worker_session = doc.text_draft_source(0, 0).unwrap().open().unwrap();
        for text in &drafts {
            std::hint::black_box(worker_session.prepare(text, &style).unwrap());
        }
        let combined = start.elapsed();
        eprintln!("30 drafts, preparation only: separate={separate:?}, combined/session={combined:?}");
    }
}
