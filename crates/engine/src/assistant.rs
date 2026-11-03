//! The AI assistant's document side (ADR "AI assistant", M13.A4): what of a document may be sent
//! with a question, and what the personal library keeps of a PDF. Nothing here sends anything;
//! `pdfcraft-ai` builds the request from it.
//!
//! - A document that forbids copying (Table 22 bit 5, as Copy uses) gives nothing.
//! - Text under redaction marks not yet applied is left out, and counted, so the preview can say
//!   so. A glyph is left out when its centre lies in a mark's rectangle (the whole rectangle, not
//!   only its quadrilaterals: when in doubt, less is sent).
//! - Library documents are read with no password: one that needs a password to open is listed as
//!   such and indexed later from an open copy ([`Session::library_document`]).

use std::ops::Range;
use std::sync::Arc;

use pdfcraft_ai::claude::Passage;
use pdfcraft_ai::library::{self, Content, Library, NewDocument};
use pdfcraft_ai::source::{DocRef, Sha256, SourceId};
use pdfcraft_render::{DocInfo, OpenError, PageRenderer, PageText, RenderConfig, RenderRequest, RequestKind, inspect};

use crate::{DocId, Session, guard};

/// The selection's text kept (bytes).
pub const MAX_SELECTION_BYTES: usize = 16 * 1024;
/// Glyphs either side of the selection sent as its surroundings.
pub const SURROUNDING_GLYPHS: usize = 1_500;
/// The surroundings kept (bytes).
pub const MAX_SURROUNDING_BYTES: usize = 8 * 1024;
/// Selection text added to the question when searching the library (bytes).
const QUERY_SELECTION_BYTES: usize = 1_024;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AssistError {
    #[error("no document with that id is open")]
    NoDocument,
    #[error("the document has no page {0}")]
    NoPage(usize),
    #[error("this document doesn't allow copying its text and images, so it can't be sent to the assistant")]
    NotPermitted,
    #[error("the page's text couldn't be read: {0}")]
    Text(String),
    #[error("the file isn't a PDF PdfCraft can read: {0}")]
    Unreadable(String),
}

/// What is selected on a page.
#[derive(Clone, Debug, PartialEq)]
pub enum Selected {
    /// Glyphs of the page's text layer, as the text selection numbers them.
    Glyphs(Range<usize>),
    /// Every glyph whose centre lies in this rectangle (view space: points, y down).
    Region([f32; 4]),
    /// The whole page.
    Page,
}

/// What a question about a selection may send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionContext {
    /// 0-based.
    pub page: u32,
    pub page_label: String,
    /// The selected glyphs' text in the text layer's order (for a run of glyphs, what Copy gives),
    /// without text under redaction marks.
    pub text: String,
    /// The page's text around the selection (empty when the whole page is selected).
    pub surrounding: String,
    /// The document's title, or its file name.
    pub title: String,
    pub encrypted: bool,
    /// Glyphs left out because unapplied redaction marks cover them.
    pub redacted_glyphs: usize,
}

impl Session {
    /// The context for a question about `selected` on `page` (0-based) of document `id`.
    pub fn assistant_context(&self, id: DocId, page: usize, selected: &Selected) -> Result<SelectionContext, AssistError> {
        let doc = self.get(id).ok_or(AssistError::NoDocument)?;
        let page_info = doc.info.pages.get(page).ok_or(AssistError::NoPage(page))?;
        if !doc.allows_copying() {
            return Err(AssistError::NotPermitted);
        }
        let text = page_text(&doc.bytes, doc.password.as_deref(), page)?;
        let areas = redaction_areas(&doc.info, page);
        let count = text.glyphs.len();
        let chosen: Vec<usize> = match selected {
            Selected::Glyphs(r) => (r.start.min(count)..r.end.min(count)).collect(),
            Selected::Region(rect) => (0..count).filter(|&i| text.glyphs.get(i).is_some_and(|g| inside(centre(g.rect), *rect))).collect(),
            Selected::Page => (0..count).collect(),
        };
        let covered = |i: &usize| text.glyphs.get(*i).is_some_and(|g| areas.iter().any(|a| inside(centre(g.rect), *a)));
        let redacted = chosen.iter().filter(|i| covered(i)).count();
        let mut selection = text_of(&text, chosen.iter().copied().filter(|i| !covered(i)));
        pdfcraft_ai::text::truncate(&mut selection, MAX_SELECTION_BYTES);
        let surrounding = match (selected, chosen.first(), chosen.last()) {
            (Selected::Page, ..) | (_, None, _) | (_, _, None) => String::new(),
            (_, Some(&first), Some(&last)) => {
                let window = first.saturating_sub(SURROUNDING_GLYPHS)..last.saturating_add(SURROUNDING_GLYPHS).saturating_add(1).min(count);
                let mut s = text_of(&text, window.filter(|i| !covered(i)));
                pdfcraft_ai::text::truncate(&mut s, MAX_SURROUNDING_BYTES);
                s
            }
        };
        Ok(SelectionContext {
            page: u32::try_from(page).unwrap_or(u32::MAX),
            page_label: page_info.label.clone(),
            text: selection,
            surrounding,
            title: title_of(&doc.info, &doc.name),
            encrypted: doc.info.encrypted,
            redacted_glyphs: redacted,
        })
    }

