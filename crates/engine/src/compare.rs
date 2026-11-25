//! Compare files: the text differences between two open documents, a report, and the
//! differences marked as comments in the newer one.

use std::collections::HashMap;
use std::sync::Arc;

use pdfcraft_cos::ObjRef;
use pdfcraft_ocr::{Line, Paragraph, Rect, Region, RegionKind, Word, analyze_page, synthesize};

pub use pdfcraft_compare::{Change, Comparison, Kind, Side};
// How far Office export reads into form XObjects; `OfficeExport::truncated_pages` lists the pages
// that draw more.
pub use pdfcraft_edit::{MAX_FORM_DEPTH, MAX_FORM_VISITS};

use crate::{DocId, Edit, EditError, Markup, NewAnnotation, NoteIcon, Session, Shape};

/// Most bytes of encoded pictures one Office export prepares, counting each image object once
/// (the Word and PowerPoint writers keep at most 512 MiB of them as well).
pub const MAX_EXPORT_IMAGE_BYTES: usize = 512 << 20;

/// Acrobat's compare colours: replaced blue, inserted green, deleted red.
pub fn colour(kind: Kind) -> crate::Rgb {
    match kind {
        Kind::Replaced => [0.2, 0.45, 0.95],
        Kind::Inserted => [0.2, 0.75, 0.3],
        Kind::Deleted => [0.9, 0.25, 0.25],
    }
}

impl crate::Document {
    /// Standards ▸ Verify PDF/A: the rules the document breaks for `level`.
    pub fn pdfa_verify(&self, level: pdfcraft_preflight::Level) -> Vec<pdfcraft_preflight::Issue> {
        self.editor.as_ref().map(|e| pdfcraft_preflight::verify(&e.cos, level)).unwrap_or_default()
    }

    /// The standards the document declares (Standards panel).
    pub fn standards(&self) -> pdfcraft_preflight::Declared {
        self.editor.as_ref().map(|e| pdfcraft_preflight::declared(&e.cos)).unwrap_or_default()
    }

    /// Every word of the document in reading order, with page and box.
    pub fn words(&self) -> Vec<pdfcraft_compare::Word> {
        let config = pdfcraft_render::RenderConfig { password: self.password.as_deref().map(Arc::from), ..Default::default() };
        let mut r = pdfcraft_render::PageRenderer::new(self.bytes.clone(), config);
        let mut out = Vec::new();
        for (page, info) in self.info.pages.iter().enumerate() {
            let res = r.render(pdfcraft_render::RenderRequest { page, kind: pdfcraft_render::RequestKind::Text, scale: 1.0, ..Default::default() });
            if let Some(t) = res.text {
                out.extend(crate::js::page_words(&t, info).into_iter().map(|(text, rect)| pdfcraft_compare::Word { text, page, rect }));
            }
        }
        out
    }
}

impl Session {
    /// Compare files: the text differences from document `old` to document `new`. The result
    /// carries both documents' text, so both must allow copying it.
    pub fn compare(&self, old: DocId, new: DocId) -> Result<Comparison, EditError> {
        let a = self.get(old).ok_or(EditError::NoDocument)?;
        let b = self.get(new).ok_or(EditError::NoDocument)?;
        a.check_copying()?;
        b.check_copying()?;
        Ok(pdfcraft_compare::compare(&a.words(), &b.words()))
    }

    /// [`Session::compare`], keeping the changes on `pages` of `new` (0-based; empty: every page).
    pub fn compare_pages(&self, old: DocId, new: DocId, pages: &[usize]) -> Result<Comparison, EditError> {
        let mut c = self.compare(old, new)?;
        if !pages.is_empty() {
            c.changes.retain(|ch| pages.iter().any(|page| (ch.new.page..=ch.new.last_page).contains(page)));
        }
        Ok(c)
    }

