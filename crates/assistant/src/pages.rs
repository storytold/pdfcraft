//! The text of each page, and where it comes from.
//!
//! The glyphs come from the renderer's text extraction (what Find and Copy read). Whether they
//! are the page's own text or an OCR layer is read from the page's operators, not guessed from
//! the words:
//!
//! - a content stream PdfCraft's Scan & OCR wrote carries `/PCMark /OCR`;
//! - other OCR tools write their layer as invisible text (text rendering mode 3 or 7) over the
//!   scanned picture.
//!
//! A page with glyphs only in such streams is an OCR layer; one with both is mixed; one with
//! neither marks is extracted text. A page with no glyphs but with pictures is a scan that needs
//! OCR before the assistant can read it.

use std::collections::HashSet;
use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, ObjRef, Object};
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind};

/// Pages read from one document at most; the rest are reported as not read.
pub const MAX_PAGES: usize = 3000;
/// Decoded bytes of one content stream examined for OCR marks.
const MAX_STREAM: usize = 32 << 20;
/// Form XObjects followed below a page.
const MAX_FORM_DEPTH: usize = 4;

/// Where a page's text comes from.
#[derive(Clone, Debug, PartialEq)]
pub enum Origin {
    /// The page's own text, extracted from the PDF.
    Extracted,
    /// An OCR text layer that was already in the file.
    OcrLayer,
    /// Both the page's own text and an OCR layer.
    Mixed,
    /// Recognised now, in memory, by the app's OCR (mean word confidence, 0-100, when known).
    OcrNow { confidence: Option<f32> },
    /// No text. `pictures`: the page draws images, so it is probably a scan.
    NoText { pictures: bool },
    /// Recognition ran but found nothing it could read with enough confidence.
    Illegible { confidence: Option<f32> },
    /// The page could not be read.
    Failed(String),
}

impl Origin {
    /// Whether the page has text the assistant can use.
    pub fn readable(&self) -> bool {
        matches!(self, Origin::Extracted | Origin::OcrLayer | Origin::Mixed | Origin::OcrNow { .. })
    }

    /// Whether the text came from character recognition (and may contain misreadings).
    pub fn is_ocr(&self) -> bool {
        matches!(self, Origin::OcrLayer | Origin::Mixed | Origin::OcrNow { .. })
    }

    /// A short label, in Spanish.
    pub fn label(&self) -> String {
        match self {
            Origin::Extracted => "texto extraído".into(),
            Origin::OcrLayer => "OCR (capa existente en el PDF)".into(),
            Origin::Mixed => "texto extraído + capa OCR".into(),
            Origin::OcrNow { confidence: Some(c) } => format!("OCR reconocido ahora (confianza media {c:.0}%)"),
            Origin::OcrNow { confidence: None } => "OCR reconocido ahora".into(),
            Origin::NoText { pictures: true } => "escaneada, sin texto: necesita OCR".into(),
            Origin::NoText { pictures: false } => "sin texto".into(),
            Origin::Illegible { confidence: Some(c) } => format!("escaneada, ilegible para el OCR (confianza {c:.0}%)"),
            Origin::Illegible { confidence: None } => "escaneada, ilegible para el OCR".into(),
            Origin::Failed(e) => format!("no se pudo leer: {e}"),
        }
    }
}

/// One page's text.
#[derive(Clone, Debug, PartialEq)]
pub struct PageSource {
    /// 0-based.
    pub page: usize,
    pub origin: Origin,
    pub text: String,
    /// The renderer cut the page's content at its limits: the text is incomplete.
    pub truncated: bool,
}

/// The text of a whole document.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DocText {
    pub pages: Vec<PageSource>,
    /// Pages beyond [`MAX_PAGES`], not read.
    pub unread_pages: usize,
}

impl DocText {
    pub fn page(&self, page: usize) -> Option<&PageSource> {
        self.pages.iter().find(|p| p.page == page)
    }

    /// Pages a scan left without text (0-based), for the OCR offer.
    pub fn scanned_without_text(&self) -> Vec<usize> {
        self.pages.iter().filter(|p| matches!(p.origin, Origin::NoText { pictures: true })).map(|p| p.page).collect()
    }