    /// The library's view of open document `id`, read with the password it was opened with: how
    /// an encrypted document gets into the library.
    pub fn library_document(&self, id: DocId, added_at: i64) -> Result<NewDocument, AssistError> {
        let doc = self.get(id).ok_or(AssistError::NoDocument)?;
        let content = if doc.allows_copying() {
            let bytes = doc.bytes.clone();
            let password = doc.password.clone();
            let info = doc.info.clone();
            Content::Pages(guard(move || extract_pages(bytes, password.as_deref(), &info)).map_err(AssistError::Text)?)
        } else {
            Content::NotPermitted
        };
        Ok(NewDocument {
            doc: DocRef::of(&doc.bytes),
            path: doc.path.clone().unwrap_or_else(|| doc.name.clone()),
            title: title_of(&doc.info, &doc.name),
            pages: page_count(&doc.info),
            added_at,
            content,
        })
    }

    /// The SHA-256 of an open document's working file, by which the library knows it.
    pub fn document_sha256(&self, id: DocId) -> Option<Sha256> {
        self.get(id).map(|d| Sha256::of(&d.bytes))
    }
}

/// The library's view of a PDF file's bytes. `path` is where it was read from, `name` its file
/// name (the title when the document has none).
pub fn library_document_from_file(path: &str, name: &str, bytes: Arc<Vec<u8>>, added_at: i64) -> Result<NewDocument, AssistError> {
    let mut new =
        NewDocument { doc: DocRef::of(&bytes), path: path.to_owned(), title: name.to_owned(), pages: 0, added_at, content: Content::NeedsPassword };
    let opened = {
        let bytes = bytes.clone();
        guard(move || inspect(bytes, None)).map_err(AssistError::Unreadable)?
    };
    let info = match opened {
        Ok(info) => info,
        Err(OpenError::NeedsPassword | OpenError::WrongPassword) => return Ok(new),
        Err(e) => return Err(AssistError::Unreadable(e.to_string())),
    };
    new.title = title_of(&info, name);
    new.pages = page_count(&info);
    let copy = {
        let bytes = bytes.clone();
        guard(move || pdfcraft_cos::Document::open_with_password(bytes, None).map(|d| d.permissions().is_none_or(|p| p.copy())))
            .map_err(AssistError::Unreadable)?
            .map_err(|e| AssistError::Unreadable(e.to_string()))?
    };
    new.content =
        if copy { Content::Pages(guard(move || extract_pages(bytes, None, &info)).map_err(AssistError::Text)?) } else { Content::NotPermitted };
    Ok(new)
}

/// The passages a question about `ctx` sends from the document being read: the selection, then
/// the text around it.
pub fn context_passages(ctx: &SelectionContext) -> Vec<Passage> {
    let id = SourceId::ThisDocument { page: ctx.page };
    let mut out = Vec::new();
    if !ctx.text.trim().is_empty() {
        out.push(Passage { id, title: format!("{}, page {} (the selection)", ctx.title, ctx.page_label), text: ctx.text.clone() });
    }
    if !ctx.surrounding.trim().is_empty() && ctx.surrounding.trim() != ctx.text.trim() {
        out.push(Passage { id, title: format!("{}, page {} (around the selection)", ctx.title, ctx.page_label), text: ctx.surrounding.clone() });
    }
    out
}

/// The library passages for a question: searched with the question and the start of the
/// selection, leaving out the document being read (`current`), whose own text is sent anyway.
pub fn library_passages(lib: &Library, question: &str, ctx: Option<&SelectionContext>, current: Option<&Sha256>, k: usize) -> Vec<Passage> {
    let mut query = question.to_owned();
    if let Some(ctx) = ctx {
        query.push(' ');
        query.push_str(pdfcraft_ai::text::prefix(&ctx.text, QUERY_SELECTION_BYTES));
    }
    lib.search(&query, k, current)
        .into_iter()
        .map(|hit| {
            let title = lib.entry(&hit.doc).map_or("Library document", |e| e.title.as_str());
            let page = hit.page.saturating_add(1);
            Passage { id: SourceId::Library { sha256: hit.doc, page: hit.page }, title: format!("{title}, page {page}"), text: hit.text }
        })
        .collect()
}