    /// Visual compare: regions where page n of `new` looks different from page n of `old`
    /// (rendered at `dpi`), as (page, user-space box in `new`).
    pub fn compare_visual(&self, old: DocId, new: DocId, dpi: f32) -> Result<Vec<(usize, [f64; 4])>, EditError> {
        let a = self.get(old).ok_or(EditError::NoDocument)?;
        let b = self.get(new).ok_or(EditError::NoDocument)?;
        let renderer = |d: &crate::Document| {
            pdfcraft_render::PageRenderer::new(
                d.bytes.clone(),
                pdfcraft_render::RenderConfig { password: d.password.as_deref().map(Arc::from), ..Default::default() },
            )
        };
        let (mut ra, mut rb) = (renderer(a), renderer(b));
        let scale = dpi.clamp(18.0, 150.0) / 72.0;
        let mut out = Vec::new();
        for (page, info) in b.info.pages.iter().enumerate().take(a.info.pages.len()) {
            let req = pdfcraft_render::RenderRequest { page, scale, ..Default::default() };
            let (x, y) = (ra.render(req), rb.render(req));
            if x.error.is_some() || y.error.is_some() {
                continue;
            }
            let s = y.width as f32 / info.width.max(1e-3);
            for r in pdfcraft_compare::visual_regions((&y.rgba, y.width, y.height), (&x.rgba, x.width, x.height), 24) {
                let p = info.view_to_user(r[0] as f32 / s, r[1] as f32 / s);
                let q = info.view_to_user(r[2] as f32 / s, r[3] as f32 / s);
                out.push((page, [p[0].min(q[0]) as f64, p[1].min(q[1]) as f64, p[0].max(q[0]) as f64, p[1].max(q[1]) as f64]));
            }
        }
        Ok(out)
    }

    /// The compare report as a new PDF (not opened).
    pub fn compare_report(&self, old: DocId, new: DocId) -> Result<Arc<Vec<u8>>, EditError> {
        self.compare_report_of(old, new, &[])
    }

    /// [`Session::compare_report`] of the changes on `pages` ([`Session::compare_pages`]).
    pub fn compare_report_of(&self, old: DocId, new: DocId, pages: &[usize]) -> Result<Arc<Vec<u8>>, EditError> {
        let c = self.compare_pages(old, new, pages)?;
        let name = |id| self.get(id).map(|d| d.name.clone()).unwrap_or_default();
        self.create_report("Compare Report", &pdfcraft_compare::report(&c, &name(old), &name(new)))
    }

    /// Mark the differences in `new` as comments: highlights over replaced and inserted text
    /// (blue, green) and a note where text was deleted (red), authored "Compare". One undoable
    /// step; returns how many comments were added.
    pub fn mark_differences(&mut self, old: DocId, new: DocId) -> Result<usize, EditError> {
        self.mark_differences_of(old, new, &[])
    }

    /// [`Session::mark_differences`] of the changes on `pages` ([`Session::compare_pages`]).
    pub fn mark_differences_of(&mut self, old: DocId, new: DocId, pages: &[usize]) -> Result<usize, EditError> {
        let c = self.compare_pages(old, new, pages)?;
        let mut edits = Vec::new();
        for ch in &c.changes {
            let color = colour(ch.kind);
            let contents = match ch.kind {
                Kind::Replaced => format!("Replaced: \"{}\" with \"{}\"", ch.old.text, ch.new.text),
                Kind::Inserted => format!("Inserted: \"{}\"", ch.new.text),
                Kind::Deleted => format!("Deleted: \"{}\"", ch.old.text),
            };
            let shape = if ch.new.rects.is_empty() {
                let Some(r) = ch.new.near else { continue };
                Shape::Note { at: [r[0], r[3]], icon: NoteIcon::Note }
            } else {
                Shape::TextMarkup {
                    kind: Markup::Highlight,
                    quads: ch.new.rects.iter().map(|r| [r[0], r[3], r[2], r[3], r[0], r[1], r[2], r[1]]).collect(),
                }
            };
            let mut style = crate::Style::default_for(&shape);
            style.color = color;
            edits.push(Edit::AddAnnotation(NewAnnotation { page: ch.new.page, shape, style, contents, author: "Compare".into() }));
        }
        let n = edits.len();
        if n > 0 {
            self.apply(new, Edit::Batch { label: "Mark differences".into(), edits })?;
        }
        Ok(n)
    }
}

/// Export a PDF ▸ Word, PowerPoint, HTML or RTF.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OfficeFormat {
    Docx,
    Pptx,
    Html,
    /// CSS-free HTML that preserves text and inline images but drops colours and layout styling.
    HtmlSimple,
    Rtf,
    Markdown,
}

impl OfficeFormat {
    pub fn extension(self) -> &'static str {
        match self {
            OfficeFormat::Docx => "docx",
            OfficeFormat::Pptx => "pptx",
            OfficeFormat::Html => "html",
            OfficeFormat::HtmlSimple => "html",
            OfficeFormat::Rtf => "rtf",
            OfficeFormat::Markdown => "md",
        }
    }

    pub fn from_extension(ext: &str) -> Option<OfficeFormat> {
        match ext.to_ascii_lowercase().as_str() {
            "docx" => Some(OfficeFormat::Docx),
            "pptx" => Some(OfficeFormat::Pptx),
            "html" | "htm" => Some(OfficeFormat::Html),
            "rtf" => Some(OfficeFormat::Rtf),
            "md" | "markdown" => Some(OfficeFormat::Markdown),
            _ => None,
        }
    }
}

