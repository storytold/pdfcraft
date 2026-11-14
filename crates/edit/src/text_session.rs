//! A text editing session for one paragraph (#2512). Opening it reads the page once; each draft
//! is laid out from that reading, on a copy of the document as it was opened, so no draft sees
//! another one's output; the commit writes into the document it was opened on, and refuses if
//! that document has changed since.

use super::{
    BlockStyle, Document, EditError, LineEdit, MAX_TYPE3_GLYPHS, TextBlock, TextEditLayout, TextLine, complete_named_fonts, group_blocks,
    layout_draft, rewrite_block_from, text_lines,
};

/// One paragraph being edited: the document as it was opened, and the page's lines then.
pub struct TextSession {
    base: Document,
    base_state: pdfcraft_cos::ReadState,
    page: usize,
    block: usize,
    lines: Vec<TextLine>,
    source: TextBlock,
}

impl TextSession {
    /// Opens paragraph `block` on `page` (both 0-based) of `doc`.
    pub fn open(doc: &Document, page: usize, block: usize) -> Result<Self, EditError> {
        let lines = text_lines(doc, page)?;
        let source = group_blocks(&lines)
            .get(block)
            .cloned()
            .ok_or_else(|| EditError::Invalid(format!("page {} has no paragraph {}", page + 1, block + 1)))?;
        Ok(Self { base: doc.clone(), base_state: doc.read_state(), page, block, lines, source })
    }

    /// The paragraph as it was when the session opened.
    pub fn source(&self) -> &TextBlock {
        &self.source
    }

    pub fn page(&self) -> usize {
        self.page
    }

    pub fn block(&self) -> usize {
        self.block
    }

    /// Whether `doc` is still the document the session was opened on (nothing applied since).
    pub fn is_current(&self, doc: &Document) -> bool {
        self.base_state.matches(doc)
    }

    /// Lays a draft out without writing it: the rewrite Apply runs, on a copy of the opened document.
    pub fn layout(&self, text: &str, style: &BlockStyle) -> Result<TextEditLayout, EditError> {
        layout_draft(&self.base, self.page, &self.lines, &self.source, self.block, text, style)
    }

    /// Writes the draft into `doc`, which must still be the document the session was opened on.
    pub fn commit(&self, doc: &mut Document, text: Option<&str>, style: &BlockStyle) -> Result<LineEdit, EditError> {
        self.commit_with_limit(doc, text, style, MAX_TYPE3_GLYPHS)
    }

    pub(crate) fn commit_with_limit(
        &self,
        doc: &mut Document,
        text: Option<&str>,
        style: &BlockStyle,
        max_glyphs: usize,
    ) -> Result<LineEdit, EditError> {
        if !self.is_current(doc) {
            return Err(EditError::Invalid("the paragraph changed while it was being edited".into()));
        }
        // A font the document only names learns the advances of the new characters first, so they
        // are written in it. That and the rewrite run on a copy: a refused edit leaves `doc` as it
        // was and this session current. Font objects put back by hand would be new objects to
        // `is_current`, and every later commit of the session would be refused.
        let mut trial = doc.clone();
        if !text.is_some_and(|draft| complete_named_fonts(&mut trial, self.page, &self.lines, &self.source, draft, style)) {
            return rewrite_block_from(doc, self.page, self.lines.clone(), self.block, text, style, max_glyphs);
        }
        let edit = rewrite_block_from(&mut trial, self.page, self.lines.clone(), self.block, text, style, max_glyphs)?;
        *doc = trial;
        Ok(edit)
    }
}