fn title_of(info: &DocInfo, name: &str) -> String {
    info.title.as_deref().map(str::trim).filter(|t| !t.is_empty()).unwrap_or(name).to_owned()
}

fn page_count(info: &DocInfo) -> u32 {
    u32::try_from(info.pages.len()).unwrap_or(u32::MAX)
}

fn page_text(bytes: &Arc<Vec<u8>>, password: Option<&str>, page: usize) -> Result<Arc<PageText>, AssistError> {
    let config = RenderConfig { password: password.map(Arc::from), ..Default::default() };
    let mut r = PageRenderer::new(bytes.clone(), config);
    let out = r.render(RenderRequest { page, kind: RequestKind::Text, scale: 1.0, ..Default::default() });
    match (out.error, out.text) {
        (Some(e), _) => Err(AssistError::Text(e)),
        (None, text) => Ok(text.unwrap_or_default()),
    }
}

/// The text of every page, without what redaction marks cover, one renderer for the document.
/// Pages past [`library::MAX_PAGES`], or once the text passes what a whole library may hold, are
/// left out (and counted by the library as without text).
fn extract_pages(bytes: Arc<Vec<u8>>, password: Option<&str>, info: &DocInfo) -> Vec<library::PageText> {
    let config = RenderConfig { password: password.map(Arc::from), ..Default::default() };
    let mut r = PageRenderer::new(bytes, config);
    let mut out = Vec::new();
    let mut total = 0usize;
    for page in 0..info.pages.len().min(library::MAX_PAGES) {
        if total >= library::MAX_TEXT_BYTES {
            break;
        }
        let rendered = r.render(RenderRequest { page, kind: RequestKind::Text, scale: 1.0, ..Default::default() });
        let Some(text) = rendered.text else { continue };
        let areas = redaction_areas(info, page);
        let kept = (0..text.glyphs.len()).filter(|&i| text.glyphs.get(i).is_some_and(|g| !areas.iter().any(|a| inside(centre(g.rect), *a))));
        let mut s = text_of(&text, kept);
        pdfcraft_ai::text::truncate(&mut s, library::MAX_PAGE_TEXT_BYTES);
        total = total.saturating_add(s.len());
        out.push(library::PageText { page: u32::try_from(page).unwrap_or(u32::MAX), text: s });
    }
    out
}

/// The rectangles of the page's redaction marks, in view space.
fn redaction_areas(info: &DocInfo, page: usize) -> Vec<[f32; 4]> {
    let Some(p) = info.pages.get(page) else { return Vec::new() };
    info.annotations
        .iter()
        .filter(|a| a.page == page && a.subtype == "Redact")
        .map(|a| {
            let [x0, y0, x1, y1] = a.rect;
            let corners = [p.user_to_view(x0, y0), p.user_to_view(x1, y0), p.user_to_view(x0, y1), p.user_to_view(x1, y1)];
            let xs = corners.map(|c| c[0]);
            let ys = corners.map(|c| c[1]);
            [
                xs.into_iter().fold(f32::MAX, f32::min),
                ys.into_iter().fold(f32::MAX, f32::min),
                xs.into_iter().fold(f32::MIN, f32::max),
                ys.into_iter().fold(f32::MIN, f32::max),
            ]
        })
        .collect()
}

fn centre(r: [f32; 4]) -> [f32; 2] {
    [(r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0]
}

/// Whether `p` lies in `rect`, whichever way round its corners are given.
fn inside(p: [f32; 2], rect: [f32; 4]) -> bool {
    let (x0, x1) = (rect[0].min(rect[2]), rect[0].max(rect[2]));
    let (y0, y1) = (rect[1].min(rect[3]), rect[1].max(rect[3]));
    (x0..=x1).contains(&p[0]) && (y0..=y1).contains(&p[1])
}

/// The text of the glyphs `indices` (increasing), with spaces and line breaks as
/// [`PageText::text_of`] puts them; a gap within a line becomes a space.
fn text_of(text: &PageText, indices: impl Iterator<Item = usize>) -> String {
    let mut s = String::new();
    let mut prev: Option<usize> = None;
    for i in indices {
        let Some(g) = text.glyphs.get(i) else { continue };
        if let Some(p) = prev {
            let line = text.line_of.get(i);
            if line != text.line_of.get(p) {
                s.push('\n');
            } else if i != p.saturating_add(1) || text.space_before.get(i).copied().unwrap_or(false) {
                s.push(' ');
            }
        }
        s.push_str(&g.text);
        prev = Some(i);
    }
    s
}

#[cfg(test)]
mod tests;