    /// Replace a page's text with what recognition read now. Below `min_confidence` (or with no
    /// words) the page is marked illegible and its text is not used. Recognised text across the
    /// document is held to the original's OCR layer limits ([`MAX_OCR_WORDS`], [`MAX_OCR_BYTES`]);
    /// a page past them is reported, not silently cut.
    pub fn apply_ocr(&mut self, page: usize, text: String, confidence: Option<f32>, min_confidence: f32) {
        let (words, bytes) = self
            .pages
            .iter()
            .filter(|p| matches!(p.origin, Origin::OcrNow { .. }) && p.page != page)
            .fold((0usize, 0usize), |(w, b), p| (w.saturating_add(p.text.split_whitespace().count()), b.saturating_add(p.text.len())));
        let Some(p) = self.pages.iter_mut().find(|p| p.page == page) else { return };
        if words.saturating_add(text.split_whitespace().count()) > MAX_OCR_WORDS || bytes.saturating_add(text.len()) > MAX_OCR_BYTES {
            p.origin = Origin::Failed("el texto reconocido supera el límite de OCR (65 536 palabras o 4 MiB)".into());
            p.text.clear();
            return;
        }
        let illegible = match confidence {
            Some(c) => c < min_confidence || text.split_whitespace().next().is_none(),
            // The engine reports no confidence (ocrs doesn't): judge by how much of the reading
            // looks like words.
            None => !looks_like_words(&text),
        };
        if illegible {
            p.origin = Origin::Illegible { confidence };
            p.text.clear();
        } else {
            p.origin = Origin::OcrNow { confidence };
            p.text = text;
        }
    }

    pub fn count(&self, f: impl Fn(&Origin) -> bool) -> usize {
        self.pages.iter().filter(|p| f(&p.origin)).count()
    }
}

/// Recognition budgets: at most 65,536 words and 4 MiB of text per page.
pub const MAX_OCR_WORDS: usize = 65_536;
pub const MAX_OCR_BYTES: usize = 4 * 1024 * 1024;

/// Recognition below 70% mean confidence is too uncertain to quote.
pub const LOW_CONFIDENCE: f32 = 70.0;
pub const HIGH_CONFIDENCE: f32 = 90.0;

/// Words a recognised page needs, and the share of its tokens that must look like words (two or
/// more letters, at most one other character), for its reading to be used when the engine gives
/// no confidence. A blurred scan read as noise fails this.
pub const MIN_OCR_WORDS: usize = 3;
pub const MIN_WORD_SHARE: f32 = 0.6;

pub fn looks_like_words(text: &str) -> bool {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let words = tokens
        .iter()
        .filter(|t| {
            let letters = t.chars().filter(|c| c.is_alphabetic()).count();
            let others = t.chars().count() - letters;
            letters >= 2 && others <= 1
        })
        .count();
    words >= MIN_OCR_WORDS && (words as f32) >= MIN_WORD_SHARE * tokens.len() as f32
}

/// Read every page's text and classify its origin. `cancelled` is polled between pages.
pub fn read_document(bytes: Arc<Vec<u8>>, password: Option<&str>, cancelled: &dyn Fn() -> bool) -> Result<DocText, String> {
    let doc = Document::open_with_password(bytes.clone(), password).map_err(|e| format!("no se pudo abrir el PDF: {e}"))?;
    if doc.permissions().is_some_and(|p| !p.copy()) {
        return Err("This document does not permit copying text.".into());
    }
    let pages = pdfcraft_model::pages(&doc);
    for page in &pages {
        if page_annotations_have_redactions(&doc, page) {
            return Err("Apply or remove redaction marks before using the assistant.".into());
        }
    }
    let config = RenderConfig { password: password.map(Arc::from), ..Default::default() };
    let mut renderer = PageRenderer::new(bytes, config);
    let mut out = DocText { unread_pages: pages.len().saturating_sub(MAX_PAGES), ..Default::default() };
    for (index, page) in pages.iter().enumerate().take(MAX_PAGES) {
        if cancelled() {
            return Err("Cancelado.".into());
        }
        let rendered = renderer.render(RenderRequest { page: index, kind: RequestKind::Text, scale: 1.0, ..Default::default() });
        if let Some(e) = rendered.error {
            out.pages.push(PageSource { page: index, origin: Origin::Failed(e.to_string()), text: String::new(), truncated: false });
            continue;
        }
        let (text, truncated) = match rendered.text {
            Some(t) => (t.plain_text(), t.truncated),
            None => (String::new(), false),
        };
        let marks = scan_page(&doc, &page.dict);
        let origin = classify(&text, &marks);
        out.pages.push(PageSource { page: index, origin, text: if text.trim().is_empty() { String::new() } else { text }, truncated });
    }
    Ok(out)
}

/// What a page's operators show about its text.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Marks {
    /// Text drawn visibly outside OCR-marked streams.
    pub visible_text: usize,
    /// Text drawn invisibly, or inside a stream PdfCraft marked as its OCR layer.
    pub ocr_text: usize,
    /// Images drawn (XObjects and inline).
    pub pictures: usize,
}