/// A Word, PowerPoint, HTML, RTF or Markdown export ([`crate::Document::export_office_checked`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OfficeExport {
    /// The exported file.
    pub bytes: Vec<u8>,
    /// Pages (0-based) that draw forms nested deeper than [`MAX_FORM_DEPTH`] or more of them
    /// than [`MAX_FORM_VISITS`], or pictures past [`MAX_EXPORT_IMAGE_BYTES`] or past what one export
    /// may decode: export stops reading there to stay bounded on hostile files, so the text and images of those forms, or
    /// those pictures, are missing from `bytes`. RTF also lists the pages that lost a picture to
    /// its own limits (it repeats a picture's data at every placement) or one whose rotation it
    /// couldn't bake in; their other content is kept. A picture no format can encode (a JPX image,
    /// say) is left out of every format without being listed.
    pub truncated_pages: Vec<usize>,
}

/// An image object prepared for Office export: its file extension and encoded bytes, or why it is
/// left out (`Err(true)`: past [`MAX_EXPORT_IMAGE_BYTES`], or what the export may decode;
/// `Err(false)`: it can't be encoded).
type PreparedImage = Result<(&'static str, Arc<[u8]>), bool>;

/// Displayed points per user-space unit on `info`'s page (`/UserUnit`), 1 when the page has no
/// usable size.
pub(crate) fn display_unit(info: &pdfcraft_render::PageInfo) -> f64 {
    let crop_width = f64::from(info.crop[2] - info.crop[0]);
    let display_width = f64::from(if info.rotation.is_multiple_of(180) { info.width } else { info.height });
    if crop_width > 0.0 && crop_width.is_finite() && display_width.is_finite() { display_width / crop_width } else { 1.0 }
}

/// Bold and italic as a font's name says them (`Arial-BoldItalicMT`).
pub(crate) fn weight_and_slant(base_font: &str) -> (bool, bool) {
    let f = base_font.to_ascii_lowercase();
    (f.contains("bold") || f.contains("black") || f.contains("heavy"), f.contains("italic") || f.contains("oblique"))
}

/// One OCR page's blocks as the paragraphs synthesis reads: each block's kind and the geometry
/// its line gives it, in the page's points. `None` for a block with no usable box.
///
/// The page-layout rules type the blocks — a running stamp or footer, a caption, a signature, an
/// exhibit, body text — and synthesis kinds them. Each block is also a region of its own, so it
/// becomes a paragraph of its own and the paragraph's kind, order and geometry go straight back
/// onto it.
fn synthesized_blocks(blocks: &[pdfcraft_edit::TextBlock], width: f32, height: f32) -> Vec<Option<Paragraph>> {
    let box_of = |b: &pdfcraft_edit::TextBlock| [b.rect[0] as f32, b.rect[1] as f32, b.rect[2] as f32, b.rect[3] as f32];
    let usable = |b: &pdfcraft_edit::TextBlock| {
        let r = box_of(b);
        !b.text.trim().is_empty() && r.iter().all(|v| v.is_finite()) && r[2] > r[0] && r[3] > r[1]
    };
    let words: Vec<Word> = blocks.iter().filter(|b| usable(b)).map(|b| Word::new(b.text.clone(), box_of(b), "text layer")).collect();
    // What each block is on the page comes from the layout rules, over one word per block.
    let classified = analyze_page(&words, (width, height), None);
    let mut kind = vec![RegionKind::Text; words.len()];
    let mut free: Vec<usize> = (0..words.len()).rev().collect();
    for region in &classified {
        for line in &region.lines {
            for word in &line.words {
                if let Some(at) = free.iter().position(|i| words.get(*i) == Some(word)) {
                    let i = free.remove(at);
                    if let Some(slot) = kind.get_mut(i) {
                        *slot = region.kind;
                    }
                }
            }
        }
    }
    let page: Vec<Region> = words
        .iter()
        .enumerate()
        .map(|(order, word)| Region {
            kind: kind.get(order).copied().unwrap_or(RegionKind::Text),
            rect: Rect::new(word.rect[0], word.rect[1], word.rect[2], word.rect[3]),
            lines: vec![Line { words: vec![word.clone()] }],
            order,
        })
        .collect();
    let paragraphs = synthesize(&page);
    let count = blocks.iter().filter(|b| usable(b)).count();
    if paragraphs.len() != count {
        return vec![None; blocks.len()];
    }
    let mut out = vec![None; blocks.len()];
    let mut paragraphs = paragraphs.into_iter();
    for (i, block) in blocks.iter().enumerate() {
        if usable(block) {
            out[i] = paragraphs.next();
        }
    }
    out
}

impl crate::Document {
    /// The pages as paragraphs and images (for Word, HTML and RTF export), including what
    /// form XObjects draw.
    pub fn export_pages(&self) -> Vec<pdfcraft_export::Page> {
        self.export_pages_checked(true, MAX_EXPORT_IMAGE_BYTES).0
    }

    /// [`Self::export_pages`], and the pages whose forms were left partly unread or whose
    /// pictures went past `image_budget` bytes ([`OfficeExport::truncated_pages`]). Without
    /// `images` no picture is prepared.
    pub(crate) fn export_pages_checked(&self, images: bool, image_budget: usize) -> (Vec<pdfcraft_export::Page>, Vec<usize>) {
        let Some(editor) = self.editor.as_ref() else { return (Vec::new(), Vec::new()) };
        self.export_pages_within(images, image_budget, &mut pdfcraft_cos::ImageBudget::new(&editor.cos))
    }

    /// [`Self::export_pages_checked`], its pictures decoded within `image_work`, which is shared
    /// by the whole export: a picture past it is left out like one past `image_budget`.
    pub(crate) fn export_pages_within(
        &self,
        images: bool,
        image_budget: usize,
        image_work: &mut pdfcraft_cos::ImageBudget,
    ) -> (Vec<pdfcraft_export::Page>, Vec<usize>) {
        let Some(cos) = self.editor.as_ref().map(|e| &e.cos) else { return (Vec::new(), Vec::new()) };
        let mut truncated = Vec::new();
        // Each image object is encoded once and shared by every place that draws it.
        let mut encoded: HashMap<ObjRef, PreparedImage> = HashMap::new();
        let mut image_bytes = image_budget;
        let pages = self
            .info
            .pages
            .iter()
            .enumerate()
            .map(|(i, info)| {
                // Export uses the visible page in points, y up. Raw PDF coordinates include
                // CropBox offsets and precede /Rotate and /UserUnit.
                let visible_rect = |r: [f64; 4]| {
                    let a = info.user_to_view(r[0] as f32, r[1] as f32);
                    let b = info.user_to_view(r[2] as f32, r[3] as f32);
                    [a[0].min(b[0]) as f64, (info.height - a[1].max(b[1])) as f64, a[0].max(b[0]) as f64, (info.height - a[1].min(b[1])) as f64]
                };
                let unit = display_unit(info);
                let reading = pdfcraft_edit::reading_blocks_checked(cos, i).unwrap_or_default();
                let forms_unread = reading.truncated;
                let text: Vec<pdfcraft_edit::TextBlock> = reading.blocks.into_iter().filter(|b| !b.text.trim().is_empty()).collect();
                let chars = |invisible: bool| text.iter().filter(|b| b.invisible == invisible).map(|b| b.text.chars().count()).sum::<usize>();
                let (invisible_chars, visible_chars) = (chars(true), chars(false));
                // A scan's OCR layer can sit beside a small visible stamp (a clerk's RECEIVED mark,
                // a bates number): the page is OCR when the invisible text carries most of its
                // characters, not only when it carries all of them. A wholly invisible page is
                // this rule's own special case.
                let ocr = !forms_unread && invisible_chars > 0 && invisible_chars > visible_chars;
                // The stamp is in the OCR layer too, so exporting the visible blocks beside it
                // would print it twice.
                let text: Vec<pdfcraft_edit::TextBlock> = text.into_iter().filter(|b| !ocr || b.invisible).collect();
                let synthesized = if ocr { synthesized_blocks(&text, info.width, info.height) } else { Vec::new() };
                let blocks = text
                    .into_iter()
                    .enumerate()
                    .map(|(n, b)| {
                        let (bold, italic) = weight_and_slant(&b.base_font);
                        // `text` is trimmed; the editor text keeps the source's own spaces.
                        let space_around = [b.editor_text.starts_with(char::is_whitespace), b.editor_text.ends_with(char::is_whitespace)];
                        let paragraph = synthesized.get(n).and_then(Option::as_ref);
                        pdfcraft_export::Block {
                            space_around,
                            text: b.text,
                            rect: visible_rect(b.rect),
                            size: b.size * unit,
                            bold,
                            italic,
                            color: b.color,
                            rotation: b.rotation - f64::from(info.rotation),
                            // A synthesized OCR page's block keeps the kind and geometry of the
                            // paragraph it is; a PDF's own visible text stays plain body text.
                            kind: paragraph.map_or(pdfcraft_export::ParagraphKind::Body, |p| p.kind),
                            order: if paragraph.is_some() { n } else { 0 },
                            metrics: paragraph.map_or_else(pdfcraft_export::Metrics::default, |p| pdfcraft_export::Metrics {
                                indent_left: p.indent_left,
                                indent_first: p.indent_first,
                                line_spacing: p.line_spacing,
                            }),
                        }
                    })
                    .collect();
                let placed = if images { pdfcraft_edit::reading_images(cos, i).unwrap_or_default() } else { Vec::new() };
                let mut over_budget = false;
                let images = placed
                    .iter()
                    .filter_map(|im| {
                        let object = im.object?;
                        let prepared = encoded.entry(object).or_insert_with(|| {
                            let refused = image_work.refused();
                            let (ext, bytes) =
                                pdfcraft_create::image_file_within(cos, object, &mut *image_work).map_err(|_| image_work.refused() > refused)?;
                            image_bytes = image_bytes.checked_sub(bytes.len()).ok_or(true)?;
                            Ok((if ext == "jpg" { "jpg" } else { "png" }, Arc::from(bytes)))
                        });
                        over_budget |= matches!(prepared, Err(true));
                        let (ext, bytes) = prepared.clone().ok()?;
                        Some(pdfcraft_export::Image {
                            ext,
                            bytes,
                            rect: visible_rect(im.rect),
                            size: [im.matrix[0].hypot(im.matrix[1]) * unit, im.matrix[2].hypot(im.matrix[3]) * unit],
                            rotation: im.matrix[1].atan2(im.matrix[0]).to_degrees() - f64::from(info.rotation),
                        })
                    })
                    .collect();
                if forms_unread || over_budget {
                    truncated.push(i);
                }
                // The page colour (Edit a PDF ▸ Background, a producer's page fill) for formats that keep one.
                let background = pdfcraft_edit::page_background(cos, i).ok().flatten();
                // The ruled lines tell a drawn table from text that merely lines up (#5149).
                let rules = pdfcraft_edit::page_rules(cos, i).unwrap_or_default().into_iter().map(visible_rect).collect();
                pdfcraft_export::Page { width: info.width as f64, height: info.height as f64, blocks, images, ocr, background, rules }
            })
            .collect();
        (pages, truncated)
    }

    /// The document as a Word, PowerPoint, HTML or RTF file.
    pub fn export_office(&self, format: OfficeFormat) -> Vec<u8> {
        self.export_office_checked(format).bytes
    }

    /// [`Self::export_office`], and the pages whose text it had to leave out
    /// ([`OfficeExport::truncated_pages`]).
    pub fn export_office_checked(&self, format: OfficeFormat) -> OfficeExport {
        // Markdown leaves pictures out.
        let (pages, mut truncated_pages) = self.export_pages_checked(!matches!(format, OfficeFormat::Markdown), MAX_EXPORT_IMAGE_BYTES);
        let title = self.info.title.clone().unwrap_or_else(|| self.name.trim_end_matches(".pdf").to_string());
        // Word and HTML name the document's language (`/Lang`), so spelling is checked in it.
        let language = self.initial_view().language;
        let bytes = match format {
            OfficeFormat::Docx => pdfcraft_export::docx_with_language(&pages, &title, language.as_deref()),
            OfficeFormat::Pptx => pdfcraft_export::pptx(&pages, &title),
            OfficeFormat::Html => pdfcraft_export::html_with_language(&pages, &title, language.as_deref()).into_bytes(),
            OfficeFormat::HtmlSimple => pdfcraft_export::html_simple_with_language(&pages, &title, language.as_deref()).into_bytes(),
            OfficeFormat::Rtf => {
                let (rtf, omitted) = pdfcraft_export::rtf_with_title_checked(&pages, &title);
                truncated_pages.extend(omitted);
                truncated_pages.sort_unstable();
                truncated_pages.dedup();
                rtf.into_bytes()
            }
            OfficeFormat::Markdown => pdfcraft_export::markdown(&pages).into_bytes(),
        };
        OfficeExport { bytes, truncated_pages }
    }
}