pub fn classify(text: &str, marks: &Marks) -> Origin {
    if text.trim().is_empty() {
        return Origin::NoText { pictures: marks.pictures > 0 };
    }
    match (marks.visible_text > 0, marks.ocr_text > 0) {
        (false, true) => Origin::OcrLayer,
        (true, true) => Origin::Mixed,
        _ => Origin::Extracted,
    }
}

fn scan_page(doc: &Document, page: &Dict) -> Marks {
    let mut marks = Marks::default();
    let resources = page.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let mut seen = HashSet::new();
    let Some(contents) = page.get(b"Contents") else { return marks };
    let mut streams: Vec<Object> = Vec::new();
    match &*doc.resolve(contents) {
        Object::Array(items) => streams.extend(items.iter().cloned()),
        _ => streams.push(contents.clone()),
    }
    for s in streams.iter().take(4096) {
        let obj = doc.resolve(s);
        let Object::Stream(stream) = &*obj else { continue };
        let ocr_stream = stream.dict.name(b"PCMark") == Some(b"OCR".as_slice());
        let Ok(data) = stream.decoded_within(MAX_STREAM) else { continue };
        scan_ops(doc, &data, &resources, ocr_stream, 0, &mut seen, &mut marks);
    }
    marks
}

fn scan_ops(doc: &Document, data: &[u8], resources: &Dict, ocr_stream: bool, depth: usize, seen: &mut HashSet<ObjRef>, marks: &mut Marks) {
    let parsed = pdfcraft_content::parse(data);
    // Text rendering mode is graphics state: saved by q, restored by Q.
    let mut mode: i64 = 0;
    let mut saved: Vec<i64> = Vec::new();
    for op in &parsed.ops {
        match op.op.as_slice() {
            b"q" if saved.len() < 1024 => saved.push(mode),
            b"Q" => mode = saved.pop().unwrap_or(0),
            b"Tr" => mode = op.operands.first().and_then(Object::as_int).unwrap_or(0),
            b"Tj" | b"TJ" | b"'" | b"\"" => {
                if ocr_stream || mode == 3 || mode == 7 {
                    marks.ocr_text = marks.ocr_text.saturating_add(1);
                } else {
                    marks.visible_text = marks.visible_text.saturating_add(1);
                }
            }
            b"BI" => marks.pictures = marks.pictures.saturating_add(1),
            b"Do" => {
                if op.inline.is_some() {
                    continue;
                }
                let Some(name) = op.last_name() else { continue };
                let Some(xobjects) = resources.get(b"XObject").map(|x| doc.resolve(x)) else { continue };
                let Some(target) = xobjects.as_dict().and_then(|d| d.get(name).cloned()) else { continue };
                if let Object::Ref(r) = &target
                    && !seen.insert(*r)
                {
                    // Drawn before: count a picture again, but don't walk a form twice.
                    if is_image(doc, &target) {
                        marks.pictures = marks.pictures.saturating_add(1);
                    }
                    continue;
                }
                let resolved = doc.resolve(&target);
                let Object::Stream(x) = &*resolved else { continue };
                match x.dict.name(b"Subtype") {
                    Some(b"Image") => marks.pictures = marks.pictures.saturating_add(1),
                    Some(b"Form") if depth < MAX_FORM_DEPTH => {
                        let inner = x.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned());
                        let inner = inner.unwrap_or_else(|| resources.clone());
                        let ocr = ocr_stream || x.dict.name(b"PCMark") == Some(b"OCR".as_slice());
                        if let Ok(d) = x.decoded_within(MAX_STREAM) {
                            scan_ops(doc, &d, &inner, ocr, depth + 1, seen, marks);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        if op.inline.is_some() && op.op.as_slice() != b"BI" {
            marks.pictures = marks.pictures.saturating_add(1);
        }
    }
}

fn is_image(doc: &Document, o: &Object) -> bool {
    doc.resolve(o).as_dict().is_some_and(|d| d.name(b"Subtype") == Some(b"Image".as_slice()))
}

pub(crate) fn page_annotations_have_redactions(doc: &Document, page: &pdfcraft_model::Page) -> bool {
    let Some(annots) = page.dict.get(b"Annots") else { return false };
    let resolved = doc.resolve(annots);
    let Some(annots) = resolved.as_array() else { return true };
    annots.iter().any(|a| {
        let resolved = doc.resolve(a);
        let Some(dict) = resolved.as_dict() else { return true };
        dict.get(b"Subtype").is_some_and(|s| doc.resolve(s).as_name() == Some(b"Redact"))
    })
}
